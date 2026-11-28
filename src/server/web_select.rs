//! #772 phase 4 trial settings: which turns are offered web tools
//! (`web.tool_offer`, moved here from #941) and typed evidence selection
//! (`web.typed_selection`). Both ship as before: every eligible turn is offered
//! tools, and listings carry no candidate ids.
//!
//! Typed selection gives each URL a turn's search listings return a
//! server-issued candidate id. The model answers a listing with `web_select`:
//! `read` one candidate, `skip` one, or `insufficient_signal`, which ends web
//! use for the turn so it answers with what it has. Only the server resolves
//! an id; an unknown, stale or invented one dispatches nothing.
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

use super::web_policy::error;
use crate::common::errors::{HarnessError, Result};

/// `web.tool_offer`, restart-only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub enum ToolOffer {
    /// Every turn the web mode allows is offered web tools (shipped).
    #[default]
    Always,
    /// A typed chat turn is offered them only when it could use them
    /// (`could_use_web`); `/loop` and goal turns always are.
    WhenAsked,
}

impl std::str::FromStr for ToolOffer {
    type Err = HarnessError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "always" => Ok(Self::Always),
            "when_asked" => Ok(Self::WhenAsked),
            _ => Err(HarnessError::config("web.tool_offer must be always or when_asked")),
        }
    }
}

/// Whether a typed chat `message` could use web tools under `when_asked`:
/// it names an http(s) URL, asks for an outside source in words, or asks a
/// question. Mechanical by design (#941): a request phrased without any of
/// these is answered without tools under this setting.
pub fn could_use_web(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("http://")
        || lower.contains("https://")
        || message.contains('?')
        || super::chat_web::asks_for_outside_source(message)
}

/// The `web_select` tool offered beside a listing when typed selection is on.
pub fn select_tool() -> Value {
    json!({"type":"function","function":{"name":"web_select","description":"Choose from this turn's web_search listings by the candidate id on each listed row. decision read reads that one candidate's page; skip records that a candidate is not worth reading; insufficient_signal means no listed candidate can answer, which ends web use this turn: then answer with what you have and say what is missing. Ids from another turn, or ones no listing gave, read nothing.","parameters":{"type":"object","properties":{"decision":{"type":"string","enum":["read","skip","insufficient_signal"]},"candidate":{"type":"string","maxLength":16}},"required":["decision"],"additionalProperties":false}}})
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Read,
    Skip,
    InsufficientSignal,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectArgs {
    pub decision: Decision,
    #[serde(default)]
    pub candidate: Option<String>,
}

impl SelectArgs {
    /// Structure only: `read` and `skip` name one candidate,
    /// `insufficient_signal` names none.
    pub fn parse(args: &str) -> Result<Self> {
        let parsed: Self =
            serde_json::from_str(args).map_err(|_| error("WEB_TOOL_ARGUMENTS", "invalid selection arguments"))?;
        match (parsed.decision, &parsed.candidate) {
            (Decision::InsufficientSignal, None) | (Decision::Read | Decision::Skip, Some(_)) => Ok(parsed),
            _ => Err(error(
                "WEB_TOOL_ARGUMENTS",
                "read and skip name one candidate; insufficient_signal names none",
            )),
        }
    }
}

/// The candidate ids one turn's listings gave, each resolving to the
/// canonical URL of the row it was put on.
#[derive(Debug, Default)]
pub struct Candidates {
    urls: BTreeMap<String, String>,
}

impl Candidates {
    /// Put a fresh id on each row of `listing` whose URL is canonical, and
    /// remember what it names. Ids run c1, c2, ... across the turn.
    pub fn assign(&mut self, listing: &mut Value) {
        let Some(rows) = listing["results"].as_array_mut() else {
            return;
        };
        for row in rows {
            let Some(url) = row["url"]
                .as_str()
                .and_then(|raw| super::web_policy::canonical_url(raw).ok())
            else {
                continue;
            };
            let id = format!("c{}", self.urls.len() + 1);
            row["candidate"] = json!(id);
            self.urls.insert(id, url.to_string());
        }
    }

    /// The URL a candidate id names, or `WEB_CANDIDATE_UNKNOWN`.
    pub fn resolve(&self, id: &str) -> Result<&str> {
        self.urls.get(id).map(String::as_str).ok_or_else(|| {
            error(
                "WEB_CANDIDATE_UNKNOWN",
                "no listing this turn gave that candidate id; nothing was read. Use an id from this turn's web_search results",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_url_a_question_or_an_outside_source_request_could_use_web() {
        for message in [
            "Reply with exactly this single word: Juniper.",
            "17 + 25",
            "Summarise our last exchange.",
        ] {
            assert!(!could_use_web(message), "{message}");
        }
        for message in [
            "What changed in the 2020 guidance?",
            "Read HTTPS://example.com/a",
            "Search the web for the retraction notice.",
        ] {
            assert!(could_use_web(message), "{message}");
        }
    }

    #[test]
    fn tool_offer_parses_only_its_two_values() {
        assert_eq!("always".parse::<ToolOffer>().unwrap(), ToolOffer::Always);
        assert_eq!("when_asked".parse::<ToolOffer>().unwrap(), ToolOffer::WhenAsked);
        for bad in ["Always", "never", ""] {
            assert!(bad.parse::<ToolOffer>().is_err(), "{bad}");
        }
    }

    #[test]
    fn selection_arguments_name_a_candidate_exactly_when_the_decision_needs_one() {
        assert_eq!(
            SelectArgs::parse(r#"{"decision":"read","candidate":"c1"}"#)
                .unwrap()
                .decision,
            Decision::Read
        );
        assert!(SelectArgs::parse(r#"{"decision":"insufficient_signal"}"#).is_ok());
        for bad in [
            r#"{"decision":"read"}"#,
            r#"{"decision":"skip"}"#,
            r#"{"decision":"insufficient_signal","candidate":"c1"}"#,
            r#"{"decision":"fetch","candidate":"c1"}"#,
            r#"{"decision":"read","candidate":"c1","url":"https://example.com/"}"#,
        ] {
            assert_eq!(SelectArgs::parse(bad).unwrap_err().code, "WEB_TOOL_ARGUMENTS", "{bad}");
        }
    }

    #[test]
    fn only_ids_a_listing_gave_resolve_and_each_names_its_own_row() {
        let mut candidates = Candidates::default();
        let mut first =
            json!({"results":[{"url":"https://a.example/x"},{"url":"not a url"},{"url":"HTTPS://B.example/y"}]});
        let mut second = json!({"results":[{"url":"https://c.example/z"}]});
        candidates.assign(&mut first);
        candidates.assign(&mut second);
        assert_eq!(first["results"][0]["candidate"], "c1");
        assert!(
            first["results"][1].get("candidate").is_none(),
            "an unreadable row gets no id"
        );
        assert_eq!(first["results"][2]["candidate"], "c2");
        assert_eq!(second["results"][0]["candidate"], "c3");
        assert_eq!(candidates.resolve("c2").unwrap(), "https://b.example/y");
        for unknown in ["c4", "c0", "C1", "", "https://a.example/x"] {
            assert_eq!(
                candidates.resolve(unknown).unwrap_err().code,
                "WEB_CANDIDATE_UNKNOWN",
                "{unknown}"
            );
        }
    }
}
