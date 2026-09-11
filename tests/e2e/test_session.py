"""Shared sessions: the terminal client, the browser client, access control."""

import json
import os
import random
import socket
import sys
import time
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Pty, Report, binary, main, workdir

try:
    import websocket
except ImportError:  # pragma: no cover
    raise SystemExit("this suite needs `websocket-client`")


class Guest:
    """A raw terminal-protocol client: newline-delimited JSON over TCP."""

    def __init__(self, host, port, token, name, cols=70, rows=20):
        self.sock = socket.create_connection((host, port), timeout=5)
        self.buffer = b""
        self.send(
            {
                "type": "hello",
                "version": 1,
                "token": token,
                "name": name,
                "cols": cols,
                "rows": rows,
            }
        )

    def send(self, message):
        self.sock.sendall((json.dumps(message) + "\n").encode())

    def keys(self, keys):
        self.send({"type": "keys", "keys": keys})

    def messages(self, timeout=1.2):
        out, end = [], time.time() + timeout
        while time.time() < end:
            self.sock.settimeout(0.1)
            try:
                chunk = self.sock.recv(65536)
            except socket.timeout:
                continue
            except OSError:
                break
            if not chunk:
                break
            self.buffer += chunk
            while b"\n" in self.buffer:
                line, self.buffer = self.buffer.split(b"\n", 1)
                if line.strip():
                    out.append(json.loads(line))
        return out

    def close(self):
        try:
            self.send({"type": "bye"})
            self.sock.close()
        except OSError:
            pass


def refuse(port, hello):
    """Send a hello that should be rejected, and return the reply."""
    sock = socket.create_connection(("127.0.0.1", port), timeout=5)
    sock.sendall((json.dumps(hello) + "\n").encode())
    sock.settimeout(2)
    try:
        reply = json.loads(sock.recv(65536).decode().split("\n")[0])
    except Exception as error:  # noqa: BLE001 - the reply is the assertion
        reply = {"type": str(error)}
    sock.close()
    return reply


