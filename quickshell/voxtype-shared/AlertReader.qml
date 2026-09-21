// Voxtype OSD alert-marker watcher.
//
// Wraps Quickshell.Io.FileView around an alert marker file at
// $XDG_RUNTIME_DIR/voxtype/alert. The file is created and removed rather
// than rewritten, so both the load and the failure path carry meaning:
// present means "something is degraded, say so on every OSD", absent
// (the common case) means stay quiet.
//
// The daemon does not write this file. It is a channel for the pieces
// that sit outside the daemon and know something it can't: a
// post-process command that fell back to a slower path, a remote engine
// that stopped answering, a model that failed to preload. Those tools
// write one line of text and the OSD surfaces it.
//
//   printf 'Cleanup LLM offline - local CPU fallback' > "$XDG_RUNTIME_DIR/voxtype/alert"
//   rm -f "$XDG_RUNTIME_DIR/voxtype/alert"
//
// Usage:
//
//   import "voxtype-shared" as VT
//   VT.AlertReader { id: alerts }
//   ...
//   Text { visible: alerts.active; text: alerts.message }

import QtQuick
import Quickshell
import Quickshell.Io

QtObject {
    id: root

    /// Filesystem path to the alert marker. Defaults to
    /// `$XDG_RUNTIME_DIR/voxtype/alert` with a `/run/user/$UID` fallback
    /// for environments that don't export XDG_RUNTIME_DIR.
    property string alertPath: {
        const xdg = Quickshell.env("XDG_RUNTIME_DIR");
        if (xdg && xdg.length > 0) {
            return xdg + "/voxtype/alert";
        }
        const uid = Quickshell.env("UID");
        if (uid && uid.length > 0) {
            return "/run/user/" + uid + "/voxtype/alert";
        }
        return "/run/user/1000/voxtype/alert";
    }

    /// True while the marker file exists.
    property bool active: false

    /// First line of the marker file, shown verbatim by the OSD. Falls
    /// back to a generic string so an empty marker still reads as an
    /// alert rather than a blank bar.
    property string message: ""

    property FileView _alertView: FileView {
        path: root.alertPath
        watchChanges: true
        printErrors: false

        onLoaded: {
            const raw = (text() || "").trim();
            const line = raw.length > 0 ? raw.split("\n")[0] : "Degraded";
            if (root.message !== line) {
                root.message = line;
            }
            if (!root.active) {
                root.active = true;
            }
        }

        onLoadFailed: {
            if (root.active) {
                root.active = false;
            }
        }

        onFileChanged: reload()
    }
}
