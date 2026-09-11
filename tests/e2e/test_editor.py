"""The editor itself: saving, quitting, resizing, buffers, macros."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Pty, Report, binary, main, workdir


def run():
    miv = binary()
    work = workdir("editor")
    report = Report("editor")

    # -- edit and save -----------------------------------------------------
    target = os.path.join(work, "edit.txt")
    with open(target, "w") as handle:
        handle.write("alpha\nbeta\ngamma\n")

    editor = Pty([miv, target])
    editor.send("ggO")
    editor.send("inserted")
    editor.send("\x1b")
    editor.send("jdd")
    editor.command(":wq")
    code = editor.wait()
    with open(target) as handle:
        saved = handle.read()
    report.check("edits are saved through ':wq'", saved == "inserted\nbeta\ngamma\n", repr(saved))
    report.check("the process exits cleanly", code == 0, f"exit {code}")
    report.check("the alternate screen is left on exit", b"\x1b[?1049l" in bytes(editor.raw))
    report.check("bracketed paste is disabled on exit", b"\x1b[?2004l" in bytes(editor.raw))
    leftovers = [name for name in os.listdir(work) if "miv~" in name]
    report.check("no temporary file is left behind", leftovers == [], str(leftovers))

    # -- quitting with unsaved changes --------------------------------------
    editor = Pty([miv, target])
    editor.send("x")
    editor.command(":q")
    report.check(
        "':q' with unsaved changes is refused",
        "E37" in editor.text(),
        editor.message(),
    )
    editor.command(":q!")
    report.check("':q!' discards and exits", editor.wait() == 0)
    with open(target) as handle:
        report.check("a forced quit did not touch the file", handle.read() == "inserted\nbeta\ngamma\n")

    # -- resizing ----------------------------------------------------------
    editor = Pty([miv, target])
    editor.resize(40, 10)
    report.check(
        "the layout follows a resize",
        "NORMAL" in editor.row(8) and len(editor.row(0)) <= 40,
        repr(editor.row(8)),
    )
    editor.resize(110, 30)
    report.check(
        "the layout follows a second resize",
        "NORMAL" in editor.row(28),
        repr(editor.row(28)),
    )
    editor.command(":q")
    report.check("it exits cleanly after resizing", editor.wait() == 0)

    # -- writing -----------------------------------------------------------
    editor = Pty([miv])
    editor.send("ihello world")
    editor.send("\x1b")
    created = os.path.join(work, "created.txt")
    editor.command(f":w {created}")
    wrote = os.path.exists(created)
    if wrote:
        with open(created) as handle:
            wrote = handle.read() == "hello world\n"
    report.check("':w path' creates a new file", wrote)

    editor.command(":w /proc/nope/denied.txt")
    report.check(
        "an unwritable path reports an error",
        "E212" in editor.text(),
        editor.message(),
    )
    editor.command(":q!")
    report.check("it is still responsive after a failed write", editor.wait() == 0)

    # -- macros ------------------------------------------------------------
    editor = Pty([miv, target])
    editor.send("qaI> \x1b")
    editor.send("jq")
    editor.send("2@a", settle=0.5)
    rows = editor.rows()[:3]
    report.check(
        "a macro replays across lines",
        all(row.strip().startswith(tuple("0123456789")) for row in rows)
        and sum("> " in row for row in rows) == 3,
        str(rows),
    )
    editor.close()

    # -- buffers -----------------------------------------------------------
    one, two = os.path.join(work, "one.txt"), os.path.join(work, "two.txt")
    with open(one, "w") as handle:
        handle.write("first file\n")
    with open(two, "w") as handle:
        handle.write("second file\n")

    editor = Pty([miv, one, two])
    report.check("the first of several files is opened", "first file" in editor.row(0), editor.row(0))
    report.check("the status line shows the buffer count", "1/2" in editor.status(), editor.status())
    editor.command(":bn")
    report.check(
        "':bn' switches buffer",
        "second file" in editor.row(0) and "2/2" in editor.status(),
        editor.row(0),
    )
    editor.command(":ls")
    listed = [row for row in editor.rows() if "one.txt" in row or "two.txt" in row]
    report.check("':ls' lists both buffers", len(listed) >= 2, str(listed))
    editor.send("\x1b")
    editor.command(f":e {work}/third.txt")
    report.check(
        "':e' on a missing file opens it as new",
        "[New]" in editor.text(),
        editor.message(),
    )
    editor.command(":bd")
    editor.command(":q")
    report.check("buffers close and the editor exits", editor.wait() == 0)

    return report.finish()


if __name__ == "__main__":
    main(run)