def run():
    miv = binary()
    work = workdir("session")
    report = Report("session")

    target = os.path.join(work, "shared.txt")
    with open(target, "w") as handle:
        handle.write("alpha\nbeta\ngamma\n")

    port = random.randint(21000, 39000)
    host = Pty([miv, target])
    host.wait_for(lambda rows: any("NORMAL" in row for row in rows))
    host.command(f":share --port {port}", settle=0.8)
    host.wait_for(lambda rows: any("Sharing this buffer" in row for row in rows), 8)

    found = host.find(r"--token ([0-9a-f]{24})")
    report.check("':share' prints a token and an attach command", found is not None, host.text())
    token = found.group(1) if found else ""
    report.check(
        "':share' prints a browser URL",
        host.find(r"http://127\.0\.0\.1:%d/s/" % port) is not None,
        host.text(),
    )

    # -- a terminal guest --------------------------------------------------
    guest = Guest("127.0.0.1", port, token, "guest")
    messages = guest.messages()
    welcome = next((m for m in messages if m["type"] == "welcome"), None)
    report.check("a guest is welcomed", welcome is not None, str(messages)[:200])
    report.check("guests are read-only by default", welcome and welcome["access"] == "read")

    frames = [m for m in messages if m["type"] == "frame"]
    report.check("the first frame is a full repaint", bool(frames) and frames[0]["full"])
    report.check(
        "the frame is sized to the guest's own terminal",
        bool(frames) and frames[0]["cols"] == 70 and len(frames[0]["cells"]) == 70 * 20,
        str(frames[0]["cols"]) if frames else "no frame",
    )
    host.pump(0.4)
    report.check("the host is told someone joined", "joined" in host.text(), host.message())

    # -- read-only enforcement ---------------------------------------------
    guest.keys("ggdd")
    guest.keys("ihacked<Esc>")
    time.sleep(0.4)
    host.pump(0.5)
    notices = [m for m in guest.messages() if m["type"] == "notice"]
    report.check(
        "a read-only guest is refused, and told why",
        any("read-only" in m["text"] for m in notices),
        str(notices)[:200],
    )
    with open(target) as handle:
        untouched = handle.read() == "alpha\nbeta\ngamma\n"
    report.check(
        "the text is untouched by a read-only guest",
        untouched and "alpha" in host.row(0),
        host.row(0),
    )
    guest.keys("G")
    time.sleep(0.3)
    host.pump(0.4)
    report.check(
        "a read-only guest can still move around",
        any(m["type"] == "frame" and not m["full"] for m in guest.messages()),
        "no frame after moving",
    )

    # -- granting ----------------------------------------------------------
    host.command(":grant guest")
    guest.keys("ggIX ")
    guest.keys("<Esc>")
    time.sleep(0.4)
    host.pump(0.6)
    report.check("after ':grant' the guest can edit", "X alpha" in host.row(0), host.row(0))

    host.command(":who")
    report.check(
        "':who' lists the host and the guest with their transports",
        "guest" in host.text() and "terminal" in host.text() and "host" in host.text(),
        host.text(),
    )
    host.send("\x1b")

    # -- the browser client ------------------------------------------------
    page = urllib.request.urlopen(f"http://127.0.0.1:{port}/s/{token}", timeout=5).read().decode()
    report.check("the browser page is served", "miv — shared session" in page)
    report.check(
        "the page carries the token so the socket can authenticate itself",
        token in page and "__MIV_TOKEN__" not in page,
    )
    wrong = urllib.request.urlopen(f"http://127.0.0.1:{port}/s/nope", timeout=5).read().decode()
    report.check("a wrong token in the URL is not echoed back", token not in wrong)

    ws = websocket.create_connection(f"ws://127.0.0.1:{port}/ws", timeout=5)
    ws.send(
        json.dumps(
            {"type": "hello", "version": 1, "token": token, "name": "web", "cols": 60, "rows": 18}
        )
    )
    web_messages = []
    end = time.time() + 2.0
    while time.time() < end:
        ws.settimeout(0.2)
        try:
            web_messages.append(json.loads(ws.recv()))
        except Exception:  # noqa: BLE001 - polling until the deadline
            pass
    report.check(
        "the websocket handshake succeeds and a welcome arrives",
        any(m["type"] == "welcome" for m in web_messages),
        str(web_messages)[:200],
    )
    web_frame = next((m for m in web_messages if m["type"] == "frame"), None)
    report.check(
        "the browser gets a frame sized to its own window",
        web_frame is not None and web_frame["cols"] == 60 and web_frame["rows"] == 18,
        str(web_frame)[:120] if web_frame else "none",
    )
    roster = next((m for m in web_messages if m["type"] == "roster"), None)
    report.check(
        "the roster names every participant",
        roster is not None and len(roster["participants"]) >= 3,
        str(roster)[:200] if roster else "none",
    )

    # -- refusals ----------------------------------------------------------
    reply = refuse(
        port,
        {"type": "hello", "version": 1, "token": "0" * 24, "name": "rogue", "cols": 40, "rows": 10},
    )
    report.check(
        "a wrong token is rejected",
        reply.get("type") == "closed" and "token" in reply.get("reason", ""),
        str(reply),
    )
    reply = refuse(
        port,
        {"type": "hello", "version": 99, "token": token, "name": "old", "cols": 40, "rows": 10},
    )
    report.check(
        "a protocol mismatch is reported",
        reply.get("type") == "closed" and "protocol" in reply.get("reason", ""),
        str(reply),
    )

    # -- the attach client -------------------------------------------------
    attached = Pty(
        [miv, "--attach", f"127.0.0.1:{port}", "--token", token, "--name", "term2"],
        cols=80,
        rows=20,
    )
    attached.pump(1.0)
    report.check(
        "'miv --attach' renders the shared buffer in a second terminal",
        any("alpha" in row for row in attached.rows()),
        attached.text(),
    )
    report.check(
        "the attached terminal draws its own status line",
        "NORMAL" in attached.text() or "shared" in attached.text(),
        attached.text(),
    )

    host.command(":who")
    report.check(
        "':who' now shows the browser and both terminals",
        "term2" in host.text() and "web" in host.text() and "browser" in host.text(),
        host.text(),
    )
    host.send("\x1b")

    host.command(":unshare")
    report.check(
        "':unshare' tells every participant the session ended",
        any(m["type"] == "closed" for m in guest.messages()),
        "no closing message",
    )

    ws.close()
    guest.close()
    attached.close()
    host.close()
    return report.finish()


if __name__ == "__main__":
    main(run)
