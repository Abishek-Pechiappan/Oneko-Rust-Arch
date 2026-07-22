// oneko-rust GNOME Shell extension: the thin GNOME-side half of the Ubuntu
// backend. All the animation/position logic and pixel rendering happens in
// the oneko-daemon Rust process (../daemon) - this extension only supplies
// the two things a normal (sandboxed) app can't get on GNOME/Wayland: the
// global cursor position, and an always-on-top surface to draw the cat into.
// It talks to the daemon over D-Bus, one round trip per 125ms animation
// tick, driven by this file's own GLib timer.
//
// Known rough edge (see ubuntu-version/README.md): the daemon's pixel
// buffer is little-endian ARGB8888 (byte order B,G,R,A in memory - see
// oneko-core::render_frame), which should line up with Cogl's
// BGRA_8888_PRE format below, but this is the one piece of the whole
// project that could only be verified on a real GNOME session, not by the
// person who wrote it. If the cat renders as a solid block, garbled, or
// with wrong colors, this is the first thing to check.

import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import Cogl from 'gi://Cogl';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const BUS_NAME = 'com.onekorust.Cat';
const OBJECT_PATH = '/com/onekorust/Cat';
const TICK_MS = 125; // matches the Hyprland backend's own tick interval

// Mirrors oneko_core::{CAT_X_OFFSET, CAT_Y_OFFSET, SIZE}: where the cat's
// own clickable 32x32 box sits within the larger canvas the daemon renders
// (the extra space above it is for the speech bubble). Kept in sync by hand
// since this extension doesn't link against oneko-core.
const CAT_X_OFFSET = 20;
const CAT_Y_OFFSET = 16;
const CAT_SIZE = 32;

const CatDBusIface = `
<node>
  <interface name="com.onekorust.Cat1">
    <method name="Tick">
      <arg type="i" direction="in"/>
      <arg type="i" direction="in"/>
      <arg type="i" direction="in"/>
      <arg type="i" direction="in"/>
      <arg type="i" direction="in"/>
      <arg type="i" direction="in"/>
      <arg type="b" direction="out"/>
      <arg type="i" direction="out"/>
      <arg type="i" direction="out"/>
      <arg type="u" direction="out"/>
      <arg type="u" direction="out"/>
      <arg type="ay" direction="out"/>
    </method>
    <method name="Click"/>
  </interface>
</node>`;

const CatProxy = Gio.DBusProxy.makeProxyWrapper(CatDBusIface);

export default class OnekoExtension extends Extension {
    enable() {
        this._proxy = null;
        this._timeoutId = null;
        this._subprocess = null;

        this._spawnDaemon();

        this._actor = new St.Widget({reactive: true, visible: false, width: 1, height: 1});
        this._actor.connect('button-press-event', (actor, event) => this._onButtonPress(actor, event));
        Main.layoutManager.addChrome(this._actor);

        this._proxy = new CatProxy(Gio.DBus.session, BUS_NAME, OBJECT_PATH, (proxy, error) => {
            if (error) {
                console.error(`oneko-rust: failed to connect to daemon: ${error}`);
                return;
            }
            this._timeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, TICK_MS, () => {
                this._tick();
                return GLib.SOURCE_CONTINUE;
            });
        });
    }

    disable() {
        if (this._timeoutId) {
            GLib.source_remove(this._timeoutId);
            this._timeoutId = null;
        }
        if (this._actor) {
            Main.layoutManager.removeChrome(this._actor);
            this._actor.destroy();
            this._actor = null;
        }
        this._proxy = null;
        if (this._subprocess) {
            try {
                this._subprocess.force_exit();
            } catch (e) {
                // Already gone - fine.
            }
            this._subprocess = null;
        }
    }

    // Starts the daemon binary (installed by ubuntu-version/install.sh to
    // ~/.local/bin/oneko-daemon). If it's already running - e.g. the
    // extension was disabled/re-enabled quickly - the new instance will
    // just fail to claim the D-Bus name and this one exits; either process
    // owning the name is fine since neither holds any state worth keeping
    // across that handoff.
    _spawnDaemon() {
        try {
            const binPath = GLib.build_filenamev([GLib.get_home_dir(), '.local', 'bin', 'oneko-daemon']);
            this._subprocess = Gio.Subprocess.new([binPath], Gio.SubprocessFlags.NONE);
        } catch (e) {
            console.error(`oneko-rust: failed to spawn daemon: ${e}`);
        }
    }

    _onButtonPress(actor, event) {
        const [x, y] = event.get_coords();
        const [actorX, actorY] = actor.get_transformed_position();
        const localX = x - actorX;
        const localY = y - actorY;
        const onCat = localX >= CAT_X_OFFSET && localX < CAT_X_OFFSET + CAT_SIZE
            && localY >= CAT_Y_OFFSET && localY < CAT_Y_OFFSET + CAT_SIZE;
        if (onCat && this._proxy) {
            this._proxy.ClickRemote(() => {});
            return Clutter.EVENT_STOP;
        }
        return Clutter.EVENT_PROPAGATE;
    }

    // One 125ms animation step: report the current global cursor position
    // (and the geometry of whichever monitor it's on, so the daemon can
    // clamp the cat's movement to that monitor - see oneko_core::tick's
    // `bounds` param) and apply whatever the daemon says changed.
    _tick() {
        if (!this._proxy)
            return;

        const [x, y] = global.get_pointer();
        const monitor = Main.layoutManager.monitors.find(m =>
            x >= m.x && x < m.x + m.width && y >= m.y && y < m.y + m.height
        ) ?? Main.layoutManager.primaryMonitor;

        this._proxy.TickRemote(x, y, monitor.x, monitor.y, monitor.width, monitor.height,
            (result, error) => {
                if (error) {
                    console.error(`oneko-rust: Tick call failed: ${error}`);
                    return;
                }
                const [changed, actorX, actorY, w, h, pixels] = result;
                if (!changed)
                    return;

                this._actor.set_position(actorX, actorY);
                this._actor.set_size(w, h);
                this._actor.set_content(this._makeImage(w, h, pixels));
                this._actor.visible = true;
            });
    }

    _makeImage(w, h, pixelBytes) {
        const bytes = new GLib.Bytes(Uint8Array.from(pixelBytes));
        const image = new Clutter.Image();
        image.set_bytes(bytes, Cogl.PixelFormat.BGRA_8888_PRE, w, h, w * 4);
        return image;
    }
}
