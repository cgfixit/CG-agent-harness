//! Exercise the actual seed, without the protocol fixture's budget overrides.
use std::path::Path;

use cgagentharness::common::config::AppConfig;

fn shipped() -> AppConfig {
    AppConfig::from_str(AppConfig::embedded_default(), Path::new("config.yaml")).unwrap()
}

#[tokio::test]
async fn shipped_local_model_settings_are_nested_and_effective() {
    let cfg = shipped();
    assert!(cfg.get("local_llm").is_none());
    assert!(cfg.get("models.local_llm").is_some());
    let backend = cgagentharness::llm::backend::resolve_local_backend(&cfg).await.unwrap();
    assert_eq!(backend.reasoning_effort.as_deref(), Some("none"));
    assert!(cfg.flag_is_true("models.local_llm.auto_tune"));
    assert!(cfg.flag_is_true("models.local_llm.warmup.enabled"));
    assert_eq!(cfg.f64_or("models.local_llm.timeout_sec", 720.0), 1800.0);
    assert_eq!(
        cgagentharness::server::compaction::reply_reservation(&backend, 4096),
        4096
    );
}

#[test]
fn shipped_web_and_workspace_settings_validate_for_a_fresh_home() {
    let cfg = shipped();
    let web = cgagentharness::server::web_search::Limits::load(&cfg).unwrap();
    assert_eq!(web.chat_tool_calls, 30);
    let root = tempfile::tempdir().unwrap();
    let home = cgagentharness::common::home::Home::at(root.path().join("home"));
    home.ensure_layout().unwrap();
    let agentic = cgagentharness::agentic::config::load_agentic_config(&cfg, &home.root).unwrap();
    assert_eq!(
        agentic.deepagent.workspace_root,
        home.root.join("data/agentic/workspaces")
    );
    assert!(agentic.deepagent.workspace_root.is_dir());
    assert!(!agentic.enabled);
    assert!(!agentic.deepagent.enabled);
    assert!(!agentic.deepagent.allow_git_write_tools);
}

#[test]
fn shipped_tool_hop_budget_validates_and_stays_under_the_answer_budget() {
    let cfg = shipped();
    let key = "models.local_llm.tool_max_tokens";
    assert!(cfg.get(key).is_some(), "the seed documents the key");
    let tool =
        cgagentharness::llm::openai_chat::parse_tool_max_tokens(cfg.get(key).and_then(|value| value.as_u64()), true)
            .unwrap();
    assert_eq!(tool, cgagentharness::llm::openai_chat::DEFAULT_TOOL_MAX_TOKENS);
    assert!(tool < cfg.u64_or("models.local_llm.max_tokens", 4096));
}

#[test]
fn shipped_tuning_values_are_preserved() {
    let cfg = shipped();
    assert_eq!(cgagentharness::server::compaction::summary_max_tokens(&cfg), 2048);
    let limits = cgagentharness::server::structured_memory::Limits::from_config(&cfg);
    assert_eq!(limits.max_proposals_per_owner, 50);
    assert_eq!(
        cfg.get("structured_memory.suggestion_mode").unwrap().as_str(),
        Some("both")
    );
    assert_eq!(cfg.bool_opt("chat.compact_follow_window"), Some(false));
    assert_eq!(cfg.u64_or("chat.compact_keep_messages", 8), 10);
    assert_eq!(cfg.u64_or("models.local_llm.max_tokens", 4096), 4096);
}

#[test]
fn configured_regexes_match_normal_whitespace() {
    let cfg = shipped();
    let patterns = cfg.str_list("policy.prompt_filter.banned_patterns");
    let scanner = cgagentharness::common::injection::Scanner::from_patterns(patterns.iter().map(String::as_str));
    assert!(!scanner.scan("execute command").is_empty());
    let pattern = cfg
        .str_list("policy.privacy.redact_secrets_like")
        .into_iter()
        .find(|pattern| pattern.starts_with("Bearer"))
        .unwrap();
    assert!(regex::Regex::new(&pattern)
        .unwrap()
        .is_match("Bearer harmless_review_canary_123"));
    let redactors = cgagentharness::common::audit::Redactors::from_config(&cfg);
    assert_eq!(
        redactors.redact("Bearer harmless_review_canary_123"),
        "[REDACTED_SECRET]"
    );
}
