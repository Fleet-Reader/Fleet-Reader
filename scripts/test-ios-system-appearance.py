#!/usr/bin/env python3
"""Check live system-theme changes in a running, seeded iOS simulator instance.

Requires Pillow, Xcode and a Day CLI. Use an isolated DAY_DATA_DIR and seed-demo.yaml
when launching the app; do not pass DAY_THEME. This test does not relaunch the app between
changes. It checks rendered Fleet Reader palette colors, not just UIKit's native toolbar.

Usage: python3 scripts/test-ios-system-appearance.py SIMULATOR_UDID --day /path/to/day
"""
import argparse
import base64
from collections import Counter
import json
from pathlib import Path
import subprocess

from PIL import Image


def run(*args):
    return subprocess.check_output(args, text=True)


def screenshots(value):
    if isinstance(value, dict):
        for key, child in value.items():
            if key == "pngBase64":
                yield base64.b64decode(child)
            else:
                yield from screenshots(child)
    elif isinstance(value, list):
        for child in value:
            yield from screenshots(child)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("simulator")
    parser.add_argument("--day", default="day")
    parser.add_argument("--output", type=Path, default=Path("build/day/system-appearance"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)

    def drive(steps):
        return json.loads(run(args.day, "drive", "-t", "ios-uikit", "--steps-json",
                              json.dumps(steps), "--format", "json"))

    original = run("xcrun", "simctl", "ui", args.simulator, "appearance").strip()
    # Choose System explicitly; testing only the picker would bypass the missing native
    # trait notification that caused stale row colors after automatic appearance changes.
    drive([{"toolbar": {"item": "settings"}}, {"tap": {"id": "prefs-group-general"}},
           {"select": {"id": "theme-picker", "index": 2}},
           {"close_window": {"window": "day.preferences"}},
           {"navigate": {"route": "all"}}, {"wait_for": {"id": "article-row-0"}}])
    try:
        for index, mode in enumerate(["dark", "light", "dark", "light"]):
            run("xcrun", "simctl", "ui", args.simulator, "appearance", mode)
            result = drive([{"pause": {"secs": 1}},
                            {"screenshot": {"name": f"system-{mode}-{index}"}}])
            path = args.output / f"{index}-{mode}.png"
            path.write_bytes(next(screenshots(result)))
            pixels = Image.open(path).convert("RGB").tobytes()
            counts = Counter(zip(pixels[::3], pixels[1::3], pixels[2::3]))
            # Palette.bg_alt paints feed headers; Palette.text paints article titles.
            header, text = ((35, 38, 41), (242, 242, 244)) if mode == "dark" else (
                (245, 245, 247), (28, 28, 30))
            stale_header = (245, 245, 247) if mode == "dark" else (35, 38, 41)
            assert counts[header] > 1000, (mode, "header", counts[header])
            assert counts[text] > 100, (mode, "article text", counts[text])
            assert counts[stale_header] < 100, (mode, "stale header", counts[stale_header])
            print(f"PASS {mode}: header={counts[header]}, text={counts[text]}, "
                  f"stale_header={counts[stale_header]} ({path})")
    finally:
        run("xcrun", "simctl", "ui", args.simulator, "appearance", original)


if __name__ == "__main__":
    main()
