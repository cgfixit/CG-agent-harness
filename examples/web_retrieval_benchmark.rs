//! Offline retrieval timing: `cargo run --example web_retrieval_benchmark`.
//! Synthetic cached pages; no network, model, or persistent index.
use cgagentharness::{
    common::sha256_hex,
    server::{
        web_index::{retrieve, retrieve_many, Passage},
        web_policy::{Policy, Rule},
        web_search::Page,
    },
};
use serde_json::json;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let policy = Policy {
        version: 1,
        rules: vec![Rule::new("https://docs.example/*", "docs", &[])?],
        revision: "a".repeat(64),
    };
    let pages: Vec<_> = (0..64)
        .map(|i| {
            let text = format!(
                "# Guide {i}\n{}\nWIDGET_RETRY_COUNT configures connection retries. Password replacement revokes sessions.",
                format!("Document {i}: café account isolation and transport configuration. ").repeat(64)
            );
            Page {
                url: format!("https://docs.example/guide/{i}"),
                title: format!("Guide {i}"),
                content_hash: sha256_hex(&text),
                chars: text.chars().count(),
                bytes: text.len(),
                text,
                links: vec![],
                status: 200,
                content_type: "text/plain".into(),
                transfer_bytes: 0,
                fetched_at: 1_800_000_000.0,
                extraction_version: 1,
                policy_revision: policy.revision.clone(),
                etag: None,
                last_modified: None,
            }
        })
        .collect();
    let queries = [
        "account isolation",
        "WIDGET_RETRY_COUNT",
        "password replacement",
        "connection retries",
    ];
    let repeated = || -> anyhow::Result<Vec<Vec<Passage>>> {
        Ok(queries
            .iter()
            .map(|query| retrieve(&pages, &policy, query, Some("docs"), 8))
            .collect::<Result<_, _>>()?)
    };
    let expected = serde_json::to_value(repeated()?)?;
    assert_eq!(
        serde_json::to_value(retrieve_many(&pages, &policy, &queries, Some("docs"), 8)?)?,
        expected
    );
    let mut repeated_us = Vec::new();
    let mut batched_us = Vec::new();
    for sample in 0..9 {
        // Alternate order to avoid consistently warming one path for the other.
        for batch in if sample % 2 == 0 { [false, true] } else { [true, false] } {
            let start = Instant::now();
            let result = if batch {
                retrieve_many(&pages, &policy, &queries, Some("docs"), 8)?
            } else {
                repeated()?
            };
            let elapsed = start.elapsed().as_micros();
            if batch { &mut batched_us } else { &mut repeated_us }.push(elapsed);
            assert_eq!(serde_json::to_value(result)?, expected);
        }
    }
    repeated_us.sort_unstable();
    batched_us.sort_unstable();
    println!(
        "{}",
        json!({"pages":pages.len(),"text_bytes":pages.iter().map(|p|p.text.len()).sum::<usize>(),
            "queries":queries,"repeated_us":repeated_us,"batched_us":batched_us,
            "repeated_median_us":repeated_us[4],"batched_median_us":batched_us[4],
            "debug_assertions":cfg!(debug_assertions),
            "scope":"offline synthetic retrieval; excludes network and model latency"})
    );
    Ok(())
}
