//! Direct W1 acceptance: `cargo run --example web_extraction_probe`.
//! `--serve` opens a disposable authenticated console for manual Computer Use.
//! Fixture DNS pins are programmatic only; this never reads an operator home.
use axum::{
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use cgagentharness::{
    common::{audit::Audit, config::AppConfig, home::Home},
    server::{
        build_app,
        web_search::{extract, WebTool},
        AppOptions,
    },
};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Notify;

#[derive(Clone)]
struct Fixture {
    attributes: Arc<String>,
    started: Arc<Notify>,
}
async fn page(State(fixture): State<Fixture>, request: Request) -> Response {
    let body = match request.uri().path() {
        "/hostile" => "<div>".repeat(262_144 / 5),
        "/maximum" => "<div/>".repeat(1_048_576 / 6),
        "/attributes" => {
            fixture.started.notify_one();
            fixture.attributes.to_string()
        }
        "/slow" => {
            tokio::time::sleep(Duration::from_secs(60)).await;
            "<p>slow fixture</p>".into()
        }
        "/robots.txt" => return ([("content-type", "text/plain")], "User-agent: *\nAllow: /\n").into_response(),
        "/oversize" => "x".repeat(1_048_577),
        "/redirect" => return (axum::http::StatusCode::FOUND, [("location", "/normal")]).into_response(),
        "/compressed" => return ([("content-type", "text/html"), ("content-encoding", "gzip")], "refused").into_response(),
        _ => "<title>Normal fixture</title><h2>Recovery verified</h2><p>usable slot &amp; Unicode 🦀</p><div hidden>HIDDEN_MARKER</div><pre> x\n  y</pre><a href='/next'>Next</a>".into(),
    };
    ([("content-type", "text/html")], body).into_response()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let serve = std::env::args().any(|a| a == "--serve");
    let dir = tempfile::tempdir()?;
    let attributes = format!(
        "<p {}>attribute fixture</p>",
        (0..100_000).map(|i| format!("a{i} ")).collect::<String>()
    );
    assert!(attributes.len() < 1_048_576);
    let fixture = Fixture {
        attributes: Arc::new(attributes),
        started: Arc::new(Notify::new()),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?; // DevSkim: ignore DS162092 - standalone acceptance fixture; loopback-only ephemeral listener, never a production entrypoint.
    let address = listener.local_addr()?;
    let app = Router::new().fallback(get(page)).with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = format!("http://fixture.invalid:{}/", address.port()); // DevSkim: ignore DS137138 - synthetic fixture hostname is pinned to the owned loopback listener; no internet traffic or real credentials.
    if serve {
        let home = Home::at(dir.path().join("home"));
        home.ensure_layout()?;
        let cfg = AppConfig::from_str(
            "auth: {enabled: true}\ntls: {enabled: false}\nweb: {concurrency: 1, request_seconds: 30, response_bytes: 1048576}\nstructured_memory: {enabled: false}\n",
            &home.config_path(),
        )?;
        let mut options = AppOptions::new(home);
        options.config = Some(cfg);
        options.web_test_resolve = Some(("fixture.invalid".into(), address));
        let (router, state) = build_app(options).await?;
        state
            .auth
            .as_ref()
            .unwrap()
            .set_password("admin", "W1-fixture-only-2026!")?;
        state.web.allow(&format!("{root}*"), true)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?; // DevSkim: ignore DS162092 - standalone acceptance fixture; loopback-only ephemeral listener, never a production entrypoint.
        println!("CONSOLE http://{}", listener.local_addr()?); // DevSkim: ignore DS137138 - disposable loopback-only console with synthetic login; production TLS defaults remain true.
        println!("FIXTURE {root} (normal, hostile, maximum, slow)");
        println!("Synthetic login: admin / W1-fixture-only-2026!");
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async { tokio::signal::ctrl_c().await.unwrap() })
        .await?;
        server.abort();
        return Ok(());
    }
    let cfg = AppConfig::from_str(
        "web: {concurrency: 1, request_seconds: 1, response_bytes: 1048576}",
        Path::new("config.yaml"),
    )?;
    let mut web = WebTool::new(dir.path(), &cfg)?;
    web.test_resolve = Some(("fixture.invalid".into(), address));
    let web = Arc::new(web);
    web.allow(&format!("{root}*"), true)?;
    let audit = Arc::new(Audit::new(dir.path().join("audit.jsonl"), &cfg));
    let base = url::Url::parse(&root)?;
    for cap in [262_144, 1_048_576] {
        for unit in [
            "<div>",
            "<div/>",
            "<div></ignored>",
            "<span>",
            "<b>",
            "<i>x",
            "<template>",
        ] {
            let started = Instant::now();
            let body = unit.repeat(cap / unit.len());
            let failure = extract(&body, "text/html", &base, web.limits.html_parser_handles, || Ok(())).unwrap_err();
            assert_eq!(failure.code, "WEB_HTML_COMPLEXITY");
            assert!(started.elapsed() < Duration::from_secs(1));
            println!(
                "shape {unit:?}, {cap} byte cap: {} in {:?}",
                failure.code,
                started.elapsed()
            );
        }
    }
    let normal = format!("{root}normal");
    for (path, expected) in [
        ("hostile", "WEB_HTML_COMPLEXITY"),
        ("maximum", "WEB_HTML_COMPLEXITY"),
        ("attributes", "WEB_TIMEOUT"),
        ("oversize", "WEB_TOO_LARGE"),
        ("redirect", "WEB_REDIRECT_REFUSED"),
        ("compressed", "WEB_ENCODING_REFUSED"),
    ] {
        let started = Instant::now();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            started.elapsed()
        });
        let bad = web
            .fetch(&format!("{root}{path}"), true, &audit, "local")
            .await
            .unwrap_err();
        assert_eq!(bad.code, expected);
        println!(
            "HTTP {path}: {} after {:?}; async timer {:?}",
            bad.code,
            started.elapsed(),
            timer.await?
        );
        let started = Instant::now();
        let good = web.fetch(&normal, true, &audit, "local").await?;
        assert!(good["text"].as_str().unwrap().contains("usable slot & Unicode 🦀"));
        assert!(!good["text"].as_str().unwrap().contains("HIDDEN_MARKER"));
        assert_eq!(good["links"][0], format!("{root}next"));
        println!("HTTP recovery: {:?}", started.elapsed());
    }
    // Drop a live caller during CPU-heavy, shallow tokenization. This must
    // stop the worker too; the one-slot follow-up exposes detached CPU work.
    fixture.started.notified().await; // Consume the preceding attributes request notification.
    let work = {
        let web = web.clone();
        let audit = audit.clone();
        let target = format!("{root}attributes");
        tokio::spawn(async move { web.fetch(&target, true, &audit, "local").await })
    };
    fixture.started.notified().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    assert!(!work.is_finished());
    work.abort();
    assert!(work.await.unwrap_err().is_cancelled());
    let good = web.fetch(&normal, true, &audit, "local").await?;
    assert_eq!(good["title"], "Normal fixture");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "cancelled CPU worker held the fetch slot"
    );
    println!("aborted caller; recovered fetch slot in {:?}", started.elapsed());
    let denied = web
        .fetch("https://unpermitted.invalid/", true, &audit, "local")
        .await
        .unwrap_err();
    assert_eq!(denied.code, "WEB_HOST_DENIED");
    println!("unpermitted URL: {}", denied.code);
    server.abort();
    println!("W1 DIRECT PROBE PASSED");
    Ok(())
}
