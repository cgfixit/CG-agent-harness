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
    let setup_code = std::fs::read_to_string(server.home.join("setup-code"))
        .unwrap()
        .trim()
        .to_string();
    let bootstrap = login(&server, "admin", &setup_code).await;
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
        json!({"current_password":setup_code, "password":"reload-fixture-password"}),
    )
    .await;
    assert_eq!(changed.status(), 200);
    let admin = cookie(&changed);
    for role in ["operator", "auditor"] {
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
            if role == "auditor" { 403 } else { 200 }
        );
        assert_eq!(
            request(&server, &user, Method::POST, "/api/config/reload", json!({}))
                .await
                .status(),
            403
        );
        // Web policy mode and the block list are administrator-only, like allow/deny.
        for (path, body) in [
            ("/api/web/mode", json!({"mode":"denylist","confirm":true})),
            ("/api/web/block", json!({"urls":["https://example.com"]})),
            ("/api/web/unblock", json!({"url":"https://example.com"})),
        ] {
            assert_eq!(
                request(&server, &user, Method::POST, path, body).await.status(),
                403,
                "{role} {path}"
            );
        }
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
    let audit = server.audit_log();
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

/// Spawn `serve` on a free loopback port and wait for `/api/status`. The port
/// is found by binding and releasing it, so another process can take it before
/// the child binds; `serve` then exits ("may already be running"). That exit is
/// retried on a fresh port, a bounded number of times.
#[cfg(unix)]
async fn spawn_serve(home: &std::path::Path) -> (tokio::process::Child, u16, reqwest::Client) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();
    for _ in 0..5 {
        let port = std::net::TcpListener::bind("127.0.0.1:0") // DevSkim: ignore DS162092 because this is an owned loopback fixture.
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut child = tokio::process::Command::new(BIN)
            .args(["serve", "--port", &port.to_string()])
            .env("CGAGENTHARNESS_HOME", home)
            .env("GROK_API_KEY", "")
            .env("ANTHROPIC_API_KEY", "")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let url = format!("http://127.0.0.1:{port}/api/status"); // DevSkim: ignore DS162092 DS137138 because this is the owned loopback-only HTTP fixture.
        let ready = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if client.get(&url).send().await.is_ok() {
                    return true;
                }
                if child.try_wait().unwrap().is_some() {
                    return false;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("fixture server did not answer within 10 s");
        if ready {
            return (child, port, client);
        }
    }
    panic!("fixture server exited during startup on 5 ports");
}

/// Status and error code of one sign-in with a wrong password.
#[cfg(unix)]
async fn wrong_sign_in(client: &reqwest::Client, port: u16, username: &str) -> (u16, String) {
    let base = format!("http://127.0.0.1:{port}"); // DevSkim: ignore DS162092 DS137138 because this is the owned loopback-only HTTP fixture.
    let response = client
        .post(format!("{base}/api/auth/login"))
        .header("Origin", &base)
        .json(&json!({"username": username, "password": "not-the-password-x9"}))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap();
    (status, body["detail"]["code"].as_str().unwrap_or_default().to_string())
}

/// A name with no account keeps its sign-in failures across a `serve`
/// restart, as the bootstrap `admin` account's stored count does, so after a
/// restart the two still answer alike. Memory-only failures once restarted
/// the unknown name at zero: it answered 401 where the account answered 423,
/// which told a guesser the account exists. Five failures each start a 2 s
/// lock, waited out before the restart so both names start the second server
/// unlocked: the sixth failure then counts (401) and starts a 4 s lock (423).
#[cfg(unix)]
#[tokio::test]
async fn a_name_with_no_account_still_locks_out_like_an_account_after_a_serve_restart() {
    let model = start_mock_model().await;
    let dir = tempfile::tempdir().unwrap();
    config_with(
        dir.path(),
        &[
            ("tls.enabled", "false"),
            ("models.local_llm.base_url", &format!("\"{}\"", model.base_url())),
            ("models.local_llm.warmup.enabled", "false"),
            ("structured_memory.enabled", "false"),
            ("api.rate_limit.max_requests", "1000"),
        ],
    );
    let (mut child, port, client) = spawn_serve(dir.path()).await;
    for name in ["admin", "zed"] {
        for attempt in 1..=5 {
            let seen = wrong_sign_in(&client, port, name).await;
            assert_eq!(seen, (401, "AUTH_LOGIN_FAILED".into()), "{name} #{attempt}");
        }
    }
    child.kill().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2_500)).await;

    let (mut child, port, client) = spawn_serve(dir.path()).await;
    let mut after = Vec::new();
    for name in ["admin", "zed"] {
        let mut seen = Vec::new();
        for _ in 0..2 {
            seen.push(wrong_sign_in(&client, port, name).await);
        }
        after.push(seen);
    }
    let locked = vec![(401, "AUTH_LOGIN_FAILED".into()), (423, "AUTH_ACCOUNT_LOCKED".into())];
    assert_eq!(after[0], locked, "admin's stored count survives the restart");
    assert_eq!(after[1], after[0], "zed must answer as admin does after the restart");
    child.kill().await.unwrap();
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
    let (mut child, port, client) = spawn_serve(dir.path()).await;
    let url = format!("http://127.0.0.1:{port}/api/status"); // DevSkim: ignore DS162092 DS137138 because this is the owned loopback-only HTTP fixture.
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

