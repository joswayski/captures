#!/usr/bin/env python3
"""Measure Windows keyboard delivery before changing the native shortcut transport.

Uses real RegisterHotKey, SendInput and an unfiltered Win32 message pump, not
posted fake keyboard messages. This is a transport prerequisite, not a Captures
host smoke or physical-keyboard acceptance. Input injection requires an explicit
opt-in and a disposable interactive Windows desktop; never use a personal runner.
"""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-input-injection", action="store_true")
    parser.add_argument("--no-hotkeys", action="store_true", help="also test RIDEV_NOHOTKEYS")
    parser.add_argument("--foreground", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("requires an interactive Windows desktop")
    if not args.allow_input_injection:
        parser.error("requires --allow-input-injection on a disposable desktop")
    if not args.foreground and args.output is None:
        parser.error("--output is required")

    user = c.WinDLL("user32", use_last_error=True)
    kernel = c.WinDLL("kernel32", use_last_error=True)
    WndProc = c.WINFUNCTYPE(c.c_ssize_t, w.HWND, w.UINT, w.WPARAM, w.LPARAM)

    class WndClass(c.Structure):
        _fields_ = [("style", w.UINT), ("proc", WndProc), ("class_extra", c.c_int),
                    ("window_extra", c.c_int), ("instance", w.HINSTANCE),
                    ("icon", w.HICON), ("cursor", w.HANDLE), ("background", w.HBRUSH),
                    ("menu", w.LPCWSTR), ("name", w.LPCWSTR)]

    class Device(c.Structure):
        _fields_ = [("page", w.USHORT), ("usage", w.USHORT), ("flags", w.DWORD),
                    ("target", w.HWND)]

    class Header(c.Structure):
        _fields_ = [("kind", w.DWORD), ("size", w.DWORD), ("device", w.HANDLE),
                    ("wparam", w.WPARAM)]

    class Keyboard(c.Structure):
        _fields_ = [("scan", w.USHORT), ("flags", w.USHORT), ("reserved", w.USHORT),
                    ("vk", w.USHORT), ("message", w.UINT), ("extra", w.ULONG)]

    class KeyInput(c.Structure):
        _fields_ = [("vk", w.WORD), ("scan", w.WORD), ("flags", w.DWORD),
                    ("time", w.DWORD), ("extra", c.c_size_t)]

    class MouseInput(c.Structure):
        _fields_ = [("dx", w.LONG), ("dy", w.LONG), ("data", w.DWORD),
                    ("flags", w.DWORD), ("time", w.DWORD), ("extra", c.c_size_t)]

    class InputUnion(c.Union):
        _fields_ = [("key", KeyInput), ("mouse", MouseInput)]

    class Input(c.Structure):
        _fields_ = [("kind", w.DWORD), ("data", InputUnion)]

    def api(dll, name, result, *arguments):
        fn = getattr(dll, name)
        fn.restype = result
        fn.argtypes = arguments
        return fn

    api(kernel, "GetModuleHandleW", w.HMODULE, w.LPCWSTR)
    api(user, "RegisterClassW", w.ATOM, c.POINTER(WndClass))
    api(user, "UnregisterClassW", w.BOOL, w.LPCWSTR, w.HINSTANCE)
    api(user, "CreateWindowExW", w.HWND, w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD,
        c.c_int, c.c_int, c.c_int, c.c_int, w.HWND, w.HMENU, w.HINSTANCE, w.LPVOID)
    api(user, "DefWindowProcW", c.c_ssize_t, w.HWND, w.UINT, w.WPARAM, w.LPARAM)
    api(user, "DestroyWindow", w.BOOL, w.HWND)
    api(user, "ShowWindow", w.BOOL, w.HWND, c.c_int)
    api(user, "SetForegroundWindow", w.BOOL, w.HWND)
    api(user, "GetForegroundWindow", w.HWND)
    api(user, "AllowSetForegroundWindow", w.BOOL, w.DWORD)
    api(user, "PeekMessageW", w.BOOL, c.POINTER(w.MSG), w.HWND, w.UINT, w.UINT, w.UINT)
    api(user, "TranslateMessage", w.BOOL, c.POINTER(w.MSG))
    api(user, "DispatchMessageW", c.c_ssize_t, c.POINTER(w.MSG))
    api(user, "RegisterHotKey", w.BOOL, w.HWND, c.c_int, w.UINT, w.UINT)
    api(user, "UnregisterHotKey", w.BOOL, w.HWND, c.c_int)
    api(user, "RegisterRawInputDevices", w.BOOL, c.POINTER(Device), w.UINT, w.UINT)
    api(user, "GetRegisteredRawInputDevices", w.UINT, c.POINTER(Device),
        c.POINTER(w.UINT), w.UINT)
    api(user, "GetRawInputData", w.UINT, w.HANDLE, w.UINT, w.LPVOID,
        c.POINTER(w.UINT), w.UINT)
    api(user, "SendInput", w.UINT, w.UINT, c.POINTER(Input), c.c_int)
    api(user, "GetAsyncKeyState", w.SHORT, c.c_int)
    api(user, "GetKeyboardLayout", w.HANDLE, w.DWORD)
    api(user, "GetKeyboardLayoutList", c.c_int, c.c_int, c.POINTER(w.HANDLE))
    api(user, "LoadKeyboardLayoutW", w.HANDLE, w.LPCWSTR, w.UINT)
    api(user, "ActivateKeyboardLayout", w.HANDLE, w.HANDLE, w.UINT)
    api(user, "UnloadKeyboardLayout", w.BOOL, w.HANDLE)

    def checked(value, description):
        if not value:
            raise OSError(f"{description}: {c.WinError(c.get_last_error())}")
        return value

    events, errors = [], []

    @WndProc
    def receive(hwnd, message, wp, lp):
        # Exceptions cannot escape a ctypes callback; rethrow from the pump.
        try:
            if message == 0x00FF:  # WM_INPUT
                size = w.UINT()
                assert user.GetRawInputData(lp, 0x10000003, None, c.byref(size),
                                            c.sizeof(Header)) == 0
                buffer = c.create_string_buffer(size.value)
                assert user.GetRawInputData(lp, 0x10000003, buffer, c.byref(size),
                                            c.sizeof(Header)) == size.value
                header = Header.from_buffer_copy(buffer)
                assert header.kind == 1, "keyboard-only subscription received another device class"
                key = Keyboard.from_buffer_copy(buffer, c.sizeof(Header))
                events.append({"kind": "raw", "vk": key.vk, "up": bool(key.flags & 1),
                               "scan": key.scan, "flags": key.flags,
                               "device": header.device, "delivery": int(wp),
                               "extra": key.extra})
            elif message == 0x0312:  # WM_HOTKEY
                events.append({"kind": "hotkey", "id": int(wp)})
            elif message in (0x0100, 0x0101, 0x0104, 0x0105):
                events.append({"kind": "legacy", "vk": int(wp),
                               "up": message in (0x0101, 0x0105)})
        except Exception as error:
            errors.append(str(error))
        # WM_INPUT foreground delivery requires DefWindowProc cleanup.
        return user.DefWindowProcW(hwnd, message, wp, lp)

    def pump(seconds):
        deadline = time.monotonic() + seconds
        message = w.MSG()
        while time.monotonic() < deadline:
            while user.PeekMessageW(c.byref(message), None, 0, 0, 1):
                user.TranslateMessage(c.byref(message))
                user.DispatchMessageW(c.byref(message))
            assert not errors, errors
            time.sleep(.002)

    instance = kernel.GetModuleHandleW(None)
    name = "CapturesNativeInputProbe"
    cls = WndClass(proc=receive, instance=instance, name=name)
    checked(user.RegisterClassW(c.byref(cls)), "RegisterClass")
    hwnd = checked(user.CreateWindowExW(0, name, name, 0x00CF0000, 80, 80, 320, 160,
                                       None, None, instance, None), "CreateWindow")
    child = None
    registrations = []
    registered_raw = False
    injection_started = False
    original_layout = user.GetKeyboardLayout(0)
    layouts = (w.HANDLE * user.GetKeyboardLayoutList(0, None))()
    user.GetKeyboardLayoutList(len(layouts), layouts)
    loaded_layout = None
    raw_flags = 0x100 | (0x200 if args.no_hotkeys else 0)
    result = {"passed": False, "checks": [], "cases": [], "failures": [],
              "raw_flags": raw_flags, "physical_acceptance": False}

    def foreground(target):
        user.ShowWindow(target, 5)
        checked(user.SetForegroundWindow(target), "SetForegroundWindow")
        pump(.2)
        assert user.GetForegroundWindow() == target, "test window does not own keyboard focus"

    def inject(edges, scan_code=False):
        nonlocal injection_started
        inputs = (Input * len(edges))()
        for value, (key, up) in zip(inputs, edges):
            value.kind = 1
            value.data.key = KeyInput(0 if scan_code else key, key if scan_code else 0,
                                     (8 if scan_code else 0) | (2 if up else 0), 0, 0xCA970001)
        injection_started = True
        assert user.SendInput(len(inputs), inputs, c.sizeof(Input)) == len(inputs), c.WinError()

    def chord(key=0x78, modifiers=(0x11, 0x10)):
        return ([(vk, False) for vk in modifiers] + [(key, False), (key, True)]
                + [(vk, True) for vk in reversed(modifiers)])

    def register(identifier, modifiers, key):
        checked(user.RegisterHotKey(hwnd, identifier, modifiers | 0x4000, key), "RegisterHotKey")
        registrations.append(identifier)

    def check_case(label, edges, hotkeys, background=False, scan_code=False, expected=None):
        events.clear()
        inject(edges, scan_code=scan_code)
        # Deliberately stop servicing the UI until the entire burst is queued.
        # A WM_HOTKEY-make/raw-break hybrid loses correlation in this state.
        time.sleep(.2)
        pump(.5)
        observed = list(events)
        result["cases"].append({"name": label, "events": observed})
        raw = [event for event in observed if event["kind"] == "raw"]
        matches = [(event["vk"], event["up"]) for event in raw] == (expected or edges)
        if not matches:
            result["failures"].append(f"{label}: raw make/break sequence differs from injection")
        if not args.no_hotkeys:
            assert sorted(event["id"] for event in observed if event["kind"] == "hotkey") == sorted(hotkeys), label
        if background:
            assert all(event["delivery"] == 1 for event in raw), "not RIM_INPUTSINK delivery"
        if matches:
            result["checks"].append(label)

    try:
        if args.foreground:
            print(hwnd, flush=True)  # Win32 GUI initialization must precede the grant.
            assert sys.stdin.readline() == "focus\n", "parent did not grant foreground permission"
            foreground(hwnd)
            print("focused", flush=True)
            pump(30)
            return
        foreground(hwnd)
        assert c.sizeof(Input) == (40 if c.sizeof(c.c_void_p) == 8 else 28)
        assert not any(user.GetAsyncKeyState(key) & 0x8000
                       for key in (0x10, 0x11, 0x12, 0x5B, 0x5C, 0x78, 0x59)), "keys already held"
        device = Device(1, 6, raw_flags, hwnd)  # keyboard only, RIDEV_INPUTSINK
        checked(user.RegisterRawInputDevices(c.byref(device), 1, c.sizeof(Device)), "raw registration")
        registered_raw = True
        count = w.UINT(1)
        actual = Device()
        assert user.GetRegisteredRawInputDevices(c.byref(actual), c.byref(count), c.sizeof(Device)) == 1
        assert (actual.page, actual.usage, actual.flags, actual.target) == (1, 6, raw_flags, hwnd)
        result["checks"].append("background-keyboard-only-subscription")
        check_case("unregistered-control", chord(), [])
        register(7, 2 | 4, 0x78)  # Ctrl+Shift+F9
        register(8, 2 | 1, 0x78)  # Ctrl+Alt+F9: same base key, distinct chord
        check_case("foreground-two-complete-taps", chord() * 2, [7, 7])
        check_case("same-base-distinct-modifiers", chord() + chord(modifiers=(0x11, 0x12)), [7, 8])
        repeat = [(0x11, False), (0x10, False)] + [(0x78, False)] * 3
        check_case("held-key-repeat", repeat + [(0x78, True), (0x10, True), (0x11, True)], [7])
        check_case("modifiers-released-before-key", [(0x11, False), (0x10, False),
                   (0x78, False), (0x10, True), (0x11, True), (0x78, True)], [7])

        # The sink must work while a DIFFERENT PROCESS owns keyboard focus.
        child = subprocess.Popen([sys.executable, __file__, "--allow-input-injection", "--foreground"],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        child_hwnd = int(child.stdout.readline())
        checked(user.AllowSetForegroundWindow(child.pid), "AllowSetForegroundWindow")
        child.stdin.write("focus\n")
        child.stdin.flush()
        assert child.stdout.readline() == "focused\n"
        assert user.GetForegroundWindow() == child_hwnd
        check_case("background-two-complete-taps", chord() * 2, [7, 7], background=True)
        assert user.GetForegroundWindow() == child_hwnd
        child.terminate()
        child.wait(timeout=5)
        child = None

        foreground(hwnd)
        loaded_layout = checked(user.LoadKeyboardLayoutW("00000407", 1 | 0x80), "German layout")
        scan_edges = [(0x1D, False), (0x2A, False), (0x2C, False),
                      (0x2C, True), (0x2A, True), (0x1D, True)]
        check_case("german-unregistered-control", scan_edges, [], scan_code=True, expected=chord(0x59))
        raw_y = [event for event in events if event["kind"] == "raw" and event["vk"] == 0x59]
        assert [event["scan"] for event in raw_y] == [0x2C, 0x2C]
        register(9, 2 | 4, 0x59)  # VK_Y, which is physical US KeyZ on QWERTZ.
        check_case("german-vk-y-physical-z", scan_edges, [9], scan_code=True, expected=chord(0x59))
        assert not result["failures"], result["failures"]
        result["passed"] = True
    except Exception as error:
        result["error"] = repr(error)
        raise
    finally:
        if child is not None:
            child.terminate()
            child.wait(timeout=5)
        if not args.foreground:
            # Release our input after interruption, but never release pre-held
            # keys when the safety check rejected the desktop before injection.
            if injection_started:
                try:
                    inject([(key, True) for key in (0x78, 0x59, 0x10, 0x11, 0x12)])
                except Exception as error:
                    result["passed"] = False
                    result["cleanup_error"] = repr(error)
            user.ActivateKeyboardLayout(original_layout, 0)
            if loaded_layout and loaded_layout not in layouts:
                user.UnloadKeyboardLayout(loaded_layout)
            for identifier in registrations:
                user.UnregisterHotKey(hwnd, identifier)
            if registered_raw:
                device = Device(1, 6, 1, None)  # RIDEV_REMOVE requires a null target.
                user.RegisterRawInputDevices(c.byref(device), 1, c.sizeof(Device))
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        user.DestroyWindow(hwnd)
        user.UnregisterClassW(name, instance)
    assert result["passed"], result
    print(f"PASS Windows keyboard transport: {len(result['checks'])} checks")


if __name__ == "__main__":
    main()
