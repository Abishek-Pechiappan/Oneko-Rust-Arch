// Which monitors are currently showing a fullscreen window, so the cat can get
// out of the way of games and videos instead of drawing on top of them.
//
// Like the cursor position, this is something Wayland doesn't tell ordinary
// clients, so it's one backend per compositor behind `FullscreenSource`. Unlike
// the cursor, it's optional: a compositor with no backend here just means the
// cat never hides, which is exactly the behavior before this existed.
//
// Currently implemented:
//   - Hyprland, over the same IPC socket as `cursor::HyprlandIpc`.

use crate::cursor::HyprlandIpc;
use crate::json::{self, Value};

pub trait FullscreenSource {
    /// Connector names (`"eDP-1"`, `"HDMI-A-1"`, ...) of monitors whose visible
    /// workspace has a fullscreen window, or `None` if that couldn't be
    /// determined this time. As with `CursorSource::position`, `None` means "no
    /// answer" and the caller keeps its previous view rather than assuming
    /// nothing is fullscreen - otherwise one IPC hiccup would flash the cat over
    /// a game.
    fn fullscreen_outputs(&mut self) -> Option<Vec<String>>;
}

/// Picks a fullscreen backend for the session we're running in, if there is one.
pub fn detect() -> Option<Box<dyn FullscreenSource>> {
    if let Some(hypr) = HyprlandIpc::detect() {
        return Some(Box::new(hypr));
    }
    None
}

impl FullscreenSource for HyprlandIpc {
    fn fullscreen_outputs(&mut self) -> Option<Vec<String>> {
        let monitors = json::parse(&self.request("j/monitors").ok()?)?;
        let clients = json::parse(&self.request("j/clients").ok()?)?;
        fullscreen_monitors(&monitors, &clients)
    }
}

/// The interesting half of the Hyprland backend, split out so it can be tested
/// against recorded replies without a live compositor.
///
/// Works per monitor rather than asking "is the focused window fullscreen?":
/// a video fullscreen on one monitor shouldn't hide the cat on another, and the
/// cat is on whichever monitor holds the cursor, which needn't be the focused
/// one.
fn fullscreen_monitors(monitors: &Value, clients: &Value) -> Option<Vec<String>> {
    // Workspace ids currently on screen, per monitor: the regular active
    // workspace, plus the special (scratchpad) workspace when one is open on
    // top of it. Hyprland reports id 0 for "no special workspace".
    let mut visible: Vec<(i64, &str)> = Vec::new();
    for m in monitors.as_array()? {
        let name = m.get("name")?.as_str()?;
        for key in ["activeWorkspace", "specialWorkspace"] {
            if let Some(id) = m.get(key).and_then(|w| w.get("id")).and_then(Value::as_i64) {
                if id != 0 {
                    visible.push((id, name));
                }
            }
        }
    }

    let mut out: Vec<String> = Vec::new();
    for c in clients.as_array()? {
        if !is_fullscreen(c) {
            continue;
        }
        let Some(ws) = c.get("workspace").and_then(|w| w.get("id")).and_then(Value::as_i64) else {
            continue;
        };
        for (_, name) in visible.iter().filter(|(id, _)| *id == ws) {
            if !out.iter().any(|o| o == name) {
                out.push((*name).to_owned());
            }
        }
    }
    Some(out)
}

/// Whether a client is truly fullscreen, as opposed to maximized.
///
/// Maximized windows deliberately don't count: on a tiling compositor that's an
/// everyday layout, and hiding the cat for it would make it vanish half the
/// time. Fullscreen is the "I'm watching / playing something" signal.
fn is_fullscreen(client: &Value) -> bool {
    match client.get("fullscreen") {
        // Hyprland >= 0.42: a bitmask of modes, 1 = maximized, 2 = fullscreen.
        Some(mode @ Value::Num(_)) => mode.as_i64().is_some_and(|m| m & 2 != 0),
        // Older Hyprland: a bool, with `fullscreenMode` 0 = fullscreen and
        // 1 = maximized.
        Some(Value::Bool(true)) => {
            client.get("fullscreenMode").and_then(Value::as_i64).unwrap_or(0) != 1
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from real `j/monitors` and `j/clients` replies on a two-monitor
    // Hyprland 0.56 session; the fields this code doesn't read are dropped.
    const MONITORS: &str = r#"[{
        "id": 0, "name": "eDP-1",
        "activeWorkspace": {"id": 1, "name": "1"},
        "specialWorkspace": {"id": 0, "name": ""}
    },{
        "id": 1, "name": "HDMI-A-1",
        "activeWorkspace": {"id": 2, "name": "2"},
        "specialWorkspace": {"id": -98, "name": "special:magic"}
    }]"#;

    fn clients(entries: &[(i64, i64)]) -> Value {
        let body: Vec<String> = entries
            .iter()
            .map(|(ws, fs)| {
                format!(r#"{{"workspace": {{"id": {ws}, "name": "x"}}, "fullscreen": {fs}, "title": "◐ t"}}"#)
            })
            .collect();
        json::parse(&format!("[{}]", body.join(","))).unwrap()
    }

    fn check(entries: &[(i64, i64)]) -> Vec<String> {
        fullscreen_monitors(&json::parse(MONITORS).unwrap(), &clients(entries)).unwrap()
    }

    #[test]
    fn nothing_fullscreen() {
        assert!(check(&[(1, 0), (2, 0)]).is_empty());
        assert!(check(&[]).is_empty());
    }

    #[test]
    fn fullscreen_on_a_visible_workspace_hides_only_that_monitor() {
        assert_eq!(check(&[(1, 2), (2, 0)]), ["eDP-1"]);
        assert_eq!(check(&[(1, 0), (2, 2)]), ["HDMI-A-1"]);
        assert_eq!(check(&[(1, 2), (2, 2)]), ["eDP-1", "HDMI-A-1"]);
    }

    #[test]
    fn maximized_does_not_count() {
        assert!(check(&[(1, 1)]).is_empty());
        // Both flags set is still fullscreen.
        assert_eq!(check(&[(1, 3)]), ["eDP-1"]);
    }

    #[test]
    fn fullscreen_on_a_hidden_workspace_does_not_count() {
        assert!(check(&[(5, 2)]).is_empty());
    }

    #[test]
    fn an_open_special_workspace_counts_for_its_monitor() {
        assert_eq!(check(&[(-98, 2)]), ["HDMI-A-1"]);
    }

    #[test]
    fn old_hyprland_bool_format() {
        let monitors = json::parse(MONITORS).unwrap();
        let old = |mode: i64| {
            json::parse(&format!(
                r#"[{{"workspace": {{"id": 1}}, "fullscreen": true, "fullscreenMode": {mode}}}]"#
            ))
            .unwrap()
        };
        assert_eq!(fullscreen_monitors(&monitors, &old(0)).unwrap(), ["eDP-1"]);
        assert!(fullscreen_monitors(&monitors, &old(1)).unwrap().is_empty());
    }

    #[test]
    fn unexpected_shapes_give_no_answer_rather_than_an_empty_one() {
        // An empty list would mean "nothing fullscreen" and unhide the cat;
        // a reply we can't read must not be mistaken for that.
        let clients = clients(&[]);
        assert_eq!(fullscreen_monitors(&json::parse("{}").unwrap(), &clients), None);
        assert_eq!(fullscreen_monitors(&json::parse(r#"[{"id": 0}]"#).unwrap(), &clients), None);
    }
}
