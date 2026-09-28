//! `scripts/smoke-ollama.sh` greps the compact `GET /api/tools` body for a
//! wired count. That count is `HARNESS_SURFACES.len()` when every catalog
//! path is registered (`list_wired_tools`, same function the route uses for
//! the `wired` field). The grep keeps a `[,}]` terminator so `"wired":690`
//! cannot satisfy a catalog of 69.

use std::path::Path;

/// Basic-regex character class that must follow the count in the smoke grep.
/// Compact JSON emits `"wired":N,` or `"wired":N}`.
const WIRED_GREP_TERMINATOR: &str = "[,}]";

fn smoke_wired_count(script: &str) -> (usize, String) {
    let marker = "\"wired\":";
    assert_eq!(
        script.matches(marker).count(),
        1,
        "smoke-ollama.sh must contain exactly one \"wired\": grep"
    );
    let start = script.find(marker).expect("marker present");
    let rest = &script[start + marker.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let parsed = digits
        .parse()
        .unwrap_or_else(|_| panic!("characters after {marker} must be a decimal wired count, got {digits:?}"));
    let suffix: String = rest[digits.len()..]
        .chars()
        .take(WIRED_GREP_TERMINATOR.chars().count())
        .collect();
    (parsed, suffix)
}

#[test]
fn smoke_ollama_wired_grep_matches_catalog() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/smoke-ollama.sh");
    let script = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    let (parsed, suffix) = smoke_wired_count(&script);
    assert_eq!(
        suffix, WIRED_GREP_TERMINATOR,
        "the character class after the wired digits must stay {WIRED_GREP_TERMINATOR} so a longer count cannot match"
    );

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
    let after_value = body
        .find(&needle)
        .map(|idx| body[idx + needle.len()..].chars().next())
        .expect("compact /api/tools JSON must contain the wired field");
    assert!(
        matches!(after_value, Some(',' | '}')),
        "compact JSON after {needle} must be ',' or '}}' so the smoke class {WIRED_GREP_TERMINATOR} matches; got {after_value:?}"
    );
    assert_eq!(
        parsed, wired,
        "scripts/smoke-ollama.sh greps \"wired\":{parsed}; GET /api/tools reports \"wired\":{wired}"
    );
}
