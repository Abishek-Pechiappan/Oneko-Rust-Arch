// Where the global cursor position comes from.
//
// Wayland deliberately does not expose the pointer position outside your own
// surfaces, so there is no portable way to ask "where is the mouse right
// now?" - every compositor needs its own answer. That's what `CursorSource`
// abstracts: implement it once per compositor and the rest of the program
// doesn't change.
//
// Currently implemented:
//   - `HyprlandIpc` - Hyprland's own IPC socket.
//
// Natural future backends (each is a new struct implementing this trait, plus
// a line in `detect()`):
//   - X11, via `XQueryPointer` - covers every X11 WM/DE in one go, and is how
//     the original oneko did it.
//   - generic wlroots (sway, river, Wayfire, niri, labwc), via a layer-shell
//     surface plus `zwlr_virtual_pointer_v1` nudged by a zero-distance
//     relative motion to provoke a `wl_pointer.motion` event carrying
//     surface-local coordinates.
//   - KWin, via a KWin script polling `workspace.cursorPos` over D-Bus.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// A source of global (layout-space) cursor coordinates.
pub trait CursorSource {
    /// The cursor's current position in global/layout coordinates, or `None`
    /// if it couldn't be determined for this poll.
    ///
    /// Returning `None` must mean "no answer", never a fallback coordinate:
    /// callers skip the tick entirely rather than moving the cat. Handing back
    /// a made-up `(0, 0)` would teleport the cat, and on multi-monitor layouts
    /// with negative offsets (e.g. a second monitor at x = -2560) `(0, 0)` is a
    /// perfectly valid on-screen point, so it can't even be recognised as
    /// bogus downstream.
    fn position(&mut self) -> Option<(f32, f32)>;

    /// Human-readable backend name, for the startup log line.
    fn name(&self) -> &'static str;
}

/// Picks a cursor backend for the session we're running in.
///
/// Ordered most-specific first; as more backends land, add them here.
pub fn detect() -> Option<Box<dyn CursorSource>> {
    if let Some(hypr) = HyprlandIpc::detect() {
        return Some(Box::new(hypr));
    }
    None
}

/// Reads `cursorpos` from Hyprland's IPC socket.
///
/// This replaces shelling out to `hyprctl cursorpos`, which is the same
/// request over the same socket but wrapped in a fork + exec + dynamic-link of
/// a whole separate binary. Measured on the author's machine, 80 polls (ten
/// seconds of runtime at the default tick rate) cost ~687 ms of CPU through
/// `hyprctl` versus ~0.8 ms through the socket - a continuous ~7% of a core,
/// spent entirely on process startup, purely to ask where the mouse is.
///
/// Hyprland serves one request per connection and closes it, so there is no
/// persistent connection to keep: `position()` reconnects each poll. That's
/// the cost already measured above, and at ~10 us a poll it isn't worth
/// optimising further. It also means the struct is just a path, so it's
/// `Clone` and the fullscreen check holds its own copy.
#[derive(Clone)]
pub struct HyprlandIpc {
    socket: PathBuf,
}

impl HyprlandIpc {
    /// Locates the IPC socket for the running Hyprland instance, or `None` if
    /// we aren't under Hyprland.
    pub fn detect() -> Option<Self> {
        let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;

        // Hyprland moved this out of /tmp into XDG_RUNTIME_DIR (0.40-ish).
        // Check both so the same binary works across distro package versions.
        let mut candidates = Vec::new();
        if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
            candidates.push(PathBuf::from(format!("{runtime}/hypr/{signature}/.socket.sock")));
        }
        candidates.push(PathBuf::from(format!("/tmp/hypr/{signature}/.socket.sock")));

        candidates
            .into_iter()
            .find(|p| p.exists())
            .map(|socket| Self { socket })
    }

    /// One request/response round trip. `Err` is deliberately collapsed into
    /// `None` by callers - a single failed poll is not worth reporting, and
    /// definitely not worth killing the cat over.
    ///
    /// Also used by the fullscreen check in `fullscreen.rs`, which asks the same
    /// socket different questions.
    pub fn request(&self, command: &str) -> std::io::Result<String> {
        let mut stream = UnixStream::connect(&self.socket)?;

        // A wedged compositor must not wedge the cat: without these, a stalled
        // Hyprland would block the event loop indefinitely and the cat would
        // freeze mid-stride with no way to recover.
        let timeout = Some(Duration::from_millis(50));
        stream.set_read_timeout(timeout)?;
        stream.set_write_timeout(timeout)?;

        stream.write_all(command.as_bytes())?;

        // Read to EOF rather than a single `read`: Hyprland closes the socket
        // after replying, so EOF is the unambiguous end of the response and a
        // short read can't truncate a coordinate mid-digit.
        let mut reply = String::new();
        stream.read_to_string(&mut reply)?;
        Ok(reply)
    }
}

impl CursorSource for HyprlandIpc {
    fn position(&mut self) -> Option<(f32, f32)> {
        let reply = self.request("cursorpos").ok()?;
        parse_cursorpos(&reply)
    }

    fn name(&self) -> &'static str {
        "hyprland-ipc"
    }
}

/// Parses Hyprland's `cursorpos` reply, which is `"<x>, <y>"` - e.g.
/// `"-1105, 434"` on a layout with a monitor left of the primary.
///
/// Split out from `position()` so it's testable without a live compositor.
fn parse_cursorpos(reply: &str) -> Option<(f32, f32)> {
    let (x, y) = reply.trim().split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::parse_cursorpos;

    #[test]
    fn parses_positive_coordinates() {
        assert_eq!(parse_cursorpos("960, 540"), Some((960.0, 540.0)));
    }

    #[test]
    fn parses_negative_coordinates() {
        // A monitor placed left of the primary gives negative x - this is the
        // real reply shape from a dual-monitor layout, and the case that made
        // the old `unwrap_or(0.0)` fallback actively harmful.
        assert_eq!(parse_cursorpos("-1105, 434"), Some((-1105.0, 434.0)));
    }

    #[test]
    fn tolerates_surrounding_and_inner_whitespace() {
        assert_eq!(parse_cursorpos("  12 ,  34 \n"), Some((12.0, 34.0)));
    }

    #[test]
    fn rejects_malformed_replies() {
        // Each of these must yield None (skip the tick) rather than a
        // coordinate, so a transient IPC hiccup can't move the cat.
        for bad in ["", "960", "960;540", "x, y", "960, ", ", 540"] {
            assert_eq!(parse_cursorpos(bad), None, "expected None for {bad:?}");
        }
    }
}
