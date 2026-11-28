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
