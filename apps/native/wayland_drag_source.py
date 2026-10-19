#!/usr/bin/python3
"""Independent GTK3 URI-list source for private Wayland editor drop checks."""
import argparse
import json
from pathlib import Path
import time

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, Gtk  # noqa: E402


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("files", nargs="+", type=Path)
    parser.add_argument("--payload", type=Path, help="Raw URI-list bytes for rejection cases")
    parser.add_argument("--delay", type=float, default=0, help="Delay serving the receive request")
    parser.add_argument("--action", choices=("copy", "move"), default="copy")
    args = parser.parse_args()
    uris = [path.resolve(strict=True).as_uri() for path in args.files]
    window = Gtk.Window(title="editor-drag-source")
    window.set_default_size(200, 150)
    window.connect("destroy", Gtk.main_quit)
    window.add(Gtk.Label(label="Drag image batch"))
    window.drag_source_set(Gdk.ModifierType.BUTTON1_MASK,
                           [Gtk.TargetEntry.new("text/uri-list", 0, 0)],
                           Gdk.DragAction.COPY if args.action == "copy" else Gdk.DragAction.MOVE)

    def send(_widget, _context, selection, _info, _timestamp):
        time.sleep(args.delay)
        if args.payload:
            selection.set(Gdk.Atom.intern("text/uri-list", False), 8, args.payload.read_bytes())
        else:
            selection.set_uris(uris)
        print("SENT " + json.dumps(uris), flush=True)

    window.connect("drag-begin", lambda *_: print("STARTED", flush=True))
    window.connect("drag-data-get", send)
    window.connect("drag-end", lambda *_: print("FINISHED", flush=True))
    window.show_all()
    print("READY", flush=True)
    Gtk.main()


if __name__ == "__main__":
    main()
