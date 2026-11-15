//! Skeleton for native computer use. Not wired: no route, CLI or config
//! reaches it yet (docs/DESKTOP.md#later-native-computer-use).
//!
//! The model loop, `browser::Session` refs and decisions carry over unchanged:
//! a desktop observation mints refs the same way, `navigate` and links have no
//! grant, and buttons hand off. Only the `browser_host::Driver` differs. The
//! first one is Kimi Computer Use's MCP server (`kimi-cu mcp`, tools checked at
//! 0.7.6). It reads the Accessibility tree and acts without moving the pointer.
//! Launch the signed app binary directly: the plugin's wrapper script is
//! world-writable. TCC Accessibility and Screen Recording stay the operator's.

use serde_json::{json, Value};

/// The signed binary behind the plugin wrapper.
pub const KIMI_CU_BINARY: &str = "/Applications/KimiCU.app/Contents/MacOS/kimi-cu";

/// One app, optionally one window. The operator grants this; the model cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopGrant {
    pub bundle_id: String,
    pub window_id: Option<u64>,
}

/// The kimi-cu tool and arguments for one wire op, scoped to the grant.
/// Everything else, including `debug_tap`, `paste`, `launch_app`,
/// `activate_window` and the drags, is refused by omission.
pub fn kimi_cu_call(grant: &DesktopGrant, op: &Value) -> Result<(&'static str, Value), String> {
    let mut args = json!({"app": grant.bundle_id});
    if let Some(id) = grant.window_id {
        args["window_id"] = json!(id);
    }
    // A desktop snapshot_ref is "<snapshot_id>:<index>" from get_app_state.
    let mut target = || -> Result<(), String> {
        let raw = op.pointer("/target/ref").and_then(Value::as_str).unwrap_or("");
        let (snapshot, index) = raw
            .rsplit_once(':')
            .and_then(|(snap, idx)| Some((snap, idx.parse::<u64>().ok()?)))
            .ok_or("stale or unknown target")?;
        args["snapshot_id"] = json!(snapshot);
        args["index"] = json!(index);
        Ok(())
    };
    let tool = match op["op"].as_str().unwrap_or("") {
        "observe" => "get_app_state",
        "click" => {
            target()?;
            "click"
        }
        "type" => {
            target()?;
            args["text"] = op["text"].clone();
            "type_text"
        }
        "press_enter" => {
            args["keys"] = json!("return");
            "press_key"
        }
        "scroll" => {
            args["dy"] = op["dy"].clone();
            "scroll"
        }
        _ => return Err("op has no desktop mapping".into()),
    };
    Ok((tool, args))
}

/// The native driver is an attended pilot that has not started.
pub fn start_desktop(_grant: &DesktopGrant) -> Result<(), String> {
    Err("native computer use is not wired yet; see docs/DESKTOP.md#later-native-computer-use".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_stay_inside_the_grant_and_unsafe_tools_are_refused() {
        let grant = DesktopGrant {
            bundle_id: "com.example.app".into(),
            window_id: Some(7),
        };
        let (tool, args) = kimi_cu_call(&grant, &json!({"op":"click","target":{"ref":"snap-3:12"}})).unwrap();
        assert_eq!(tool, "click");
        assert_eq!(
            args,
            json!({"app":"com.example.app","window_id":7,"snapshot_id":"snap-3","index":12})
        );
        let (tool, _) = kimi_cu_call(&grant, &json!({"op":"press_enter"})).unwrap();
        assert_eq!(tool, "press_key");
        for op in [
            "navigate",
            "debug_tap",
            "paste",
            "launch_app",
            "activate_window",
            "drag",
        ] {
            assert!(kimi_cu_call(&grant, &json!({ "op": op })).is_err(), "{op}");
        }
        assert!(kimi_cu_call(&grant, &json!({"op":"click","target":{"ref":"no-index"}})).is_err());
        assert!(start_desktop(&grant).is_err());
    }
}