#[cfg(unix)]
#[tokio::test]
async fn ctrl_c_and_sigterm_stop_serve_after_its_queued_audit_lines_land() {
    let model = start_mock_model().await;
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let dir = tempfile::tempdir().unwrap();
        config_with(
            dir.path(),
            &[
                ("auth.enabled", "false"),
                ("tls.enabled", "false"),
                ("models.local_llm.base_url", &format!("\"{}\"", model.base_url())),
                ("models.local_llm.warmup.enabled", "false"),
                ("structured_memory.enabled", "false"),
                // The writer may wait 5 s for the lease this test holds.
                ("logging.audit_lease_wait_ms", "5000"),
            ],
        );
        let (mut child, port, client) = spawn_serve(dir.path()).await;
        let base = format!("http://127.0.0.1:{port}"); // DevSkim: ignore DS162092 DS137138 because this is the owned loopback-only HTTP fixture.
                                                       // The request's audit line stays queued while this test holds the lease.
        let lease = hold_audit_lease(dir.path()).await;
        let checks = client.get(format!("{base}/api/agent/checks")).send().await.unwrap();
        assert_eq!(checks.status(), 200);
        // Only this test's owned child receives a signal.
        assert_eq!(unsafe { libc::kill(child.id().unwrap() as i32, signal) }, 0);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            child.try_wait().unwrap().is_none(),
            "signal {signal}: serve exited with an audit line still queued"
        );
        drop(lease);
        let status = tokio::time::timeout(std::time::Duration::from_secs(15), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success(), "signal {signal}: {status:?}");
        let audit = std::fs::read_to_string(dir.path().join("logs/audit.jsonl")).unwrap();
        assert!(
            audit.contains("\"route\":\"/api/agent/checks\""),
            "signal {signal}: {audit}"
        );
    }
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
                "/api/status" | "/api/auth/setup-status" | "/api/auth/login"
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
    let setup_code = std::fs::read_to_string(server.home.join("setup-code"))
        .unwrap()
        .trim()
        .to_string();
    let response = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"username":"admin","password":setup_code}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let bootstrap = cookie(&response);
    let header = response.headers()["set-cookie"].to_str().unwrap();
    assert!(header.contains("; Secure") && header.contains("HttpOnly") && header.contains("SameSite=Strict"));
    let login_body = response.json::<Value>().await.unwrap();
    assert_eq!(login_body["must_change_password"], true);
    // Request CSRF is the process token in the console page; login returns none.
    assert!(login_body.get("csrf_token").is_none(), "{login_body}");
    for path in ["/api/keys", "/api/sessions", "/api/web/allow", "/api/auth/users"] {
        let response = request(&server, &bootstrap, Method::POST, path, json!({})).await;
        assert_eq!(response.status(), 403);
        assert_eq!(
            code(&response.json::<Value>().await.unwrap()),
            "AUTH_PASSWORD_CHANGE_REQUIRED"
        );
    }
    // The bootstrap scope holds for every API client, not only the console: a
    // plain client with the session cookie and no Origin, CSRF token or bearer
    // key may only read whoami, replace the password or log out.
    for path in cgagentharness::server::routes::registered_paths() {
        if !path.starts_with("/api/")
            || matches!(
                path.as_str(),
                "/api/status"
                    | "/api/auth/setup-status"
                    | "/api/auth/login"
                    | "/api/auth/whoami"
                    | "/api/auth/password"
                    | "/api/auth/logout"
            )
        {
            continue;
        }
        let path = path
            .replace("{username}", "admin")
            .replace("{session_id}", &"0".repeat(12))
            .replace("{run_id}", &"0".repeat(32))
            .replace("{job_id}", &"0".repeat(32))
            .replace("{id}", &"0".repeat(64));
        for method in [Method::GET, Method::POST, Method::DELETE] {
            let response = server
                .client
                .request(method.clone(), server.url(&path))
                .header("cookie", &bootstrap)
                .json(&json!({}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 403, "{method} {path}");
            assert_eq!(
                code(&response.json::<Value>().await.unwrap()),
                "AUTH_PASSWORD_CHANGE_REQUIRED",
                "{method} {path}"
            );
        }
    }
    let body = json!({"current_password":setup_code,"password":"replacement-admin-password"});
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
            json!({"current_password":setup_code,"password":"short"})
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
    for (name, role) in [("op", "operator"), ("reader", "auditor")] {
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
                "/api/browser/run",
                json!({"task":"read the page","origins":["https://example.com"]}),
            ),
            (
                Method::POST,
                "/api/browser/jobs",
                json!({"task":"read the page","origins":["https://example.com"]}),
            ),
            (
                Method::GET,
                "/api/browser/jobs/0123456789abcdef0123456789abcdef",
                Value::Null,
            ),
            (
                Method::POST,
                "/api/browser/jobs/0123456789abcdef0123456789abcdef/cancel",
                Value::Null,
            ),
            (
                Method::POST,
                "/api/browser/jobs/0123456789abcdef0123456789abcdef/approve",
                json!({"request":1,"sha256":"ab".repeat(32),"confirm":true,"reason":"fixture"}),
            ),
            (
                Method::POST,
                "/api/browser/jobs/0123456789abcdef0123456789abcdef/deny",
                json!({"request":1}),
            ),
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
    // An auditor chats (local-only), where the retired `audit` role was
    // refused outright (#965); the withheld surfaces are covered in diag_routes.
    assert_eq!(
        request(
            &server,
            &audit,
            Method::POST,
            "/api/chat",
            json!({"message":"say hello"})
        )
        .await
        .status(),
        200
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
    // #894: a held model gate refuses an administrator's browser task at
    // once and names the holder; nothing is queued.
    {
        let browse = json!({"task":"read the page","origins":["https://example.com"]});
        let _tuning = server
            .state
            .generation_gate
            .claim(cgagentharness::server::model_limits::TUNE_OWNER)
            .unwrap();
        let busy = request(&server, &admin, Method::POST, "/api/browser/run", browse.clone()).await;
        assert_eq!(busy.status(), 409);
        let busy = busy.json::<Value>().await.unwrap();
        assert_eq!(busy["detail"]["code"], "BROWSER_BUSY", "{busy}");
        assert!(
            busy["detail"]["message"].as_str().unwrap().contains("/model use"),
            "{busy}"
        );
        // A job is the same checked request: it refuses the same way, synchronously,
        // and records nothing. An unknown or malformed id reads as such. Browser
        // jobs are macOS and Linux only: elsewhere every job start is refused first.
        let busy = request(&server, &admin, Method::POST, "/api/browser/jobs", browse).await;
        assert_eq!(busy.status(), 409);
        let jobs_code = if cfg!(unix) {
            "BROWSER_BUSY"
        } else {
            "BROWSER_JOBS_UNSUPPORTED"
        };
        assert_eq!(busy.json::<Value>().await.unwrap()["detail"]["code"], jobs_code);
        for (method, path, status, code) in [
            (
                Method::GET,
                "/api/browser/jobs/0123456789abcdef0123456789abcdef",
                404,
                "BROWSER_JOB_NOT_FOUND",
            ),
            (
                Method::POST,
                "/api/browser/jobs/0123456789abcdef0123456789abcdef/cancel",
                404,
                "BROWSER_JOB_NOT_FOUND",
            ),
            (Method::GET, "/api/browser/jobs/not-a-job-id", 400, "INVALID_JOB_ID"),
        ] {
            let reply = request(&server, &admin, method, path, Value::Null).await;
            assert_eq!(reply.status(), status, "{path}");
            assert_eq!(reply.json::<Value>().await.unwrap()["detail"]["code"], code, "{path}");
        }
        // #995: an approval names a waiting request of the caller's own job, and
        // says confirm and why. Nothing here has a default.
        let approve = "/api/browser/jobs/0123456789abcdef0123456789abcdef/approve";
        let full = json!({"request":1,"sha256":"ab".repeat(32),"confirm":true,"reason":"the order I asked for"});
        let without = |key: &str| {
            let mut body = full.clone();
            body.as_object_mut().unwrap().remove(key);
            body
        };
        for (body, status, code) in [
            (full.clone(), 404, "BROWSER_JOB_NOT_FOUND"),
            (
                json!({"request":1,"sha256":"ab".repeat(32),"confirm":false,"reason":"x"}),
                422,
                "VALIDATION_ERROR",
            ),
            (
                json!({"request":1,"sha256":"ab".repeat(32),"confirm":true,"reason":"  "}),
                422,
                "VALIDATION_ERROR",
            ),
            (
                json!({"request":1,"sha256":"short","confirm":true,"reason":"x"}),
                422,
                "VALIDATION_ERROR",
            ),
            (without("confirm"), 422, "VALIDATION_ERROR"),
            (without("reason"), 422, "VALIDATION_ERROR"),
            (without("sha256"), 422, "VALIDATION_ERROR"),
        ] {
            let reply = request(&server, &admin, Method::POST, approve, body.clone()).await;
            assert_eq!(reply.status(), status, "{body}");
            assert_eq!(reply.json::<Value>().await.unwrap()["detail"]["code"], code, "{body}");
        }
        let deny = request(
            &server,
            &admin,
            Method::POST,
            "/api/browser/jobs/0123456789abcdef0123456789abcdef/deny",
            json!({"request":1}),
        )
        .await;
        assert_eq!(deny.status(), 404);
        // The shared URL policy refuses before the model gate (still held here,
        // so a check that let a task through would answer BROWSER_BUSY), and a
        // stored report is withheld on every read while the policy covers it.
        let admin_id = server.state.auth.as_ref().unwrap().get_user("admin").unwrap().user_id;
        let job_id = "fedcba9876543210".repeat(2);
        let job_path = format!("/api/browser/jobs/{job_id}");
        let (input, _child_input) = std::sync::mpsc::channel();
        server.state.browser_jobs.insert(
            &job_id,
            &admin_id,
            &["https://example.com".to_string()],
            cgagentharness::server::browser_jobs::CancelHandle {
                input,
                token: Default::default(),
            },
        );
        server.state.browser_jobs.finish(
            &job_id,
            cgagentharness::server::browser_jobs::Outcome::Report(json!({"answer":"stored"})),
        );
        let stored = |blocked: bool| {
            let (server, admin, job_path) = (&server, &admin, &job_path);
            async move {
                let job = request(server, admin, Method::GET, job_path, Value::Null).await;
                assert_eq!(job.status(), 200);
                let job = job.json::<Value>().await.unwrap();
                if blocked {
                    assert!(job.get("result").is_none(), "{job}");
                    assert_eq!(job["error"]["code"], "WEB_POLICY_CHANGED", "{job}");
                } else {
                    assert_eq!(job["result"]["answer"], "stored", "{job}");
                    assert!(job.get("error").is_none(), "{job}");
                }
            }
        };
        let refused = |status: u16, code: &'static str| {
            let (server, admin) = (&server, &admin);
            async move {
                for path in ["/api/browser/run", "/api/browser/jobs"] {
                    let browse = json!({"task":"read the page","origins":["https://example.com"]});
                    let reply = request(server, admin, Method::POST, path, browse).await;
                    let (status, code) = if cfg!(unix) || path == "/api/browser/run" {
                        (status, code)
                    } else {
                        (409, "BROWSER_JOBS_UNSUPPORTED")
                    };
                    assert_eq!(reply.status(), status, "{path}");
                    assert_eq!(reply.json::<Value>().await.unwrap()["detail"]["code"], code, "{path}");
                }
            }
        };
        stored(false).await;
        let block = json!({"urls":["https://example.com"]});
        assert_eq!(
            request(&server, &admin, Method::POST, "/api/web/block", block)
                .await
                .status(),
            200
        );
        refused(403, "BROWSER_ORIGIN_BLOCKED").await;
        stored(true).await;
        let unblock = json!({"url":"https://example.com"});
        assert_eq!(
            request(&server, &admin, Method::POST, "/api/web/unblock", unblock)
                .await
                .status(),
            200
        );
        stored(false).await;
        // A policy file that cannot be read refuses a new task and fails closed on a read.
        let (policy_file, _) = server.state.web.policy_file();
        let aside = policy_file.with_extension("aside");
        std::fs::rename(&policy_file, &aside).unwrap();
        refused(409, "WEB_POLICY_UNAVAILABLE").await;
        stored(true).await;
        std::fs::rename(&aside, &policy_file).unwrap();
        refused(409, "BROWSER_BUSY").await;
        stored(false).await;
        // The same recheck when a child exits: its report is delivered only
        // while the policy still allows every origin the task was granted.
        let exited = |policy_allows: bool| {
            let state = &server.state;
            async move {
                let origins = ["https://example.com".to_string()];
                let report = r#"{"event":"turn"}
{"answer":"fresh"}
"#;
                let delivered =
                    cgagentharness::server::routes::browser::deliver_exited_child(state, &origins, 0, report).await;
                if policy_allows {
                    let body = delivered.unwrap_or_else(|err| panic!("refused: {}", err.code));
                    assert_eq!(body["finished"], true, "{body}");
                    assert_eq!(body["report"]["answer"], "fresh", "{body}");
                } else {
                    assert_eq!(delivered.unwrap_err().code, "WEB_POLICY_CHANGED");
                }
            }
        };
        exited(true).await;
        let block = json!({"urls":["https://example.com"]});
        assert_eq!(
            request(&server, &admin, Method::POST, "/api/web/block", block)
                .await
                .status(),
            200
        );
        exited(false).await;
        let unblock = json!({"url":"https://example.com"});
        assert_eq!(
            request(&server, &admin, Method::POST, "/api/web/unblock", unblock)
                .await
                .status(),
            200
        );
        std::fs::rename(&policy_file, &aside).unwrap();
        exited(false).await;
        std::fs::rename(&aside, &policy_file).unwrap();
        exited(true).await;
    }
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
            json!({"role":"auditor"})
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
    // The request line holds the route template; the account events name the target.
    assert!(
        !events.to_string().contains("account.role_set"),
        "the audit route shows request lines only"
    );
    let log: Vec<Value> = server
        .audit_log()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let changed = |event: &str| {
        log.iter()
            .filter(|v| v["event"] == event && v["actor"] == "admin" && v["target"] == "op")
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(changed("account.user_created").len(), 1);
    assert_eq!(changed("account.user_created")[0]["role"], "operator");
    let disabled: Vec<Value> = changed("account.disabled_set")
        .iter()
        .map(|v| v["disabled"].clone())
        .collect();
    assert_eq!(disabled, [json!(true), json!(false)]);
    assert_eq!(changed("account.role_set").len(), 1);
    assert_eq!(changed("account.role_set")[0]["role"], "auditor");
    // The three refused changes to the last admin left no account event.
    assert!(!log
        .iter()
        .any(|v| v["target"] == "admin" && v["event"] != "account.password_changed"));
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

/// #1123 phase 1: the approve route checks again after the wait, through
/// `POST /api/browser/jobs/{id}/approve` itself. A block rule added while the
/// request waited answers 409 `WEB_POLICY_CHANGED`, sends the child a deny
/// and audits `refused_by_policy`. A wait that ran out answers 409
/// `BROWSER_NOTHING_WAITING` and sends nothing. An allowed, unexpired request
/// is approved, so the fixture can tell an approval from a refusal. The job
/// is a real owner-bound record whose child input is a channel the test
/// reads, so every verdict that would reach the child is seen.
#[tokio::test]
async fn approving_a_held_request_rechecks_policy_and_expiry_after_the_wait() {
    // The loopback-fixture switch skips the policy recheck; with it set this
    // test would pass without exercising it.
    assert!(
        std::env::var_os("CGAH_TEST_PRIVATE_BROWSER_ORIGINS").is_none(),
        "this test must run with the production policy recheck"
    );
    let model = start_mock_model().await;
    let options = ServerOptions::default()
        .with("auth.enabled", "true")
        .with("browser.enabled", "true")
        .with("browser.takeover.enabled", "true")
        .with("api.rate_limit.max_requests", "1000");
    let server = spawn_server(&model.base_url(), options).await;
    let auth = server.state.auth.as_ref().unwrap();
    let password = cgagentharness::common::random_hex(24);
    auth.create_user("orders", &password, "admin").unwrap();
    let owner = auth.get_user("orders").unwrap().user_id;
    let login = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"username":"orders","password":password}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let session = cookie(&login);

    let job_id = "0a1b2c3d4e5f6789".repeat(2);
    let approve = format!("/api/browser/jobs/{job_id}/approve");
    let (input, child) = std::sync::mpsc::channel::<Vec<u8>>();
    server.state.browser_jobs.insert(
        &job_id,
        &owner,
        &["https://example.com:443".to_string()],
        cgagentharness::server::browser_jobs::CancelHandle {
            input,
            token: Default::default(),
        },
    );
    let sha256 = "5e".repeat(32);
    // The child's `waiting` line for request `id`, which may be answered for
    // `wait_sec` seconds.
    let hold = |id: u64, wait_sec: u64| {
        let waiting = json!({
            "event": "waiting",
            "wait_sec": wait_sec,
            "question": {
                "approve": id,
                "method": "POST",
                "url": "https://example.com/api/save",
                "content_type": "application/x-www-form-urlencoded",
                "size": 5,
                "sha256": sha256,
                "preview": "qty=2",
            },
        });
        server.state.browser_jobs.note_line(&job_id, &waiting.to_string());
        let job = server.state.browser_jobs.get(&owner, &job_id).unwrap();
        assert_eq!(job["status"], "waiting", "{job}");
    };
    let answer = |id: u64| json!({"request":id,"sha256":sha256,"confirm":true,"reason":"the order I asked for"});
    let sent = || -> Vec<Value> {
        child
            .try_iter()
            .map(|line| serde_json::from_slice(&line).unwrap())
            .collect()
    };
    let approvals = || -> Vec<Value> {
        server
            .audit_log()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|line| line["event"] == "browser.approval")
            .collect()
    };

    // Control: allowed and unexpired, so it is approved and the child is told so.
    hold(1, 60);
    let reply = request(&server, &session, Method::POST, &approve, answer(1)).await;
    assert_eq!(reply.status(), 200);
    assert_eq!(reply.json::<Value>().await.unwrap()["decision"], "approved");
    assert_eq!(sent(), [json!({"verdict":"approve","id":1})]);
    assert_eq!(approvals().last().unwrap()["decision"], "approved");

    // Policy changed during the wait: the URL is blocked before the owner says yes.
    hold(2, 60);
    let block = json!({"urls":["https://example.com"]});
    assert_eq!(
        request(&server, &session, Method::POST, "/api/web/block", block)
            .await
            .status(),
        200
    );
    let reply = request(&server, &session, Method::POST, &approve, answer(2)).await;
    assert_eq!(reply.status(), 409);
    assert_eq!(
        reply.json::<Value>().await.unwrap()["detail"]["code"],
        "WEB_POLICY_CHANGED"
    );
    assert_eq!(
        sent(),
        [json!({"verdict":"deny","id":2})],
        "the owner's no, never a yes"
    );
    let refused = approvals().last().cloned().unwrap();
    assert_eq!(refused["decision"], "refused_by_policy", "{refused}");
    assert_eq!(refused["origin"], "https://example.com", "{refused}");
    assert!(
        refused["reason"].is_null(),
        "a refused approval records no reason: {refused}"
    );
    // The deny ended that wait: a second try finds nothing and sends nothing.
    let again = request(&server, &session, Method::POST, &approve, answer(2)).await;
    assert_eq!(again.status(), 409);
    assert_eq!(
        again.json::<Value>().await.unwrap()["detail"]["code"],
        "BROWSER_NOTHING_WAITING"
    );
    assert!(sent().is_empty());
    let unblock = json!({"url":"https://example.com"});
    assert_eq!(
        request(&server, &session, Method::POST, "/api/web/unblock", unblock)
            .await
            .status(),
        200
    );

    // The wait ran out: nothing is approved and nothing reaches the child.
    hold(3, 0);
    let audited = approvals().len();
    let reply = request(&server, &session, Method::POST, &approve, answer(3)).await;
    assert_eq!(reply.status(), 409);
    assert_eq!(
        reply.json::<Value>().await.unwrap()["detail"]["code"],
        "BROWSER_NOTHING_WAITING"
    );
    assert!(sent().is_empty(), "an expired wait sends no verdict");
    assert_eq!(approvals().len(), audited, "an expired wait audits no approval");
}

