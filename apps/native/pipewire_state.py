"""Read the final private PipeWire graph, including removals during pw-dump."""
import json
import subprocess


def pipewire_objects(env):
    # Even without --monitor, pw-dump writes separate JSON arrays for objects
    # removed while its initial sync is in flight. Later records replace earlier
    # ones; flattening them could mistake a removed/rerouted stream for a live one.
    text = subprocess.check_output(["pw-dump"], env=env, timeout=3, text=True).strip()
    if not text:
        raise ValueError("pw-dump returned no graph")
    decoder = json.JSONDecoder()
    objects = {}
    while text:
        updates, end = decoder.raw_decode(text)
        if not isinstance(updates, list):
            raise ValueError("pw-dump did not return an object array")
        for item in updates:
            identifier = item["id"]
            if item.get("info", False) is None or item.get("props", False) is None:
                objects.pop(identifier, None)
            else:
                objects[identifier] = item
        text = text[end:].lstrip()
    return list(objects.values())
