#!/usr/bin/env python3
"""Bundle native colors/metrics from the shipping CSS, never a second palette."""
import json
import re
import sys
from pathlib import Path


def blocks(css):
    css = re.sub(r"/\*.*?\*/", "", css, flags=re.S)
    return [(selector.strip(), dict(re.findall(r"(--[\w-]+)\s*:\s*([^;]+);", body)))
            for selector, body in re.findall(r"([^{}]+)\{([^{}]*)\}", css)]


def resolve(name, values, seen=()):
    if name in seen:
        raise ValueError(f"Cyclic CSS token: {name}")
    raw = values[name].strip()
    return re.sub(r"var\((--[\w-]+)\)",
                  lambda match: resolve(match[1], values, (*seen, name)), raw)


def generate(root):
    design = blocks((root / "shared/design.css").read_text())
    palettes = blocks((root / "shared/themes.css").read_text())
    base = next(values for selector, values in design if selector == ":root")
    dark = next(values for selector, values in design if '[data-appearance="dark"]' in selector)
    light = next(values for selector, values in design if selector == '[data-appearance="light"]')
    themes = {}
    for selector, values in palettes:
        names = re.findall(r'data-capture-theme="([\w-]+)"', selector)
        for name in names:
            if name != "custom" and "--theme-accent" in values:
                themes[name] = {"accent": values["--theme-accent"].strip(),
                                "signal": values["--theme-signal"].strip()}
    default = palettes[0][1]
    result = {"themes": themes}
    # Keep only values used by the native renderer; var expansion still uses all
    # declarations so semantic aliases resolve from the same source of truth.
    for name, overrides in [("dark", dark), ("light", light)]:
        values = {**default, **base, **dark, **overrides}
        result[name] = {key.removeprefix("--"): resolve(key, values)
                        for key in values if key.startswith(("--surface-", "--text", "--border",
                                                             "--glass", "--s-", "--r-", "--h-",
                                                             "--dur-", "--positive", "--info"))}
    return result


if __name__ == "__main__":
    destination = Path(sys.argv[1])
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(generate(Path(__file__).resolve().parents[2]), indent=2) + "\n")
