//! Private parent/sidecar protocol. It carries ownership, never API authority.
//! No route here can approve, edit, push, publish, or spawn a worker.
use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use serde::Deserialize;
use serde_json::{json, Value};
use subtle::ConstantTimeEq;

use crate::common::home::Home;
use crate::common::home_lock::HomeLock;

pub const PROTOCOL: u32 = 1;
const MAX_FRAME: u64 = 8192;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    protocol: u32,
    challenge: String,
    #[serde(default)]
    initialize_key: Option<String>,
}

fn read_frame(reader: &mut impl BufRead) -> anyhow::Result<Option<Value>> {
    let mut line = String::new();
    let n = std::io::Read::take(reader, MAX_FRAME + 1).read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    if n as u64 > MAX_FRAME || !line.ends_with('\n') {
        anyhow::bail!("invalid control frame");
    }
    Ok(Some(serde_json::from_str(&line)?))
}

fn send(value: Value) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

fn local_endpoint(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && crate::llm::backend::is_loopback_url(raw)
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

async fn model_readiness(endpoint: &str, model: &str, key: &str) -> Value {
    if !local_endpoint(endpoint) {
        return json!({"model":model,"state":"not_probed","detail":"Only a configured loopback model endpoint can be checked here."});
    }
    let result = async {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()?;
        let mut request = client.get(format!("{}/models", endpoint.trim_end_matches('/')));
        if !key.is_empty() {
            request = request.bearer_auth(key);
        }
        let mut response = request.send().await?.error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 262144 {
                return Ok::<_, reqwest::Error>(None);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice::<Value>(&bytes).ok())
    }
    .await;
    match result {
        Ok(Some(value)) if value["data"].is_array() => {
            let found = value["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"].as_str() == Some(model));
            json!({"endpoint":endpoint,"model":model,"state":if found {"installed"} else {"tag_missing"},
                "detail":"Inventory only; use an explicit console chat to test inference. No model was downloaded."})
        }
        _ => {
            json!({"endpoint":endpoint,"model":model,"state":"unavailable","detail":"Start the configured local model service and retry. No runtime was switched."})
        }
    }
}

pub fn run() -> anyhow::Result<()> {
    // No tracing subscriber: arbitrary config/error fields cannot reach stdout.
    let result = start();
    if result.is_err() {
        let _ = send(
            json!({"error":"startup_failed", "message":"Backend startup failed. Check config.yaml, home permissions and recovery records, then retry."}),
        );
    }
    result
}

