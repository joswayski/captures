#!/usr/bin/python3
"""Disposable XDND file source for real native drop tests, not an app hook.

Offers one file as text/uri-list to a target window, serves the XdndSelection
conversion, and drops when stdin receives a line ("drop" or "leave"). The real
pointer stays with the harness, so the target sees ordinary motion events.
"""
import argparse
import json
import select
import sys
from pathlib import Path

from Xlib import X, display, protocol


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", type=lambda value: int(value, 0), required=True)
    parser.add_argument("--file", type=Path, required=True)
    parser.add_argument("--log", type=Path, required=True)
    args = parser.parse_args()
    connection = display.Display()
    screen = connection.screen()
    source = screen.root.create_window(-10, -10, 1, 1, 0, screen.root_depth, X.InputOutput,
                                       X.CopyFromParent, override_redirect=True)
    target = connection.create_resource_object("window", args.target)
    atoms = {name: connection.intern_atom(name) for name in (
        "XdndAware", "XdndEnter", "XdndPosition", "XdndStatus", "XdndDrop", "XdndLeave",
        "XdndFinished", "XdndActionCopy", "XdndSelection", "text/uri-list")}
    uri = (args.file.resolve().as_uri() + "\r\n").encode()
    source.set_selection_owner(atoms["XdndSelection"], X.CurrentTime)
    connection.sync()

    def record(event, **values):
        with args.log.open("a") as out:
            out.write(json.dumps({"event": event, **values}) + "\n")

    def send(name, values):
        target.send_event(protocol.event.ClientMessage(
            window=target, client_type=atoms[name], data=(32, values + [0] * (5 - len(values)))),
            event_mask=0)
        connection.flush()

    send("XdndEnter", [source.id, 5 << 24, atoms["text/uri-list"], 0, 0])
    send("XdndPosition", [source.id, 0, 0, X.CurrentTime, atoms["XdndActionCopy"]])
    print("entered", flush=True)
    finished = False
    while not finished:
        while connection.pending_events():
            event = connection.next_event()
            if event.type == X.SelectionRequest:
                event.requestor.change_property(event.property, atoms["text/uri-list"], 8, uri)
                connection.send_event(event.requestor, protocol.event.SelectionNotify(
                    time=event.time, requestor=event.requestor, selection=event.selection,
                    target=event.target, property=event.property), event_mask=0)
                connection.flush()
                record("converted")
            elif event.type == X.ClientMessage and event.client_type == atoms["XdndStatus"]:
                record("status", accepted=bool(event.data[1][1] & 1))
            elif event.type == X.ClientMessage and event.client_type == atoms["XdndFinished"]:
                record("finished", accepted=bool(event.data[1][1] & 1))
                finished = True
        if select.select([sys.stdin, connection.display.socket], [], [], .05)[0]:
            if sys.stdin in select.select([sys.stdin], [], [], 0)[0]:
                command = sys.stdin.readline().strip()
                if command == "drop":
                    send("XdndDrop", [source.id, 0, X.CurrentTime])
                    record("drop")
                elif command == "position":
                    send("XdndPosition", [source.id, 0, 0, X.CurrentTime,
                                          atoms["XdndActionCopy"]])
                else:
                    send("XdndLeave", [source.id])
                    record("leave")
                    finished = True
    print("done", flush=True)


if __name__ == "__main__":
    main()
