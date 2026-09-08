# miv

A modal text editor for the terminal, written in Rust.

```
cargo install --path .
miv src/main.rs
```

Press `:help` inside the editor for the full key reference.

## What works

**Modes** — normal, insert, replace (`R`), visual, visual line, plus the `:`
and `/` prompts.

**The full operator grammar**, not a list of special cases:
`["register]{count}{operator}{count}{motion-or-text-object}`. So `3dw`,
`d2f)`, `"ay}`, `2>>` and `ci"` all work because they are the same code path.

**Motions** — `h j k l`, `w W b B e E`, `0 ^ $ g_`, `gg G {n}G`, `f F t T`
with `;` and `,`, `{ }`, `%`, `H M L`, `|`, marks (`m` `` ` `` `'`), search
(`n N`), and the jump list (`Ctrl-O` / `Ctrl-I`).

**Operators** — `d c y`, `> <`, `gu gU g~`, each with a doubled line-wise
form (`dd`, `yy`, `>>`).

**Text objects** — `iw aw iW aW`, `i" i' i\``, `i( i[ i{ i<`, `ip ap`, each in
inner and around flavours.

**Editing** — `i a I A o O`, `x X s S D C Y`, `r`, `~`, `J`, `p P`, `u`,
`Ctrl-R`, and `.` to repeat the last change.

**Registers** — unnamed, `a`–`z` (uppercase appends), the yank register `0`,
the delete ring `1`–`9`, the blackhole `_`, and `+`/`*`, which copy to the
system clipboard over OSC 52 so it works through SSH and tmux.

**Macros** — `q{register}` to record, `q` to stop, `@{register}` to replay.
Macros are stored as editable text in registers, so `:reg` shows them and you
can yank one into a buffer and fix it.

**Search** — incremental, with match highlighting, `smartcase`, and wrapping.
`*` searches for the word under the cursor.

**Commands** — `:w :wq :x :q :q! :wa :wqa`, `:e :e!`, `:ls :b :bn :bp :bd`,
`:{line}`, `:s///` with ranges (`%`, `.`, `$`, `'<,'>`, `.,+5`), `:set`,
`:reg`, `:marks`, `:noh`, `:help`.

There is one window, so `:q` leaves the editor rather than closing a buffer
(`:bd` does that), and it refuses while any buffer is unsaved.

**Files** — multiple buffers, atomic saves, UTF-8, and round-tripping of CRLF
line endings and files with no trailing newline.

## What is deliberately not here

Visual block mode (`Ctrl-V`), splits and tabs, folds, LSP, tree-sitter,
sentence motions (`(` `)`), `gq` formatting, quickfix, and `:!` shell
commands. Lines do not wrap: long lines scroll horizontally.

Two intentional divergences from Vim:

- **Search and `:s` take Rust regular expressions**, not Vim's dialect. So
  `\d+` and `(a|b)` work as written and `\(...\)` does not. Patterns cannot
  span a newline. In a replacement, `\1` is accepted as a synonym for `${1}`.
- **`j`/`k` remember a character column**, not a screen column, so they can
  drift on lines that mix tabs and text.

## Configuration

`$XDG_CONFIG_HOME/miv/config.toml`, or `~/.config/miv/config.toml`. See
[example-config.toml](example-config.toml) for every option and its default.
`$MIV_CONFIG` and `--config PATH` override the location; `--no-config` skips
it. Unknown keys are errors, not silent no-ops.

Almost everything is also settable at runtime: `:set nu`, `:set ts=2`,
`:set noet`, `:set theme=Solarized (dark)`, and `:set opt?` to ask.

## Command line

```
miv [FILE]...              open one or more files as buffers
miv +42 file.rs            open at line 42 (also --line 42)
miv --list-themes          print the available themes
miv --config PATH          use a specific configuration file
miv --no-config            start with built-in defaults
```

## How it is put together

| Module | Responsibility |
| --- | --- |
| `text` | Positions, rope queries, character classes, display widths |
| `buffer` | One file: rope, cursor, viewport, marks, atomic save |
| `history` | Invertible transactions; undo granularity |
| `keymap` | The pending-state machine that parses the grammar |
| `motion` | Motions, and turning `cursor → target` into a range |
| `textobject` | `iw`, `a(`, … |
| `operator` | Applying `d c y > < gu gU g~` to a range |
| `command` | The `:` and `/` prompts, ex commands, `:s` |
| `search` | Regex search over the rope |
| `register` | Registers, including the OSC 52 clipboard bridge |
| `syntax` | syntect with checkpointed parser state |
| `app` | Editor state and the editing actions |
| `ui` | ratatui rendering |

Three decisions worth knowing about:

**The rope always ends with a newline.** `text::line_count` reports one fewer
line than ropey does. This makes every line index in `0..line_count()`
addressable and removes a whole class of last-line edge cases.

**Every buffer mutation goes through `Buffer::insert`/`remove`.** They record
history and invalidate the syntax cache, so no call site can forget to.
Transactions nest by depth, so a command built from several primitives still
undoes as one step.

**Syntax state is checkpointed every 64 lines, and catch-up is bounded.**
syntect needs the parser state from the previous line, so highlighting line
28,000 from a cold cache means parsing 28,000 lines — measured at just over a
second, on every `G`. A redraw resumes from the nearest checkpoint, and if
none is within ~1,500 lines it starts from a fresh parser state at the
viewport, which brings that jump down to 15ms. The cost is that a construct
opened far above (a long block comment) can be mis-coloured until you scroll
through the gap.

## Development

```
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The editing tests drive the real key handler with Vim notation
(`press(&mut app, "cwgamma<Esc>")`), so they exercise the whole path a
keystroke takes rather than calling internals.

`Cargo.lock` is committed and the crate declares `rust-version = "1.81"`.
Build with `--locked` for a reproducible build.
