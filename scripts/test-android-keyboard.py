#!/usr/bin/env python3
"""Verify Day's native IME lifecycle using the synthetic Feedbin form.

Launch Fleet Reader on the chosen emulator with DAY_NEWS_SYNC_TEST=1 and an isolated
DAY_DATA_DIR, without signing in. Day's current android-material drive target must
be that emulator. Tests use Android's actual IME state, not a UI focus proxy.
"""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("serial")
    parser.add_argument("--day", default="day")
    parser.add_argument("--adb", default="adb")
    parser.add_argument("--output", type=Path, default=Path("build/day/keyboard-regression"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)

    def drive(*steps):
        result = subprocess.run(
            [args.day, "drive", "-t", "android-material", "--steps-json", json.dumps(steps),
             "--format", "json"], text=True,
            env={**os.environ, "ANDROID_SERIAL": args.serial}, capture_output=True)
        output = result.stdout
        assert result.returncode == 0, output + result.stderr
        # The CLI may print the forwarded process id before its JSON result.
        result = json.loads(output[output.index("{"):])
        assert result["failed"] == 0, result

    def ime(expected, name):
        deadline = time.monotonic() + 8
        stable_since = None
        while time.monotonic() < deadline:
            state = subprocess.check_output(
                [args.adb, "-s", args.serial, "shell", "dumpsys", "input_method"], text=True)
            found = re.search(r"\bmInputShown=(true|false)\b", state)
            assert found, "Android did not expose mInputShown"
            if (found[1] == "true") == expected:
                if stable_since is None:
                    stable_since = time.monotonic()
                # A late show request can arrive after an initial successful hide.
                if time.monotonic() - stable_since >= 0.5:
                    print(f"PASS {name}: keyboard {'shown' if expected else 'hidden'}", flush=True)
                    return
            else:
                stable_since = None
            time.sleep(0.15)
        raise AssertionError(f"{name}: keyboard did not become {expected}")

    def capture(name):
        data = subprocess.check_output([args.adb, "-s", args.serial, "exec-out", "screencap", "-p"])
        (args.output / f"{name}.png").write_bytes(data)

    drive({"wait_for": {"id": "nav"}},
          {"toolbar": {"item": "settings"}}, {"tap": {"id": "prefs-group-accounts"}},
          {"wait_for": {"id": "sync-username"}}, {"focus": {"id": "sync-username"}})
    ime(True, "email editing")
    drive({"input": {"id": "sync-username", "text": "fixture@example.invalid"}},
          {"focus": {"id": "sync-password"}},
          {"input": {"id": "sync-password", "text": "fixture-password"}},
          {"assert_focused": {"id": "sync-password"}})
    ime(True, "field-to-field transfer")
    capture("password-editing")

    drive({"focus": {"id": "sync-password", "focused": False}})
    ime(False, "explicit focus release")
    for attempt in range(3):
        drive({"focus": {"id": "sync-password"}},
              {"focus": {"id": "sync-password", "focused": False}})
        ime(False, f"immediate show/release {attempt + 1}")
    drive({"focus": {"id": "sync-password"}})
    ime(True, "refocus after release")
    # Pop the navigation state directly while the IME is up; system Back alone
    # would dismiss the IME first and would not exercise this regression.
    drive({"nav_back": {}}, {"wait_for": {"id": "prefs-group-accounts"}})
    ime(False, "navigation back to settings")
    capture("settings-after-back")

    drive({"tap": {"id": "prefs-group-accounts"}},
          {"wait_for": {"id": "sync-username"}},
          {"input": {"id": "sync-username", "text": "fixture@example.invalid"}},
          {"input": {"id": "sync-password", "text": "fixture-password"}},
          {"focus": {"id": "sync-password"}})
    ime(True, "login editing")
    drive({"tap": {"id": "sync-sign-in"}}, {"wait_for": {"id": "sync-preview"}})
    ime(False, "successful login removes editor")
    capture("login-preview")

    drive({"tap": {"id": "sync-cancel"}}, {"wait_for": {"id": "sync-password"}},
          {"focus": {"id": "sync-password"}})
    ime(True, "editing in modal")
    drive({"close_window": {"window": "day.preferences"}})
    ime(False, "modal dismissal")
    capture("after-modal-dismissal")


if __name__ == "__main__":
    main()
