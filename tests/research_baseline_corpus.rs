//! #772 baseline corpus shape. While the corpus is a proposal no label exists;
//! once frozen, a sha256 pin replaces the second test, as for d7/corpus.json.
//! The provenance matrix names, per read path, the test that locks it.
use serde_json::Value;
use std::collections::BTreeSet;

const CORPUS: &str = include_str!("fixtures/research-baseline/corpus.json");
const PROVENANCE: &str = include_str!("fixtures/research-baseline/provenance.json");

fn corpus() -> Value {
    serde_json::from_str(CORPUS).expect("the corpus is valid JSON")
}

fn strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .expect("an array")
        .iter()
        .map(|v| v.as_str().expect("a string"))
        .collect()
}

/// Every `.rs` file under `src/`, as text.
fn sources() -> Vec<(std::path::PathBuf, String)> {
    let mut found = Vec::new();
    let mut dirs = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("src is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source file");
                found.push((path, text));
            }
        }
    }
    found
}

/// The run manifest names only settings the shipped config has, audit events
/// the code writes, and fields some source writes as a JSON key: a renamed
/// key, event or field fails here instead of leaving a run recording nothing.
#[test]
fn the_run_manifest_names_only_settings_events_and_fields_that_exist() {
    let manifest: Value = serde_json::from_str(include_str!("fixtures/research-baseline/manifest.json"))
        .expect("the manifest is valid JSON");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let shipped: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&std::fs::read_to_string(root.join("assets/config.default.yaml")).unwrap())
            .expect("the shipped config is YAML");
    for key in strings(&manifest["settings"]) {
        let found = key.split('.').try_fold(&shipped, |node, part| node.get(part));
        assert!(found.is_some(), "settings: {key} is not in assets/config.default.yaml");
    }
    for section in ["identity", "permissions"] {
        for entry in manifest[section].as_array().expect(section) {
            for field in ["field", "how"] {
                assert!(
                    entry[field].as_str().is_some_and(|text| !text.is_empty()),
                    "{section}: {entry}"
                );
            }
        }
    }
    assert_eq!(
        manifest["repetitions"]["min"],
        corpus()["model"]["min_repetitions"],
        "the manifest and the corpus declare the same repetitions"
    );
    // Undecided values say what they wait on, rather than look decided.
    for (section, value) in [("repetitions", "variability_rule"), ("thresholds", "values")] {
        if manifest[section][value].is_null() {
            assert!(
                manifest[section]["pending_on"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "{section}.{value} is unset and names nothing it waits on"
            );
        }
    }
    let sources = sources();
    let mut names = BTreeSet::new();
    for metric in manifest["metrics"].as_array().expect("metrics") {
        let name = metric["name"].as_str().expect("name");
        assert!(names.insert(name), "{name} is listed twice");
        let key = |text: &str| format!("\"{text}\":");
        match metric["source"].as_str().expect("source") {
            "audit" => {
                let event = metric["event"].as_str().expect("event");
                let field = metric["field"].as_str().expect("field");
                let writers: Vec<_> = sources
                    .iter()
                    .filter(|(_, text)| {
                        text.contains(&format!("\"event\":\"{event}\""))
                            || text.contains(&format!("\"event\": \"{event}\""))
                    })
                    .collect();
                assert!(!writers.is_empty(), "{name}: no source writes audit event {event}");
                // A turn's fields can be built apart from the line that logs
                // them (`chat_web_turn` takes `web_ledger::audit_fields`).
                assert!(
                    sources.iter().any(|(_, text)| text.contains(&key(field))),
                    "{name}: no source writes the field {field}"
                );
            }
            "reply" => {
                let field = metric["field"].as_str().expect("field");
                assert!(
                    sources.iter().any(|(_, text)| text.contains(&key(field))),
                    "{name}: no source writes the reply field {field}"
                );
            }
            "driver" | "label" => assert!(
                metric["how"].as_str().is_some_and(|text| !text.is_empty()),
                "{name}: says how it is measured"
            ),
            other => panic!("{name}: unknown source {other}"),
        }
    }
}