#[tokio::test]
async fn cli_bootstrap_session_can_only_replace_the_password_and_gets_no_machine_key() {
    let model = start_mock_model().await;
    let options = ServerOptions {
        api_key: Some(String::new()),
        ..Default::default()
    }
    .with("auth.enabled", "true")
    .with("tls.enabled", "true")
    .with("api.rate_limit.max_requests", "1000");
    let server = spawn_server(&model.base_url(), options).await;
    let stderr = |out: &std::process::Output| String::from_utf8_lossy(&out.stderr).into_owned();
    // The machine-key store opens only in an owner-only home.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&server.home, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let setup_code = std::fs::read_to_string(server.home.join("setup-code"))
        .unwrap()
        .trim()
        .to_string();
    let admin_id = server.state.auth.as_ref().unwrap().get_user("admin").unwrap().user_id;
    let mint = [
        "mcp-key",
        "create",
        "--owner",
        &admin_id,
        "--label",
        "bootstrap",
        "--confirm",
        "--reason",
        "fixture",
    ];
    // Local file authority still refuses a machine key to an owner that has
    // not replaced its setup code.
    let refused = cli(&server, &mint, "").await;
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("MCP_KEY_OWNER_REFUSED"),
        "{}",
        stderr(&refused)
    );
    assert!(!String::from_utf8_lossy(&refused.stdout).contains("token"));

    let result = cli(
        &server,
        &["account", "--url", &server.base, "login", "admin"],
        &format!("{setup_code}\n"),
    )
    .await;
    assert!(result.status.success(), "{}", stderr(&result));
    let result = cli(&server, &["account", "--url", &server.base, "whoami"], "").await;
    assert!(result.status.success(), "{}", stderr(&result));
    for args in [
        &["web", "--url", &server.base, "status"][..],
        &[
            "web",
            "--url",
            &server.base,
            "allow",
            "https://example.com/reference",
            "--group",
            "cli",
        ][..],
    ] {
        let result = cli(&server, args, "").await;
        assert!(!result.status.success(), "{args:?}");
        assert!(
            stderr(&result).contains("AUTH_PASSWORD_CHANGE_REQUIRED"),
            "{args:?}: {}",
            stderr(&result)
        );
    }
    let result = cli(
        &server,
        &["account", "--url", &server.base, "password"],
        &format!("{setup_code}\nreplacement-cli-password\n"),
    )
    .await;
    assert!(result.status.success(), "{}", stderr(&result));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("replacement-cli-password"));
    let result = cli(&server, &["web", "--url", &server.base, "status"], "").await;
    assert!(result.status.success(), "{}", stderr(&result));
    let minted = cli(&server, &mint, "").await;
    assert!(minted.status.success(), "{}", stderr(&minted));
    let result = cli(&server, &["account", "--url", &server.base, "logout"], "").await;
    assert!(result.status.success());
}

