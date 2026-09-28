//! `scripts/smoke-ollama.sh` greps the compact `GET /api/tools` body for a
//! literal wired count. That count is `HARNESS_SURFACES.len()` when every
//! catalog path is registered (`list_wired_tools`, same function the route
//! uses for the `wired` field). A hardcoded literal drifts; this test fails
//! until the script matches the catalog.

use std::path::Path;

fn smoke_wired_literal(script: &str) -> usize {
    let marker = "\"wired\":";
    assert_eq!(
        script.matches(marker).count(),
        1,
        "smoke-ollama.sh must contain exactly one \"wired\": grep"
    );
    let start = script.find(marker).expect("marker present");
    let digits: String = script[start + marker.len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("characters after {marker} must be a decimal wired count, got {digits:?}"))
}

#[test]
fn smoke_ollama_wired_grep_matches_catalog() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/smoke-ollama.sh");
    let script = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    let parsed = smoke_wired_literal(&script);

    let catalog = cgagentharness::server::views::HARNESS_SURFACES.len();
    let registered = cgagentharness::server::routes::registered_paths();
    let report = cgagentharness::server::views::list_wired_tools(&registered);
    let wired = usize::try_from(report["wired"].as_u64().expect("wired is a JSON number")).expect("wired fits usize");
    assert_eq!(
        wired, catalog,
        "GET /api/tools wired count diverged from HARNESS_SURFACES"
    );
    assert_eq!(report["total"], catalog);

    let body = serde_json::to_string(&report).expect("compact tools JSON");
    let needle = format!("\"wired\":{wired}");
    assert!(
        body.contains(&needle),
        "compact /api/tools JSON must contain {needle} so the smoke grep can match it"
    );
    assert_eq!(
        parsed, wired,
        "scripts/smoke-ollama.sh greps \"wired\":{parsed}; GET /api/tools reports \"wired\":{wired}"
    );
}
