"""Run every end-to-end suite and report a single exit status."""

import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SUITES = ["test_editor.py", "test_navigation.py", "test_session.py", "test_tooling.py"]


def main():
    failed = []
    for suite in SUITES:
        result = subprocess.run([sys.executable, os.path.join(HERE, suite)])
        if result.returncode != 0:
            failed.append(suite)
    print()
    if failed:
        print(f"end-to-end: {len(failed)} suite(s) failed: {failed}")
        return 1
    print(f"end-to-end: all {len(SUITES)} suites passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
