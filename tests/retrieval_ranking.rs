//! Golden ranking order for the two request-local BM25 rankers:
//! `passage_index::retrieve_passages` (attachments and notes corpus) and
//! `web_index::retrieve_many` (authorized web cache).
//!
//! These lock today's observable ranking (order, rounded score, heading)
//! so a refactor or a tuning change shows up as a reviewed diff here
//! instead of a silent shift in what local chat or web research injects.

use cgagentharness::common::sha256_hex;
use cgagentharness::server::passage_index::{chunk_text, retrieve_passages, PassageDoc, SourceKind};
use cgagentharness::server::web_index::{retrieve, retrieve_many, Passage};
use cgagentharness::server::web_policy::{Policy, Rule};
use cgagentharness::server::web_search::Page;

fn docs(sources: &[(&str, &str)]) -> Vec<PassageDoc> {
    sources
        .iter()
        .flat_map(|(source_id, text)| {
            chunk_text(text)
                .into_iter()
                .map(|(start, end, heading, chunk)| PassageDoc {
                    source: SourceKind::Attachment,
                    source_id: (*source_id).into(),
                    heading,
                    text: chunk,
                    sha256: sha256_hex(text),
                    start,
                    end,
                    score: 0.0,
                })
        })
        .collect()
}

fn passage_rows(hits: &[PassageDoc]) -> Vec<String> {
    hits.iter()
        .map(|h| format!("{}:{}:{} {:.3} [{}]", h.source_id, h.start, h.end, h.score, h.heading))
        .collect()
}

fn corpus() -> Vec<PassageDoc> {
    let filler = "Unrelated maintenance narrative about seasonal inventory and staffing. ".repeat(20);
    let duplicate = "# Retries\nWIDGET_RETRY_COUNT sets how many connection retries the client attempts.\n";
    docs(&[
        (
            "alpha",
            &format!(
                "# Retries\nConnection retries back off exponentially. WIDGET_RETRY_COUNT defaults to three.\n{filler}"
            ),
        ),
        ("beta", duplicate),
        ("gamma", duplicate),
        (
            "delta",
            "# Counters\nWIDGET_RETRY_COUNTER is a different identifier that tracks connection retries seen.\n",
        ),
        (
            "epsilon",
            &format!("# Transport\n{filler}\n# Retries\nRetries on the transport reuse the connection pool.\n"),
        ),
        (
            "zeta",
            "# Retries\nconnection retries connection retries connection retries for the zeta service.\n",
        ),
    ])
}

#[test]
fn passage_ranking_order_is_locked() {
    let docs = corpus();
    let hits = retrieve_passages(&docs, "connection retries", 8).unwrap();
    // beta and gamma are byte-identical: only one survives. The first pass
    // takes one passage per source; alpha's second chunk comes back on the
    // second pass, matched through its carried heading.
    assert_eq!(
        passage_rows(&hits),
        vec![
            "zeta:0:89 6.125 [Retries]",
            "beta:0:83 5.386 [Retries]",
            "delta:0:95 4.888 [Counters]",
            "alpha:0:1194 3.732 [Retries]",
            "epsilon:1195:1495 1.270 [Retries]",
            "alpha:1194:1511 0.423 [Retries]",
        ]
    );
}

#[test]
fn passage_identifier_query_keeps_whole_identifier_matches_only() {
    let docs = corpus();
    let hits = retrieve_passages(&docs, "WIDGET_RETRY_COUNT", 8).unwrap();
    // delta's WIDGET_RETRY_COUNTER is a different identifier and is filtered.
    assert_eq!(
        passage_rows(&hits),
        vec!["beta:0:83 11.077 [Retries]", "alpha:0:1194 6.005 [Retries]",]
    );
}

#[test]
fn passage_results_are_capped_at_sixteen() {
    let texts: Vec<String> = (0..24)
        .map(|i| {
            format!(
                "# Note {i}\nShared ranking token appears here with distinct marker m{i} q{} r{}.\n",
                i * 7,
                i * 13
            )
        })
        .collect();
    let ids: Vec<String> = (0..24).map(|i| format!("s{i:02}")).collect();
    let sources: Vec<(&str, &str)> = ids
        .iter()
        .map(String::as_str)
        .zip(texts.iter().map(String::as_str))
        .collect();
    let hits = retrieve_passages(&docs(&sources), "ranking token", 100).unwrap();
    assert_eq!(hits.len(), 16);
}

fn policy() -> Policy {
    Policy {
        version: 1,
        rules: vec![Rule::new("https://docs.example/*", "docs", &[]).unwrap()],
        revision: "a".repeat(64),
    }
}

