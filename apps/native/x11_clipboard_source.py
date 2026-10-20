#!/usr/bin/python3
"""Private X11 clipboard owner exercising real INCR transfers, not app hooks.

The editor smoke owns its display and this process. Chunks advance only when
the requestor deletes the preceding property; an empty property ends the stream.
"""
import argparse
import json
import select
import time
from pathlib import Path

from Xlib import X, Xatom, display, error, protocol


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--payload", type=Path, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--png", type=Path)
    parser.add_argument("--png-incr", action="store_true",
                        help="Offer only PNG and transfer it through INCR")
    parser.add_argument("--replacement-png", type=Path)
    parser.add_argument("--replace-on", choices=("text/uri-list", "image/png"), default="text/uri-list",
                        help="Reacquire the same owner window during this format's read")
    parser.add_argument("--targets-incr", action="store_true")
    parser.add_argument("--mode", choices=["normal", "exact-limit", "oversized-hint", "overflow",
                        "missing-end", "slow", "wrong-type", "wrong-terminator", "malformed-header"],
                        default="normal")
    args = parser.parse_args()
    connection = display.Display()
    source = connection.screen().root.create_window(0, 0, 1, 1, 0, 0, X.InputOnly,
                                                    X.CopyFromParent)
    atoms = {name: connection.intern_atom(name) for name in (
        "CLIPBOARD", "TARGETS", "INCR", "text/uri-list", "UTF8_STRING", "image/png")}
    payload = args.payload.read_bytes()
    png = args.png.read_bytes() if args.png else None
    replaced = False
    formats = [atoms["TARGETS"]]
    if not args.png_incr:
        formats.append(atoms["text/uri-list"])
    if png is not None:
        # Keep egui's eager text read from consuming the URI offer first.
        formats.extend([atoms["image/png"], atoms["UTF8_STRING"]])
    if args.mode == "exact-limit":
        payload += b"#" + b"x" * (1024 * 1024 - len(payload) - 3) + b"\r\n"
    if args.mode == "overflow":
        payload += b"#" * (1024 * 1024)
    transfers = {}

    def record(event, **detail):
        with args.log.open("a") as out:
            out.write(json.dumps({"event": event, **detail}) + "\n")

    def x_error(failure, _request):
        # An error/timeout destroys the app's private requestor window.
        if isinstance(failure, error.BadWindow):
            record("requestor-gone")
        else:
            raise failure

    connection.set_error_handler(x_error)
    source.set_selection_owner(atoms["CLIPBOARD"], X.CurrentTime)
    connection.sync()
    print("ready", flush=True)
    try:
        while True:
            while connection.pending_events():
                event = connection.next_event()
                if event.type == X.SelectionClear:
                    return
                if event.type == X.DestroyNotify:
                    transfers = {key: value for key, value in transfers.items()
                                 if key[0] != event.window.id}
                elif event.type == X.SelectionRequest:
                    property_ = event.property or event.target
                    if args.replacement_png and event.target == atoms[args.replace_on] and not replaced:
                        png = args.replacement_png.read_bytes()
                        source.set_selection_owner(atoms["CLIPBOARD"], X.CurrentTime)
                        connection.sync()
                        replaced = True
                        record("replaced", same_owner=True)
                    if event.target == atoms["TARGETS"] and not args.targets_incr:
                        event.requestor.change_property(property_, Xatom.ATOM, 32, formats)
                    elif png is not None and event.target == atoms["image/png"] and not args.png_incr:
                        event.requestor.change_property(property_, atoms["image/png"], 8, png)
                        record("png", replaced=replaced)
                    elif png is not None and event.target == atoms["UTF8_STRING"]:
                        event.requestor.change_property(property_, atoms["UTF8_STRING"], 8, b"")
                    elif event.target in (atoms["TARGETS"], atoms["text/uri-list"]) or (
                            args.png_incr and png is not None and event.target == atoms["image/png"]):
                        targets = event.target == atoms["TARGETS"]
                        is_png = event.target == atoms["image/png"]
                        values = formats if targets else png if is_png else payload
                        event.requestor.change_attributes(event_mask=X.PropertyChangeMask | X.StructureNotifyMask)
                        transfers[(event.requestor.id, property_)] = (values, 0, event.target)
                        if args.mode == "malformed-header" and not targets:
                            event.requestor.change_property(property_, atoms["INCR"], 8, b"bad")
                        else:
                            # Zero is a valid lower bound, not the final byte count.
                            maximum = (256 if is_png else 1) * 1024 * 1024
                            hint = maximum + 1 if args.mode == "oversized-hint" and not targets else 0
                            event.requestor.change_property(property_, atoms["INCR"], 32, [hint])
                        record("incr", target="TARGETS" if targets else "image/png" if is_png else "text/uri-list")
                    else:
                        property_ = X.NONE
                    connection.send_event(event.requestor, protocol.event.SelectionNotify(
                        time=event.time, requestor=event.requestor, selection=event.selection,
                        target=event.target, property=property_), event_mask=0)
                    connection.flush()
                elif event.type == X.PropertyNotify and event.state == X.PropertyDelete:
                    key = event.window.id, event.atom
                    if key not in transfers:
                        continue
                    values, offset, target = transfers[key]
                    targets = target == atoms["TARGETS"]
                    is_png = target == atoms["image/png"]
                    # Force escape/CRLF boundaries independently of path lengths.
                    # Large cap probes finish well inside the deadline instead.
                    size = 1 if targets else (65536 if is_png or args.mode in ("exact-limit", "overflow") else 13)
                    end = offset + size
                    if not targets and size == 13:
                        for separator in (b"%", b"\r"):
                            split = values.find(separator, offset, end)
                            if split >= 0:
                                end = min(end, split + 1)
                    chunk = values[offset:end]
                    if not chunk and args.mode == "missing-end" and not targets:
                        record("withheld-terminator")
                        continue
                    if args.mode == "slow" and not targets:
                        # Progress must not reset the app's absolute deadline.
                        time.sleep(.35)
                    type_ = Xatom.ATOM if targets else target
                    if not targets and (args.mode == "wrong-type"
                                        or (not chunk and args.mode == "wrong-terminator")):
                        type_ = atoms["UTF8_STRING"]
                    event.window.change_property(event.atom, type_, 32 if targets else 8, chunk)
                    connection.flush()
                    record("chunk", target="TARGETS" if targets else "image/png" if is_png else "text/uri-list", count=len(chunk))
                    if not targets and not is_png and chunk and chunk[-1:] in (b"%", b"\r"):
                        record("split", kind="escape" if chunk[-1:] == b"%" else "CRLF")
                    if chunk:
                        transfers[key] = values, offset + len(chunk), target
                    else:
                        del transfers[key]
            select.select([connection.display.socket], [], [], .05)
    finally:
        connection.close()


if __name__ == "__main__":
    main()