fn start() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let hello: Hello =
        serde_json::from_value(read_frame(&mut reader)?.ok_or_else(|| anyhow::anyhow!("missing hello"))?)?;
    if hello.protocol != PROTOCOL
        || hello.challenge.len() != 64
        || !hello.challenge.bytes().all(|c| c.is_ascii_hexdigit())
    {
        anyhow::bail!("incompatible protocol");
    }
    let home = Home::resolve(None);
    let _ownership = match HomeLock::acquire(&home.root) {
        Ok(lock) => lock,
        Err(_) => {
            send(
                json!({"error":"home_unavailable", "message":"Home is unavailable or another server owns it. Quit that server and retry; do not delete its data or lock file."}),
            )?;
            return Ok(());
        }
    };
    home.ensure_layout()?;
    let config = home.load_config()?;
    let primary = config.str_or("models.local_llm.base_url", "http://127.0.0.1:11434/v1");
    if !local_endpoint(&primary) {
        send(
            json!({"error":"local_endpoint_required","message":"Desktop chat requires a loopback endpoint without URL credentials or query parameters. Correct models.local_llm.base_url in config.yaml."}),
        )?;
        return Ok(());
    }
    let mut keys = match super::env_keys::read_startup_keys(&home.env_path()) {
        Ok(keys) => keys,
        Err(error) => {
            send(json!({"error":"credentials_unreadable", "message":error.to_string()}))?;
            return Ok(());
        }
    };
    let key_name = crate::common::apikey::API_KEY_ENV;
    if let Some(key) = hello.initialize_key {
        if keys.contains_key(key_name) || std::env::var(key_name).is_ok() {
            send(
                json!({"error":"key_exists", "message":"An API key already exists; enter it in the console. Setup will not overwrite it."}),
            )?;
            return Ok(());
        }
        super::env_keys::validate_value(key_name, &key)?;
        let update = [(key_name.to_string(), key)].into_iter().collect();
        super::env_keys::write_keys(&home.env_path(), &update)?;
        keys.extend(update);
    }
    for (name, value) in keys {
        // This entrypoint is still single-threaded; preserve explicit env precedence.
        if std::env::var_os(&name).is_none() {
            std::env::set_var(name, value);
        }
    }
    let rt = tokio::runtime::Runtime::new()?;
    let outcome = rt.block_on(async {
        let mut options = super::AppOptions::new(home);
        options.config = Some(config);
        let (app, state) = super::build_app(options).await?;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let proof = hello.challenge.clone();
        let readiness = get(move |headers: HeaderMap| {
            let proof = proof.clone();
            async move {
                let given = headers
                    .get("x-cgah-readiness")
                    .map(|h| h.as_bytes())
                    .unwrap_or_default();
                if given.len() == proof.len() && bool::from(given.ct_eq(proof.as_bytes())) {
                    (
                        StatusCode::OK,
                        axum::Json(json!({"protocol":PROTOCOL,"pid":std::process::id()})),
                    )
                } else {
                    (StatusCode::UNAUTHORIZED, axum::Json(json!({"error":"unauthorized"})))
                }
            }
        });
        let app = app.route("/_desktop/ready", readiness);
        let server = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await
        });
        send(
            json!({"protocol":PROTOCOL,"challenge":hello.challenge,"pid":std::process::id(),"port":port,
            "key_configured":state.api_key.as_deref().is_some_and(|s| !s.is_empty()),
            "api_key_optional":state.api_key_optional,
            "home":state.home.root,"chat_model":state.current_model(),
            "planner_model":state.cfg.str_or("agentic.deepagent_github.model", ""),
            "auth_enabled":state.auth.is_some()}),
        )?;
        // Blocking stdin reader lives on this caller thread while the multithread
        // runtime serves requests. EOF after parent death enters the same shutdown.
        loop {
            let frame = match read_frame(&mut reader) {
                Ok(Some(v)) => v,
                _ => break,
            };
            match frame.get("command").and_then(Value::as_str) {
                Some("status") => send(json!({"active":state.jobs.running_count(),
                    "chat_active":state.generation_gate.is_held(),"run_active":state.agent_run_gate.is_held() || crate::shim::active_operations() > 0}))?,
                Some("models") => {
                    let chat = model_readiness(&state.backend.base_url, &state.current_model(), &state.backend.api_key).await;
                    let planner = if matches!(state.cfg.str_or("agentic.deepagent_github.provider", "ollama").as_str(), "ollama" | "local") {
                        model_readiness(&state.cfg.str_or("agentic.deepagent_github.base_url", "http://127.0.0.1:11434/v1"),
                            &state.cfg.str_or("agentic.deepagent_github.model", ""),
                            &std::env::var("DEEPAGENT_API_KEY").unwrap_or_default()).await
                    } else { json!({"state":"not_probed","detail":"Configured cloud planner is never probed by desktop readiness."}) };
                    send(json!({"chat":chat,"planner":planner}))?;
                }
                Some("shutdown") => break,
                _ => {
                    send(json!({"error":"unknown_control_command"}))?;
                }
            }
        }
        state.chat.abort_in_flight();
        for job in state.jobs.list() {
            if let Some(id) = job["job_id"].as_str() {
                state.jobs.cancel(id);
            }
        }
        server.abort();
        let _ = server.await;
        // Permit cancellation guards and their bounded native cleanup to run.
        tokio::time::sleep(Duration::from_millis(250)).await;
        Ok::<(), anyhow::Error>(())
    });
    rt.shutdown_timeout(Duration::from_secs(10));
    outcome
}
