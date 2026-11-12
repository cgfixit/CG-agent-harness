//! Search provider choice and the fallback hop plan. Every provider here is a
//! public HTTPS service reached through the pinned client: no search provider is
//! a private-address exception.
use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};

/// `web.search.provider`, restart-only. `Auto` follows the saved keys: an active
/// Tavily key selects Tavily, else an active SerpAPI key selects SerpAPI, else
/// public Google (operator decision, 2026-11-10; before, a Tavily key never
/// changed `Auto`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchProvider {
    Auto,
    GooglePublic,
    Serpapi,
    Tavily,
}

impl SearchProvider {
    /// Strict: the removed SearXNG provider and a non-empty `web.search.searxng_url`
    /// fail boot by name, so a home that relied on SearXNG never silently searches
    /// somewhere else. A missing or empty `searxng_url` (older seeded homes) loads.
    pub fn load(cfg: &AppConfig) -> Result<Self> {
        let text = |key: &str| match cfg.get(key) {
            None => Ok(""),
            Some(serde_yaml_ng::Value::String(s)) => Ok(s.trim()),
            Some(_) => Err(HarnessError::config(format!("{key} must be a string"))),
        };
        if !text("web.search.searxng_url")?.is_empty() {
            return Err(HarnessError::config(
                "web.search.searxng_url is no longer read: SearXNG support was removed; delete the key",
            ));
        }
        match text("web.search.provider")? {
            "" | "auto" => Ok(Self::Auto),
            "google-public" => Ok(Self::GooglePublic),
            "serpapi" => Ok(Self::Serpapi),
            "tavily" => Ok(Self::Tavily),
            "searxng" => Err(HarnessError::config(
                "web.search.provider searxng was removed; use auto, google-public, serpapi or tavily",
            )),
            _ => Err(HarnessError::config(
                "web.search.provider must be auto, google-public, serpapi or tavily",
            )),
        }
    }

    /// The `/web status` wording. `keyed` and `tavily_keyed` say which keys are
    /// active; only `Auto` reads them.
    pub fn describe(&self, keyed: bool, tavily_keyed: bool) -> &'static str {
        match self {
            Self::Auto if tavily_keyed => "Tavily (auto: a saved Tavily key is searched first)",
            Self::Auto if keyed => "Google via SerpAPI",
            Self::Auto | Self::GooglePublic => "public Google (may require JavaScript/CAPTCHA)",
            Self::Serpapi => "Google via SerpAPI (web.search.provider: serpapi)",
            Self::Tavily => "Tavily (web.search.provider: tavily)",
        }
    }
}

/// Hops after the configured provider (#647 Phase 2), restart-only. Both read
/// through `flag_is_true`; the public Google hop ships on, the SerpAPI hop off;
/// the order is fixed. The SerpAPI hop follows a Tavily first hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fallback {
    pub to_serpapi: bool,
    pub to_public_google: bool,
    /// SerpAPI fallbacks per clock hour before one `web_search_fallback_burst` line.
    pub alert_per_hour: u64,
}

impl Fallback {
    pub fn load(cfg: &AppConfig) -> Result<Self> {
        let key = "web.search.fallback_alert_per_hour";
        let alert_per_hour = match cfg.get(key) {
            None => 20,
            Some(v) => v
                .as_u64()
                .filter(|n| (1..=1000).contains(n))
                .ok_or_else(|| HarnessError::config(format!("{key} must be 1..=1000")))?,
        };
        Ok(Self {
            to_serpapi: cfg.flag_is_true("web.search.fallback_to_serpapi"),
            to_public_google: cfg.flag_is_true("web.search.fallback_to_public_google"),
            alert_per_hour,
        })
    }
}

/// `web.search.result_reads`, restart-only and off as shipped (operator
/// decision, 2026-11-11). When on, a chat or `/loop` web turn may read the exact
/// URLs its own search listings returned, for that turn or loop run only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResultReads {
    pub enabled: bool,
    /// Listed URLs one turn or loop run may hold.
    pub max_urls: usize,
}

impl ResultReads {
    pub fn load(cfg: &AppConfig) -> Result<Self> {
        Ok(Self {
            enabled: cfg.flag_is_true("web.search.result_reads.enabled"),
            max_urls: cfg.u64_in("web.search.result_reads.max_urls", 20, 1..=100)? as usize,
        })
    }
}

