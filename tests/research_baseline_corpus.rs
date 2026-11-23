//! #772 baseline corpus shape. While the corpus is a proposal no label exists;
//! once frozen, a sha256 pin replaces the second test, as for d7/corpus.json.
use serde_json::Value;
use std::collections::BTreeSet;

const CORPUS: &str = include_str!("fixtures/research-baseline/corpus.json");

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
