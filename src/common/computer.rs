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

/// One app and one of its windows. The operator grants this; the model
/// cannot. The window is required: an action never goes to "whichever window
/// the app has in front".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopGrant {
    pub bundle_id: String,
    pub window_id: u64,
}

/// The kimi-cu tool and arguments for one wire op, scoped to the grant.
/// Every action names the element it acts on, by a ref from the latest
/// observation; none goes to whatever has focus. Typing and Enter are off for
/// the pilot (#996). Everything else, including `debug_tap`, `paste`,
/// `set_value`, `launch_app`, `activate_window` and the drags, is refused by
/// omission. Argument names are as checked at kimi-cu 0.7.6; recheck them
/// against the installed `tools/list` before this is wired.
pub fn kimi_cu_call(grant: &DesktopGrant, op: &Value) -> Result<(&'static str, Value), String> {
    let mut args = json!({"app": grant.bundle_id, "window_id": grant.window_id});
    // A desktop snapshot_ref is "<snapshot_id>:<index>" from get_app_state.
    let mut target = || -> Result<(), String> {
        let raw = op.pointer("/target/ref").and_then(Value::as_str).unwrap_or("");
        let (snapshot, index) = raw
            .rsplit_once(':')
            .filter(|(snapshot, _)| !snapshot.is_empty())
            .and_then(|(snap, idx)| Some((snap, idx.parse::<u64>().ok()?)))
            .ok_or("stale or unknown target")?;
        args["snapshot_id"] = json!(snapshot);
        args["index"] = json!(index);
        Ok(())
    };
    let tool = match op["op"].as_str().unwrap_or("") {
        "observe" => {
            // A whole tree each time: a diff is against state this side does not keep.
            args["disable_diff"] = json!(true);
            "get_app_state"
        }
        "click" => {
            target()?;
            "click"
        }
        "scroll" => {
            target()?;
            let dy = op["dy"]
                .as_i64()
                .filter(|dy| (-MAX_SCROLL..=MAX_SCROLL).contains(dy) && *dy != 0)
                .ok_or("scroll distance is out of bounds")?;
            args["dy"] = json!(dy);
            "scroll"
        }
        "type" | "press_enter" => return Err("typing and Enter are off for the native pilot".into()),
        _ => return Err("op has no desktop mapping".into()),
    };
    Ok((tool, args))
}

/// The same bound the browser kernel puts on one scroll.
const MAX_SCROLL: i64 = 2_000;

/// What an action's result says happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The driver delivered the input and saw its effect.
    Verified,
    /// The driver delivered the input but could not confirm the effect. Not a
    /// success, and never a reason to send the input again: observe instead.
    Unverified,
    /// The driver did not deliver it.
    Failed,
}

/// Reads an action result's `ok` and `verified` fields. Anything but a
/// literal `true` counts against it.
pub fn delivery(result: &Value) -> Delivery {
    match (result["ok"] == true, result["verified"] == true) {
        (true, true) => Delivery::Verified,
        (true, false) => Delivery::Unverified,
        (false, _) => Delivery::Failed,
    }
}

/// The native driver is an attended pilot that has not started.
pub fn start_desktop(_grant: &DesktopGrant) -> Result<(), String> {
    Err("native computer use is not wired yet; see docs/DESKTOP.md#later-native-computer-use".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant() -> DesktopGrant {
        DesktopGrant {
            bundle_id: "com.example.app".into(),
            window_id: 7,
        }
    }

    #[test]
    fn ops_stay_inside_the_grant_and_unsafe_tools_are_refused() {
        let grant = grant();
        let (tool, args) = kimi_cu_call(&grant, &json!({"op":"click","target":{"ref":"snap-3:12"}})).unwrap();
        assert_eq!(tool, "click");
        assert_eq!(
            args,
            json!({"app":"com.example.app","window_id":7,"snapshot_id":"snap-3","index":12})
        );
        let (tool, args) = kimi_cu_call(&grant, &json!({"op":"observe"})).unwrap();
        assert_eq!(
            (tool, args),
            (
                "get_app_state",
                json!({"app":"com.example.app","window_id":7,"disable_diff":true})
            )
        );
        for op in [
            "navigate",
            "debug_tap",
            "paste",
            "set_value",
            "launch_app",
            "activate_window",
            "drag",
        ] {
            assert!(kimi_cu_call(&grant, &json!({ "op": op })).is_err(), "{op}");
        }
        for stale in ["no-index", ":12", "snap-3:", "snap-3:x", ""] {
            assert!(
                kimi_cu_call(&grant, &json!({"op":"click","target":{"ref": stale}})).is_err(),
                "{stale}"
            );
        }
        assert!(start_desktop(&grant).is_err());
    }

    /// #996: no action goes to whatever has focus, and input is off for the pilot.
    #[test]
    fn every_action_names_its_element_and_typing_is_off() {
        let grant = grant();
        let (tool, args) = kimi_cu_call(&grant, &json!({"op":"scroll","target":{"ref":"snap-3:4"},"dy":400})).unwrap();
        assert_eq!(
            (tool, args),
            (
                "scroll",
                json!({"app":"com.example.app","window_id":7,"snapshot_id":"snap-3","index":4,"dy":400})
            )
        );
        assert!(
            kimi_cu_call(&grant, &json!({"op":"scroll","dy":400})).is_err(),
            "no target"
        );
        for dy in [json!(0), json!(2_001), json!(-2_001), json!("400"), json!(null)] {
            let op = json!({"op":"scroll","target":{"ref":"snap-3:4"},"dy": dy});
            assert!(kimi_cu_call(&grant, &op).is_err(), "{dy}");
        }
        for op in [
            json!({"op":"type","target":{"ref":"snap-3:4"},"text":"hello"}),
            json!({"op":"press_enter","target":{"ref":"snap-3:4"}}),
            json!({"op":"press_enter"}),
        ] {
            let refused = kimi_cu_call(&grant, &op).unwrap_err();
            assert!(refused.contains("off for the native pilot"), "{op}: {refused}");
        }
    }

    #[test]
    fn only_a_verified_result_is_a_delivered_action() {
        assert_eq!(delivery(&json!({"ok": true, "verified": true})), Delivery::Verified);
        for unverified in [json!({"ok": true, "verified": false}), json!({"ok": true})] {
            assert_eq!(delivery(&unverified), Delivery::Unverified, "{unverified}");
        }
        for failed in [
            json!({"ok": false, "verified": true}),
            json!({"verified": true}),
            json!({"ok": "true", "verified": true}),
            json!(null),
        ] {
            assert_eq!(delivery(&failed), Delivery::Failed, "{failed}");
        }
    }
}
