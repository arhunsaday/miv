"""Diagnostics, git signs, formatting and colour swatches, on screen."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Pty, Report, binary, executable, git, init_repo, main, workdir


def run():
    miv = binary()
    work = workdir("tooling")
    report = Report("tooling")

    # -- git signs ---------------------------------------------------------
    init_repo(work)
    tracked = os.path.join(work, "deploy.yaml")
    with open(tracked, "w") as handle:
        handle.write("replicas: 1\nimage: old\nports: 8080\nkeep: yes\nlast: 1\n")
    git(work, "add", ".")
    git(work, "commit", "-q", "-m", "init")
    # Line 2 changed; line 4 deleted outright, leaving its neighbours intact.
    with open(tracked, "w") as handle:
        handle.write("replicas: 1\nimage: new\nports: 8080\nlast: 1\n")

    editor = Pty([miv, tracked])
    editor.wait_for(lambda rows: any("┃" in row or "▁" in row for row in rows), 10)
    rows = editor.rows()
    report.check("a changed line is signed", "┃" in rows[1], rows[1])
    report.check(
        "an unchanged line is not signed",
        "┃" not in rows[0] and "▁" not in rows[0],
        rows[0],
    )
    report.check(
        "a deleted line leaves a marker behind",
        any("▁" in row for row in rows),
        editor.text(),
    )

    editor.send("gg")
    editor.send("]h")
    report.check("']h' jumps to the first change", " 2:" in editor.status(), editor.status())
    editor.send("]h")
    report.check("']h' reaches the next hunk", " 4:" in editor.status(), editor.status())
    editor.close()

    # -- diagnostics -------------------------------------------------------
    checker = executable(
        os.path.join(work, "fake-checker"),
        'echo "$1:2:3: [error] image tag must be pinned"\n'
        'echo "$1:3:1: [warning] keys are not sorted"\n'
        'echo "Checked 1 file."',
    )
    config = os.path.join(work, "config.toml")
    with open(config, "w") as handle:
        handle.write(
            "[diagnostics]\n"
            "use_builtin = false\n"
            "\n"
            "[[diagnostics.checker]]\n"
            'name = "policy"\n'
            f'command = ["{checker}", "$FILE"]\n'
            'pattern = "^[^:]*:(?<line>\\\\d+):(?<col>\\\\d+): '
            '\\\\[(?<severity>\\\\w+)\\\\] (?<message>.*)$"\n'
            "\n"
            "[signs]\n"
            "git = false\n"
        )

    editor = Pty([miv, "--config", config, tracked])
    editor.wait_for(lambda rows: any("●" in row for row in rows), 10)
    rows = editor.rows()
    report.check("an error sign appears in the gutter", "●" in rows[1], rows[1])
    report.check("a warning sign appears in the gutter", "▲" in rows[2], rows[2])
    report.check(
        "an unaffected line has no sign",
        "●" not in rows[0] and "▲" not in rows[0],
        rows[0],
    )
    report.check(
        "the tool's summary line is not mistaken for a diagnostic",
        "Checked 1 file" not in editor.text(),
        editor.text(),
    )
    report.check(
        "the status line counts them",
        "1E" in editor.status() and "1W" in editor.status(),
        editor.status(),
    )

    editor.send("j", settle=0.4)
    rows = editor.rows()
    report.check(
        "the cursor line shows its diagnostic inline",
        "image tag must be pinned" in rows[1] and "[policy]" in rows[1],
        rows[1],
    )
    report.check(
        "other lines stay uncluttered",
        "keys are not sorted" not in rows[2],
        rows[2],
    )

    editor.send("gg")
    editor.send("]d", settle=0.4)
    report.check(
        "']d' jumps and reports what it found",
        "image tag must be pinned" in editor.message(),
        editor.message(),
    )

    editor.command(":diag")
    listing = editor.text()
    report.check(
        "':diag' lists everything with a summary",
        "image tag must be pinned" in listing
        and "keys are not sorted" in listing
        and "1 error(s)" in listing,
        listing,
    )
    editor.send("\x1b")

    editor.command(":set nodiagnostics")
    report.check(
        "':set nodiagnostics' clears the gutter",
        not any("●" in row or "▲" in row for row in editor.rows()),
        editor.text(),
    )
    editor.close()

    # -- a checker that is not installed -----------------------------------
    missing_config = os.path.join(work, "missing.toml")
    with open(missing_config, "w") as handle:
        handle.write(
            "[diagnostics]\n"
            "use_builtin = false\n"
            "\n"
            "[[diagnostics.checker]]\n"
            'command = ["miv-no-such-checker"]\n'
            'pattern = "(?<line>\\\\d+)"\n'
        )
    editor = Pty([miv, "--config", missing_config, tracked])
    editor.wait_for(lambda rows: "not installed" in "\n".join(rows), 8)
    report.check(
        "a checker you have not installed says so once, as information",
        "not installed" in editor.message(),
        editor.message(),
    )
    editor.close()

    # -- formatting --------------------------------------------------------
    source = os.path.join(work, "sample.rs")
    with open(source, "w") as handle:
        handle.write('fn  main( ){let x=1;println!("{}",x);}\n')

    editor = Pty([miv, source])
    editor.command(":fmt", settle=2.0)
    rows = editor.rows()
    report.check(
        "':fmt' reformats through the configured tool",
        any("fn main() {" in row for row in rows)
        and any("let x = 1;" in row for row in rows),
        editor.text(),
    )
    report.check(
        "':fmt' says what it changed",
        "rustfmt" in editor.message() and "line" in editor.message(),
        editor.message(),
    )
    editor.send("u", settle=0.4)
    report.check(
        "the whole reformat undoes in one step",
        any("fn  main( ){let x=1;" in row for row in editor.rows()),
        editor.text(),
    )
    editor.close()

    # format on save, through a real save
    fos_config = os.path.join(work, "fos.toml")
    with open(fos_config, "w") as handle:
        handle.write("[format]\non_save = true\n")
    with open(source, "w") as handle:
        handle.write("fn  main( ){}\n")
    editor = Pty([miv, "--config", fos_config, source])
    editor.command(":w", settle=2.0)
    with open(source) as handle:
        on_disk = handle.read()
    report.check(
        "format-on-save writes the formatted file",
        on_disk == "fn main() {}\n",
        repr(on_disk),
    )
    editor.close()

    # -- colour swatches ---------------------------------------------------
    styles = os.path.join(work, "theme.css")
    with open(styles, "w") as handle:
        handle.write("a { color: #ff0000; }\nb { color: rgb(0, 128, 255); }\n")
    editor = Pty([miv, styles])
    editor.pump(0.6)
    first = editor.row(0)
    index = first.find("#ff0000")
    report.check(
        "a hex colour is painted in the colour it names",
        index >= 0 and editor.cell(0, index).bg in ("ff0000", "red"),
        f"{first!r} bg={editor.cell(0, index).bg if index >= 0 else 'n/a'}",
    )
    second = editor.row(1)
    index = second.find("rgb(")
    report.check(
        "an rgb() colour is painted too",
        index >= 0 and editor.cell(1, index).bg == "0080ff",
        f"{second!r} bg={editor.cell(1, index).bg if index >= 0 else 'n/a'}",
    )
    report.check(
        "the swatch stops at the end of the colour",
        index >= 0 and editor.cell(1, second.find(";")).bg != "0080ff",
        second,
    )
    editor.command(":set noswatches")
    index = editor.row(0).find("#ff0000")
    report.check(
        "':set noswatches' turns them off",
        index >= 0 and editor.cell(0, index).bg != "ff0000",
        f"bg={editor.cell(0, index).bg if index >= 0 else 'n/a'}",
    )
    editor.close()

    return report.finish()


if __name__ == "__main__":
    main(run)