/// #965 schema 3 at boot: a home whose legacy account file still holds the
/// retired `audit` role starts with that account folded into `auditor`,
/// writes exactly one `auth.role_migrated` row naming the user id and both
/// roles (never the username), and holds the account disabled: it signs in as
/// auditor only after an admin re-enables it.
#[tokio::test]
async fn a_retired_role_is_migrated_and_audited_once_at_boot() {
    let model = start_mock_model().await;
    let prepared = prepare_server(&model.base_url(), ServerOptions::default().with("auth.enabled", "true")).await;
    // Minted per run: no literal reaches the password hasher.
    let secret = cgagentharness::common::random_urlsafe(24);
    let hash = cgagentharness::common::authn::hash_password(&secret).unwrap();
    let reader_id = cgagentharness::common::random_hex(16);
    let legacy = json!({"users": {
        "admin": {"password_hash": hash, "created_ts": 123.0, "role": "admin"},
        "retired-reader": {"user_id": reader_id, "password_hash": hash, "created_ts": 123.0, "role": "audit"},
    }, "sessions": {}});
    cgagentharness::common::atomic::write_atomic(
        &prepared.home_dir.join("auth.json"),
        &serde_json::to_vec(&legacy).unwrap(),
        Some(0o600),
    )
    .unwrap();
    // A machine key minted for the account before the upgrade: the fold that
    // disables the account must retire it, as the admin disable route does.
    let home = cgagentharness::common::home::Home::at(prepared.home_dir.clone());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&home.root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let keys = cgagentharness::server::mcp_keys::KeyStore::open(&home, 8).unwrap();
    let owners = [reader_id.clone()];
    let (_, token) = keys
        .mint(
            &reader_id,
            "pre-upgrade",
            cgagentharness::server::mcp_keys::OwnerCheck::EnabledOwners(&owners),
        )
        .unwrap();
    assert!(
        keys.authenticate(&token).is_ok(),
        "the pre-upgrade key works before boot"
    );
    let (router, state) = cgagentharness::server::build_app(prepared.app_opts).await.unwrap();
    let server = start_test_server(prepared.tmp, prepared.home_dir, prepared.api_key, router, state).await;

    let log = server.audit_log();
    let rows: Vec<Value> = log
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| row["event"] == "auth.role_migrated")
        .collect();
    assert_eq!(rows.len(), 1, "exactly one auth.role_migrated row");
    assert_eq!(rows[0]["from"], "audit");
    assert_eq!(rows[0]["to"], "auditor");
    assert_eq!(
        rows[0]["disabled"], true,
        "the row records that the fold disabled the account"
    );
    assert!(
        rows[0]["user_id"].as_str() == Some(reader_id.as_str()),
        "the row names the account by its hex user id"
    );
    assert!(
        keys.authenticate(&token).is_err(),
        "the fold retired the disabled account's machine key"
    );
    assert!(
        !log.contains("retired-reader"),
        "the migration row never names the account"
    );
    // Confirmed in the log, so the store's pending report was acknowledged.
    let auth = server.state.auth.as_ref().unwrap();
    assert!(auth.pending_role_migrations().unwrap().is_empty());

    let refused = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"username": "retired-reader", "password": secret}))
        .send()
        .await
        .unwrap();
    assert_ne!(refused.status(), 200, "a former audit account waits for an admin");
    auth.enable_user("retired-reader").unwrap();
    let session = login(&server, "retired-reader", &secret).await;
    let whoami: Value = request(&server, &session, Method::GET, "/api/auth/whoami", json!({}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(whoami["role"], "auditor", "the migrated account signs in as auditor");
}

/// The gateway authenticates by the key row alone and `auditor` holds no memory
/// read, so a role change to `auditor` retires that owner's machine keys and
/// none is minted for one. A later promotion does not bring a key back.
#[tokio::test]
async fn an_account_made_auditor_loses_its_machine_keys_and_gets_no_new_one() {
    let model = start_mock_model().await;
    let server = spawn_server(&model.base_url(), ServerOptions::default().with("auth.enabled", "true")).await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&server.home, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let auth = server.state.auth.as_ref().unwrap();
    // Minted per run: no literal reaches the password hasher.
    let secret = cgagentharness::common::random_urlsafe(24);
    auth.create_user("ops-admin", &secret, "admin").unwrap();
    auth.create_user("worker", &cgagentharness::common::random_urlsafe(24), "operator")
        .unwrap();
    let worker_id = auth.get_user("worker").unwrap().user_id;
    let admin = login(&server, "ops-admin", &secret).await;
    let mint = [
        "mcp-key",
        "create",
        "--owner",
        &worker_id,
        "--label",
        "fixture",
        "--confirm",
        "--reason",
        "fixture",
    ];
    let minted = cli(&server, &mint, "").await;
    assert!(minted.status.success(), "an operator owner gets a key");
    let token = serde_json::from_slice::<Value>(&minted.stdout).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let home = cgagentharness::common::home::Home::at(server.home.clone());
    let keys = cgagentharness::server::mcp_keys::KeyStore::open(&home, 8).unwrap();
    assert!(keys.authenticate(&token).is_ok(), "the key works for an operator");

    let role = |role: &'static str| {
        request(
            &server,
            &admin,
            Method::POST,
            "/api/auth/users/worker/role",
            json!({"role": role}),
        )
    };
    assert_eq!(role("auditor").await.status(), 200);
    assert!(
        keys.authenticate(&token).is_err(),
        "the role change retired the auditor's machine key"
    );
    let refused = cli(&server, &mint, "").await;
    assert!(!refused.status.success(), "no key is minted for an auditor");
    assert!(String::from_utf8_lossy(&refused.stderr).contains("MCP_KEY_OWNER_REFUSED"));
    assert!(!String::from_utf8_lossy(&refused.stdout).contains("token"));

    assert_eq!(role("operator").await.status(), 200);
    assert!(
        keys.authenticate(&token).is_err(),
        "a promotion does not re-enable a key"
    );
}

