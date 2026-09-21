//! Real HTTPS and HTTP account boundaries; explicit legacy behavior is tested
//! separately. No browser bypass flags or trusted-root installation.
mod common;
use common::*;
use reqwest::{Method, Response};
use serde_json::{json, Value};

async fn cli(server: &TestServer, args: &[&str], input: &str) -> std::process::Output {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new(BIN)
        .args(args)
        .env("CGAGENTHARNESS_HOME", &server.home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(20), child.wait_with_output())
        .await
        .unwrap()
        .unwrap()
}

fn cookie(response: &Response) -> String {
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}
async fn login(server: &TestServer, name: &str, password: &str) -> String {
    let response = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"username":name,"password":password}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    cookie(&response)
}
async fn request(server: &TestServer, session: &str, method: Method, path: &str, body: Value) -> Response {
    server
        .req(method, path)
        .header("cookie", session)
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn configuration_reload_is_admin_csrf_guarded_atomic_and_effective() {
    let model = start_mock_model().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this content fixture binds only to loopback.
    let addr = listener.local_addr().unwrap();
    let content = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route("/page", axum::routing::get(|| async { "x".repeat(2000) })),
        )
        .await
        .unwrap();
    });
    let options = ServerOptions {
        web_resolve: Some(("reload.invalid".into(), addr)),
        ..Default::default()
    }
    .with("auth.enabled", "true")
    .with("api.rate_limit.max_requests", "1000")
    .with("web.response_bytes", "4096");
    let server = spawn_server(&model.base_url(), options).await;
    let bootstrap = login(&server, "admin", "admin").await;
    assert_eq!(
        request(&server, &bootstrap, Method::GET, "/api/analytics/summary", json!({}))
            .await
            .status(),
        403
    );
    assert_eq!(
        request(&server, &bootstrap, Method::POST, "/api/config/reload", json!({}))
            .await
            .status(),
        403
    );
    let changed = request(
        &server,
        &bootstrap,
        Method::POST,
        "/api/auth/password",
        json!({"current_password":"admin", "password":"reload-fixture-password"}),
    )
    .await;
    assert_eq!(changed.status(), 200);
    let admin = cookie(&changed);
    for role in ["operator", "audit"] {
        assert_eq!(
            request(
                &server,
                &admin,
                Method::POST,
                "/api/auth/users",
                json!({"username":role, "role":role, "password":"reload-fixture-password"})
            )
            .await
            .status(),
            200
        );
        let user = login(&server, role, "reload-fixture-password").await;
        assert_eq!(
            request(&server, &user, Method::GET, "/api/analytics/summary", json!({}))
                .await
                .status(),
            if role == "audit" { 403 } else { 200 }
        );
        assert_eq!(
            request(&server, &user, Method::POST, "/api/config/reload", json!({}))
                .await
                .status(),
            403
        );
    }
    assert_eq!(
        server
            .client
            .post(server.url("/api/config/reload"))
            .header("cookie", &admin)
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        server
            .req(Method::POST, "/api/config/reload")
            .header("cookie", &admin)
            .header("origin", "https://untrusted.invalid")
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        request(
            &server,
            &admin,
            Method::POST,
            "/api/config/reload",
            json!({"web":{"pages":2}})
        )
        .await
        .status(),
        422
    );

    // Plain HTTP is limited to the owned loopback fixture through the test resolver.
    let url = "http://reload.invalid/page"; // DevSkim: ignore DS137138 because this URL is mapped only to the loopback fixture above.
    assert_eq!(
        request(&server, &admin, Method::POST, "/api/web/allow", json!({"url":url}))
            .await
            .status(),
        200
    );
    assert_eq!(
        request(&server, &admin, Method::POST, "/api/web/fetch", json!({"url":url}))
            .await
            .status(),
        200
    );
    let old_web = server.state.web_snapshot();
    let mut cfg = server.state.cfg.raw.clone();
    cfg["web"]["response_bytes"] = serde_yaml_ng::Value::from(1024);
    cfg["api"]["harness_loop_rate_limit"]["max_tokens"] = serde_yaml_ng::Value::from(512);
    std::fs::write(&server.state.cfg.path, serde_yaml_ng::to_string(&cfg).unwrap()).unwrap();
    let response = request(&server, &admin, Method::POST, "/api/config/reload", json!({})).await;
    assert_eq!(response.status(), 200);
    let result: Value = response.json().await.unwrap();
    assert_eq!(result["limits"]["revision"], 1);
    assert_eq!(result["limits"]["web"]["response_bytes"], 1024);
    assert_eq!(result["limits"]["loop_max_tokens"], 512);
    assert_eq!(
        old_web.limits.response_bytes, 4096,
        "in-flight web snapshots keep their old limits"
    );
    assert_eq!(
        code(
            &request(&server, &admin, Method::POST, "/api/web/fetch", json!({"url":url}))
                .await
                .json::<Value>()
                .await
                .unwrap()
        ),
        "WEB_TOO_LARGE"
    );
    let repeat: Value = request(&server, &admin, Method::POST, "/api/config/reload", json!({}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(repeat["changed"], false);
    assert_eq!(repeat["limits"]["revision"], 1);

    let running = server.state.runtime_limits();
    let valid = serde_yaml_ng::to_string(&cfg).unwrap();
    let mut invalid = cfg.clone();
    invalid["api"]["rate_limit"]["max_requests"] = serde_yaml_ng::Value::from(1);
    invalid["web"]["pages"] = serde_yaml_ng::Value::from(0);
    let mut restart = cfg.clone();
    restart["auth"]["enabled"] = serde_yaml_ng::Value::from(false);
    let mut unknown = cfg.clone();
    unknown["private_reload_marker"] = serde_yaml_ng::Value::from("PRIVATE_RELOAD_TEXT");
    for text in [
        serde_yaml_ng::to_string(&invalid).unwrap(),
        serde_yaml_ng::to_string(&restart).unwrap(),
        serde_yaml_ng::to_string(&unknown).unwrap(),
        "web: [PRIVATE_RELOAD_TEXT\n".into(),
        format!("#PRIVATE_RELOAD_TEXT{}", "x".repeat(1_048_577)),
    ] {
        std::fs::write(&server.state.cfg.path, text).unwrap();
        let response = request(&server, &admin, Method::POST, "/api/config/reload", json!({})).await;
        assert_eq!(response.status(), 400);
        assert!(!response.text().await.unwrap().contains("PRIVATE_RELOAD_TEXT"));
        assert_eq!(
            *server.state.runtime_limits(),
            *running,
            "refused reload cannot partially apply limits"
        );
    }
    std::fs::write(&server.state.cfg.path, &valid).unwrap();
    // A new loop call uses the reloaded generation cap, not the startup value.
    let owner = server.state.auth.as_ref().unwrap().get_user("admin").unwrap().user_id;
    let session = server
        .state
        .store
        .for_owner(&owner)
        .create(&server.state.current_model(), "reload fixture")
        .unwrap();
    server
        .state
        .store
        .for_owner(&owner)
        .rename(&session.session_id, None, Some("say hello"))
        .unwrap();
    let before = model.requests.lock().unwrap().len();
    let reply = request(
        &server,
        &admin,
        Method::POST,
        "/api/chat",
        json!({"session_id":session.session_id, "message":"continue", "loop":true}),
    )
    .await;
    assert_eq!(reply.status(), 200, "{}", reply.text().await.unwrap());
    assert_eq!(model.requests.lock().unwrap()[before]["max_tokens"], 512);

    cfg["api"]["rate_limit"]["max_requests"] = serde_yaml_ng::Value::from(1);
    std::fs::write(&server.state.cfg.path, serde_yaml_ng::to_string(&cfg).unwrap()).unwrap();
    assert_eq!(
        request(&server, &admin, Method::POST, "/api/config/reload", json!({}))
            .await
            .status(),
        200
    );
    let refused = request(&server, &admin, Method::GET, "/api/sessions", Value::Null).await;
    assert_eq!(refused.status(), 429, "retained hits must survive reload");
    assert!(refused.headers().contains_key("retry-after"));
    let audit = std::fs::read_to_string(server.home.join("logs/audit.jsonl")).unwrap();
    assert!(audit.contains("config_reloaded") && audit.contains("config_reload_refused"));
    assert!(!audit.contains("PRIVATE_RELOAD_TEXT"));
    let legacy = spawn_server(&model.base_url(), ServerOptions::default()).await;
    assert_eq!(
        legacy.post_json("/api/config/reload", json!({})).await.0,
        403,
        "auth-disabled homes have no HTTP admin authority"
    );
    content.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn sighup_reloads_the_owned_server_and_invalid_reload_keeps_it_running() {
    let model = start_mock_model().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with(
        dir.path(),
        &[
            ("auth.enabled", "false"),
            ("tls.enabled", "false"),
            ("models.local_llm.base_url", &format!("\"{}\"", model.base_url())),
            ("models.local_llm.warmup.enabled", "false"),
            ("structured_memory.enabled", "false"),
            ("api.rate_limit.max_requests", "1000"),
        ],
    );
    let port = std::net::TcpListener::bind("127.0.0.1:0") // DevSkim: ignore DS162092 because this is an owned loopback fixture.
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = tokio::process::Command::new(BIN)
        .args(["serve", "--port", &port.to_string()])
        .env("CGAGENTHARNESS_HOME", dir.path())
        .env("GROK_API_KEY", "")
        .env("ANTHROPIC_API_KEY", "")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}/api/status"); // DevSkim: ignore DS162092 DS137138 because this is the owned loopback-only HTTP fixture.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if client.get(&url).send().await.is_ok() {
                break;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "fixture server exited during startup"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    let mut next = cfg.raw.clone();
    next["api"]["rate_limit"]["max_requests"] = serde_yaml_ng::Value::from(1);
    let audit_path = dir.path().join("logs/audit.jsonl");
    for (round, text, event, status) in [
        (1, serde_yaml_ng::to_string(&next).unwrap(), "config_reloaded", 429),
        (1, "web: [invalid".into(), "config_reload_refused", 429),
        (2, serde_yaml_ng::to_string(&cfg.raw).unwrap(), "config_reloaded", 200),
    ] {
        std::fs::write(&cfg.path, text).unwrap();
        // Only this test's owned child receives a signal.
        assert_eq!(unsafe { libc::kill(child.id().unwrap() as i32, libc::SIGHUP) }, 0);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let audit = std::fs::read_to_string(&audit_path).unwrap_or_default();
                if audit
                    .lines()
                    .filter(|line| line.contains(event) && line.contains("sighup"))
                    .count()
                    >= round
                {
                    break;
                }
                assert!(child.try_wait().unwrap().is_none(), "reload terminated the server");
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(client.get(&url).send().await.unwrap().status(), status);
    }
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

#[tokio::test]
async fn https_bootstrap_roles_revocation_and_private_web_context() {
    let model = start_mock_model().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let content = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/doc",
                axum::routing::get(|| async {
                    axum::response::Html("<h1>Private selection</h1><p>Scoped zebra evidence.</p>")
                }),
            ),
        )
        .await
        .unwrap();
    });
    let options = ServerOptions {
        web_resolve: Some(("scope.invalid".into(), addr)),
        api_key: Some(String::new()),
        ..Default::default()
    }
    .with("auth.enabled", "true")
    .with("tls.enabled", "true")
    .with("security.api_key_optional", "false")
    .with("structured_memory.enabled", "true")
    .with("api.rate_limit.max_requests", "1000");
    let server = spawn_server(&model.base_url(), options).await;
    assert!(
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(server.url("/api/status"))
            .send()
            .await
            .is_err(),
        "unknown self-signed certificate must fail ordinary browser trust"
    );
    let status = server.open_get("/api/status").await.1;
    assert!(status.get("home").is_none() && status.get("base_url").is_none());
    for path in cgagentharness::server::routes::registered_paths() {
        if !path.starts_with("/api/")
            || matches!(
                path.as_str(),
                "/api/status" | "/api/auth/setup-status" | "/api/auth/login" | "/api/auth/bootstrap-password"
            )
        {
            continue;
        }
        let response = server
            .req(
                Method::GET,
                &path
                    .replace("{username}", "admin")
                    .replace("{session_id}", &"0".repeat(12))
                    .replace("{run_id}", &"0".repeat(32))
                    .replace("{job_id}", &"0".repeat(32))
                    .replace("{id}", &"0".repeat(64)),
            )
            .bearer_auth("stale-key")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401, "unprotected API {path}");
    }
    let response = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"username":"admin","password":"admin"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let bootstrap = cookie(&response);
    let header = response.headers()["set-cookie"].to_str().unwrap();
    assert!(header.contains("; Secure") && header.contains("HttpOnly") && header.contains("SameSite=Strict"));
    assert_eq!(response.json::<Value>().await.unwrap()["must_change_password"], true);
    for path in ["/api/keys", "/api/sessions", "/api/web/allow", "/api/auth/users"] {
        let response = request(&server, &bootstrap, Method::POST, path, json!({})).await;
        assert_eq!(response.status(), 403);
        assert_eq!(
            code(&response.json::<Value>().await.unwrap()),
            "AUTH_PASSWORD_CHANGE_REQUIRED"
        );
    }
    let body = json!({"current_password":"admin","password":"replacement-admin-password"});
    assert_eq!(
        server
            .client
            .post(server.url("/api/auth/password"))
            .header("cookie", &bootstrap)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        server
            .req(Method::POST, "/api/auth/password")
            .header("cookie", &bootstrap)
            .header("origin", server.base.replace("https:", "http:"))
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        request(
            &server,
            &bootstrap,
            Method::POST,
            "/api/auth/password",
            json!({"current_password":"wrong","password":"replacement-admin-password"})
        )
        .await
        .status(),
        401
    );
    assert_eq!(
        request(
            &server,
            &bootstrap,
            Method::POST,
            "/api/auth/password",
            json!({"current_password":"admin","password":"short"})
        )
        .await
        .status(),
        422
    );
    let response = server
        .req(Method::POST, "/api/auth/password")
        .header("cookie", &bootstrap)
        .header("origin", &server.base)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let admin = cookie(&response);
    assert_ne!(bootstrap, admin);
    assert_eq!(
        request(&server, &bootstrap, Method::GET, "/api/auth/whoami", Value::Null)
            .await
            .status(),
        401
    );
    for (name, role) in [("op", "operator"), ("reader", "audit")] {
        assert_eq!(
            request(
                &server,
                &admin,
                Method::POST,
                "/api/auth/users",
                json!({"username":name,"role":role,"password":"fixture-account-password"})
            )
            .await
            .status(),
            200
        );
    }
    let op = login(&server, "op", "fixture-account-password").await;
    let audit = login(&server, "reader", "fixture-account-password").await;
    for session in [&op, &audit] {
        for (method, path, body) in [
            (Method::GET, "/api/keys", Value::Null),
            (
                Method::POST,
                "/api/keys",
                json!({"keys":{"GROK_API_KEY":"cannot-save-this-value"}}),
            ),
            (Method::GET, "/api/auth/users", Value::Null),
            (
                Method::POST,
                "/api/auth/users",
                json!({"username":"evil","role":"operator","password":"cannot-create-account"}),
            ),
            (Method::POST, "/api/web/allow", json!({"url":"https://example.com"})),
            (
                Method::POST,
                "/api/structured-memory/gates",
                json!({"gate":"retrieval","enabled":true}),
            ),
        ] {
            assert_eq!(
                request(&server, session, method.clone(), path, body).await.status(),
                403,
                "{method} {path}"
            );
        }
    }
    let status = request(&server, &audit, Method::GET, "/api/status", Value::Null)
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert!(status.get("home").is_none());
    assert_eq!(
        request(
            &server,
            &audit,
            Method::POST,
            "/api/chat",
            json!({"message":"refuse this"})
        )
        .await
        .status(),
        403
    );
    assert_eq!(
        request(&server, &op, Method::POST, "/api/chat", json!({"message":"hello"}))
            .await
            .status(),
        200
    );
    assert_eq!(
        request(
            &server,
            &admin,
            Method::POST,
            "/api/structured-memory/gates",
            json!({"gate":"retrieval","enabled":true})
        )
        .await
        .status(),
        200
    );
    assert_eq!(
        request(&server, &admin, Method::POST, "/api/web", json!({"enabled":true}))
            .await
            .status(),
        200
    );
    assert_eq!(
        request(
            &server,
            &admin,
            Method::POST,
            "/api/web/allow",
            json!({"url":"http://scope.invalid/doc"})
        )
        .await
        .status(),
        200
    );
    assert_eq!(
        request(
            &server,
            &op,
            Method::POST,
            "/api/web/fetch",
            json!({"url":"http://scope.invalid/doc"})
        )
        .await
        .status(),
        200
    );
    assert_eq!(
        request(&server, &op, Method::POST, "/api/web/inject", json!({}))
            .await
            .status(),
        200
    );
    for (session, expected) in [(&op, true), (&admin, false)] {
        let preview = request(&server, session, Method::POST, "/api/prompt/preview", json!({}))
            .await
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(
            preview["prompt"].as_str().unwrap().contains("Scoped zebra evidence"),
            expected
        );
    }
    assert_eq!(
        request(
            &server,
            &admin,
            Method::POST,
            "/api/web/deny",
            json!({"url":"http://scope.invalid/doc"})
        )
        .await
        .status(),
        200
    );
    let preview = request(&server, &op, Method::POST, "/api/prompt/preview", json!({}))
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert!(!preview["prompt"].as_str().unwrap().contains("Scoped zebra evidence"));
    for (method, path, body) in [
        (Method::POST, "/api/auth/users/admin/disabled", json!({"disabled":true})),
        (Method::POST, "/api/auth/users/admin/role", json!({"role":"operator"})),
        (Method::DELETE, "/api/auth/users/admin", Value::Null),
    ] {
        assert_eq!(request(&server, &admin, method, path, body).await.status(), 403);
    }
    assert_eq!(
        request(
            &server,
            &admin,
            Method::POST,
            "/api/auth/users/op/disabled",
            json!({"disabled":true})
        )
        .await
        .status(),
        200
    );
    assert_eq!(
        request(&server, &op, Method::GET, "/api/sessions", Value::Null)
            .await
            .status(),
        401
    );
    assert_eq!(
        request(
            &server,
            &admin,
            Method::POST,
            "/api/auth/users/op/disabled",
            json!({"disabled":false})
        )
        .await
        .status(),
        200
    );
    assert_eq!(
        request(&server, &op, Method::GET, "/api/sessions", Value::Null)
            .await
            .status(),
        401
    );
    let op = login(&server, "op", "fixture-account-password").await;
    assert_eq!(
        request(
            &server,
            &admin,
            Method::POST,
            "/api/auth/users/op/role",
            json!({"role":"audit"})
        )
        .await
        .status(),
        200
    );
    assert_eq!(
        request(&server, &op, Method::GET, "/api/sessions", Value::Null)
            .await
            .status(),
        401
    );
    let events = request(&server, &audit, Method::GET, "/api/audit", Value::Null)
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert!(events["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["actor"] == "admin" && v["route"] == "/api/auth/users/{username}/role"));
    assert!(!events.to_string().contains("password\":") && !events.to_string().contains("Scoped zebra"));
    let response = request(&server, &admin, Method::POST, "/api/auth/logout", json!({})).await;
    assert_eq!(response.status(), 200);
    assert!(response.headers()["set-cookie"].to_str().unwrap().contains("Secure"));
    let result = cli(
        &server,
        &["account", "--url", &server.base, "login", "admin"],
        "replacement-admin-password\n",
    )
    .await;
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let result_text = String::from_utf8_lossy(&result.stdout);
    assert!(
        !result_text.contains("replacement-admin-password")
            && !result_text.contains("csrf_token")
            && !result_text.contains("cookie")
    );
    let result = cli(
        &server,
        &[
            "web",
            "--url",
            &server.base,
            "allow",
            "https://example.com/reference",
            "--group",
            "cli",
        ],
        "",
    )
    .await;
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(server
        .state
        .web
        .policy()
        .unwrap()
        .rules
        .iter()
        .any(|r| r.group == "cli"));
    let result = cli(&server, &["web", "--url", &server.base, "status"], "").await;
    assert!(result.status.success());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(server.home.join("cli-session.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let result = cli(&server, &["account", "--url", &server.base, "logout"], "").await;
    assert!(result.status.success());
    assert!(!server.home.join("cli-session.json").exists());
    let result = cli(&server, &["web", "--url", &server.base, "status"], "").await;
    assert!(!result.status.success());
    content.abort();
}
