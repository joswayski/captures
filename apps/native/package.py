#!/usr/bin/env python3
"""Stage an unsigned native DEVELOPMENT package; never install or register it.

Build first. Registration files contain absolute paths: choose the final output
location before opting into Open With. Existing output is never overwritten.
"""
import argparse
from pathlib import Path
import plistlib
import shutil

ROOT = Path(__file__).resolve().parents[2]
IDENTITY = "es.captur.native-development"
NAME = "Captures Native Development"
FORMATS = (
    ("PNG image", ("png",), "image/png", "public.png"),
    ("JPEG image", ("jpg", "jpeg"), "image/jpeg", "public.jpeg"),
    ("WebP image", ("webp",), "image/webp", "org.webmproject.webp"),
    ("GIF", ("gif",), "image/gif", "com.compuserve.gif"),
    ("MPEG-4 video", ("mp4",), "video/mp4", "public.mpeg-4"),
    ("WebM video", ("webm",), "video/webm", "org.webmproject.webm"),
)


def mac_info():
    info = {
        "CFBundleIdentifier": IDENTITY,
        "CFBundleName": NAME,
        "CFBundleDisplayName": NAME,
        "CFBundleExecutable": "CapturesNative",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": "0.0.0",
        "CFBundleVersion": "1",
        "CFBundleIconFile": "Captures.icns",
        "LSMinimumSystemVersion": "13.0",
        "NSHighResolutionCapable": True,
        "CapturesNativeLive": True,
        "CFBundleDocumentTypes": [
            {"CFBundleTypeName": name, "CFBundleTypeExtensions": list(extensions),
             "CFBundleTypeMIMETypes": [mime], "LSItemContentTypes": [uti],
             "CFBundleTypeRole": "Editor", "LSHandlerRank": "Alternate"}
            for name, extensions, mime, uti in FORMATS
        ],
        "UTImportedTypeDeclarations": [
            {"UTTypeIdentifier": uti, "UTTypeDescription": name,
             "UTTypeConformsTo": ["public.image" if mime.startswith("image/") else "public.movie"],
             "UTTypeTagSpecification": {"public.filename-extension": list(extensions),
                                        "public.mime-type": mime}}
            for name, extensions, mime, uti in FORMATS if uti.startswith("org.webmproject.")
        ],
    }
    # Same purpose strings as shipping; no shipping identity/association changes.
    shipping = plistlib.loads((ROOT / "apps/desktop/src-tauri/Info.plist").read_bytes())
    info.update({key: value for key, value in shipping.items() if key.endswith("UsageDescription")})
    return info


def desktop_entry(binary):
    value = str(binary)
    # GIO validates executable existence before unescaping %% field codes.
    # Reject that staging location instead of generating an unusable launcher.
    if any(char in value for char in "\r\n\0=%"):
        raise ValueError("Desktop executable paths cannot contain newlines, NUL, '=' or '%'")
    # Exec quoting is distinct from shell quoting: first escape the quoted
    # argument, then the desktop-entry string.
    quoted = "".join("\\" + char if char in '\\"`$' else char for char in value)
    quoted = quoted.replace("\\", "\\\\")
    return ("[Desktop Entry]\nType=Application\nVersion=1.0\n"
            f"Name={NAME}\nComment=Experimental native screenshot, GIF and video editor\n"
            f'Exec="{quoted}" --live -- %F\n'
            "Terminal=false\nCategories=Graphics;Utility;\nStartupNotify=false\n"
            f"MimeType={';'.join(item[2] for item in FORMATS)};\n")


def windows_registry(binary):
    value = str(binary)
    if any(char in value for char in '\r\n\0"'):
        raise ValueError("Invalid Windows executable path")

    def string(value):
        return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'

    base = "HKEY_CURRENT_USER\\Software\\Classes"
    progid = "CapturesNativeDevelopment.Media"
    application = base + "\\Applications\\CapturesNative.exe"
    command = f'"{value}" --live -- "%1"'
    sections = ["Windows Registry Editor Version 5.00", "",
                f"[{base}\\{progid}]", f"@={string(NAME)}", "",
                f"[{base}\\{progid}\\shell\\open]", '"MultiSelectModel"="Document"', "",
                f"[{base}\\{progid}\\shell\\open\\command]", f"@={string(command)}", "",
                f"[{application}]", f'"FriendlyAppName"={string(NAME)}', "",
                f"[{application}\\shell\\open]", '"MultiSelectModel"="Document"', "",
                f"[{application}\\shell\\open\\command]", f"@={string(command)}", "",
                f"[{application}\\SupportedTypes]"]
    extensions = [ext for _, group, _, _ in FORMATS for ext in group]
    sections.extend(f'".{ext}"=""' for ext in extensions)
    removal = ["Windows Registry Editor Version 5.00", "",
               f"[-{base}\\{progid}]", "", f"[-{application}]", ""]
    for ext in extensions:
        key = f"[{base}\\.{ext}\\OpenWithProgids]"
        sections.extend(["", key, f'"{progid}"=hex(0):'])
        # Delete only our alternate value, never the shared extension key/default.
        removal.extend([key, f'"{progid}"=-', ""])
    return "\r\n".join(sections) + "\r\n", "\r\n".join(removal)


def stage(platform, binary, output, resources=None):
    binary = binary.resolve(strict=True)
    output = output.resolve()
    if not binary.is_file():
        raise ValueError("Binary must be a file")
    if platform == "macos":
        if resources is None or not resources.is_dir():
            raise ValueError("macOS requires the SwiftPM resource bundle directory")
        launcher = output / f"{NAME}.app"
        executable = launcher / "Contents/MacOS/CapturesNative"
    else:
        executable = output / ("CapturesNative.exe" if platform == "windows" else "captures-native")
        launcher = output / ("register-open-with.reg" if platform == "windows" else f"{IDENTITY}.desktop")
    # Validate path serialization before creating any staging files.
    descriptor = desktop_entry(executable) if platform == "linux" else None
    registry = windows_registry(executable) if platform == "windows" else None
    output.mkdir(parents=True, exist_ok=False)
    executable.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(binary, executable)
    executable.chmod(executable.stat().st_mode | 0o111)
    if platform == "macos":
        destination = launcher / "Contents/Resources"
        destination.mkdir()
        shutil.copytree(resources, destination / "CapturesNative_CapturesNative.bundle")
        shutil.copy2(ROOT / "apps/desktop/src-tauri/icons/icon.icns", destination / "Captures.icns")
        (launcher / "Contents/Info.plist").write_bytes(plistlib.dumps(mac_info()))
    elif platform == "windows":
        launcher.write_text(registry[0], encoding="utf-16", newline="")
        (output / "unregister-open-with.reg").write_text(registry[1], encoding="utf-16", newline="")
    else:
        launcher.write_text(descriptor, encoding="utf-8")
    shutil.copy2(ROOT / "LICENSE", output / "LICENSE")
    shutil.copy2(ROOT / "TRADEMARKS.md", output / "TRADEMARKS.md")
    print(f"Staged unsigned development package: {output}")
    print("Nothing was installed or registered. See DEVELOPMENT.md for opt-in Open With and removal.")
    return launcher, executable


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("macos", "windows", "linux"), required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--resources", type=Path, help="macOS SwiftPM .bundle directory")
    args = parser.parse_args()
    stage(args.platform, args.binary, args.output, args.resources)


if __name__ == "__main__":
    main()
