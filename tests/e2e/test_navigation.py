"""Windows, pickers, the sidebar, auto-pairs and completion, on screen."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Pty, Report, binary, main, workdir


def run():
    miv = binary()
    work = workdir("navigation")
    report = Report("navigation")

    alpha = os.path.join(work, "alpha.txt")
    beta = os.path.join(work, "beta.txt")
    with open(alpha, "w") as handle:
        handle.write("alpha one\nalpha two\nalpha three\n")
    with open(beta, "w") as handle:
        handle.write("beta one\nbeta two\n")

    # -- splits ------------------------------------------------------------
    editor = Pty([miv, alpha], cols=90, rows=14)
    editor.command(f":vsplit {beta}")
    screen = editor.text()
    report.check("a vertical split draws a separator", "│" in screen, screen)
    report.check(
        "both buffers are on screen at once",
        "alpha one" in screen and "beta one" in screen,
        screen,
    )
    report.check(
        "each window has its own status line, and only one is active",
        screen.count("NORMAL") == 1,
        screen,
    )
    report.check(
        "the unfocused window still names its file",
        "alpha.txt" in screen and "beta.txt" in screen,
        screen,
    )

    # Focus follows Ctrl-W, and the cursor stays put in the window you left.
    editor.send("G")
    active_before = [row for row in editor.rows() if "NORMAL" in row]
    editor.send("\x17h")
    active_after = [row for row in editor.rows() if "NORMAL" in row]
    report.check(
        "'Ctrl-W h' moves focus to the other window",
        active_before != active_after and len(active_after) == 1,
        f"{active_before}\n{active_after}",
    )
    # The window we left keeps its own cursor. Its status line is truncated by
    # the split, so read it from the gutter instead: relative numbering puts a
    # `0` on the cursor's line, and that window's cursor is on its second.
    right_column = [
        row.split("│", 1)[1] if "│" in row else "" for row in editor.rows()[:3]
    ]
    report.check(
        "the window we left kept its own cursor",
        right_column[1].strip().startswith("0"),
        "\n".join(right_column),
    )

    editor.command(":only")
    report.check(
        "':only' leaves a single window",
        "│" not in editor.text() and editor.text().count("NORMAL") == 1,
        editor.text(),
    )
    editor.close()

    # -- the file picker ---------------------------------------------------
    editor = Pty([miv, alpha], cols=90, rows=16, cwd=work)
    editor.send("\x10")  # Ctrl-P
    editor.wait_for(lambda rows: any("Files" in row for row in rows), 10)
    report.check("Ctrl-P opens the file picker", "Files" in editor.text(), editor.text())

    editor.send("beta", settle=0.5)
    screen = editor.text()
    report.check("typing filters the list", "beta.txt" in screen, screen)
    report.check(
        "the picker reports how much it narrowed",
        "/" in "".join(row for row in editor.rows() if "Files" in row),
        screen,
    )
    editor.send("\r", settle=0.6)
    report.check(
        "Enter opens the chosen file",
        "beta one" in editor.text() and editor.picker_closed(),
        editor.text(),
    )
    editor.close()

    # -- the command palette -----------------------------------------------
    editor = Pty([miv, alpha], cols=90, rows=16)
    editor.send("\x0b")  # Ctrl-K
    editor.wait_for(lambda rows: any("Commands" in row for row in rows), 8)
    report.check("Ctrl-K opens the command palette", "Commands" in editor.text())
    editor.send("side by side", settle=0.4)
    report.check(
        "commands are shown with what they run",
        ":vsplit" in editor.text(),
        editor.text(),
    )
    editor.send("\r", settle=0.6)
    report.check(
        "the chosen command runs",
        "│" in editor.text(),
        editor.text(),
    )
    editor.close()

    # -- the sidebar -------------------------------------------------------
    editor = Pty([miv, alpha], cols=90, rows=16, cwd=work)
    editor.send("\x17e", settle=0.6)  # Ctrl-W e
    screen = editor.text()
    report.check("Ctrl-W e opens the explorer", "EXPLORER" in screen, screen)
    report.check(
        "the explorer lists the project directory",
        "alpha.txt" in screen and "beta.txt" in screen,
        screen,
    )
    editor.send("j\r", settle=0.6)
    report.check(
        "Enter in the explorer opens the file and returns the keyboard",
        "NORMAL" in editor.status(),
        editor.text(),
    )
    editor.send("\x17e", settle=0.5)
    report.check(
        "Ctrl-W e closes it again",
        "EXPLORER" not in editor.text(),
        editor.text(),
    )
    editor.close()

    # -- auto-pairs and completion -----------------------------------------
    source = os.path.join(work, "sample.rs")
    with open(source, "w") as handle:
        handle.write("let calculation = 1;\n")

    editor = Pty([miv, source], cols=90, rows=16)
    editor.send("Go", settle=0.3)
    editor.send("call(", settle=0.4)
    report.check(
        "an opening bracket closes itself",
        "call()" in editor.text(),
        editor.text(),
    )
    editor.send("\x1b", settle=0.3)
    editor.send("cc", settle=0.3)
    editor.send("calc", settle=0.6)
    screen = editor.text()
    report.check(
        "a suggestion list appears from words in the buffer",
        "calculation" in screen,
        screen,
    )
    editor.send("\t", settle=0.5)
    report.check(
        "Tab accepts the suggestion",
        "calculation" in editor.row(1) or "calculation" in editor.row(2),
        editor.text(),
    )
    editor.send("\x1b", settle=0.3)
    editor.close()

    return report.finish()


if __name__ == "__main__":
    main(run)