/// Each replay capture names a corpus question (or is the one synthetic format
/// example), and every served document's sha256 is of its exact bytes, so a
/// capture cannot change without changing its hash.
#[test]
fn every_replay_capture_matches_its_hashes() {
    use sha2::{Digest, Sha256};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/research-baseline/replay");
    let corpus = corpus();
    let ids: BTreeSet<_> = corpus["questions"]
        .as_array()
        .expect("questions")
        .iter()
        .map(|q| q["id"].as_str().expect("id").to_string())
        .collect();
    let web_url = |raw: &str| url::Url::parse(raw).is_ok_and(|url| matches!(url.scheme(), "http" | "https"));
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).expect("replay/ exists") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        seen += 1;
        let file = path.file_name().unwrap().to_string_lossy().to_string();
        let capture: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).expect("valid JSON");
        let question = capture["question"].as_str().expect("question");
        if capture["synthetic"] == true {
            assert_eq!(
                question, "SYNTHETIC",
                "{file}: a synthetic capture names no corpus question"
            );
            assert!(!ids.contains(question), "{file}");
        } else {
            assert!(ids.contains(question), "{file}: {question} is not a corpus question");
            assert_eq!(file, format!("{question}.json"), "{file}: named after its question");
        }
        for listing in capture["listings"].as_array().expect("listings") {
            for field in ["provider", "query", "captured_at"] {
                assert!(listing[field].as_str().is_some(), "{file}: listing {field}");
            }
            for row in listing["results"].as_array().expect("results") {
                assert!(web_url(row["url"].as_str().unwrap_or("")), "{file}: listed URL {row}");
            }
        }
        for document in capture["documents"].as_array().expect("documents") {
            let url = document["url"].as_str().unwrap_or("");
            assert!(web_url(url), "{file}: document URL {url}");
            let bytes = match (document["body"].as_str(), document["body_file"].as_str()) {
                (Some(body), None) => body.as_bytes().to_vec(),
                (None, Some(name)) => {
                    assert!(!name.contains(".."), "{file}: {name} stays in replay/");
                    std::fs::read(dir.join(name)).unwrap_or_else(|_| panic!("{file}: {name} is missing"))
                }
                _ => panic!("{file}: {url} has exactly one of body or body_file"),
            };
            let actual: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(
                document["sha256"].as_str(),
                Some(actual.as_str()),
                "{file}: {url} sha256"
            );
        }
    }
    assert!(seen > 0, "replay/ holds at least the synthetic format example");
}

#[test]
fn the_corpus_is_six_investigations_and_six_forecasts_covering_every_required_case() {
    let corpus = corpus();
    let questions = corpus["questions"].as_array().expect("questions");
    assert_eq!(questions.len(), 12);
    for kind in ["investigation", "forecast"] {
        assert_eq!(questions.iter().filter(|q| q["kind"] == kind).count(), 6, "{kind}");
    }
    let ids: BTreeSet<_> = questions.iter().map(|q| q["id"].as_str().expect("id")).collect();
    assert_eq!(ids.len(), 12, "ids are unique");
    let required: BTreeSet<_> = strings(&corpus["required_coverage"]).into_iter().collect();
    let covered: BTreeSet<_> = questions.iter().flat_map(|q| strings(&q["covers"])).collect();
    assert_eq!(covered, required, "every required case is covered and none is invented");
    for q in questions {
        let needs_fixture = q["fixture_needed"].as_bool().expect("fixture_needed");
        assert_eq!(
            needs_fixture,
            q.get("fixture").is_some(),
            "{}: a fixture is described iff needed",
            q["id"]
        );
    }
}

#[test]
fn a_proposal_carries_no_label_and_no_approval() {
    let corpus = corpus();
    assert!(corpus["status"].as_str().expect("status").starts_with("proposal"));
    assert_eq!(corpus["model"]["context"], 32768);
    for q in corpus["questions"].as_array().expect("questions") {
        assert_eq!(q["label_status"], "unlabeled", "{}", q["id"]);
        assert!(
            q["label"].is_null(),
            "{}: labels come from an independent labeler",
            q["id"]
        );
    }
}

#[test]
fn every_provenance_row_names_a_test_that_exists_or_what_it_waits_on() {
    let matrix: Value = serde_json::from_str(PROVENANCE).expect("the matrix is valid JSON");
    let rows = matrix["rows"].as_array().expect("rows");
    let ids: BTreeSet<_> = rows.iter().map(|row| row["id"].as_str().expect("id")).collect();
    assert_eq!(ids.len(), rows.len(), "ids are unique");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for row in rows {
        let id = row["id"].as_str().expect("id");
        for field in ["path", "expected"] {
            assert!(
                row[field].as_str().is_some_and(|text| !text.is_empty()),
                "{id}: {field}"
            );
        }
        let locked_by = strings(&row["locked_by"]);
        assert_ne!(
            locked_by.is_empty(),
            row["pending_on"].as_str().is_none(),
            "{id}: a row is locked by a test, or says what it waits on, not both"
        );
        for reference in locked_by {
            let (file, name) = reference.split_once("::").expect("file::test");
            let source = std::fs::read_to_string(root.join(file)).unwrap_or_default();
            assert!(
                source.contains(&format!("fn {name}(")),
                "{id}: {reference} does not exist"
            );
        }
    }
}
