#!/usr/bin/env python3
"""Small independent GTK3 Wayland drop target for wayland_drag_smoke.py."""
import argparse
import json
from pathlib import Path
from urllib.parse import unquote, urlsplit

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, Gtk  # noqa: E402


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--reject", action="store_true")
    parser.add_argument("--no-finish", action="store_true")
    args = parser.parse_args()

    window = Gtk.Window(title="drag-receiver")
    window.set_default_size(300, 220)
    window.connect("destroy", Gtk.main_quit)
    targets = [Gtk.TargetEntry.new("text/uri-list", 0, 0)]
    window.drag_dest_set(Gtk.DestDefaults.MOTION | Gtk.DestDefaults.HIGHLIGHT, targets, Gdk.DragAction.COPY)

    def dropped(widget, context, _x, _y, timestamp):
        widget.drag_get_data(context, Gdk.Atom.intern("text/uri-list", False), timestamp)
        return True

    def received(_widget, context, _x, _y, selection, _info, timestamp):
        raw = bytes(selection.get_data() or b"")
        print("RECEIVED " + json.dumps(raw.decode("utf-8")), flush=True)
        uri = raw.decode("ascii").strip()
        print("BYTES " + Path(unquote(urlsplit(uri).path)).read_bytes().hex(), flush=True)
        if not args.no_finish:
            Gtk.drag_finish(context, not args.reject, False, timestamp)

    window.connect("drag-drop", dropped)
    window.connect("drag-data-received", received)
    window.show_all()
    print("READY", flush=True)
    Gtk.main()


if __name__ == "__main__":
    main()
