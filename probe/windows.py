"""Holder registers Ctrl+Shift+Space; a second process registers it the way
global-hotkey does (RegisterHotKey) and reports the result."""
import ctypes, subprocess, sys, time
from ctypes import wintypes as w
user = ctypes.WinDLL("user32", use_last_error=True)
user.RegisterHotKey.argtypes = [w.HWND, ctypes.c_int, w.UINT, w.UINT]
MODS, VK = 0x0002 | 0x0004 | 0x4000, 0x20  # Ctrl+Shift, norepeat; Space
def register(name):
    ok = user.RegisterHotKey(None, 1, MODS, VK)
    print(f"{name}: RegisterHotKey ok={bool(ok)} last_error={ctypes.get_last_error()}", flush=True)
if len(sys.argv) > 1:
    register(sys.argv[1])
    if sys.argv[1] == "holder": time.sleep(5)
    sys.exit()
holder = subprocess.Popen([sys.executable, __file__, "holder"])
time.sleep(2)
subprocess.run([sys.executable, __file__, "captures"])
holder.wait()
