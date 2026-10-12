#!/usr/bin/env python3
"""Diagnose VideoToolbox selection/property rejection with disposable synthetic media.

No capture permission, profiles, installations or production encoder changes.
Failed policy cases are diagnostic data, not skipped or passing acceptance tests.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys


SESSION_PROBE = r"""
import Foundation
import VideoToolbox

let policies: [(String, [String: Any])] = [
    ("hardware-only", [kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder as String: true]),
    ("allow-software", [kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder as String: true]),
    ("software-only", [kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder as String: false])
]
let properties: [(String, CFString, NSNumber)] = [
    ("quality", kVTCompressionPropertyKey_Quality, NSNumber(value: 0.9)),
    ("bitrate", kVTCompressionPropertyKey_AverageBitRate, NSNumber(value: 1_500_000))
]
for (width, height) in [(Int32(160), Int32(90)), (Int32(640), Int32(360))] {
    for (policy, specification) in policies {
        for (mode, property, value) in properties {
            let label = "\(width)x\(height)/\(policy)/\(mode)"
            var encoderID: CFString?
            var advertised: CFDictionary?
            let selection = VTCopySupportedPropertyDictionaryForEncoder(width: width, height: height,
                codecType: kCMVideoCodecType_H264, encoderSpecification: specification as CFDictionary,
                encoderIDOut: &encoderID, supportedPropertiesOut: &advertised)
            print(label, "selectionStatus:", selection, "candidateEncoderID:", encoderID as Any)
            var session: VTCompressionSession?
            let created = VTCompressionSessionCreate(allocator: nil, width: width, height: height,
                codecType: kCMVideoCodecType_H264, encoderSpecification: specification as CFDictionary,
                imageBufferAttributes: nil, compressedDataAllocator: nil, outputCallback: nil,
                refcon: nil, compressionSessionOut: &session)
            print(label, "createStatus:", created)
            guard created == noErr, let session else { continue }
            var supported: CFDictionary?
            let dictionaryStatus = VTSessionCopySupportedPropertyDictionary(session,
                supportedPropertyDictionaryOut: &supported)
            let hasProperty = supported.map {
                CFDictionaryContainsKey($0, Unmanaged.passUnretained(property).toOpaque())
            }
            let configured = VTSessionSetProperty(session, key: property, value: value)
            let prepared = VTCompressionSessionPrepareToEncodeFrames(session)
            var hardware: Unmanaged<CFTypeRef>?
            let hardwareStatus = VTSessionCopyProperty(session,
                key: kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
                allocator: nil, valueOut: &hardware)
            let hardwareValue = hardware?.takeRetainedValue()
            print(label, "dictionaryStatus:", dictionaryStatus, "propertyAdvertised:", hasProperty as Any,
                  "hardwareQueryStatus:", hardwareStatus, "usingHardware:", hardwareValue as Any,
                  "propertyStatus:", configured, "prepareStatus:", prepared)
            VTCompressionSessionInvalidate(session)
        }
    }
}
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ffmpeg", default=os.environ.get("CAPTURES_FFMPEG") or shutil.which("ffmpeg"))
    parser.add_argument("--ffprobe", default=os.environ.get("CAPTURES_FFPROBE") or shutil.which("ffprobe"))
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("VideoToolbox diagnostics require macOS; no other-platform fallback")
    if not args.ffmpeg or not args.ffprobe:
        parser.error("ffmpeg and ffprobe are required")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output / "session-probe.swift").write_text(SESSION_PROBE)
    session = subprocess.run(["swift", str(output / "session-probe.swift")], capture_output=True,
                             text=True, timeout=120)
    (output / "session-probe.log").write_text(session.stdout + session.stderr)
    print(session.stdout, end="")
    print(session.stderr, end="", file=sys.stderr)
    # A broken diagnostic must not masquerade as an encoder result.
    if session.returncode:
        raise RuntimeError(f"VideoToolbox probe failed to execute: {session.returncode}")
    results = {"platform": platform.platform(), "diagnosticOnly": True, "cases": []}
    for width, height in [(160, 90), (640, 360)]:
        for policy, options in [("hardware-only", ["-allow_sw", "0"]),
                                ("allow-software", ["-allow_sw", "1"]),
                                ("software-only", ["-allow_sw", "1", "-require_sw", "1"])]:
            for mode, rate in [("quality", ["-q:v", "90"]),
                               ("bitrate", ["-b:v", "1500000", "-maxrate", "1500000"])]:
                name = f"{width}x{height}-{policy}-{mode}"
                destination = output / f"{name}.mp4"
                command = [args.ffmpeg, "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                           f"testsrc2=size={width}x{height}:rate=12", "-frames:v", "3", "-pix_fmt",
                           "yuv420p", "-an", "-c:v", "h264_videotoolbox", *options, *rate,
                           "-movflags", "+faststart", "-y", str(destination)]
                try:
                    encoded = subprocess.run(command, capture_output=True, text=True, timeout=20)
                    case = {"name": name, "command": command, "exitCode": encoded.returncode,
                            "stderr": encoded.stderr}
                except subprocess.TimeoutExpired:
                    case = {"name": name, "command": command, "timedOut": True}
                if case.get("exitCode") == 0:
                    inspected = subprocess.run([args.ffprobe, "-v", "error", "-show_streams", "-of",
                                                "json", str(destination)], capture_output=True,
                                               text=True, timeout=20)
                    case["probeExitCode"] = inspected.returncode
                    case["streams"] = json.loads(inspected.stdout) if inspected.returncode == 0 else inspected.stderr
                results["cases"].append(case)
                print(json.dumps(case), flush=True)
                # Preserve completed cases even if a subsequent diagnostic fails.
                (output / "results.json").write_text(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
