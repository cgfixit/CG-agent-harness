//! Phase 7 structured-memory evaluation.
//! `cargo run --locked --example structured_memory_eval -- OUTPUT.json`
//!
//! Fixture-model JSON only. No cloud LLM, no gate flips, no silent fact apply.
use cgagentharness::common::config::AppConfig;
use cgagentharness::server::prompts::{assemble_memory_sections, MemoryBudget, MAX_MEMORY_CHARS};
use cgagentharness::server::structured_memory::{
    format_selected_facts, EpisodeDraft, ProposalDraft, RecalledFact, StructuredMemoryStore,
};
use cgagentharness::server::structured_memory_consolidate::{
    bind_candidates, drafts_from_bound, parse_consolidator_output,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const CORPUS_JSON: &str = include_str!("../tests/fixtures/structured_memory/phase7/corpus.json");

fn cfg(dir: &Path) -> AppConfig {
    AppConfig::from_str(AppConfig::embedded_default(), &dir.join("config.yaml")).expect("embedded config")
}

fn open_store(dir: &Path) -> StructuredMemoryStore {
    StructuredMemoryStore::open(&dir.join("structured.sqlite3"), &cfg(dir)).expect("open store")
}

fn substitute(model: &Value, episode_id: &str, facts: &BTreeMap<String, String>) -> String {
    let mut text = model.to_string();
    text = text.replace("{{episode_id}}", episode_id);
    text = text.replace("{{foreign_id}}", &cgagentharness::common::random_hex(16));
    for (label, id) in facts {
        text = text.replace(&format!("{{{{fact:{label}}}}}"), id);
    }
    text
}

fn stage(store: &StructuredMemoryStore, owner: &str, summary: &str) -> String {
    let episode = store
        .stage_episode(
            owner,
            EpisodeDraft {
                model_id: "fixture-phase7",
                outcome: "completed",
                user_chars: summary.chars().count(),
                assistant_chars: 8,
                sensitivity: "normal",
            },
        )
        .expect("stage");
    store
        .set_episode_summary(owner, &episode.id, summary, "phase7 fixture summary")
        .expect("summary");
    episode.id
}

fn seed_facts(store: &StructuredMemoryStore, seeds: &[Value], default_owner: &str) -> BTreeMap<String, String> {
    let mut ids = BTreeMap::new();
    for seed in seeds {
        let owner = seed["owner"]
            .as_str()
            .filter(|o| !o.is_empty())
            .unwrap_or(default_owner);
        let fact = store
            .add_fact(
                owner,
                seed["content"].as_str().unwrap(),
                seed["category"].as_str().unwrap_or(""),
                "phase7 seed",
            )
            .expect("seed");
        if seed["active"] == false {
            store
                .deactivate_fact(owner, &fact.id, fact.revision, "phase7 deactivate stale")
                .expect("deactivate");
        }
        ids.insert(seed["label"].as_str().unwrap().to_string(), fact.id);
    }
    ids
}

fn main() -> anyhow::Result<()> {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: structured_memory_eval OUTPUT.json"),
    );
    let corpus: Value = serde_json::from_str(CORPUS_JSON)?;
    let dir = tempfile::tempdir()?;
    let store = open_store(dir.path());
    let limits = store.limits();
    let mut retained = 0usize;
    let mut supported_retained = 0usize;
    let mut unsupported_retained = 0usize;
    let mut sensitive_candidates = 0usize;
    let mut sensitive_retained = 0usize;
    let mut silent = 0usize;
    for case in corpus["consolidation_cases"].as_array().unwrap() {
        let owner = case["owner"].as_str().unwrap();
        let before = store.list_facts(owner)?.len();
        let seeds = case["seed_facts"].as_array().cloned().unwrap_or_default();
        let labeled = seed_facts(&store, &seeds, owner);
        let episode_id = stage(&store, owner, case["episode_summary"].as_str().unwrap());
        let raw = substitute(&case["fixture_model"], &episode_id, &labeled);
        let parsed = parse_consolidator_output(&raw, limits.max_consolidation_candidates)?;
        let facts = store.list_facts(owner)?;
        let (bound, _) = bind_candidates(
            &parsed,
            std::slice::from_ref(&episode_id),
            &facts,
            limits.max_fact_chars,
            limits.max_category_chars,
        );
        if case["sensitive"] == true {
            sensitive_candidates += parsed.candidates.len();
        }
        let mut kept = 0usize;
        for draft in drafts_from_bound(&bound) {
            if store.create_proposal(owner, draft).is_ok() {
                kept += 1;
            }
        }
        retained += kept;
        if kept > 0 && case["supported"] == true {
            supported_retained += kept;
        }
        if kept > 0 && case["supported"] == false {
            unsupported_retained += kept;
        }
        if case["sensitive"] == true {
            sensitive_retained += kept;
        }
        if store.list_facts(owner)?.len() != before + seeds.len() {
            silent += 1;
        }
    }

    let recall_dir = tempfile::tempdir()?;
    let recall_store = open_store(recall_dir.path());
    let mut useful = Vec::new();
    let mut stale = Vec::new();
    for query in corpus["recall_queries"].as_array().unwrap() {
        let owner = query["owner"].as_str().unwrap();
        let labeled = seed_facts(&recall_store, query["seed_facts"].as_array().unwrap(), owner);
        let hits = recall_store.search_facts_fts(
            owner,
            query["query"].as_str().unwrap(),
            query["top_k"].as_u64().unwrap() as usize,
        )?;
        let hit_ids: std::collections::BTreeSet<_> = hits.iter().map(|h| h.fact.id.as_str()).collect();
        let relevant: Vec<_> = query["relevant"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| labeled[l.as_str().unwrap()].as_str())
            .collect();
        let forbidden: Vec<_> = query["forbidden"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| labeled[l.as_str().unwrap()].as_str())
            .collect();
        useful.push(if relevant.is_empty() {
            1.0
        } else {
            relevant.iter().filter(|id| hit_ids.contains(*id)).count() as f64 / relevant.len() as f64
        });
        stale.push(forbidden.iter().filter(|id| hit_ids.contains(*id)).count() as f64 / hits.len().max(1) as f64);
    }

    let budget = &corpus["prompt_budget"];
    let pinned = budget["pinned_char"]
        .as_str()
        .unwrap()
        .repeat(budget["pinned_repeat"].as_u64().unwrap() as usize);
    let body = budget["fact_char"]
        .as_str()
        .unwrap()
        .repeat(budget["fact_repeat"].as_u64().unwrap() as usize);
    let budget_dir = tempfile::tempdir()?;
    let budget_store = open_store(budget_dir.path());
    let mut recalled = Vec::new();
    for idx in 0..budget["fact_count"].as_u64().unwrap() {
        let fact = budget_store.add_fact("user_alice", &format!("{body}-{idx}"), "pref", "phase7 budget")?;
        recalled.push(RecalledFact {
            id: fact.id,
            revision: fact.revision,
            category: fact.category,
            content: fact.content,
            source: "selected",
        });
    }
    let assembled = assemble_memory_sections(
        &pinned,
        &format_selected_facts(&recalled),
        MemoryBudget::from_limits(1_500, 1_500),
    );

    let prune_text = AppConfig::embedded_default().replace(
        "max_episodes_per_owner: 128",
        &format!(
            "max_episodes_per_owner: {}",
            corpus["pruning"]["max_episodes_per_owner"]
        ),
    );
    let prune_dir = tempfile::tempdir()?;
    let prune_cfg = AppConfig::from_str(&prune_text, &prune_dir.path().join("config.yaml"))?;
    let prune_store = StructuredMemoryStore::open(&prune_dir.path().join("structured.sqlite3"), &prune_cfg)?;
    let owner = corpus["pruning"]["owner"].as_str().unwrap();
    let first = prune_store.stage_episode(
        owner,
        EpisodeDraft {
            model_id: "fixture-phase7",
            outcome: "completed",
            user_chars: 1,
            assistant_chars: 1,
            sensitivity: "normal",
        },
    )?;
    let _second = prune_store.stage_episode(
        owner,
        EpisodeDraft {
            model_id: "fixture-phase7",
            outcome: "completed",
            user_chars: 2,
            assistant_chars: 2,
            sensitivity: "normal",
        },
    )?;
    prune_store.create_proposal(
        owner,
        ProposalDraft {
            action: "add",
            content: Some("Keep the referenced episode"),
            category: Some("pref"),
            target_fact_id: None,
            expected_revision: None,
            expected_digest: None,
            source_episode_ids: std::slice::from_ref(&first.id),
        },
    )?;
    let _third = prune_store.stage_episode(
        owner,
        EpisodeDraft {
            model_id: "fixture-phase7",
            outcome: "completed",
            user_chars: 3,
            assistant_chars: 3,
            sensitivity: "normal",
        },
    )?;
    let listed = prune_store.list_episodes(owner)?;
    let preserved = listed.iter().any(|e| e.id == first.id);

    let precision = if retained == 0 {
        1.0
    } else {
        supported_retained as f64 / retained as f64
    };
    let report = json!({
        "name": corpus["name"],
        "mode": "deterministic-fixture-model",
        "cloud_egress": false,
        "silent_fact_apply": silent != 0,
        "metrics": {
            "candidate_precision": precision,
            "unsupported_hallucinated_rate": if retained == 0 { 0.0 } else { unsupported_retained as f64 / retained as f64 },
            "sensitive_content_retention_rate": if sensitive_candidates == 0 { 0.0 } else { sensitive_retained as f64 / sensitive_candidates as f64 },
            "useful_recall_at_k": useful.iter().sum::<f64>() / useful.len() as f64,
            "false_stale_conflicting_recall": stale.iter().sum::<f64>() / stale.len() as f64,
            "prompt_body_chars": assembled.combined.chars().count(),
            "prompt_body_chars_max": MAX_MEMORY_CHARS,
            "pruned_episode_count": listed.len(),
            "pending_refs_preserved": preserved,
            "retained_proposals": retained
        },
        "thresholds": corpus["thresholds"],
        "documented_only": corpus["documented_only"],
        "re_run": [
            "cargo test --locked --test structured_memory_phase7 -- --nocapture",
            "cargo run --locked --example structured_memory_eval -- /tmp/phase7-report.json"
        ]
    });
    std::fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
