"""Shared machinery for the end-to-end suites.

These tests drive the real binary inside a pseudo-terminal and render its
output with a terminal emulator, so they cover the parts unit tests cannot:
the event loop, terminal setup and teardown, real subprocesses, real sockets
and what is actually on screen.

Requires `pyte`; the session suite also needs `websocket-client`.
"""

import fcntl
import os
import pty
import select
import shutil
import struct
import subprocess
import sys
import termios
import time

import pyte

COLS, ROWS = 100, 24


def binary():
    """The debug binary, which the suites expect to have been built already."""
    path = os.path.join(os.getcwd(), "target", "debug", "miv")
    if not os.path.exists(path):
        raise SystemExit(f"{path} not found; run `cargo build` first")
    return path


class Report:
    """Collects pass/fail lines and decides the exit status."""

    def __init__(self, title):
        self.title = title
        self.failures = []
        print(f"\n=== {title} " + "=" * max(0, 60 - len(title)))

    def check(self, label, ok, detail=""):
        print(("PASS  " if ok else "FAIL  ") + label)
        if not ok:
            if detail:
                for line in str(detail).splitlines():
                    print("        " + line)
            self.failures.append(label)
        return ok

    def finish(self):
        print(f"--- {self.title}: ", end="")
        if self.failures:
            print(f"{len(self.failures)} failure(s): {self.failures}")
        else:
            print("all passed")
        return len(self.failures)


class Pty:
    """The editor running in a pseudo-terminal, with its screen emulated."""

    def __init__(self, args, cols=COLS, rows=ROWS, ready=True):
        self.screen = pyte.Screen(cols, rows)
        self.stream = pyte.ByteStream(self.screen)
        self.raw = bytearray()
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ["TERM"] = "xterm-256color"
            # Size the terminal before exec so the editor never sees a stale
            # size, and so a CR written early is not translated to LF by the
            # line discipline before raw mode is enabled.
            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
            os.execv(args[0], args)
        self.pump(0.3)
        if ready:
            self.wait_for(lambda lines: any(line.strip() for line in lines), 10)

    def pump(self, duration):
        end = time.time() + duration
        while time.time() < end:
            readable, _, _ = select.select([self.fd], [], [], 0.02)
            if not readable:
                continue
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                return False
            if not data:
                return False
            self.raw += data
            self.stream.feed(data)
        return True

    def wait_for(self, predicate, timeout=6.0):
        """Pump until the rendered screen satisfies `predicate`."""
        end = time.time() + timeout
        while time.time() < end:
            readable, _, _ = select.select([self.fd], [], [], 0.05)
            if readable:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    break
                if not data:
                    break
                self.raw += data
                self.stream.feed(data)
            if predicate(self.rows()):
                return True
        return False

    def send(self, text, settle=0.3):
        os.write(self.fd, text.encode())
        self.pump(settle)

    def command(self, text, settle=0.5):
        """Type an ex command, sending Enter separately."""
        self.send(text)
        self.send("\r", settle)

    def resize(self, cols, rows):
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.screen.resize(rows, cols)
        self.pump(0.4)

    def rows(self):
        return [row.rstrip() for row in self.screen.display]

    def row(self, index):
        return self.rows()[index]

    def text(self):
        return "\n".join(self.rows())

    def cell(self, y, x):
        return self.screen.buffer[y][x]

    def status(self):
        return next((row for row in self.rows() if "NORMAL" in row or "INSERT" in row), "")

    def message(self):
        return self.rows()[-1]

    def find(self, pattern):
        import re

        for row in self.rows():
            found = re.search(pattern, row)
            if found:
                return found
        return None

    def wait(self, timeout=3.0):
        self.pump(timeout)
        try:
            os.close(self.fd)
        except OSError:
            pass
        _, status = os.waitpid(self.pid, 0)
        return os.waitstatus_to_exitcode(status)

    def close(self):
        try:
            os.write(self.fd, b"\x1b:q!\r")
            self.pump(0.3)
            os.close(self.fd)
        except OSError:
            pass


def workdir(name):
    """A clean scratch directory for one suite."""
    path = os.path.join("/tmp", f"miv-e2e-{name}")
    shutil.rmtree(path, ignore_errors=True)
    os.makedirs(path)
    return path


def git(directory, *args):
    subprocess.run(
        ["git", *args],
        cwd=directory,
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def init_repo(directory):
    git(directory, "init", "-q")
    git(directory, "config", "user.email", "e2e@example.com")
    git(directory, "config", "user.name", "End To End")


def executable(path, body):
    with open(path, "w") as handle:
        handle.write("#!/bin/sh\n" + body + "\n")
    os.chmod(path, 0o755)
    return path


def main(run):
    """Entry point shared by the suites: run, report, set the exit status."""
    sys.exit(1 if run() else 0)
