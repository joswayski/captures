#!/usr/bin/env python3
"""Check native Windows image acquisition in a private window station.

Never opens, reads, or writes the interactive station's clipboard. No desktop
switching and no fallback when isolation fails. This is not physical acceptance.
"""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
import os
from pathlib import Path
import struct
import subprocess
import time
import uuid
import zlib


def png(pixels):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    rows = b"".join(b"\0" + bytes(pixels[index:index + 4]) for index in range(0, len(pixels), 4))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 1, len(pixels) // 4, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def dib(pixels):
    data = bytearray(124)
    struct.pack_into("<IiiHHI", data, 0, 124, 1, -(len(pixels) // 4), 1, 32, 0)
    struct.pack_into("<II", data, 52, 0xff000000, 0x73524742)
    for index in range(0, len(pixels), 4):
        r, g, b, a = pixels[index:index + 4]
        data.extend((b, g, r, a))
    return bytes(data)


def files(paths):
    return struct.pack("<IiiII", 20, 0, 0, 0, 1) + ("\0".join(map(str, paths)) + "\0\0").encode("utf-16-le")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if os.name != "nt":
        raise SystemExit("Windows is required; no clipboard was accessed")
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    user = c.WinDLL("user32", use_last_error=True)
    kernel = c.WinDLL("kernel32", use_last_error=True)

    def api(dll, name, result, *arguments):
        function = getattr(dll, name)
        function.restype, function.argtypes = result, arguments
        return function

    def check(value):
        if not value:
            raise c.WinError(c.get_last_error())
        return value

    create_station = api(user, "CreateWindowStationW", w.HANDLE, w.LPCWSTR, w.DWORD, w.DWORD, c.c_void_p)
    set_station = api(user, "SetProcessWindowStation", w.BOOL, w.HANDLE)
    get_station = api(user, "GetProcessWindowStation", w.HANDLE)
    create_desktop = api(user, "CreateDesktopW", w.HANDLE, w.LPCWSTR, c.c_void_p, c.c_void_p, w.DWORD, w.DWORD, c.c_void_p)
    set_desktop = api(user, "SetThreadDesktop", w.BOOL, w.HANDLE)
    get_desktop = api(user, "GetThreadDesktop", w.HANDLE, w.DWORD)
    get_tid = api(kernel, "GetCurrentThreadId", w.DWORD)
    get_info = api(user, "GetUserObjectInformationW", w.BOOL, w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.POINTER(w.DWORD))
    open_clipboard = api(user, "OpenClipboard", w.BOOL, w.HWND)
    close_clipboard = api(user, "CloseClipboard", w.BOOL)
    empty = api(user, "EmptyClipboard", w.BOOL)
    set_data = api(user, "SetClipboardData", w.HANDLE, w.UINT, w.HANDLE)
    alloc = api(kernel, "GlobalAlloc", w.HANDLE, w.UINT, c.c_size_t)
    lock = api(kernel, "GlobalLock", c.c_void_p, w.HANDLE)
    unlock = api(kernel, "GlobalUnlock", w.BOOL, w.HANDLE)
    free = api(kernel, "GlobalFree", w.HANDLE, w.HANDLE)
    peek = api(user, "PeekMessageW", w.BOOL, c.POINTER(w.MSG), w.HWND, w.UINT, w.UINT, w.UINT)
    dispatch = api(user, "DispatchMessageW", c.c_ssize_t, c.POINTER(w.MSG))
    wait = api(kernel, "WaitForSingleObject", w.DWORD, w.HANDLE, w.DWORD)
    close_handle = api(kernel, "CloseHandle", w.BOOL, w.HANDLE)
    def_proc = api(user, "DefWindowProcW", c.c_ssize_t, w.HWND, w.UINT, w.WPARAM, w.LPARAM)
    callback_type = c.WINFUNCTYPE(c.c_ssize_t, w.HWND, w.UINT, w.WPARAM, w.LPARAM)

    class WindowClass(c.Structure):
        _fields_ = [("style", w.UINT), ("proc", callback_type), ("cls_extra", c.c_int), ("wnd_extra", c.c_int),
                    ("instance", w.HINSTANCE), ("icon", w.HICON), ("cursor", w.HANDLE), ("background", w.HBRUSH),
                    ("menu", w.LPCWSTR), ("name", w.LPCWSTR)]

    class Startup(c.Structure):
        _fields_ = [("size", w.DWORD), ("reserved", w.LPWSTR), ("desktop", w.LPWSTR), ("title", w.LPWSTR),
                    ("x", w.DWORD), ("y", w.DWORD), ("width", w.DWORD), ("height", w.DWORD),
                    ("cols", w.DWORD), ("rows", w.DWORD), ("fill", w.DWORD), ("flags", w.DWORD),
                    ("show", w.WORD), ("reserved_size", w.WORD), ("reserved_ptr", c.c_void_p),
                    ("stdin", w.HANDLE), ("stdout", w.HANDLE), ("stderr", w.HANDLE)]

    class Process(c.Structure):
        _fields_ = [("process", w.HANDLE), ("thread", w.HANDLE), ("pid", w.DWORD), ("tid", w.DWORD)]

    create_process = api(kernel, "CreateProcessW", w.BOOL, w.LPCWSTR, w.LPWSTR, c.c_void_p, c.c_void_p,
                         w.BOOL, w.DWORD, c.c_void_p, w.LPCWSTR, c.POINTER(Startup), c.POINTER(Process))
    original_station, original_desktop = get_station(), get_desktop(get_tid())
    station_name = "CapturesClipboard-" + uuid.uuid4().hex
    # CREATE_ONLY avoids accidentally attaching to any existing station.
    station = check(create_station(station_name, 1, 0x10000000, None))
    desktop = hwnd = None
    results, events, callback_errors = [], [], []
    offered, replace = {}, False
    armed = False
    pixels = [11, 73, 211, 255, 31, 51, 91, 73, 53, 97, 17, 0]
    replacement = [199, 17, 2, 255]

    def publish(format_id, data):
        handle = check(alloc(2, len(data)))  # GMEM_MOVEABLE
        try:
            address = check(lock(handle))
            c.memmove(address, data, len(data))
            unlock(handle)
            check(set_data(format_id, handle))
        except BaseException:
            free(handle)
            raise

    @callback_type
    def owner(window, message, parameter, other):
        nonlocal armed
        if message == 0x0305:  # WM_RENDERFORMAT: do not OpenClipboard here.
            try:
                if offered[parameter] is not None:
                    publish(parameter, offered[parameter])
                events.append(f"render:{parameter}")
                if parameter == 15 and replace:
                    armed = True
            except BaseException as error:
                callback_errors.append(str(error))
            return 0
        return def_proc(window, message, parameter, other)

    try:
        check(set_station(station))
        desktop = check(create_desktop("Default", None, None, 0, 0x10000000, None))
        check(set_desktop(desktop))
        name, flags, needed = c.create_unicode_buffer(256), (w.DWORD * 3)(), w.DWORD()
        check(get_info(get_station(), 2, name, c.sizeof(name), c.byref(needed)))
        check(get_info(get_station(), 1, flags, c.sizeof(flags), c.byref(needed)))
        if name.value != station_name or flags[2] & 1:
            raise RuntimeError("Refusing interactive clipboard access")
        instance = api(kernel, "GetModuleHandleW", w.HMODULE, w.LPCWSTR)(None)
        cls = WindowClass(proc=owner, instance=instance, name=station_name)
        check(api(user, "RegisterClassW", w.ATOM, c.POINTER(WindowClass))(c.byref(cls)))
        hwnd = check(api(user, "CreateWindowExW", w.HWND, w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD,
                         c.c_int, c.c_int, c.c_int, c.c_int, w.HWND, w.HMENU, w.HINSTANCE, c.c_void_p)
                     (0, station_name, "Clipboard fixture", 0, 0, 0, 1, 1, None, None, instance, None))
        png_format = check(api(user, "RegisterClipboardFormatW", w.UINT, w.LPCWSTR)("PNG"))
        image_path, text_path = output / "source.png", output / "ignored.txt"
        image_path.write_bytes(png(replacement))
        text_path.write_text("not an image", encoding="utf-8")
        broken_path = output / "broken.png"
        broken_path.write_bytes(b"broken")
        cases = [
            ("empty", {}, [], False),
            ("text-only", {13: "ignored\0".encode("utf-16-le")}, [], False),
            ("delayed-png", {png_format: png(pixels)}, pixels, False),
            ("delayed-dib", {17: dib(pixels)}, pixels, False),
            ("png-priority", {png_format: png(pixels), 17: dib(replacement)}, pixels, False),
            ("bad-png-no-dib-fallback", {png_format: b"broken", 17: dib(pixels)}, "error", False),
            ("files-priority", {15: files([image_path]), png_format: b"broken"}, replacement, False),
            ("files-ignore-unreadable-fallback", {15: files([image_path]), png_format: None}, replacement, False),
            ("bad-file-no-png-fallback", {15: files([broken_path]), png_format: png(pixels)}, "error", False),
            ("replacement-during-files", {15: files([text_path] * 15000), png_format: png(pixels)}, pixels, True),
        ]
        for label, offered, expected, replace in cases:
            events.clear()
            armed = False
            check(open_clipboard(hwnd))
            try:
                check(empty())
                for format_id in offered:
                    # NULL publishes a delayed format and is also the normal
                    # return value; availability is checked separately.
                    set_data(format_id, None)
                    check(api(user, "IsClipboardFormatAvailable", w.BOOL, w.UINT)(format_id))
            finally:
                check(close_clipboard())
            result_path = output / f"{label}.json"
            result_path.unlink(missing_ok=True)
            startup = Startup(size=c.sizeof(Startup), desktop=station_name + "\\Default")
            process = Process()
            command = c.create_unicode_buffer(subprocess.list2cmdline([str(binary), "--clipboard-probe", str(result_path)]))
            check(create_process(None, command, None, None, False, 0, None, None, c.byref(startup), c.byref(process)))
            try:
                deadline = time.monotonic() + 30
                while True:
                    message = w.MSG()
                    while peek(c.byref(message), None, 0, 0, 1):
                        dispatch(c.byref(message))
                    if armed and open_clipboard(hwnd):
                        try:
                            check(empty())
                            publish(png_format, png(replacement))
                            events.append("replacement")
                            armed = False
                        finally:
                            check(close_clipboard())
                    if wait(process.process, 0) == 0:
                        break
                    assert time.monotonic() < deadline, f"{label}: probe timeout"
                    time.sleep(0.001)
                assert not callback_errors, callback_errors
                result = json.loads(result_path.read_text())
                if expected == "error":
                    assert "Err" in result, (label, result)
                elif not expected:
                    assert result == {"Ok": []}, (label, result)
                else:
                    images = result.get("Ok", [])
                    assert len(images) == 1 and images[0]["width"] == 1, (label, result)
                    assert images[0]["height"] == len(expected) // 4 and images[0]["rgba"] == expected, (label, result)
                if replace:
                    assert "replacement" in events and f"render:{png_format}" in events, events
                    assert events.index(f"render:{png_format}") < events.index("replacement"), events
                results.append({"case": label, "passed": True, "events": list(events)})
            finally:
                if wait(process.process, 0) != 0:
                    api(kernel, "TerminateProcess", w.BOOL, w.HANDLE, w.UINT)(process.process, 1)
                    wait(process.process, 5000)
                close_handle(process.thread)
                close_handle(process.process)
        report = {"station": station_name, "interactive": False, "cases": results}
        (output / "summary.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(json.dumps(report, indent=2))
    finally:
        if hwnd:
            api(user, "DestroyWindow", w.BOOL, w.HWND)(hwnd)
        check(set_desktop(original_desktop))
        check(set_station(original_station))
        if desktop:
            api(user, "CloseDesktop", w.BOOL, w.HANDLE)(desktop)
        api(user, "CloseWindowStation", w.BOOL, w.HANDLE)(station)


if __name__ == "__main__":
    main()
