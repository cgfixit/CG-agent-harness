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
