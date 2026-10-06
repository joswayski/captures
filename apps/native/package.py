#!/usr/bin/env python3
"""Stage an unsigned native DEVELOPMENT package; never install or register it.

Build first. Registration files contain absolute paths: choose the final output
location before opting into Open With. Existing output is never overwritten.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tarfile
import zipfile

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
MEDIA_TARGETS = {
    "macos": ("aarch64-apple-darwin", "x86_64-apple-darwin"),
    "windows": ("x86_64-pc-windows-msvc",),
    "linux": ("x86_64-unknown-linux-gnu",),
}


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


def stage(platform, binary, output, resources=None, media_target=None):
    binary = binary.resolve(strict=True)
    output = output.resolve()
    if not binary.is_file():
        raise ValueError("Binary must be a file")
    media_tools, media_sources = [], []
    if media_target is not None:
        if media_target not in MEDIA_TARGETS[platform]:
            raise ValueError(f"Media target {media_target} does not match {platform}")
        suffix = ".exe" if platform == "windows" else ""
        media_tools = [ROOT / "apps/desktop/src-tauri/binaries" / f"{name}-{media_target}{suffix}"
                       for name in ("ffmpeg", "ffprobe")]
        # Reuse the builder's pin and complete corresponding-source payload,
        # rather than copying arbitrary system FFmpeg builds or only notices.
        builder = (ROOT / "scripts/build-ffmpeg-sidecars.sh").read_text()
        version = re.search(r'^FFMPEG_VERSION="([^"]+)"', builder, re.MULTILINE).group(1)
        dist = ROOT / "target/ffmpeg-dist"
        media_sources = [dist / f"ffmpeg-{version}{ending}" for ending in (
            ".tar.xz", ".tar.xz.asc", "-BUILD_CONFIG.txt", "-COPYING.LGPLv2.1", "-NOTICE.md")]
        media_sources += [ROOT / "apps/desktop/src-tauri/openh264" / name
                          for name in ("LICENSE", "NOTICE.md")]
        for source in media_tools + media_sources:
            if not source.is_file():
                raise ValueError(f"Missing media package input: {source}; run npm run prepare:media first")
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
    if media_tools:
        tools_directory = executable.parent / "binaries"
        tools_directory.mkdir()
        for source in media_tools:
            destination = tools_directory / source.name
            shutil.copy2(source, destination)
            destination.chmod(destination.stat().st_mode | 0o111)
        licenses = (launcher / "Contents/Resources" if platform == "macos" else output) / "media-licenses"
        for source in media_sources:
            destination = licenses / ("ffmpeg" if source.parent == dist else "openh264") / source.name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
    shutil.copy2(ROOT / "LICENSE", output / "LICENSE")
    shutil.copy2(ROOT / "TRADEMARKS.md", output / "TRADEMARKS.md")
    shutil.copy2(ROOT / "apps/native/TESTING.md", output / "TESTING.md")
    _write_build_info(platform, output, executable, media_target)
    print(f"Staged unsigned development package: {output}")
    print("Nothing was installed or registered. See DEVELOPMENT.md for opt-in Open With and removal.")
    return launcher, executable


def _write_build_info(platform, output, executable, media_target):
    # Every staged host needs layout metadata for the cooperative package guard.
    # Archive refreshes the hash after signing; this is not protocol enrollment.
    digest = hashlib.sha256()
    with executable.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    info = {"development": True, "platform": platform, "media_target": media_target,
            "binary": executable.relative_to(output).as_posix(), "binary_sha256": digest.hexdigest(),
            "source_commit": os.environ.get("GITHUB_SHA") if os.environ.get("GITHUB_ACTIONS") == "true" else None}
    (output / "BUILD_INFO.json").write_text(json.dumps(info, indent=2) + "\n", encoding="utf-8")


def archive(platform, output, executable, destination, media_target=None):
    """Archive the staged package after signing, preserving executable modes."""
    output = output.resolve(strict=True)
    executable = executable.resolve(strict=True)
    executable.relative_to(output)  # Reject external inputs before writing metadata.
    destination = destination.resolve()
    if destination.is_relative_to(output):
        raise ValueError("Archive must be outside the staged package")
    if destination.exists():
        raise FileExistsError(destination)
    _write_build_info(platform, output, executable, media_target)
    destination.parent.mkdir(parents=True, exist_ok=True)
    stream = destination.open("xb")
    # These descriptors bind absolute staging paths. Portable archives must
    # never offer CI-runner paths as usable local Open With registration.
    excluded = {f"{output.name}/{name}" for name in (
        "register-open-with.reg", "unregister-open-with.reg", f"{IDENTITY}.desktop")}
    try:
        with stream:
            if platform == "linux":
                with tarfile.open(fileobj=stream, mode="w:gz") as bundle:
                    bundle.add(output, arcname=output.name,
                               filter=lambda item: None if item.name in excluded else item)
            else:
                with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
                    for path in sorted(output.rglob("*")):
                        name = path.relative_to(output.parent).as_posix()
                        if name not in excluded:
                            bundle.write(path, arcname=name)
    except Exception:
        destination.unlink()
        raise
    print(f"Archived development package: {destination}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("macos", "windows", "linux"), required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--resources", type=Path, help="macOS SwiftPM .bundle directory")
    parser.add_argument("--media-target", choices=tuple(target for targets in MEDIA_TARGETS.values() for target in targets),
                        help="Bundle this target's prepared FFmpeg/FFprobe and corresponding source/licenses")
    parser.add_argument("--adhoc-sign", action="store_true", help="macOS only: ad-hoc sign locally, not notarize")
    parser.add_argument("--archive", type=Path, help="Also archive as ZIP (macOS/Windows) or tar.gz (Linux)")
    args = parser.parse_args()
    if args.adhoc_sign and (args.platform != "macos" or sys.platform != "darwin"):
        parser.error("--adhoc-sign requires a macOS package on a macOS host")
    launcher, executable = stage(args.platform, args.binary, args.output, args.resources, args.media_target)
    if args.adhoc_sign:
        subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(launcher)], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(launcher)], check=True)
    if args.archive:
        archive(args.platform, args.output, executable, args.archive, args.media_target)


if __name__ == "__main__":
    main()