/// Work an account already started stops when the account loses the
/// authority to start it: disable, deletion and a demotion out of
/// `run_coding` abort its running coding jobs (audited
/// `account.work_stopped`); a role that keeps `run_coding` keeps them, and
/// another account's jobs are never touched.
#[tokio::test]
async fn an_accounts_running_coding_jobs_stop_when_it_loses_run_coding() {
    let model = start_mock_model().await;
    let server = spawn_server(&model.base_url(), ServerOptions::default().with("auth.enabled", "true")).await;
    let auth = server.state.auth.as_ref().unwrap();
    let secret = cgagentharness::common::random_urlsafe(24);
    auth.create_user("ops-admin", &secret, "admin").unwrap();
    for name in ["worker", "bystander", "leaver", "keeper"] {
        auth.create_user(name, &cgagentharness::common::random_urlsafe(24), "operator")
            .unwrap();
    }
    let id = |name: &str| auth.get_user(name).unwrap().user_id;
    let admin = login(&server, "ops-admin", &secret).await;
    let jobs = &server.state.jobs;
    let start =
        |owner: &str, job: &str| jobs.insert_running(owner, job, "real-repo-run", tokio::spawn(std::future::pending()));
    let status = |owner: &str, job: &str| jobs.get(owner, job).unwrap()["status"].clone();
    let (worker, bystander, leaver, keeper) = (id("worker"), id("bystander"), id("leaver"), id("keeper"));
    start(&worker, "w1");
    start(&bystander, "b1");
    start(&leaver, "l1");
    start(&keeper, "k1");

    let post = |path: &'static str, body: Value| request(&server, &admin, Method::POST, path, body);
    assert_eq!(
        post("/api/auth/users/keeper/role", json!({"role":"admin"}))
            .await
            .status(),
        200
    );
    assert_eq!(
        status(&keeper, "k1"),
        "running",
        "a role that keeps run_coding keeps its job"
    );

    assert_eq!(
        post("/api/auth/users/worker/disabled", json!({"disabled":true}))
            .await
            .status(),
        200
    );
    assert_eq!(status(&worker, "w1"), "cancelled", "disable stops the running job");
    assert_eq!(status(&bystander, "b1"), "running", "another account's job runs on");

    assert_eq!(
        post("/api/auth/users/keeper/role", json!({"role":"auditor"}))
            .await
            .status(),
        200
    );
    assert_eq!(
        status(&keeper, "k1"),
        "cancelled",
        "a demotion out of run_coding stops it"
    );

    let deleted = request(&server, &admin, Method::DELETE, "/api/auth/users/leaver", Value::Null).await;
    assert_eq!(deleted.status(), 200);
    assert_eq!(status(&leaver, "l1"), "cancelled", "deletion stops the running job");
    assert_eq!(status(&bystander, "b1"), "running");

    let audit = server.audit_log();
    let stopped: Vec<Value> = audit
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["event"] == "account.work_stopped")
        .collect();
    assert_eq!(stopped.len(), 3, "one audit line per account whose work stopped");
    assert!(stopped.iter().all(|line| line["coding_jobs"] == 1));
}

