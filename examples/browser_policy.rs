//! Run the browser session authority against a fixed script.
//! This does not launch Chrome. It prints the decisions the supervisor must enforce.

use cgagentharness::common::browser::{
    AssertionRecord, AssertionStatus, Decision, ElementDraft, ExpectationSource, InputKind, Session, VerifyReport,
};
use serde_json::json;

fn show(label: &str, decision: &Decision) {
    match decision {
        Decision::Allow => println!("{label}: allow"),
        Decision::Handoff { reason } => println!("{label}: handoff: {reason}"),
        Decision::Refuse { reason } => println!("{label}: refuse: {reason}"),
    }
}

fn main() {
    let mut session = Session::general(["https://example.com", "https://docs.example.com"]).expect("grants");
    let seen = session
        .mint(
            vec!["All".into(), "Open".into()],
            vec![
                ElementDraft {
                    role: "tab".into(),
                    name: "Open".into(),
                    href: None,
                    input: InputKind::None,
                    snapshot_ref: None,
                },
                ElementDraft {
                    role: "link".into(),
                    name: "Docs".into(),
                    href: Some("https://docs.example.com/guide".into()),
                    input: InputKind::None,
                    snapshot_ref: None,
                },
                ElementDraft {
                    role: "textbox".into(),
                    name: "Filter".into(),
                    href: None,
                    input: InputKind::Text,
                    snapshot_ref: None,
                },
            ],
        )
        .expect("mint");
    let tab = seen.elements.iter().find(|el| el.role == "tab").unwrap();
    let link = seen.elements.iter().find(|el| el.role == "link").unwrap();
    let filter = seen.elements.iter().find(|el| el.name == "Filter").unwrap();

    show(
        "navigate example",
        &session.decide(&json!({"action":"navigate","url":"https://example.com/start"})),
    );
    show(
        "open tab",
        &session.decide(&json!({"action":"select_tab","tab":"Open"})),
    );
    show(
        "type filter",
        &session.decide(&json!({"action":"type","ref_id": filter.ref_id, "text":"open"})),
    );
    show(
        "follow docs",
        &session.decide(&json!({"action":"follow_link","ref_id": link.ref_id})),
    );
    show("blocked cdn", &session.authorize_url("https://cdn.example.com/app.js"));
    show(
        "click tab",
        &session.decide(&json!({"action":"click","ref_id": tab.ref_id})),
    );

    session.begin_takeover();
    show(
        "type during takeover",
        &session.decide(&json!({"action":"type","ref_id": filter.ref_id, "text":"x"})),
    );
    session.end_takeover().unwrap();

    let pr = Session::pr_verify("http://127.0.0.1:49152/", &[]).unwrap();
    show("pr owned", &pr.authorize_url("http://127.0.0.1:49152/health"));
    show("pr public", &pr.authorize_url("https://example.com/"));
    show(
        "forged grant",
        &session.decide(&json!({"action":"grant","origin":"https://evil.test"})),
    );

    let exploratory = VerifyReport {
        scenario: "reload keeps the fixture message".into(),
        expectation: ExpectationSource::Exploratory {
            text: "the model said it passed".into(),
        },
        revision: "261efd9af709c9214e0a2a3e260ee4ee4ed375cd".into(),
        assertions: vec![AssertionRecord {
            name: "message present after reload".into(),
            status: AssertionStatus::Passed,
        }],
        containment: "not a containment claim",
    };
    println!("exploratory passed={}", exploratory.passed());
    let trusted = VerifyReport {
        expectation: ExpectationSource::TrustedTest {
            id: "fixture-reload".into(),
        },
        ..exploratory
    };
    println!("trusted passed={}", trusted.passed());
}
