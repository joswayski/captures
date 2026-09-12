#!/usr/bin/env python3
"""Portable structural checks for the experimental platform tooling."""

from pathlib import Path
import plistlib


ROOT = Path(__file__).resolve().parent


def check_plist() -> None:
    with (ROOT / "macos" / "Info.plist").open("rb") as source:
        info = plistlib.load(source)
    assert info["CFBundleExecutable"] == "captures-gpui"
    assert info["CFBundleIdentifier"] == "io.github.joswayski.captures.gpui-experiment"
    assert info["LSMinimumSystemVersion"] == "13.0"
    assert info["NSScreenCaptureUsageDescription"]
    assert info["NSAudioCaptureUsageDescription"]
    assert info["NSMicrophoneUsageDescription"]
    assert "NSCameraUsageDescription" not in info
    assert "CFBundleDocumentTypes" not in info, (
        "Do not advertise Finder file opening until native kAEOpenDocuments routing is tested"
    )


def check_workflow() -> None:
    workflow = (ROOT.parents[2] / ".github" / "workflows" / "gpui.yml").read_text()
    for value in ("workflow_dispatch", "release:", "contents: write", "id-token: write"):
        assert value not in workflow, f"private-test workflow contains {value!r}"
    assert "x86_64-pc-windows-msvc" in workflow
    assert "GPUI_FXC_PATH" in (ROOT / "windows" / "prepare-msvc.ps1").read_text()
    for helper in ROOT.rglob("*"):
        if helper.is_file() and helper.suffix in {".sh", ".ps1"}:
            contents = helper.read_text()
            assert "export PATH=" not in contents, f"{helper} mutates process PATH"
            assert "$env:Path =" not in contents, f"{helper} mutates process PATH"


if __name__ == "__main__":
    check_plist()
    check_workflow()
    print("GPUI platform metadata and private-test workflow policy are valid")
