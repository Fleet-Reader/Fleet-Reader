#!/usr/bin/env python3
"""Real-touch regression; start on the root list of an isolated English fixture app.

Usage: python3 tests/android-navigation.py --adb /path/to/adb --serial emulator-5554
Unlike dayscript, this exercises Android touch dispatch and context-menu ownership.
"""
import argparse
import re
import subprocess
import time
import xml.etree.ElementTree as ET

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--adb", default="adb")
parser.add_argument("--serial", default="emulator-5554")
parser.add_argument("--feed", default="Reader View Fixtures")
parser.add_argument("--back-cycles", type=int, default=0)
modes = parser.add_mutually_exclusive_group()
modes.add_argument("--tablet", action="store_true")
modes.add_argument("--edge-swipes", action="store_true")
parser.add_argument("--article", default="How monarch butterflies find their way to Mexico")
args = parser.parse_args()


def adb(*command):
    return subprocess.check_output([args.adb, "-s", args.serial, *command], text=True)


def screen():
    adb("shell", "uiautomator", "dump", "/sdcard/day-navigation-test.xml")
    return ET.fromstring(adb("shell", "cat", "/sdcard/day-navigation-test.xml"))


def bounds(node):
    return list(map(int, re.findall(r"\d+", node.get("bounds"))))


def text_node(root, title):
    return next(node for node in root.iter("node") if node.get("text") == title)


def tap(node, long=False):
    left, top, right, bottom = bounds(node)
    x, y = str((left + right) // 2), str((top + bottom) // 2)
    if long:
        adb("shell", "input", "swipe", x, y, x, y, "1200")
    else:
        adb("shell", "input", "tap", x, y)
    time.sleep(1)


def assert_destination(title):
    # Accessibility also exposes hidden SlidingPaneLayout children. Assert the
    # toolbar title, not merely a sidebar label that still exists off screen.
    root = screen()
    assert any(node.get("text") == title and bounds(node)[1] < 300
               for node in root.iter("node")), f"Real tap did not open {title}"
    if title == "Fleet Reader":
        # Hidden sidebar children are still in the accessibility tree. The native
        # detail pane must actually be off-screen, or this could be the blank root.
        hierarchy = adb("shell", "dumpsys", "activity", "top")
        pane = re.search(r"(?:SlidingPaneLayout|DayNavHost\$\d+)\{[^\n]*? (\d+),\d+-(\d+),\d+\}", hierarchy)
        detail = re.search(r"SlidingPaneLayout\$TouchBlocker\{[^\n]*? (\d+),\d+-(\d+),\d+\}", hierarchy)
        assert pane and detail, "Native adaptive pane hierarchy missing"
        width = int(pane[2]) - int(pane[1])
        assert int(detail[1]) >= width, "Blank detail pane still covers the feeds root"


def toolbar_up():
    node = next(node for node in screen().iter("node")
                if node.get("class") == "android.widget.ImageButton"
                and bounds(node)[0] < 120 and bounds(node)[1] < 300)
    tap(node)


def test_tablet():
    original_log = adb("shell", "getprop", "log.tag.DayNavigation").strip()
    override = re.search(r"Override size: (\d+x\d+)", adb("shell", "wm", "size"))
    original_size = override[1] if override else "reset"
    try:
        adb("shell", "setprop", "log.tag.DayNavigation", "DEBUG")
        adb("shell", "wm", "size", "2400x1600")
        time.sleep(3)
        for title in ["All Articles", "Starred", "Settings", "All Articles"]:
            tap(text_node(screen(), title))
            assert_destination(title)
            trace = adb("logcat", "-d", "-s", "DayNavigation:D", "*:S")
            latest = [line for line in trace.splitlines() if "DayNavigation:" in line][-1]
            assert "entries=0" in latest and "native=0" in latest, latest
            assert "slideable=false" in latest, "Emulator density is too high for tablet layout"
            print(f"PASS wide section without history: {title}", flush=True)
        tap(text_node(screen(), args.article))
        adb("shell", "input", "keyevent", "4")
        time.sleep(1)
        assert_destination("All Articles")
        print("PASS wide reader Back retains section", flush=True)
        tap(text_node(screen(), args.article))
        adb("shell", "wm", "size", original_size)
        time.sleep(3)
        assert_destination("All Articles")
        adb("shell", "input", "keyevent", "4")
        time.sleep(1)
        assert_destination("All Articles")
        adb("shell", "input", "keyevent", "4")
        time.sleep(1)
        assert_destination("Fleet Reader")
        print("PASS wide reader → compact reader → list → feeds", flush=True)
    finally:
        adb("shell", "wm", "size", original_size)
        adb("shell", "setprop", "log.tag.DayNavigation", original_log)


def test_edge_swipes():
    sizes = re.findall(r"(?:Physical|Override) size: (\d+)x(\d+)",
                       adb("shell", "wm", "size"))
    width, height = map(int, sizes[-1])

    def back_swipe():
        adb("shell", "input", "swipe", "2", str(height // 2),
            str(width * 5 // 6), str(height // 2), "300")
        time.sleep(1)

    assert_destination("Fleet Reader")
    tap(text_node(screen(), "All Articles"))
    back_swipe()
    assert_destination("Fleet Reader")
    print("PASS committed edge swipe: section → feeds", flush=True)
    tap(text_node(screen(), "All Articles"))
    tap(text_node(screen(), args.article))
    back_swipe()
    assert_destination("All Articles")
    back_swipe()
    assert_destination("Fleet Reader")
    print("PASS committed edge swipes: reader → list → feeds", flush=True)


if args.tablet:
    test_tablet()
elif args.edge_swipes:
    test_edge_swipes()
else:
    assert_destination("Fleet Reader")
    for title in ["Today", "All Unread", "Starred", "All Articles", "Settings", args.feed]:
        tap(text_node(screen(), title))
        assert_destination(title)
        print(f"PASS real tap: {title}")
        adb("shell", "input", "keyevent", "4")
        time.sleep(1)
        assert_destination("Fleet Reader")

    tap(text_node(screen(), args.feed), long=True)
    root = screen()
    assert any(node.get("text") == "Feed overview" for node in root.iter("node")), \
        "Feed long press did not show its context menu"
    print("PASS native long-press feed menu")
    adb("shell", "input", "keyevent", "4")
    time.sleep(1)
    assert_destination("Fleet Reader")


    for cycle in range(args.back_cycles):
        tap(text_node(screen(), "All Articles"))
        assert_destination("All Articles")
        tap(text_node(screen(), args.article))
        assert any(node.get("class") == "android.webkit.WebView"
                   for node in screen().iter("node")), "Article reader missing"
        if cycle % 2:
            adb("shell", "input", "keyevent", "4")
            time.sleep(1)
        else:
            toolbar_up()
        assert any(node.get("class") == "androidx.recyclerview.widget.RecyclerView"
                   for node in screen().iter("node")), "Article list missing after Back"
        toolbar_up()
        assert_destination("Fleet Reader")
        tap(text_node(screen(), "Settings"))
        assert_destination("Settings")
        adb("shell", "input", "keyevent", "4")
        time.sleep(1)
        assert_destination("Fleet Reader")
        print(f"PASS reader Back/Up → section Up → settings Back: {cycle + 1}", flush=True)
