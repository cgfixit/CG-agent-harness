//! A fixed-shape status block for local chat turns and `/prompt`, rendered
//! from values the caller already holds. It opens no file and reports no key,
//! masked key tail, sandbox backend, allowlist URL or other account.

/// Build commit, when the build recorded one (`CGAH_BUILD_COMMIT` at compile time).
pub const BUILD_COMMIT: Option<&str> = option_env!("CGAH_BUILD_COMMIT");
const MAX_CHARS: usize = 1200;

#[derive(Debug, Clone)]
pub struct LiveState<'a> {
    pub model: &'a str,
    pub provider: &'a str,
    pub web_enabled: bool,
    /// `off`, `fetch` or `all`: the tools this turn actually carries.
    pub web_mode: &'a str,
    pub agentic_enabled: bool,
    pub structured_memory_open: bool,
    pub mcp_enabled: bool,
    pub netconnect_enabled: bool,
    pub style: Option<&'a str>,
    pub soul_enabled: bool,
    pub soul_present: bool,
    pub session_id: Option<&'a str>,
    pub home: &'a str,
}

fn on(flag: bool) -> &'static str {
    if flag {
        "on"
    } else {
        "off"
    }
}

/// One line per fact, in a fixed order, clipped to 1200 characters.
pub fn render(state: &LiveState<'_>) -> String {
    let text = format!(
        "version: {} (build commit {})\n\
         chat model: {} (provider {})\n\
         web: {} (chat tools this turn: {})\n\
         agentic.enabled: {}\n\
         structured_memory store: {}\n\
         mcp.enabled: {}\n\
         netconnect.enabled: {}\n\
         style: {}\n\
         soul: {} ({})\n\
         session: {}\n\
         home: {}",
        crate::VERSION,
        BUILD_COMMIT.unwrap_or("not recorded"),
        state.model,
        state.provider,
        on(state.web_enabled),
        state.web_mode,
        on(state.agentic_enabled),
        if state.structured_memory_open { "open" } else { "closed" },
        on(state.mcp_enabled),
        on(state.netconnect_enabled),
        state.style.unwrap_or("off"),
        on(state.soul_enabled),
        if state.soul_present { "file present" } else { "no file" },
        state.session_id.unwrap_or("none"),
        state.home,
    );
    crate::common::clip_chars(&text, MAX_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample<'a>() -> LiveState<'a> {
        LiveState {
            model: "qwen",
            provider: "ollama",
            web_enabled: true,
            web_mode: "fetch",
            agentic_enabled: false,
            structured_memory_open: true,
            mcp_enabled: false,
            netconnect_enabled: false,
            style: None,
            soul_enabled: true,
            soul_present: false,
            session_id: Some("0123456789ab"),
            home: "/tmp/home",
        }
    }

    #[test]
    fn the_block_has_a_fixed_shape_and_a_bound() {
        let text = render(&sample());
        let keys: Vec<&str> = text.lines().map(|l| l.split(':').next().unwrap()).collect();
        assert_eq!(
            keys,
            [
                "version",
                "chat model",
                "web",
                "agentic.enabled",
                "structured_memory store",
                "mcp.enabled",
                "netconnect.enabled",
                "style",
                "soul",
                "session",
                "home"
            ]
        );
        assert!(text.contains("agentic.enabled: off") && text.contains("web: on (chat tools this turn: fetch)"));
        let long = "h".repeat(5000);
        let mut big = sample();
        big.home = &long;
        assert!(render(&big).chars().count() <= MAX_CHARS);
        for absent in ["sandbox", "key", "bwrap", "seatbelt"] {
            assert!(!text.to_lowercase().contains(absent), "{absent}");
        }
    }
}
