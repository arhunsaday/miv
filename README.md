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

## Shared sessions

Run `:share` and your editor becomes a host that other people can join — from
another terminal, or from a browser, with nothing to install.

```
:share                 start hosting; prints a URL and an attach command
:share --write         guests can edit immediately
:share --port 7420     fixed port, for a stable SSH tunnel
:who                   who is here, where they are, how they connected
:grant arhun           let a guest edit
:follow arhun          mirror someone's viewport
:say ready when you are
:unshare               end it
```

Guests join either way:

```bash
miv --attach 127.0.0.1:7420 --token <token>     # another terminal, Ctrl-\ detaches
```

or by opening the printed URL in a browser.

**This is not screen sharing.** There is one copy of the text — the host's —
but every participant gets a genuinely independent view: their own cursor,
their own viewport, their own mode, their own search and command line. You can
be in insert mode on line 12 while someone else is in visual mode on line 400
of the same buffer. Carets are drawn in each other's views with a name tag, and
when anyone edits, everyone else's cursor moves with the text it was pointing
at rather than drifting.

Because participants exchange keystrokes and screen cells rather than document
state, there is no convergence problem to get wrong — and a guest sees every
feature the host's editor has, including ones their client has never heard of.
The browser client is 300 lines and renders whatever the host draws.

Over SSH there are two ways round:

```bash
# you are already on the box: attach locally
ssh box; miv --attach 127.0.0.1:7420 --token <token>

# or forward the port and use the browser from your laptop
ssh -L 7420:127.0.0.1:7420 box
```

### What sharing does and does not grant

The defaults are chosen so that nothing surprising happens:

- The listener binds to **loopback only**. Reaching it from another machine
  takes a deliberate act — an SSH tunnel, or a `bind` you set yourself.
- A session **token** is required to connect. It is not in the config; it is
  generated per session from `/dev/urandom` and printed once.
- Guests are **read-only** by default. A read-only guest can move, scroll,
  search and select — but any edit it manages to trigger is rolled back,
  because every mutation goes through the undo history and can therefore be
  undone unconditionally.
- Guests **cannot run `:` commands**, even with write access, until you set
  `session.guest_commands`. Editing text is a much smaller grant than
  `:w /some/other/path` or `:e` on anything the host can read.
- A guest can never quit the host's editor.

Write access still means someone is typing into your buffer, and
`guest_commands` means they can run ex commands as you. Grant them to people
you would hand your keyboard to.

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
| `session` | Shared sessions: participants, protocol, server, web and terminal clients |

Three decisions worth knowing about:

**The rope always ends with a newline.** `text::line_count` reports one fewer
line than ropey does. This makes every line index in `0..line_count()`
addressable and removes a whole class of last-line edge cases.

**Every buffer mutation goes through `Buffer::insert`/`remove`.** They record
history and invalidate the syntax cache, so no call site can forget to.
Transactions nest by depth, so a command built from several primitives still
undoes as one step.

**A participant is a small bundle of swapped-in state.** `session::PerUser`
holds what belongs to a person rather than to the document — cursor, viewport,
mode, pending keys, prompt. Swapping that bundle in and out around a command is
what lets every existing command work per-participant without knowing sessions
exist, and it is why adding a feature to the editor adds it to every guest for
free.

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
keystroke takes rather than calling internals. The session tests drive a real
`Session` without opening a socket, covering per-participant state, cursor
transformation under concurrent edits, and the read-only guarantee.

`Cargo.lock` is committed and the crate declares `rust-version = "1.81"`.
Build with `--locked` for a reproducible build.
