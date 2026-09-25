#!/usr/bin/python3
"""Disposable XDND receiver for real native file-transfer tests, not an app hook."""
import argparse
import json
import os
from pathlib import Path
from urllib.parse import unquote_to_bytes, urlsplit

from Xlib import X, Xatom, display, protocol


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--mode", choices=("accept", "reject", "nofinish", "disappear"), default="accept")
    args = parser.parse_args()
    connection = display.Display()
    root = connection.screen().root
    window = root.create_window(600, 480, 320, 200, 0, connection.screen().root_depth,
                                X.InputOutput, X.CopyFromParent, override_redirect=True,
                                background_pixel=0x456789, event_mask=X.ExposureMask)
    atoms = {name: connection.intern_atom(name) for name in (
        "XdndAware", "XdndEnter", "XdndPosition", "XdndStatus", "XdndDrop", "XdndLeave",
        "XdndFinished", "XdndActionCopy", "XdndSelection", "text/uri-list", "TRANSFER")}
    window.change_property(atoms["XdndAware"], Xatom.ATOM, 32, [5])
    window.set_wm_name("XDND receiver")
    window.map()
    connection.sync()
    print(window.id, flush=True)
    source = None

    def record(event, **values):
        with args.output.with_suffix(".jsonl").open("a") as out:
            out.write(json.dumps({"event": event, **values}) + "\n")

    def send(name, values):
        source.send_event(protocol.event.ClientMessage(window=source, client_type=atoms[name],
                                                       data=(32, values)), event_mask=0)
        connection.flush()

    while True:
        event = connection.next_event()
        if event.type == X.ClientMessage:
            values = event.data[1]
            if event.client_type == atoms["XdndEnter"]:
                source = connection.create_resource_object("window", values[0])
                record("enter", types=list(values[2:]))
                assert atoms["text/uri-list"] in values[2:]
            elif event.client_type == atoms["XdndPosition"]:
                assert values[4] == atoms["XdndActionCopy"]
                send("XdndStatus", [window.id, 2 if args.mode == "reject" else 3, 0, 0,
                                     0 if args.mode == "reject" else atoms["XdndActionCopy"]])
                record("position")
            elif event.client_type == atoms["XdndLeave"]:
                record("leave")
            elif event.client_type == atoms["XdndDrop"]:
                record("drop")
                if args.mode == "disappear":
                    window.destroy()
                    connection.sync()
                    return
                window.convert_selection(atoms["XdndSelection"], atoms["text/uri-list"],
                                         atoms["TRANSFER"], values[2])
                connection.flush()
        elif event.type == X.SelectionNotify:
            assert event.property != X.NONE
            value = bytes(window.get_full_property(event.property, atoms["text/uri-list"]).value)
            assert value.endswith(b"\r\n")
            uri = value.decode("ascii").strip()
            path = Path(os.fsdecode(unquote_to_bytes(urlsplit(uri).path)))
            args.output.write_bytes(path.read_bytes())
            record("received", uri=uri, path=str(path))
            if args.mode == "accept":
                # A stale/unrelated completion must not terminate the active source.
                send("XdndFinished", [window.id + 1, 0, 0, 0, 0])
                send("XdndFinished", [window.id, 1, atoms["XdndActionCopy"], 0, 0])


if __name__ == "__main__":
    main()
