#!/usr/bin/env python3
"""Native Up regression, including cold launch into the selected Today page.

Run against an English Fleet Reader install. Uses real toolbar touches, not nav_back,
which can pass even when the native button is absent. Does not edit articles.
"""

import argparse
from pathlib import Path
import re
import subprocess
import time
import xml.etree.ElementTree as ET

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--adb", default="adb")
parser.add_argument("--serial", default="emulator-5554")
parser.add_argument("--output", type=Path, default=Path("build/day/navigation-up"))
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)


def adb(*command):
    return subprocess.check_output([args.adb, "-s", args.serial, *command], text=True, timeout=45)


def screen():
    for _ in range(5):
        # uiautomator may return success with "null root node" during launch.
        # Never read an older dump and mistake it for the current screen.
        adb("shell", "rm", "-f", "/sdcard/day-up.xml")
        result = adb("shell", "uiautomator", "dump", "/sdcard/day-up.xml")
        if "dumped to" in result:
            return ET.fromstring(adb("shell", "cat", "/sdcard/day-up.xml"))
        time.sleep(1)
    raise AssertionError("Android accessibility did not provide a fresh snapshot")


def bounds(node):
    return list(map(int, re.findall(r"\d+", node.get("bounds"))))


def up(root):
    return next((node for node in root.iter("node")
                 if node.get("content-desc") == "Navigate up"
                 and node.get("clickable") == "true"), None)


def tap(node):
    assert node is not None, "Navigation control is missing"
    left, top, right, bottom = bounds(node)
    adb("shell", "input", "tap", str((left + right) // 2), str((top + bottom) // 2))
    time.sleep(0.5)


def assert_root():
    root = screen()
    assert up(root) is None, "Root should not have Up"
    assert any(node.get("text") == "Fleet Reader" and bounds(node)[1] < 300
               for node in root.iter("node")), "Did not return to feeds"
    # Hidden accessibility children alone do not prove that the root is visible.
    native = adb("shell", "dumpsys", "activity", "top")
    pane = re.search(r"DayNavHost\$\d+\{[^\n]*? (\d+),\d+-(\d+),\d+\}", native)
    detail = re.search(r"SlidingPaneLayout\$TouchBlocker\{[^\n]*? (\d+),\d+-(\d+),\d+\}", native)
    assert pane and detail, "Adaptive layout is missing"
    assert int(detail[1]) >= int(pane[2]) - int(pane[1]), "Detail still covers root"


def capture(name):
    data = subprocess.check_output([args.adb, "-s", args.serial, "exec-out", "screencap", "-p"])
    (args.output / f"{name}.png").write_bytes(data)


for attempt in range(3):
    adb("shell", "am", "force-stop", "org.appfair.app.FleetReader")
    adb("shell", "am", "start", "-W", "-n", "org.appfair.app.FleetReader/dev.daybrite.day.bridge.DayActivity")
    time.sleep(2)
    root = screen()
    assert up(root) is not None, "Cold launch into Today has no Up button"
    capture(f"cold-launch-{attempt + 1}")
    tap(up(root))
    assert_root()
    print(f"PASS cold launch {attempt + 1}: Today → toolbar Up → feeds", flush=True)

for title in ["Today", "All Unread", "Starred", "All Articles"]:
    tap(next(node for node in screen().iter("node") if node.get("text") == title))
    root = screen()
    assert up(root) is not None, f"{title} has no Up button"
    tap(up(root))
    assert_root()
    print(f"PASS {title} → toolbar Up → feeds", flush=True)

tap(next(node for node in screen().iter("node") if node.get("content-desc") == "Settings"))
root = screen()
# Settings is a nested host inside a dismissible modal on mobile.
adb("shell", "input", "keyevent", "4")
time.sleep(0.5)
assert_root()
print("PASS Settings → system Back → feeds", flush=True)
capture("feeds-root")

override = re.search(r"Override size: (\d+x\d+)", adb("shell", "wm", "size"))
original_size = override[1] if override else "reset"
try:
    adb("shell", "wm", "size", "2400x1600")
    time.sleep(2)
    root = screen()
    assert up(root) is None, "Tiled base section should expose its root list alongside it"
    native = adb("shell", "dumpsys", "activity", "top")
    detail = re.search(r"SlidingPaneLayout\$TouchBlocker\{[^\n]*? (\d+),\d+-(\d+),\d+\}", native)
    assert detail and 0 < int(detail[1]) < 2400, "Tiled root is not alongside the detail"
    capture("wide-root-and-section")
finally:
    adb("shell", "wm", "size", original_size)
time.sleep(2)
tap(up(screen()))
assert_root()
print("PASS tiled root remains visible; narrowing restores Up → feeds", flush=True)