/// One provider attempt in a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Hop {
    Tavily,
    Serpapi,
    GooglePublic,
}

impl Hop {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Tavily => "tavily",
            Self::Serpapi => "google-serpapi",
            Self::GooglePublic => "google-public",
        }
    }
}

/// The hops a search tries, in order: the configured provider, then each
/// enabled later hop of tavily → google-serpapi → google-public. A SerpAPI
/// fallback needs an active key; a forced provider is always tried first.
/// `Auto` starts at Tavily when its key is active (`tavily_keyed`), else at
/// SerpAPI when that key is (`keyed`), else at public Google.
pub(super) fn plan_hops(provider: &SearchProvider, keyed: bool, tavily_keyed: bool, fallback: &Fallback) -> Vec<Hop> {
    let mut hops = match provider {
        SearchProvider::Tavily => vec![Hop::Tavily],
        SearchProvider::Serpapi => vec![Hop::Serpapi],
        SearchProvider::Auto if tavily_keyed => vec![Hop::Tavily],
        SearchProvider::Auto if keyed => vec![Hop::Serpapi],
        SearchProvider::Auto | SearchProvider::GooglePublic => return vec![Hop::GooglePublic],
    };
    if hops[0] == Hop::Tavily && fallback.to_serpapi && keyed {
        hops.push(Hop::Serpapi);
    }
    if fallback.to_public_google {
        hops.push(Hop::GooglePublic);
    }
    hops
}

/// The hops that take a share of the deadline: the one about to run, and each
/// later hop that is not cooling down. A paused provider answers
/// `WEB_SEARCH_COOLDOWN` at once, without a request, so time held back for it
/// only shortened the hop before it.
pub(super) fn timed_hops(remaining: &[Hop], cooling: impl Fn(Hop) -> bool) -> Vec<Hop> {
    remaining
        .iter()
        .copied()
        .enumerate()
        .filter(|(index, hop)| *index == 0 || !cooling(*hop))
        .map(|(_, hop)| hop)
        .collect()
}

/// Lend unused time forward while reserving a paced public request when possible.
pub(super) fn hop_budget(
    remaining: std::time::Duration,
    hops: &[Hop],
    public_reserve: std::time::Duration,
) -> std::time::Duration {
    if hops.len() <= 1 {
        remaining
    } else if hops.last() == Some(&Hop::GooglePublic) && remaining > public_reserve {
        (remaining - public_reserve) / (hops.len() - 1) as u32
    } else {
        remaining / hops.len() as u32
    }
}

/// Whether a hop's failure moves the search to the next hop. A hop that cannot
/// answer does: an unusable provider and a keyed quota that is spent (the next
/// hop costs less or nothing). A clean zero-result answer, a refused query and a
/// rejected key (the operator's error, loud on every search) do not. A keyed
/// provider's cooldown is skipped without a request.
pub(super) fn falls_through(hop: Hop, code: &str) -> bool {
    match hop {
        Hop::Tavily | Hop::Serpapi => matches!(
            code,
            "WEB_SEARCH_PROVIDER" | "WEB_TIMEOUT" | "WEB_SEARCH_COOLDOWN" | "WEB_SEARCH_QUOTA"
        ),
        Hop::GooglePublic => false,
    }
}

/// SerpAPI fallbacks in the current clock hour; in memory, lost on restart.
#[derive(Debug, Default)]
pub(crate) struct FallbackBurst(std::sync::Mutex<(u64, u64, bool)>);

