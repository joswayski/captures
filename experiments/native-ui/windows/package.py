"""Assemble a private-test runtime from an MSYS2 MinGW prefix, also on Linux.

This is not a release packager or a substitute for a redistribution license audit.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess


# Windows-provided libraries; everything else must resolve from the supplied
# prefix. In particular, the GNU runtime and GTK plugin imports are bundled.
SYSTEM_DLLS = set("""
advapi32 avicap32 avrt bcrypt bcryptprimitives cfgmgr32 combase comctl32 comdlg32 crypt32
d3d11 d3d12 d3d9 dcomp dnsapi dsound dwmapi dwrite dxgi dxva2 gdi32 gdiplus glu32
hid imm32 iphlpapi kernel32 ksuser mf mfplat mfuuid mfreadwrite mpr
msacm32 msimg32 msvcrt msvfw32 ncrypt netapi32 normaliz ntdll ole32 oleacc oleaut32
opengl32 powrprof propsys psapi rpcrt4 secur32 setupapi shell32 shcore
shlwapi strmiids user32 userenv usp10 uxtheme version win32u winhttp
wininet winmm winspool wintrust ws2_32 wsock32 wtsapi32
""".split())


def imports(binary, objdump):
    result = subprocess.run([objdump, "-p", str(binary)], check=True,
                            capture_output=True, text=True)
    return re.findall(r"DLL Name:\s*(\S+)", result.stdout)


def package(prefix, binary, output, objdump):
    # Never erase a caller-supplied directory, even after a failed build.
    output.mkdir(parents=True, exist_ok=False)
    (output / "bin").mkdir()
    queue = []

    def copy(source, destination):
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        queue.append(source)

    copy(binary, output / "bin" / binary.name)
    for name in ["gdk-pixbuf-query-loaders", "ffmpeg", "ffprobe", "ffplay"]:
        copy(prefix / "bin" / f"{name}.exe", output / "bin" / f"{name}.exe")
    for relative in ["lib/gdk-pixbuf-2.0/2.10.0", "share/glib-2.0/schemas",
                     "share/icons/Adwaita", "share/icons/hicolor",
                     "etc/fonts", "share/fontconfig", "share/licenses"]:
        source = prefix / relative
        shutil.copytree(source, output / relative)
    # The host compiler emits platform-independent GVariant schemas.
    subprocess.run(["glib-compile-schemas", str(output / "share/glib-2.0/schemas")], check=True)
    loaders = output / "lib/gdk-pixbuf-2.0/2.10.0"
    (loaders / "loaders.cache").unlink(missing_ok=True)
    plugin_sources = list((prefix / "lib/gdk-pixbuf-2.0/2.10.0/loaders").glob("*.dll"))
    if not any("svg" in p.name for p in plugin_sources):
        raise RuntimeError("SVG pixbuf loader missing; install the matching librsvg package")
    queue.extend(plugin_sources)
    available = {p.name.lower(): p for p in (prefix / "bin").glob("*.dll")}
    copied = set()
    while queue:
        source = queue.pop()
        for dll in imports(source, objdump):
            name = dll.lower()
            if name in copied or Path(name).stem in SYSTEM_DLLS or name.startswith(("api-ms-win-", "ext-ms-win-")):
                continue
            if name not in available:
                raise RuntimeError(f"Unresolved {dll} imported by {source}")
            copied.add(name)
            copy(available[name], output / "bin" / available[name].name)
    root = Path(__file__).resolve().parents[3]
    shutil.copy2(root / "LICENSE", output / "Captures-LICENSE.txt")
    shutil.copy2(Path(__file__).with_name("README.md"), output / "README.md")
    shutil.copy2(Path(__file__).with_name("benchmark.ps1"), output / "benchmark.ps1")
    (output / "Captures.cmd").write_text(
        '@echo off\r\n"%~dp0bin\\captures-windows-native.exe" %*\r\n', encoding="ascii")
    files = {str(p.relative_to(output)).replace("\\", "/"): {
        "bytes": p.stat().st_size, "sha256": hashlib.sha256(p.read_bytes()).hexdigest()
    } for p in sorted(output.rglob("*")) if p.is_file()}
    (output / "files.json").write_text(json.dumps(files, indent=2) + "\n")
    print(f"Packaged {len(copied)} non-system DLLs, SVG loaders and media tools in {output}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--objdump", default="objdump")
    args = parser.parse_args()
    package(args.prefix.resolve(), args.binary.resolve(), args.output.resolve(), args.objdump)