/// A synchronous coding run lasts as long as its child, and the account was
/// checked only when the request arrived. Disabling the account mid-run kills
/// the child and answers 401 AUTH_REQUIRED (audited `agent_run_withheld`)
/// instead of waiting for the child and delivering its result.
#[cfg(unix)]
#[tokio::test]
async fn a_sync_coding_run_stops_when_its_account_is_disabled_mid_run() {
    sync_run_stops_when(|auth| auth.disable_user("worker").unwrap()).await;
}

/// An admin password reset leaves the account enabled and permitted but
/// revokes its sign-ins, so the run asked under them stops as `/browse` does.
#[cfg(unix)]
#[tokio::test]
async fn a_sync_coding_run_stops_when_its_password_is_reset_mid_run() {
    sync_run_stops_when(|auth| {
        auth.set_password("worker", &cgagentharness::common::random_urlsafe(24))
            .unwrap()
    })
    .await;
}

/// Starts `POST /api/agent/run` for operator `worker` against a child that
/// sleeps 20 seconds, applies `change` once the child runs, and checks the
/// run stopped early, its child is gone and the stop was audited.
#[cfg(unix)]
async fn sync_run_stops_when(change: impl FnOnce(&cgagentharness::common::auth_store::AuthManager)) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (exe, pidfile) = (dir.path().join("slow-shim"), dir.path().join("child.pid"));
    std::fs::write(
        &exe,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nsleep 20\nprintf '{{\"status\":\"finished\"}}\\n'\n",
            pidfile.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let model = start_mock_model().await;
    let opts = ServerOptions {
        shim_exe: Some(exe),
        ..Default::default()
    }
    .with("auth.enabled", "true");
    let server = spawn_server(&model.base_url(), opts).await;
    let auth = server.state.auth.as_ref().unwrap();
    let secret = cgagentharness::common::random_urlsafe(24);
    auth.create_user("worker", &secret, "operator").unwrap();
    let session = login(&server, "worker", &secret).await;
    let body = json!({"instruction": "fix the parser", "branch": "claude/parser-fix", "commit_message": "fix: parser", "reason": "triage"});

    let started = std::time::Instant::now();
    let run = request(&server, &session, Method::POST, "/api/agent/run", body);
    let disable = async {
        for _ in 0..200 {
            if pidfile.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(pidfile.exists(), "the child started");
        change(auth);
    };
    let (response, ()) = tokio::join!(run, disable);
    assert_eq!(response.status(), 401);
    let reply: Value = response.json().await.unwrap();
    assert_eq!(reply["detail"]["code"], "AUTH_REQUIRED");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the run stopped without waiting for its child"
    );

    let pid = std::fs::read_to_string(&pidfile).unwrap().trim().to_string();
    let mut alive = true;
    for _ in 0..100 {
        alive = std::process::Command::new("kill")
            .args(["-0", &pid])
            .status()
            .is_ok_and(|status| status.success());
        if !alive {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!alive, "the child was killed with the run");
    let audit = server.audit_log();
    assert!(audit
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|line| line["event"] == "agent_run_withheld" && line["outcome"] == "stopped"));
}