fn page(path: &str, title: &str, text: &str, policy: &Policy) -> Page {
    Page {
        url: format!("https://docs.example/{path}"),
        title: title.into(),
        content_hash: sha256_hex(text),
        chars: text.chars().count(),
        bytes: text.len(),
        text: text.into(),
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
}

fn web_rows(hits: &[Passage]) -> Vec<String> {
    hits.iter()
        .map(|h| format!("{} {}:{} {:.3} [{}]", h.url, h.start, h.end, h.score, h.heading))
        .collect()
}

fn web_pages(policy: &Policy) -> Vec<Page> {
    let filler = "Unrelated maintenance narrative about seasonal inventory and staffing. ".repeat(20);
    let duplicate = "# Retries\nWIDGET_RETRY_COUNT sets how many connection retries the client attempts.\n";
    vec![
        page(
            "alpha",
            "Retry guide",
            &format!(
                "# Retries\nConnection retries back off exponentially. WIDGET_RETRY_COUNT defaults to three.\n{filler}"
            ),
            policy,
        ),
        page("beta", "Mirror one", duplicate, policy),
        page("gamma", "Mirror two", duplicate, policy),
        page(
            "delta",
            "Counters",
            "# Counters\nWIDGET_RETRY_COUNTER is a different identifier that tracks connection retries seen.\n",
            policy,
        ),
        page(
            "zeta",
            "Connection retries",
            "# Retries\nconnection retries connection retries connection retries for the zeta service.\n",
            policy,
        ),
        page(
            "outside",
            "Unrelated",
            "# Garden\nTomatoes need staking in late spring.\n",
            policy,
        ),
    ]
}

#[test]
fn web_ranking_order_is_locked() {
    let policy = policy();
    let pages = web_pages(&policy);
    let queries = ["connection retries", "WIDGET_RETRY_COUNT", "retry guide"];
    let ranked = retrieve_many(&pages, &policy, &queries, Some("docs"), 8).unwrap();
    let rows: Vec<Vec<String>> = ranked.iter().map(|hits| web_rows(hits)).collect();
    assert_eq!(
        rows,
        vec![
            vec![
                "https://docs.example/zeta 0:89 17.870 [Retries]",
                "https://docs.example/gamma 0:83 5.692 [Retries]",
                "https://docs.example/delta 0:95 5.101 [Counters]",
                "https://docs.example/alpha 0:1194 4.887 [Retries]",
                "https://docs.example/alpha 1194:1511 1.725 [Retries]",
            ],
            vec![
                "https://docs.example/gamma 0:83 9.844 [Retries]",
                "https://docs.example/alpha 0:1194 6.223 [Retries]",
            ],
            vec![
                "https://docs.example/alpha 0:1194 9.778 [Retries]",
                "https://docs.example/zeta 0:89 2.441 [Retries]",
                "https://docs.example/gamma 0:83 1.168 [Retries]",
                "https://docs.example/delta 0:95 0.620 [Counters]",
                "https://docs.example/alpha 1194:1511 9.429 [Retries]",
            ],
        ]
    );
    for (query, batched) in queries.iter().zip(&ranked) {
        let single = retrieve(&pages, &policy, query, Some("docs"), 8).unwrap();
        assert_eq!(web_rows(&single), web_rows(batched), "{query}");
    }
}

#[test]
fn chunk_heading_labels_match_for_multiple_headings() {
    let policy = policy();
    let text = format!(
        "# Overview\n{}\n# Install\n{}",
        "overview words ".repeat(20),
        "install steps ".repeat(150)
    );
    let passage_chunks = chunk_text(&text);
    let web_chunks = cgagentharness::server::web_index::passages(&page("headings", "Guide", &text, &policy), &policy);
    let passage_rows: Vec<_> = passage_chunks
        .iter()
        .map(|(start, end, heading, chunk)| (*start, *end, heading.as_str(), chunk.as_str()))
        .collect();
    let web_rows: Vec<_> = web_chunks
        .iter()
        .map(|chunk| (chunk.start, chunk.end, chunk.heading.as_str(), chunk.text.as_str()))
        .collect();
    assert_eq!(passage_rows, web_rows);
    assert_eq!(passage_rows[0].2, "Overview");
    assert!(passage_rows[0].3.contains("# Install"));
    assert!(passage_rows[1..].iter().all(|row| row.2 == "Install"));
}

#[test]
fn multi_heading_ranking_scores_are_locked() {
    let policy = policy();
    let text = format!(
        "# Overview\n{}\n# Install\n{}",
        "overview words ".repeat(20),
        "install steps ".repeat(150)
    );
    let passage_hits = retrieve_passages(&docs(&[("headings", &text)]), "install steps", 8).unwrap();
    let web_hits = retrieve(
        &[page("headings", "Guide", &text, &policy)],
        &policy,
        "install steps",
        Some("docs"),
        8,
    )
    .unwrap();
    assert_eq!(
        passage_rows(&passage_hits),
        ["headings:1198:2394 4.341 [Install]", "headings:0:1198 3.721 [Overview]"]
    );
    assert_eq!(
        web_rows(&web_hits),
        [
            "https://docs.example/headings 1198:2394 4.341 [Install]",
            "https://docs.example/headings 0:1198 3.721 [Overview]",
        ]
    );
}
