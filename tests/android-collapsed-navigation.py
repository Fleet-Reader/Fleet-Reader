#!/usr/bin/env python3
"""Real-touch regression: a collapsed feeds header must never hide destination Up.

Run on an English compact emulator with enough feeds to scroll. Navigation only:
does not open articles, refresh feeds, or change account data.
"""
import argparse
from pathlib import Path
import re
import subprocess
import time
import xml.etree.ElementTree as ET

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--adb', default='adb')
p.add_argument('--serial', default='emulator-5554')
p.add_argument('--output', type=Path, default=Path('build/day/collapsed-navigation'))
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)


def adb(*args):
    return subprocess.check_output([a.adb, '-s', a.serial, *args], timeout=45)


def screen(name):
    for _ in range(5):
        adb('shell', 'rm', '-f', '/sdcard/day-collapsed.xml')
        result = adb('shell', 'uiautomator', 'dump', '/sdcard/day-collapsed.xml')
        if b'dumped to' in result:
            xml = adb('shell', 'cat', '/sdcard/day-collapsed.xml')
            (a.output / f'{name}.xml').write_bytes(xml)
            (a.output / f'{name}.png').write_bytes(adb('exec-out', 'screencap', '-p'))
            return ET.fromstring(xml)
        time.sleep(1)
    raise AssertionError('No fresh accessibility snapshot')


def find(root, text):
    return next((n for n in root.iter('node')
                 if text in (n.get('text'), n.get('content-desc'))), None)


def bounds(n):
    return list(map(int, re.findall(r'\d+', n.get('bounds'))))


def tap(n):
    assert n is not None, 'Missing native control'
    l, t, r, b = bounds(n)
    assert r > l and b > t and 0 <= t < 2300, 'Control is offscreen'
    adb('shell', 'input', 'tap', str((l+r)//2), str((t+b)//2))
    time.sleep(.7)


def swipe(start, end):
    adb('shell', 'input', 'swipe', '500', str(start), '500', str(end), '450')
    time.sleep(.7)


def assert_up(root):
    n = find(root, 'Navigate up')
    assert n is not None, 'Destination has no visible Up'
    l, t, r, b = bounds(n)
    assert n.get('clickable') == 'true' and 0 <= t < b <= 300 and r-l >= 80, (
        'Up must have a full, usable touch target below the status bar')
    assert find(root, 'Search articles') is None, 'Root search leaked into destination'
    return n


root = screen('initial')
if find(root, 'Navigate up') is not None:
    tap(find(root, 'Navigate up'))
# Start with full root chrome regardless of its retained scroll position.
swipe(500, 1900)
assert find(screen('root-expanded'), 'Fleet Reader') is not None

# Test collapsed first: the old implementation must fail before any repair-inducing
# back-stack transition or viewport change. Choose a real feed visible after scrolling.
for name, distance in [('collapsed', 700), ('expanded', 0), ('partial', 250),
                       ('collapsed-repeat', 700)]:
    swipe(500, 1900)
    if distance:
        swipe(1500, 1500-distance)
    root = screen(f'{name}-root')
    if name.startswith('collapsed'):
        assert find(root, 'Fleet Reader') is None and find(root, 'Search articles') is None
    if name == 'partial':
        assert find(root, 'Fleet Reader') is None and find(root, 'Search articles') is not None, (
            'Expected partially collapsed root: title hidden, search still visible')
    rows = [n for n in root.iter('node')
            if n.get('resource-id', '').endswith('/design_menu_item_text')
            and 600 < bounds(n)[1] < 1600]
    assert rows, 'Seed enough feeds to scroll'
    title = rows[0].get('text')
    tap(rows[0])
    root = screen(f'{name}-destination')
    assert_up(root)
    swipe(1800, 650)
    assert_up(screen(f'{name}-list-scrolled'))
    swipe(650, 1800)
    tap(assert_up(screen(f'{name}-list-returned')))
    root = screen(f'{name}-back')
    assert find(root, 'Fleet Reader') is not None and find(root, 'Navigate up') is None
    # A real Up tap must reveal the native sidebar; a retained hidden label is insufficient.
    native = adb('shell', 'dumpsys', 'activity', 'top').decode()
    pane = re.search(r'DayNavHost\$\d+\{[^\n]*? (\d+),\d+-(\d+),\d+\}', native)
    detail = re.search(r'SlidingPaneLayout\$TouchBlocker\{[^\n]*? (\d+),\d+-(\d+),\d+\}', native)
    assert pane and detail and int(detail[1]) >= int(pane[2])-int(pane[1]), (
        'Destination still covers the feeds root')
    print(f'PASS {name}: {title} → list scroll → visible native Up → feeds', flush=True)