/// A key an `auditor` account still holds at boot (a fold acknowledged by an
/// earlier build, or a role set while the key store was unwritable) is retired
/// before the gateway can serve it.
#[tokio::test]
async fn boot_retires_a_machine_key_an_auditor_account_still_holds() {
    let model = start_mock_model().await;
    let prepared = prepare_server(&model.base_url(), ServerOptions::default().with("auth.enabled", "true")).await;
    // Minted per run: no literal reaches the password hasher.
    let hash = cgagentharness::common::authn::hash_password(&cgagentharness::common::random_urlsafe(24)).unwrap();
    let (watcher_id, worker_id) = (
        cgagentharness::common::random_hex(16),
        cgagentharness::common::random_hex(16),
    );
    let legacy = json!({"users": {
        "admin": {"password_hash": hash, "created_ts": 123.0, "role": "admin"},
        "watcher": {"user_id": watcher_id, "password_hash": hash, "created_ts": 123.0, "role": "auditor"},
        "worker": {"user_id": worker_id, "password_hash": hash, "created_ts": 123.0, "role": "operator"},
    }, "sessions": {}});
    cgagentharness::common::atomic::write_atomic(
        &prepared.home_dir.join("auth.json"),
        &serde_json::to_vec(&legacy).unwrap(),
        Some(0o600),
    )
    .unwrap();
    let home = cgagentharness::common::home::Home::at(prepared.home_dir.clone());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&home.root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let keys = cgagentharness::server::mcp_keys::KeyStore::open(&home, 8).unwrap();
    let owners = [watcher_id.clone(), worker_id.clone()];
    let mint = |owner: &str| {
        keys.mint(
            owner,
            "before-boot",
            cgagentharness::server::mcp_keys::OwnerCheck::EnabledOwners(&owners),
        )
        .unwrap()
        .1
    };
    let (watcher, worker) = (mint(&watcher_id), mint(&worker_id));
    assert!(keys.authenticate(&watcher).is_ok(), "the key works before boot");
    let (router, state) = cgagentharness::server::build_app(prepared.app_opts).await.unwrap();
    let _server = start_test_server(prepared.tmp, prepared.home_dir, prepared.api_key, router, state).await;
    assert!(keys.authenticate(&watcher).is_err(), "boot retired the auditor's key");
    assert!(keys.authenticate(&worker).is_ok(), "an operator's key is untouched");
}

