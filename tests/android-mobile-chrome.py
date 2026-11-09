#!/usr/bin/env python3
"""Native scroll/return and tablet Settings checks; run on a seeded isolated English store."""
import argparse
from pathlib import Path
import re
import subprocess
import time
import xml.etree.ElementTree as ET

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--adb', default='adb')
parser.add_argument('--serial', default='emulator-5554')
parser.add_argument('--output', type=Path, default=Path('build/day/mobile-chrome'))
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)

def adb(*argv):
    return subprocess.check_output([args.adb, '-s', args.serial, *argv], timeout=45, text=True)

def screen():
    for _ in range(5):
        adb('shell', 'rm', '-f', '/sdcard/day-chrome.xml')
        if 'dumped to' in adb('shell', 'uiautomator', 'dump', '/sdcard/day-chrome.xml'):
            return ET.fromstring(adb('shell', 'cat', '/sdcard/day-chrome.xml'))
        time.sleep(1)
    raise AssertionError('No fresh accessibility snapshot')

def find(root, value):
    return next((n for n in root.iter('node') if value in (n.get('text'), n.get('content-desc'))), None)

def bounds(node):
    return list(map(int, re.findall(r'\d+', node.get('bounds'))))

def tap(node):
    assert node is not None, 'Missing control'
    x, y, r, b = bounds(node)
    adb('shell', 'input', 'tap', str((x+r)//2), str((y+b)//2))
    time.sleep(0.7)

def capture(name):
    (args.output / f'{name}.png').write_bytes(subprocess.check_output(
        [args.adb, '-s', args.serial, 'exec-out', 'screencap', '-p'], timeout=30))

root = screen()
if find(root, 'Navigate up') is not None:
    tap(find(root, 'Navigate up'))
assert find(screen(), 'Fleet Reader') is not None
capture('phone-expanded')
adb('shell', 'input', 'swipe', '500', '1900', '500', '650', '600')
time.sleep(1)
root = screen()
assert find(root, 'Fleet Reader') is None, 'App-bar title did not scroll away'
assert find(root, 'Search articles') is None, 'Search did not scroll away'
capture('phone-collapsed')
adb('shell', 'input', 'swipe', '500', '650', '500', '1900', '600')
time.sleep(1)
root = screen()
assert find(root, 'Fleet Reader') is not None, 'App bar did not return'
assert find(root, 'Search articles') is not None, 'Search did not return'
# Opening a destination must restore the toolbar with an accessible Up action.
tap(find(root, 'Today'))
tap(find(screen(), 'Navigate up'))
print('PASS native scroll-away, quick return, and Today Up', flush=True)

override = re.search(r'Override size: (\d+x\d+)', adb('shell', 'wm', 'size'))
original = override[1] if override else 'reset'
try:
    adb('shell', 'wm', 'size', '2400x1600')
    time.sleep(2)
    assert find(screen(), 'Search articles') is not None
    # Tiled layouts keep shared navigation/actions pinned while only sidebar search
    # collapses. Scrolling the neighboring article pane must not move that chrome.
    adb('shell', 'input', 'swipe', '300', '1250', '300', '450', '600')
    time.sleep(1)
    root = screen()
    assert find(root, 'Search articles') is None, 'Tablet sidebar search did not collapse'
    settings = find(root, 'Settings')
    assert settings is not None and bounds(settings)[3] <= 300, 'Tablet toolbar scrolled away'
    capture('tablet-search-collapsed')
    adb('shell', 'input', 'swipe', '1500', '1250', '1500', '450', '600')
    time.sleep(1)
    root = screen()
    assert find(root, 'Settings') is not None, 'Article scroll hid tablet actions'
    assert find(root, 'Search articles') is None, 'Article scroll moved sidebar search'
    adb('shell', 'input', 'swipe', '300', '450', '300', '1250', '600')
    time.sleep(1)
    assert find(screen(), 'Search articles') is not None, 'Tablet search did not return'
    tap(find(screen(), 'Settings'))
    root = screen()
    general = find(root, 'General')
    assert general is not None
    x, _, _, _ = bounds(general)
    assert x > 200, 'Tablet Settings is not inset from the window edge'
    capture('tablet-settings')
    adb('shell', 'input', 'keyevent', '4')
    time.sleep(0.7)
    assert find(screen(), 'General') is None
    print('PASS tablet Settings sheet and Back dismissal', flush=True)
finally:
    adb('shell', 'wm', 'size', original)