impl FallbackBurst {
    /// Count one fallback to SerpAPI; returns the count the first time this hour
    /// it exceeds `threshold`, and `None` otherwise.
    pub(super) fn note(&self, now_unix: u64, threshold: u64) -> Option<u64> {
        let hour = now_unix - now_unix % 3600;
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if state.0 != hour {
            *state = (hour, 0, false);
        }
        state.1 += 1;
        if state.1 > threshold && !state.2 {
            state.2 = true;
            return Some(state.1);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_budgets_reserve_pacing_and_lend_unused_time_forward() {
        use std::time::Duration;
        let seconds = Duration::from_secs;
        assert_eq!(
            hop_budget(seconds(8), &[Hop::Tavily, Hop::Serpapi, Hop::GooglePublic], seconds(4)),
            seconds(2)
        );
        assert_eq!(
            hop_budget(seconds(7), &[Hop::Serpapi, Hop::GooglePublic], seconds(4)),
            seconds(3)
        );
        assert_eq!(hop_budget(seconds(7), &[Hop::GooglePublic], seconds(4)), seconds(7));
        assert_eq!(
            hop_budget(seconds(2), &[Hop::Serpapi, Hop::GooglePublic], seconds(4)),
            seconds(1)
        );
        assert_eq!(
            hop_budget(seconds(8), &[Hop::Tavily, Hop::Serpapi], seconds(4)),
            seconds(4)
        );
    }

    #[test]
    fn a_cooling_later_hop_takes_no_share_of_the_deadline() {
        use std::time::Duration;
        use Hop::*;
        let chain = [Tavily, Serpapi, GooglePublic];
        assert_eq!(timed_hops(&chain, |_| false), chain);
        // Public Google paused: the keyed hops split the whole deadline.
        let timed = timed_hops(&chain, |hop| hop == GooglePublic);
        assert_eq!(timed, [Tavily, Serpapi]);
        assert_eq!(
            hop_budget(Duration::from_secs(8), &timed, Duration::from_secs(4)),
            Duration::from_secs(4)
        );
        // The hop about to run is always timed: its own pause answers at once.
        assert_eq!(timed_hops(&chain, |_| true), [Tavily]);
        assert_eq!(
            hop_budget(
                Duration::from_secs(8),
                &timed_hops(&[Serpapi, GooglePublic], |hop| hop == GooglePublic),
                Duration::from_secs(4)
            ),
            Duration::from_secs(8),
            "a lone keyed hop keeps the whole deadline"
        );
    }

    fn load(yaml: &str) -> Result<SearchProvider> {
        SearchProvider::load(&AppConfig::from_str(yaml, std::path::Path::new("config.yaml")).unwrap())
    }

    #[test]
    fn provider_defaults_to_today_and_the_removed_searxng_settings_fail_by_name() {
        assert_eq!(load("{}").unwrap(), SearchProvider::Auto);
        assert_eq!(
            load("web: {search: {provider: auto, searxng_url: ''}}").unwrap(),
            SearchProvider::Auto,
            "an older seeded home keeps its empty searxng_url"
        );
        assert_eq!(
            load("web: {search: {provider: google-public}}").unwrap(),
            SearchProvider::GooglePublic
        );
        assert_eq!(
            load("web: {search: {provider: serpapi}}").unwrap(),
            SearchProvider::Serpapi
        );
        assert_eq!(
            load("web: {search: {provider: tavily}}").unwrap(),
            SearchProvider::Tavily
        );
        let removed = load("web: {search: {provider: searxng}}").unwrap_err();
        assert_eq!(removed.code, "CONFIG_ERROR");
        assert!(removed.message.contains("searxng was removed"), "{removed}");
        for url in [
            "http://127.0.0.1:8888", // DevSkim: ignore DS137138,DS162092 because this asserts the removed loopback setting is refused.
            "http://searx.example:8888", // DevSkim: ignore DS137138 because this asserts the removed setting is refused.
        ] {
            let stale = load(&format!("web: {{search: {{provider: auto, searxng_url: '{url}'}}}}")).unwrap_err();
            assert_eq!(stale.code, "CONFIG_ERROR", "{url}");
            assert!(stale.message.contains("web.search.searxng_url"), "{url}");
            assert!(!stale.message.contains(url), "the value is never quoted");
        }
        for bad in [
            "web: {search: {provider: bing}}",
            "web: {search: {provider: Tavily}}",
            "web: {search: {provider: true}}",
            "web: {search: {searxng_url: 7}}",
        ] {
            assert_eq!(load(bad).unwrap_err().code, "CONFIG_ERROR", "{bad}");
        }
    }

    #[test]
    fn fallback_ships_off_and_plans_the_fixed_hop_order() {
        let cfg = |yaml: &str| AppConfig::from_str(yaml, std::path::Path::new("config.yaml")).unwrap();
        let off = Fallback::load(&cfg("{}")).unwrap();
        assert_eq!(
            off,
            Fallback {
                to_serpapi: false,
                to_public_google: false,
                alert_per_hour: 20
            }
        );
        let quoted = Fallback::load(&cfg(
            "web: {search: {fallback_to_serpapi: 'true', fallback_to_public_google: 'true'}}",
        ))
        .unwrap();
        assert!(!quoted.to_serpapi && !quoted.to_public_google, "quoted true stays off");
        for bad in ["0", "1001", "'20'", "-1"] {
            assert_eq!(
                Fallback::load(&cfg(&format!("web: {{search: {{fallback_alert_per_hour: {bad}}}}}")))
                    .unwrap_err()
                    .code,
                "CONFIG_ERROR",
                "{bad}"
            );
        }
        let on = Fallback {
            to_serpapi: true,
            to_public_google: true,
            alert_per_hour: 20,
        };
        use Hop::*;
        assert_eq!(plan_hops(&SearchProvider::Tavily, true, true, &off), [Tavily]);
        assert_eq!(
            plan_hops(&SearchProvider::Tavily, true, true, &on),
            [Tavily, Serpapi, GooglePublic]
        );
        assert_eq!(
            plan_hops(&SearchProvider::Tavily, false, true, &on),
            [Tavily, GooglePublic],
            "no SerpAPI key, no SerpAPI hop"
        );
        assert_eq!(
            plan_hops(&SearchProvider::Serpapi, true, true, &on),
            [Serpapi, GooglePublic],
            "a forced SerpAPI is not reordered by a Tavily key"
        );
        // Auto follows the saved keys: Tavily first, then SerpAPI, then public Google.
        assert_eq!(plan_hops(&SearchProvider::Auto, true, false, &off), [Serpapi]);
        assert_eq!(
            plan_hops(&SearchProvider::Auto, true, false, &on),
            [Serpapi, GooglePublic]
        );
        assert_eq!(plan_hops(&SearchProvider::Auto, false, false, &on), [GooglePublic]);
        assert_eq!(
            plan_hops(&SearchProvider::Auto, false, true, &off),
            [Tavily],
            "a lone Tavily key selects Tavily"
        );
        assert_eq!(
            plan_hops(&SearchProvider::Auto, true, true, &off),
            [Tavily],
            "with both keys Tavily wins"
        );
        assert_eq!(
            plan_hops(&SearchProvider::Auto, true, true, &on),
            [Tavily, Serpapi, GooglePublic]
        );
        assert_eq!(
            plan_hops(&SearchProvider::Auto, false, true, &on),
            [Tavily, GooglePublic]
        );
        assert_eq!(
            plan_hops(&SearchProvider::GooglePublic, true, true, &on),
            [GooglePublic],
            "a forced public Google ignores both keys"
        );
        let auto = SearchProvider::Auto;
        assert!(auto.describe(true, true).starts_with("Tavily"));
        assert_eq!(auto.describe(true, false), "Google via SerpAPI");
        assert!(auto.describe(false, false).starts_with("public Google"));
        assert!(SearchProvider::Serpapi.describe(true, true).contains("serpapi"));
    }

    #[test]
    fn only_an_unusable_provider_or_a_spent_quota_falls_through() {
        for hop in [Hop::Serpapi, Hop::Tavily] {
            for code in [
                "WEB_SEARCH_PROVIDER",
                "WEB_TIMEOUT",
                "WEB_SEARCH_COOLDOWN",
                "WEB_SEARCH_QUOTA",
            ] {
                assert!(falls_through(hop, code), "{hop:?} {code}");
            }
            for code in [
                "WEB_SEARCH_KEY_REJECTED",
                "WEB_SEARCH_RESPONSE",
                "WEB_SEARCH_KEY_CHANGED",
                "WEB_SEARCH_KEY_UNAVAILABLE",
                "WEB_BAD_QUERY",
                "WEB_BUSY",
            ] {
                assert!(!falls_through(hop, code), "{hop:?} {code}");
            }
        }
        assert!(!falls_through(Hop::GooglePublic, "WEB_GOOGLE_CHALLENGE"));
        assert!(!falls_through(Hop::GooglePublic, "WEB_TIMEOUT"));
    }

    #[test]
    fn fallback_burst_alerts_once_per_clock_hour() {
        let burst = FallbackBurst::default();
        let hour = 500_000 * 3600;
        assert_eq!(burst.note(hour + 1, 2), None);
        assert_eq!(burst.note(hour + 2, 2), None);
        assert_eq!(burst.note(hour + 3, 2), Some(3));
        assert_eq!(burst.note(hour + 4, 2), None, "once per window");
        assert_eq!(burst.note(hour + 3600, 2), None, "a new hour restarts the count");
        assert_eq!(burst.note(hour + 3601, 2), None);
        assert_eq!(burst.note(hour + 3602, 2), Some(3));
    }
}