/// A browser task's report reaches only an account unchanged since it asked:
/// a password reset or a disable during the run withholds it (401
/// AUTH_REQUIRED, audited `browser.withheld`), as chat withholds a reply.
#[tokio::test]
async fn a_browser_report_is_withheld_once_the_asking_account_changes() {
    use cgagentharness::server::routes::browser::deliver_exited_child_as;
    let model = start_mock_model().await;
    let server = spawn_server(&model.base_url(), ServerOptions::default().with("auth.enabled", "true")).await;
    let auth = server.state.auth.as_ref().unwrap();
    auth.create_user("ops", &cgagentharness::common::random_hex(24), "admin")
        .unwrap();
    let origins = ["https://example.com".to_string()];
    let report = "{\"answer\":\"fresh\"}\n";
    let user = auth.get_user("ops").unwrap();
    let seen = auth.sign_in_revocations("ops");
    let body = deliver_exited_child_as(&server.state, user.clone(), seen, &origins, report)
        .await
        .unwrap_or_else(|err| panic!("refused: {}", err.code));
    assert_eq!(body["report"]["answer"], "fresh", "{body}");
    auth.set_password("ops", &cgagentharness::common::random_hex(24))
        .unwrap();
    let refused = deliver_exited_child_as(&server.state, user.clone(), seen, &origins, report)
        .await
        .unwrap_err();
    assert_eq!((refused.status.as_u16(), refused.code.as_str()), (401, "AUTH_REQUIRED"));
    assert!(server.audit_log().contains("\"browser.withheld\""));
    let seen = auth.sign_in_revocations("ops");
    auth.disable_user("ops").unwrap();
    let refused = deliver_exited_child_as(&server.state, user, seen, &origins, report)
        .await
        .unwrap_err();
    assert_eq!(refused.code, "AUTH_REQUIRED");
}

/// A browser task's account is read before `prepare`'s waits, not after. A
/// tuned model Ollama reports not loaded is loaded under the gate first; an
/// admin demoted while that load runs gets no task: 401 AUTH_REQUIRED,
/// audited `browser.withheld`, and the model load is the last thing that ran.
/// Before the fix the revocation count was read after the load, so it already
/// included the demotion and every later check passed (#1057's promise).
#[tokio::test]
async fn a_browser_task_asked_by_an_account_demoted_while_the_model_loads_never_starts() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let (arrived, release) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    let loaded = Arc::new(AtomicBool::new(false));
    let (arrived_tx, release_rx, loaded_ps, loaded_gen) =
        (arrived.clone(), release.clone(), loaded.clone(), loaded.clone());
    let app = common::ollama_router()
        .route(
            "/api/ps",
            axum::routing::get(move || {
                let rows = if loaded_ps.load(Ordering::SeqCst) {
                    json!([{"name": "mock-model", "model": "mock-model", "context_length": 32768}])
                } else {
                    json!([])
                };
                async move { axum::Json(json!({"models": rows})) }
            }),
        )
        .route(
            "/api/generate",
            axum::routing::post(move || {
                let (arrived_tx, release_rx, loaded_gen) = (arrived_tx.clone(), release_rx.clone(), loaded_gen.clone());
                async move {
                    arrived_tx.notify_one();
                    release_rx.notified().await;
                    loaded_gen.store(true, Ordering::SeqCst);
                    axum::Json(json!({"done": true}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture binds only an ephemeral loopback port.
    let addr = listener.local_addr().unwrap();
    let mock = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let server = spawn_server(
        &format!("http://{addr}/v1"), // DevSkim: ignore DS137138 because this synthetic model uses no credentials and binds only to loopback.
        ServerOptions::default()
            .with("auth.enabled", "true")
            .with("browser.enabled", "true")
            .with("models.local_llm.model", "mock-model"),
    )
    .await;
    server
        .state
        .set_tuning(Some(cgagentharness::server::model_limits::Tuning {
            model: "mock-model".into(),
            window: 32768,
            state: "tuned".into(),
            tools_off: false,
            max_tokens: 4096,
            compact_prompt_tokens: 20_000,
            web: None,
            timeout_sec: None,
            synthesis_seconds: None,
            research_seconds: None,
            planner_timeout_sec: None,
            speed: None,
            notes: Vec::new(),
        }));
    let auth = server.state.auth.as_ref().unwrap();
    let secret = cgagentharness::common::random_urlsafe(24);
    auth.create_user("ops", &secret, "admin").unwrap();
    auth.create_user("keeper", &cgagentharness::common::random_urlsafe(24), "admin")
        .unwrap();
    let session = login(&server, "ops", &secret).await;
    let body = json!({"task": "read the page title", "origins": ["https://example.com"]});
    let ask = request(&server, &session, Method::POST, "/api/browser/run", body);
    let demote = async {
        tokio::time::timeout(std::time::Duration::from_secs(10), arrived.notified())
            .await
            .expect("the task loaded the model");
        auth.set_role("ops", "operator").unwrap();
        release.notify_one();
    };
    let (response, ()) = tokio::join!(ask, demote);
    assert_eq!(response.status(), 401, "the demoted account's task is refused");
    let reply: Value = response.json().await.unwrap();
    assert_eq!(reply["detail"]["code"], "AUTH_REQUIRED");
    assert!(server.audit_log().contains("\"browser.withheld\""));
    assert!(
        !server.audit_log().contains("\"browser.run\""),
        "no browser child ran for the demoted account"
    );
    mock.abort();
}
