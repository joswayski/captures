"""Observe private PipeWire node/client state without enumerating parameters."""
import json
from pathlib import Path
import subprocess


def pipewire_objects(env):
    # pw-dump enumerates parameters and can trigger PipeWire 1.0.5's async-result
    # collision during allocation. This example only binds node/client info.
    # Later records replace earlier ones; removals must not leave stale streams.
    binary = Path(__file__).parent / "wgpu/target/debug/examples/pipewire_state"
    text = subprocess.check_output([str(binary)], env=env, timeout=3, text=True).strip()
    if not text:
        raise ValueError("PipeWire state observer returned no graph")
    decoder = json.JSONDecoder()
    objects = {}
    while text:
        updates, end = decoder.raw_decode(text)
        if not isinstance(updates, list):
            raise ValueError("PipeWire state observer did not return an object array")
        for item in updates:
            identifier = item["id"]
            if item.get("info", False) is None or item.get("props", False) is None:
                objects.pop(identifier, None)
            else:
                objects[identifier] = item
        text = text[end:].lstrip()
    return list(objects.values())
