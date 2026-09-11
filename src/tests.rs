//! Tests.
//!
//! The editing tests drive the real key handler rather than calling internals,
//! so they cover the whole path a keystroke actually takes: pending state,
//! motion resolution, operator application, history and cursor placement.

use crate::app::{App, Viewport};
use crate::config::Config;
use crate::core::search::{self, Direction};
use crate::core::text::{self, Position};
use crate::keymap;
use crate::keys;
use crate::mode::Mode;
use ropey::Rope;

fn app_with(content: &str) -> App {
    let mut app = App::new(Config::default(), &[], None).expect("app");
    let mut normalized = content.to_string();
    if !normalized.ends_with('\n') {
        normalized.push('\n');
    }
    app.buffer_mut().rope = Rope::from_str(&normalized);
    app.viewport = Viewport {
        height: 20,
        text_width: 80,
    };
    app
}

/// Feed a key sequence in Vim notation, draining anything `.` or a macro
/// queues, exactly as the main loop does.
fn press(app: &mut App, input: &str) {
    for key in keys::parse(input) {
        app.begin_input();
        keymap::handle(app, key);
        while let Some(queued) = app.queue.pop_front() {
            keymap::handle(app, queued);
        }
    }
}

fn content(app: &App) -> String {
    app.buffer().rope.to_string()
}

fn cursor(app: &App) -> (usize, usize) {
    let position = app.buffer().cursor;
    (position.line, position.col)
}

// -- text primitives --------------------------------------------------------

#[test]
fn line_queries_exclude_the_line_ending() {
    let rope = Rope::from_str("alpha\nbeta\n");
    assert_eq!(text::line_count(&rope), 2);
    assert_eq!(text::line(&rope, 0).to_string(), "alpha");
    assert_eq!(text::line_len(&rope, 1), 4);
}

#[test]
fn positions_round_trip_through_character_offsets() {
    let rope = Rope::from_str("alpha\nbeta\n");
    let position = Position::new(1, 2);
    let offset = text::pos_to_char(&rope, position);
    assert_eq!(offset, 8);
    assert_eq!(text::char_to_pos(&rope, offset), position);
}

#[test]
fn normal_mode_cannot_rest_past_the_last_character() {
    let rope = Rope::from_str("abc\n");
    assert_eq!(text::clamp(&rope, Position::new(0, 9), false).col, 2);
    assert_eq!(text::clamp(&rope, Position::new(0, 9), true).col, 3);
}

#[test]
fn screen_columns_account_for_tabs_and_wide_characters() {
    let rope = Rope::from_str("\tab\n");
    // A tab at column 0 fills the first tab stop.
    assert_eq!(text::screen_col(text::line(&rope, 0), 1, 4), 4);

    let wide = Rope::from_str("日本語x\n");
    assert_eq!(text::screen_col(text::line(&wide, 0), 3, 4), 6);
}

// -- buffer, history and persistence ---------------------------------------

#[test]
fn undo_restores_text_and_cursor() {
    let mut app = app_with("hello world");
    press(&mut app, "dw");
    assert_eq!(content(&app), "world\n");
    press(&mut app, "u");
    assert_eq!(content(&app), "hello world\n");
    assert_eq!(cursor(&app), (0, 0));
    press(&mut app, "<C-r>");
    assert_eq!(content(&app), "world\n");
}

#[test]
fn an_insert_session_undoes_as_one_step() {
    let mut app = app_with("x");
    press(&mut app, "ihello<Esc>");
    assert_eq!(content(&app), "hellox\n");
    press(&mut app, "u");
    assert_eq!(content(&app), "x\n");
}

#[test]
fn change_and_the_typing_that_follows_undo_together() {
    let mut app = app_with("alpha beta");
    press(&mut app, "cwgamma<Esc>");
    assert_eq!(content(&app), "gamma beta\n");
    press(&mut app, "u");
    assert_eq!(content(&app), "alpha beta\n");
}

#[test]
fn saving_preserves_crlf_and_a_missing_final_newline() {
    let directory = std::env::temp_dir().join(format!("miv-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();

    let crlf_path = directory.join("crlf.txt");
    std::fs::write(&crlf_path, b"one\r\ntwo\r\n").unwrap();
    let mut buffer = crate::core::buffer::Buffer::open(1, &crlf_path).unwrap();
    buffer.write(&crlf_path).unwrap();
    assert_eq!(std::fs::read(&crlf_path).unwrap(), b"one\r\ntwo\r\n");

    let bare_path = directory.join("bare.txt");
    std::fs::write(&bare_path, b"no newline").unwrap();
    let mut buffer = crate::core::buffer::Buffer::open(2, &bare_path).unwrap();
    buffer.write(&bare_path).unwrap();
    assert_eq!(std::fs::read(&bare_path).unwrap(), b"no newline");

    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn writing_leaves_no_temporary_file_behind() {
    let directory = std::env::temp_dir().join(format!("miv-atomic-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("file.txt");
    std::fs::write(&path, b"before\n").unwrap();

    let mut buffer = crate::core::buffer::Buffer::open(1, &path).unwrap();
    buffer.insert(0, "after ");
    buffer.write(&path).unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "after before\n");
    let leftovers: Vec<_> = std::fs::read_dir(&directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains("miv~"))
        .collect();
    assert!(leftovers.is_empty(), "temporary file was left behind");
    std::fs::remove_dir_all(&directory).ok();
}

// -- motions ----------------------------------------------------------------

#[test]
fn word_motions_step_by_class() {
    let mut app = app_with("foo.bar baz");
    press(&mut app, "w");
    assert_eq!(cursor(&app), (0, 3)); // onto the punctuation
    press(&mut app, "w");
    assert_eq!(cursor(&app), (0, 4));
    press(&mut app, "W");
    assert_eq!(cursor(&app), (0, 8));
    press(&mut app, "b");
    assert_eq!(cursor(&app), (0, 4));
    press(&mut app, "e");
    assert_eq!(cursor(&app), (0, 6));
}

#[test]
fn an_empty_line_counts_as_a_word() {
    let mut app = app_with("a\n\nb");
    press(&mut app, "w");
    assert_eq!(cursor(&app), (1, 0));
    press(&mut app, "w");
    assert_eq!(cursor(&app), (2, 0));
}

#[test]
fn find_character_motions_and_their_repeats() {
    let mut app = app_with("a.b.c.d");
    press(&mut app, "f.");
    assert_eq!(cursor(&app), (0, 1));
    press(&mut app, ";");
    assert_eq!(cursor(&app), (0, 3));
    press(&mut app, ",");
    assert_eq!(cursor(&app), (0, 1));
    press(&mut app, "t.");
    assert_eq!(cursor(&app), (0, 2));
    press(&mut app, "$F.");
    assert_eq!(cursor(&app), (0, 5));
}

#[test]
fn counts_multiply_across_an_operator() {
    let mut app = app_with("one two three four five six seven");
    press(&mut app, "2d3w");
    assert_eq!(content(&app), "seven\n");
}

#[test]
fn goto_line_and_paragraph_motions() {
    let mut app = app_with("1\n2\n\n4\n5");
    press(&mut app, "G");
    assert_eq!(cursor(&app), (4, 0));
    press(&mut app, "gg");
    assert_eq!(cursor(&app), (0, 0));
    press(&mut app, "}");
    assert_eq!(cursor(&app), (2, 0));
    press(&mut app, "3G");
    assert_eq!(cursor(&app), (2, 0));
}

#[test]
fn percent_jumps_between_matching_brackets() {
    let mut app = app_with("if (a(b)) {\n  x\n}");
    press(&mut app, "f(%");
    assert_eq!(cursor(&app), (0, 8));
    press(&mut app, "%");
    assert_eq!(cursor(&app), (0, 3));
}

// -- operators and text objects --------------------------------------------

#[test]
fn delete_word_at_the_end_of_a_line_does_not_join_lines() {
    let mut app = app_with("alpha beta\ngamma");
    press(&mut app, "$bdw");
    assert_eq!(content(&app), "alpha \ngamma\n");
}

#[test]
fn change_word_stops_at_the_word_end_like_vim() {
    let mut app = app_with("alpha beta");
    press(&mut app, "cwX<Esc>");
    assert_eq!(content(&app), "X beta\n");
}

#[test]
fn doubled_operators_act_on_whole_lines() {
    let mut app = app_with("one\ntwo\nthree\nfour");
    press(&mut app, "j2dd");
    assert_eq!(content(&app), "one\nfour\n");
    press(&mut app, "yyP");
    assert_eq!(content(&app), "one\nfour\nfour\n");
}

#[test]
fn inner_and_around_word_objects() {
    let mut app = app_with("alpha beta gamma");
    press(&mut app, "wdiw");
    assert_eq!(content(&app), "alpha  gamma\n");
    let mut app = app_with("alpha beta gamma");
    press(&mut app, "wdaw");
    assert_eq!(content(&app), "alpha gamma\n");
}

#[test]
fn quoted_and_bracketed_objects() {
    let mut app = app_with("say(\"hello there\", x)");
    press(&mut app, "ci\"bye<Esc>");
    assert_eq!(content(&app), "say(\"bye\", x)\n");

    // `a(` needs the cursor inside the block, exactly as in Vim.
    let mut app = app_with("say(\"hello\", x)");
    press(&mut app, "fhda(");
    assert_eq!(content(&app), "say\n");

    let mut app = app_with("call(a, b)");
    press(&mut app, "f,di(");
    assert_eq!(content(&app), "call()\n");
}

#[test]
fn paragraph_object_is_linewise() {
    let mut app = app_with("a\nb\n\nc\n");
    press(&mut app, "dip");
    assert_eq!(content(&app), "\nc\n");
}

#[test]
fn indent_and_dedent_shift_by_shiftwidth() {
    let mut app = app_with("a\nb\nc");
    press(&mut app, "2>>");
    assert_eq!(content(&app), "    a\n    b\nc\n");
    press(&mut app, "<<");
    assert_eq!(content(&app), "a\n    b\nc\n");
}

#[test]
fn case_operators_apply_over_a_motion() {
    let mut app = app_with("hello world");
    press(&mut app, "gUw");
    assert_eq!(content(&app), "HELLO world\n");
    press(&mut app, "g~$");
    assert_eq!(content(&app), "hello WORLD\n");
}

#[test]
fn change_on_a_whole_line_keeps_its_indentation() {
    let mut app = app_with("    indented text\nnext");
    press(&mut app, "ccnew<Esc>");
    assert_eq!(content(&app), "    new\nnext\n");
}

// -- single key edits -------------------------------------------------------

#[test]
fn character_deletes_joins_and_replacements() {
    let mut app = app_with("abcdef");
    press(&mut app, "3x");
    assert_eq!(content(&app), "def\n");
    press(&mut app, "$X");
    assert_eq!(content(&app), "df\n");

    let mut app = app_with("one\n   two");
    press(&mut app, "J");
    assert_eq!(content(&app), "one two\n");

    let mut app = app_with("aaa");
    press(&mut app, "2rz");
    assert_eq!(content(&app), "zza\n");

    let mut app = app_with("aBc");
    press(&mut app, "3~");
    assert_eq!(content(&app), "AbC\n");
}

#[test]
fn open_line_carries_the_current_indentation() {
    let mut app = app_with("    first");
    press(&mut app, " osecond<Esc>");
    assert_eq!(content(&app), "    first\n    second\n");
    press(&mut app, "Ozeroth<Esc>");
    assert_eq!(content(&app), "    first\n    zeroth\n    second\n");
}

#[test]
fn insert_mode_editing_keys() {
    let mut app = app_with("");
    press(&mut app, "iabc<BS>d<Esc>");
    assert_eq!(content(&app), "abd\n");

    let mut app = app_with("");
    press(&mut app, "ione two<C-w><Esc>");
    assert_eq!(content(&app), "one \n");

    let mut app = app_with("");
    press(&mut app, "iabc<C-u>xyz<Esc>");
    assert_eq!(content(&app), "xyz\n");
}

#[test]
fn append_at_line_end_and_insert_at_first_non_blank() {
    let mut app = app_with("  text");
    press(&mut app, "A!<Esc>");
    assert_eq!(content(&app), "  text!\n");
    press(&mut app, "I>><Esc>");
    assert_eq!(content(&app), "  >>text!\n");
}

#[test]
fn replace_mode_overwrites() {
    let mut app = app_with("abcdef");
    press(&mut app, "RXY<Esc>");
    assert_eq!(content(&app), "XYcdef\n");
}

// -- registers, put, macros, dot -------------------------------------------

#[test]
fn named_registers_are_independent_of_the_unnamed_one() {
    let mut app = app_with("alpha\nbeta\n");
    press(&mut app, "\"ayy");
    press(&mut app, "jdd");
    assert_eq!(content(&app), "alpha\n");
    press(&mut app, "\"ap");
    assert_eq!(content(&app), "alpha\nalpha\n");
}

#[test]
fn deletes_shift_the_numbered_registers() {
    let mut app = app_with("one\ntwo\nthree\n");
    press(&mut app, "dd");
    press(&mut app, "dd");
    press(&mut app, "\"1p");
    assert_eq!(content(&app), "three\ntwo\n");
    press(&mut app, "\"2p");
    assert_eq!(content(&app), "three\ntwo\none\n");
}

#[test]
fn charwise_put_lands_after_the_cursor() {
    let mut app = app_with("ab");
    press(&mut app, "ylp");
    assert_eq!(content(&app), "aab\n");
    let mut app = app_with("ab");
    press(&mut app, "ylP");
    assert_eq!(content(&app), "aab\n");
}

#[test]
fn dot_repeats_the_last_change_with_a_new_count() {
    let mut app = app_with("aaaaaaaa");
    press(&mut app, "2x");
    assert_eq!(content(&app), "aaaaaa\n");
    press(&mut app, ".");
    assert_eq!(content(&app), "aaaa\n");
    press(&mut app, "3.");
    assert_eq!(content(&app), "a\n");
}

#[test]
fn dot_repeats_an_insertion() {
    let mut app = app_with("one\ntwo\nthree");
    press(&mut app, "I- <Esc>");
    press(&mut app, "j.");
    press(&mut app, "j.");
    assert_eq!(content(&app), "- one\n- two\n- three\n");
}

#[test]
fn macros_record_and_replay() {
    let mut app = app_with("1\n2\n3\n4");
    press(&mut app, "qaI#<Esc>jq");
    assert_eq!(content(&app), "#1\n2\n3\n4\n");
    press(&mut app, "3@a");
    assert_eq!(content(&app), "#1\n#2\n#3\n#4\n");
}

#[test]
fn a_macro_that_calls_itself_is_stopped() {
    let mut app = app_with("x");
    // Put a self-referential macro in register a by hand.
    app.registers.yank(
        Some('a'),
        crate::edit::register::RegisterContent::charwise("@a"),
    );
    press(&mut app, "@a");
    let mut drained = 0;
    while app.queue.pop_front().is_some() && drained < 200_000 {
        drained += 1;
    }
    assert!(app.message.is_some());
}

// -- visual mode ------------------------------------------------------------

#[test]
fn visual_character_selection_deletes_inclusively() {
    let mut app = app_with("abcdef");
    press(&mut app, "vlld");
    assert_eq!(content(&app), "def\n");
}

#[test]
fn visual_line_selection_is_linewise() {
    let mut app = app_with("one\ntwo\nthree\n");
    press(&mut app, "Vjd");
    assert_eq!(content(&app), "three\n");
}

#[test]
fn visual_mode_accepts_text_objects_and_case_operators() {
    let mut app = app_with("alpha beta");
    press(&mut app, "viwU");
    assert_eq!(content(&app), "ALPHA beta\n");
}

#[test]
fn visual_swap_ends_moves_the_cursor_to_the_anchor() {
    let mut app = app_with("abcdef");
    press(&mut app, "lllvhh");
    assert_eq!(cursor(&app), (0, 1));
    press(&mut app, "o");
    assert_eq!(cursor(&app), (0, 3));
    assert_eq!(app.mode, Mode::Visual(crate::mode::VisualKind::Char));
}

// -- search -----------------------------------------------------------------

#[test]
fn backward_search_without_a_match_does_not_panic() {
    // This was a real crash: the scan decremented an unsigned line index
    // past zero.
    let rope = Rope::from_str("alpha\nbeta\ngamma\n");
    let regex = regex::Regex::new("zzzz").unwrap();
    let found = search::find_with(
        &regex,
        &rope,
        Position::new(2, 2),
        Direction::Backward,
        true,
    );
    assert!(found.is_none());
}

#[test]
fn search_moves_between_matches_and_wraps() {
    let mut app = app_with("foo\nbar\nfoo\nbaz");
    press(&mut app, "/foo<CR>");
    assert_eq!(cursor(&app), (2, 0));
    press(&mut app, "n");
    assert_eq!(cursor(&app), (0, 0));
    press(&mut app, "N");
    assert_eq!(cursor(&app), (2, 0));
}

#[test]
fn search_supports_regular_expressions_and_smart_case() {
    let mut app = app_with("alpha\nBeta\nbeta");
    press(&mut app, "/^b.ta$<CR>");
    // Lowercase pattern is case-insensitive under smartcase.
    assert_eq!(cursor(&app), (1, 0));

    let mut app = app_with("alpha\nBeta\nbeta");
    press(&mut app, "/Beta<CR>");
    assert_eq!(cursor(&app), (1, 0));
    press(&mut app, "n");
    assert_eq!(cursor(&app), (1, 0));
}

#[test]
fn cancelling_a_search_restores_the_cursor() {
    let mut app = app_with("foo\nbar\nfoo");
    press(&mut app, "jj");
    let before = cursor(&app);
    press(&mut app, "/bar<Esc>");
    assert_eq!(cursor(&app), before);
}

#[test]
fn star_searches_for_the_word_under_the_cursor() {
    let mut app = app_with("total\nsubtotal\ntotal");
    press(&mut app, "*");
    // `\btotal\b` must not match inside "subtotal".
    assert_eq!(cursor(&app), (2, 0));
}

// -- ex commands ------------------------------------------------------------

#[test]
fn substitute_over_a_range_with_capture_groups() {
    let mut app = app_with("a1\na2\na3");
    press(&mut app, ":%s/a(\\d)/b\\1/g<CR>");
    assert_eq!(content(&app), "b1\nb2\nb3\n");
}

#[test]
fn substitute_defaults_to_the_current_line_and_first_match() {
    let mut app = app_with("x x x\nx x x");
    press(&mut app, ":s/x/y/<CR>");
    assert_eq!(content(&app), "y x x\nx x x\n");
    press(&mut app, ":s/x/y/g<CR>");
    assert_eq!(content(&app), "y y y\nx x x\n");
}

#[test]
fn substitute_accepts_a_relative_range() {
    let mut app = app_with("a\na\na\na");
    press(&mut app, ":.,+2s/a/b/<CR>");
    assert_eq!(content(&app), "b\nb\nb\na\n");
}

#[test]
fn substitute_over_a_visual_selection() {
    let mut app = app_with("a\na\na");
    press(&mut app, "Vj:s/a/b/<CR>");
    assert_eq!(content(&app), "b\nb\na\n");
}

#[test]
fn a_bare_range_jumps_to_that_line() {
    let mut app = app_with("1\n2\n3\n4\n5");
    press(&mut app, ":4<CR>");
    assert_eq!(cursor(&app), (3, 0));
    press(&mut app, ":$<CR>");
    assert_eq!(cursor(&app), (4, 0));
}

#[test]
fn set_changes_options_at_runtime() {
    let mut app = app_with("a");
    press(&mut app, ":set ts=2 noet sw=8<CR>");
    assert_eq!(app.config.editor.tab_width, 2);
    assert!(!app.config.editor.expand_tab);
    assert_eq!(app.config.editor.shift_width, 8);

    press(&mut app, ":set nonumber<CR>");
    assert_eq!(
        app.config.editor.line_numbers,
        crate::config::LineNumbers::None
    );

    press(&mut app, ":set bogusoption<CR>");
    assert!(app.message.as_ref().unwrap().text.contains("E518"));
}

#[test]
fn quitting_a_modified_buffer_requires_a_bang() {
    let mut app = app_with("text");
    press(&mut app, "x");
    press(&mut app, ":q<CR>");
    assert!(!app.should_quit);
    assert!(app.message.as_ref().unwrap().text.contains("E37"));
    press(&mut app, ":q!<CR>");
    assert!(app.should_quit);
}

#[test]
fn quitting_is_blocked_by_any_unsaved_buffer_not_just_the_current_one() {
    let directory = std::env::temp_dir().join(format!("miv-quit-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("other.txt");
    std::fs::write(&path, b"content\n").unwrap();

    let mut app = app_with("here");
    app.open_file(&path).unwrap();
    press(&mut app, "x"); // modify the newly opened buffer
    press(&mut app, ":bp<CR>"); // ...and move away from it
    press(&mut app, ":q<CR>");
    assert!(!app.should_quit);
    assert!(app.message.as_ref().unwrap().text.contains("other.txt"));
    press(&mut app, ":q!<CR>");
    assert!(app.should_quit);

    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn unknown_commands_report_an_error() {
    let mut app = app_with("a");
    press(&mut app, ":frobnicate<CR>");
    assert!(app.message.as_ref().unwrap().text.contains("E492"));
}

// -- marks, jumps and key notation -----------------------------------------

#[test]
fn marks_can_be_set_and_used_as_motions() {
    let mut app = app_with("one\ntwo\nthree\nfour");
    press(&mut app, "jma");
    press(&mut app, "G");
    press(&mut app, "'a");
    assert_eq!(cursor(&app), (1, 0));
    press(&mut app, "Gd'a");
    assert_eq!(content(&app), "one\n");
}

#[test]
fn the_jump_list_returns_to_the_previous_position() {
    let mut app = app_with("1\n2\n3\n4\n5\n6");
    press(&mut app, "jj");
    press(&mut app, "G");
    assert_eq!(cursor(&app), (5, 0));
    press(&mut app, "<C-o>");
    assert_eq!(cursor(&app), (2, 0));
    press(&mut app, "<C-i>");
    assert_eq!(cursor(&app), (5, 0));
}

#[test]
fn an_alt_key_is_treated_as_escape_followed_by_that_key() {
    // Terminals encode Alt+j as Esc then j and can deliver both in one read,
    // which used to leave insert mode running and insert the character.
    let mut app = app_with("one\ntwo");
    press(&mut app, "iX");
    assert_eq!(app.mode, Mode::Insert);
    keymap::handle(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('j'),
            crossterm::event::KeyModifiers::ALT,
        ),
    );
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(content(&app), "Xone\ntwo\n");
    assert_eq!(cursor(&app), (1, 0));
}

#[test]
fn unbound_control_keys_do_not_insert_their_letter() {
    // Ctrl-K used to type a literal `k`, because the insert-mode arm matched
    // the character without looking at the modifiers.
    let mut app = app_with("");
    press(&mut app, "iab<C-k>c<Esc>");
    assert_eq!(content(&app), "abc\n");

    // Ctrl-J and Ctrl-M really are LF and CR, so they break the line.
    let mut app = app_with("");
    press(&mut app, "iab<C-j>c<Esc>");
    assert_eq!(content(&app), "ab\nc\n");

    // The same applies on the command line, where Ctrl-J submits.
    let mut app = app_with("one\ntwo\nthree");
    press(&mut app, ":2<C-j>");
    assert_eq!(cursor(&app), (1, 0));

    let mut app = app_with("a");
    press(&mut app, ":se<C-k>t nonumber<CR>");
    assert_eq!(
        app.config.editor.line_numbers,
        crate::config::LineNumbers::None
    );
}

#[test]
fn key_notation_round_trips() {
    let input = "iabc<Esc>3dd<C-r><CR>";
    let parsed = keys::parse(input);
    assert_eq!(keys::encode_all(&parsed), input);
}

// -- windows ----------------------------------------------------------------

#[test]
fn splitting_gives_each_window_its_own_cursor() {
    let mut app = app_with("1\n2\n3\n4\n5\n6");
    press(&mut app, ":vsplit<CR>");
    assert_eq!(app.workspace.count(), 2);

    // Move in the new window; the other one must stay where it was.
    press(&mut app, "G");
    assert_eq!(cursor(&app), (5, 0));
    press(&mut app, "<C-w>w");
    assert_eq!(cursor(&app), (0, 0), "the other window kept its cursor");
    press(&mut app, "<C-w>w");
    assert_eq!(cursor(&app), (5, 0), "and so did this one");
}

#[test]
fn two_windows_can_show_different_buffers() {
    let directory = scratch_dir("windows");
    let other = directory.join("other.txt");
    std::fs::write(&other, "other file\n").unwrap();

    let mut app = app_with("first\n");
    press(&mut app, ":vsplit<CR>");
    app.open_file(&other).unwrap();
    app.sync_window();
    assert_eq!(app.buffer().short_name(), "other.txt");

    press(&mut app, "<C-w>w");
    assert_eq!(
        app.buffer().short_name(),
        "[No Name]",
        "the first window still shows its own buffer"
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn windows_can_be_navigated_by_direction() {
    let mut app = app_with("a\nb\nc");
    // Lay out two columns, then stack the right one.
    press(&mut app, ":vsplit<CR>");
    press(&mut app, ":split<CR>");
    assert_eq!(app.workspace.count(), 3);

    // Arranging assigns the rectangles that directional focus needs.
    app.workspace.arrange(ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    });
    let before = app.workspace.focused_id();
    press(&mut app, "<C-w>h");
    assert_ne!(app.workspace.focused_id(), before, "should move left");
    press(&mut app, "<C-w>l");
    assert_ne!(
        app.workspace.focused_id(),
        0,
        "and back into the right-hand column"
    );
}

#[test]
fn closing_windows_and_refusing_the_last_one() {
    let mut app = app_with("text\n");
    press(&mut app, ":vsplit<CR>");
    press(&mut app, ":split<CR>");
    assert_eq!(app.workspace.count(), 3);

    press(&mut app, ":only<CR>");
    assert_eq!(app.workspace.count(), 1);

    press(&mut app, ":close<CR>");
    assert_eq!(app.workspace.count(), 1);
    assert!(app.message.as_ref().unwrap().text.contains("last window"));
    assert!(!app.should_quit, "closing a window is not quitting");
}

#[test]
fn quit_closes_a_window_before_it_leaves_the_editor() {
    let mut app = app_with("text\n");
    press(&mut app, ":vsplit<CR>");
    press(&mut app, ":q<CR>");
    assert_eq!(app.workspace.count(), 1);
    assert!(!app.should_quit);
    press(&mut app, ":q<CR>");
    assert!(app.should_quit);
}

#[test]
fn a_window_showing_a_closed_buffer_falls_back() {
    let directory = scratch_dir("closed");
    let other = directory.join("other.txt");
    std::fs::write(&other, "other\n").unwrap();

    let mut app = app_with("first\n");
    app.open_file(&other).unwrap();
    app.sync_window();
    press(&mut app, ":vsplit<CR>");
    press(&mut app, ":bd<CR>");

    // Neither window may be left pointing at a buffer that has gone.
    for window in app.workspace.windows() {
        assert!(window.buffer_index < app.buffers.len(), "{window:?}");
    }
    std::fs::remove_dir_all(&directory).ok();
}

// -- pickers ----------------------------------------------------------------

#[test]
fn the_buffer_picker_filters_and_switches() {
    let directory = scratch_dir("bufpick");
    let alpha = directory.join("alpha.txt");
    let beta = directory.join("beta.txt");
    std::fs::write(&alpha, "a\n").unwrap();
    std::fs::write(&beta, "b\n").unwrap();

    let mut app = app_with("scratch\n");
    app.open_file(&alpha).unwrap();
    app.open_file(&beta).unwrap();
    app.sync_window();

    press(&mut app, ":buffers<CR>");
    let picker = app.picker.as_ref().expect("a picker");
    assert_eq!(picker.total(), 3);

    // Typing narrows it, and the closest match sorts first. Matching is
    // subsequence-based, so other paths can still match — what matters is the
    // ranking, not that everything else is excluded.
    press(&mut app, "alpha");
    let picker = app.picker.as_ref().unwrap();
    assert!(picker.matches().len() < 3);
    assert!(
        picker.selection().unwrap().label.contains("alpha"),
        "{:?}",
        picker.selection().map(|item| item.label.clone())
    );

    press(&mut app, "<CR>");
    assert!(app.picker.is_none());
    assert_eq!(app.buffer().short_name(), "alpha.txt");
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn the_command_palette_runs_what_you_pick() {
    let mut app = app_with("text\n");
    press(&mut app, "<C-k>");
    assert!(app.picker.is_some());

    press(&mut app, "side by side");
    let picker = app.picker.as_ref().unwrap();
    assert!(
        picker.selection().unwrap().label.contains("Split side"),
        "{:?}",
        picker.selection().map(|item| item.label.clone())
    );
    press(&mut app, "<CR>");
    assert!(app.picker.is_none());
    assert_eq!(app.workspace.count(), 2, "the command should have run");
}

#[test]
fn a_palette_entry_that_needs_an_argument_opens_the_command_line() {
    let mut app = app_with("text\n");
    press(&mut app, "<C-k>");
    press(&mut app, "Search project");
    press(&mut app, "<CR>");
    assert!(app.picker.is_none());
    // Prefilled rather than run, because it needs a pattern.
    assert_eq!(app.mode, Mode::Command);
    assert_eq!(app.prompt.as_ref().unwrap().input, "grep ");
}

#[test]
fn picker_navigation_wraps_and_escape_closes() {
    let mut app = app_with("text\n");
    press(&mut app, "<C-k>");
    let total = app.picker.as_ref().unwrap().matches().len();
    assert!(total > 2);

    press(&mut app, "<C-n>");
    assert_eq!(app.picker.as_ref().unwrap().selected, 1);
    press(&mut app, "<C-p><C-p>");
    assert_eq!(
        app.picker.as_ref().unwrap().selected,
        total - 1,
        "should wrap to the end"
    );

    press(&mut app, "<Esc>");
    assert!(app.picker.is_none());
}

#[test]
fn backspacing_out_of_an_empty_picker_closes_it() {
    let mut app = app_with("text\n");
    press(&mut app, "<C-k>");
    press(&mut app, "ab");
    assert_eq!(app.picker.as_ref().unwrap().input, "ab");
    press(&mut app, "<BS><BS>");
    assert!(app.picker.is_some(), "still open with an empty query");
    press(&mut app, "<BS>");
    assert!(app.picker.is_none());
}

#[test]
fn a_picker_keeps_the_keyboard_away_from_the_buffer() {
    let mut app = app_with("untouched\n");
    press(&mut app, "<C-k>");
    // These would be destructive in normal mode.
    press(&mut app, "dd");
    press(&mut app, "x");
    assert_eq!(content(&app), "untouched\n");
    assert!(app.picker.is_some());
}

// -- auto-pairs and completion ----------------------------------------------

#[test]
fn auto_pairs_close_and_step_over() {
    let mut app = app_with("");
    press(&mut app, "icall(");
    assert_eq!(content(&app), "call()\n");
    assert_eq!(cursor(&app), (0, 5), "the cursor sits between them");

    // Typing the closer steps over rather than doubling it.
    press(&mut app, ")");
    assert_eq!(content(&app), "call()\n");
    assert_eq!(cursor(&app), (0, 6));
    press(&mut app, "<Esc>");
}

#[test]
fn auto_pairs_can_be_turned_off() {
    let mut app = app_with("");
    app.config.editor.auto_pairs = false;
    press(&mut app, "icall(<Esc>");
    assert_eq!(content(&app), "call(\n");
}

#[test]
fn backspace_removes_an_empty_pair_as_a_unit() {
    let mut app = app_with("");
    press(&mut app, "if(");
    assert_eq!(content(&app), "f()\n");
    press(&mut app, "<BS>");
    assert_eq!(content(&app), "f\n");
    press(&mut app, "<Esc>");
}

#[test]
fn enter_between_a_pair_opens_the_block() {
    let mut app = app_with("");
    press(&mut app, "ifn f() {");
    assert_eq!(content(&app), "fn f() {}\n");
    press(&mut app, "<CR>");
    assert_eq!(content(&app), "fn f() {\n    \n}\n");
    assert_eq!(cursor(&app), (1, 4), "on the indented middle line");
    press(&mut app, "<Esc>");
}

#[test]
fn completion_offers_words_from_the_buffer_and_accepts_with_tab() {
    let mut app = app_with("let calculation = 1;\n");
    press(&mut app, "Go");
    press(&mut app, "calc");
    let completion = app.completion.as_ref().expect("suggestions");
    assert!(
        completion.items.contains(&"calculation".to_string()),
        "{:?}",
        completion.items
    );

    press(&mut app, "<Tab>");
    assert!(content(&app).contains("calculation\n"));
    assert!(app.completion.is_none(), "accepting closes the list");
    press(&mut app, "<Esc>");
}

#[test]
fn ctrl_n_offers_suggestions_even_below_the_automatic_threshold() {
    let mut app = app_with("alphabetical\n");
    app.config.editor.auto_complete = false;
    press(&mut app, "Go");
    press(&mut app, "a");
    assert!(app.completion.is_none(), "automatic completion is off");
    press(&mut app, "<C-n>");
    let completion = app.completion.as_ref().expect("suggestions on request");
    assert!(completion.items.contains(&"alphabetical".to_string()));
    press(&mut app, "<C-y>");
    assert!(content(&app).contains("alphabetical"));
    press(&mut app, "<Esc>");
}

#[test]
fn escape_always_leaves_insert_mode_even_with_the_list_open() {
    // A popup must never take Esc away from the modal contract.
    let mut app = app_with("alphabetical\n");
    press(&mut app, "Go");
    press(&mut app, "alp");
    assert!(app.completion.is_some());
    press(&mut app, "<Esc>");
    assert_eq!(app.mode, Mode::Normal);
    assert!(app.completion.is_none());
}

#[test]
fn ctrl_e_dismisses_the_list_without_leaving_insert_mode() {
    let mut app = app_with("alphabetical\n");
    press(&mut app, "Go");
    press(&mut app, "alp");
    assert!(app.completion.is_some());
    press(&mut app, "<C-e>");
    assert!(app.completion.is_none());
    assert_eq!(app.mode, Mode::Insert);
    press(&mut app, "<Esc>");
}

// -- the file explorer ------------------------------------------------------

#[test]
fn the_explorer_lists_directories_first_and_expands_on_demand() {
    use crate::view::explorer::Explorer;
    let directory = scratch_dir("explorer");
    std::fs::create_dir(directory.join("src")).unwrap();
    std::fs::write(directory.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(directory.join("README.md"), "# hi\n").unwrap();

    let mut explorer = Explorer::new(directory.clone(), false);
    let names: Vec<&str> = explorer
        .entries()
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, vec!["src", "README.md"], "directories first");
    assert!(explorer.entries()[0].is_dir);

    // Expanding reveals the children, indented.
    assert!(explorer.activate().is_none(), "a directory does not open");
    let entries = explorer.entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[1].name, "main.rs");
    assert_eq!(entries[1].depth, 1);

    // Selecting a file hands back its path.
    explorer.selected = 1;
    let chosen = explorer.activate().expect("a file opens");
    assert!(chosen.ends_with("main.rs"));

    // Collapsing hides them again.
    explorer.selected = 0;
    explorer.collapse();
    assert_eq!(explorer.entries().len(), 2);
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn the_explorer_hides_what_git_ignores_unless_asked() {
    use crate::view::explorer::Explorer;
    let directory = scratch_dir("explorer-ignore");
    std::fs::write(directory.join(".gitignore"), "target\n").unwrap();
    std::fs::create_dir(directory.join("target")).unwrap();
    std::fs::write(directory.join("keep.txt"), "\n").unwrap();

    let hidden = Explorer::new(directory.clone(), false);
    let names: Vec<&str> = hidden.entries().iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&"target"), "{names:?}");
    assert!(names.contains(&"keep.txt"));
    // Dotfiles are not hidden: `.github` and `.gitlab-ci.yml` are the point.
    assert!(names.contains(&".gitignore"), "{names:?}");

    let shown = Explorer::new(directory.clone(), true);
    let names: Vec<&str> = shown.entries().iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"target"), "{names:?}");
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn the_sidebar_does_not_swallow_the_global_keys() {
    // Standing in the file tree must not stop you opening the command line or
    // the picker.
    let mut app = app_with("text\n");
    press(&mut app, "<C-w>e");
    assert!(app.sidebar_focused());

    press(&mut app, "<C-p>");
    assert!(app.picker.is_some(), "Ctrl-P should still work");
    assert!(!app.sidebar_focused(), "and focus should leave the sidebar");
    press(&mut app, "<Esc>");

    press(&mut app, "<C-w>E");
    assert!(app.sidebar_focused());
    press(&mut app, ":");
    assert_eq!(app.mode, Mode::Command, "`:` should open the command line");
    press(&mut app, "<Esc>");

    press(&mut app, "<C-w>E");
    press(&mut app, "<C-k>");
    assert!(app.picker.is_some(), "Ctrl-K should still work");
}

// -- diagnostics, formatting and signs --------------------------------------

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "miv-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::remove_dir_all(&directory).ok();
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

/// An executable stand-in for a real checker, so the pipeline can be tested
/// without depending on what happens to be installed.
fn fake_checker(directory: &std::path::Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = directory.join("fake-checker");
    std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn inject_diagnostics(app: &mut App, lines: &[(usize, crate::tools::diagnostics::Severity)]) {
    let items = lines
        .iter()
        .map(|(line, severity)| crate::tools::diagnostics::Diagnostic {
            line: *line,
            col: None,
            severity: *severity,
            message: format!("problem on line {}", line + 1),
            source: "test".to_string(),
        })
        .collect();
    app.buffer_mut().diagnostics.replace("test", items);
}

/// Wait for background tools to deliver, since they run on their own threads.
fn settle(app: &mut App) {
    settle_until(app, |app| {
        app.poll_background();
        false
    });
}

/// Pump background results until `ready` says so, or time runs out.
///
/// Waiting for "anything finished" is not enough: an unrelated checker
/// finishing would end the wait before the thing under test arrived.
fn settle_until(app: &mut App, mut ready: impl FnMut(&mut App) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        app.poll_background();
        if ready(app) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Wait for the AI provider to answer.
fn settle_ai(app: &mut App) {
    settle_until(app, |app| !app.asking);
}

#[test]
fn formatting_applies_a_minimal_diff_and_keeps_the_cursor() {
    let mut app = app_with(
        "one
two
three
four",
    );
    press(&mut app, "jj$");
    let before = cursor(&app);

    // Only the second line differs.
    crate::tools::format::apply(
        &mut app,
        "one
two
three
four
",
        "one
TWO
three
four
",
    );
    assert_eq!(
        content(&app),
        "one
TWO
three
four
"
    );
    assert_eq!(cursor(&app), before, "the cursor should not move");

    // ...and the whole reformat is a single undo step.
    press(&mut app, "u");
    assert_eq!(
        content(&app),
        "one
two
three
four
"
    );
}

#[test]
fn formatting_moves_the_cursor_along_with_inserted_lines() {
    let mut app = app_with(
        "a
b
c",
    );
    press(&mut app, "G");
    assert_eq!(cursor(&app), (2, 0));
    crate::tools::format::apply(
        &mut app,
        "a
b
c
",
        "a
new
b
c
",
    );
    assert_eq!(
        content(&app),
        "a
new
b
c
"
    );
    assert_eq!(
        cursor(&app),
        (3, 0),
        "the cursor should follow its line down"
    );
}

#[test]
fn formatting_with_a_real_tool() {
    let directory = scratch_dir("fmt");
    let path = directory.join("sample.rs");
    std::fs::write(&path, "fn  main( ){let x=1;println!(\"{}\",x);}\n").unwrap();

    let mut app = App::new(Config::default(), &[path.clone()], None).unwrap();
    app.viewport = Viewport {
        height: 20,
        text_width: 80,
    };
    press(&mut app, ":fmt<CR>");

    let formatted = content(&app);
    assert!(formatted.contains("fn main() {"), "{formatted}");
    assert!(formatted.contains("    let x = 1;"), "{formatted}");

    // Formatting an already-formatted buffer changes nothing and says so.
    press(&mut app, ":fmt<CR>");
    assert_eq!(content(&app), formatted);
    assert!(app.message.as_ref().unwrap().text.contains("already"));

    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn formatting_reports_when_nothing_is_configured() {
    let mut app = app_with("plain text");
    press(&mut app, ":fmt<CR>");
    let message = &app.message.as_ref().unwrap().text;
    assert!(message.contains("no formatter"), "{message}");
}

#[test]
fn format_on_save_runs_before_the_write() {
    let directory = scratch_dir("fos");
    let path = directory.join("sample.rs");
    std::fs::write(&path, "fn  main( ){}\n").unwrap();

    let mut app = App::new(Config::default(), &[path.clone()], None).unwrap();
    app.viewport = Viewport {
        height: 20,
        text_width: 80,
    };
    app.config.format.on_save = true;
    press(&mut app, ":w<CR>");

    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(on_disk, "fn main() {}\n", "the file should be formatted");
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn save_hygiene_is_opt_in() {
    let directory = scratch_dir("hygiene");
    let path = directory.join("messy.txt");
    std::fs::write(&path, "trailing   \nspaces\t\n").unwrap();

    // By default the file round-trips untouched.
    let mut app = App::new(Config::default(), &[path.clone()], None).unwrap();
    app.viewport = Viewport {
        height: 20,
        text_width: 80,
    };
    press(&mut app, ":w<CR>");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "trailing   \nspaces\t\n"
    );

    // Turned on, it cleans up.
    app.config.editor.trim_trailing_whitespace = true;
    press(&mut app, ":w<CR>");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "trailing\nspaces\n"
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn ensure_final_newline_overrides_the_round_trip() {
    let directory = scratch_dir("newline");
    let path = directory.join("bare.txt");
    std::fs::write(&path, "no newline").unwrap();

    let mut app = App::new(Config::default(), &[path.clone()], None).unwrap();
    app.viewport = Viewport {
        height: 20,
        text_width: 80,
    };
    app.config.editor.ensure_final_newline = true;
    press(&mut app, ":w<CR>");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "no newline\n");
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn trimming_whitespace_is_one_undo_step_and_leaves_the_cursor_valid() {
    let mut app = app_with(
        "a   
b		
c",
    );
    press(&mut app, "jj$");
    let trimmed = app.trim_trailing_whitespace();
    assert_eq!(trimmed, 2);
    assert_eq!(
        content(&app),
        "a
b
c
"
    );
    press(&mut app, "u");
    assert_eq!(
        content(&app),
        "a   
b		
c
"
    );
}

#[test]
fn diagnostic_navigation_visits_each_one_and_wraps() {
    use crate::tools::diagnostics::Severity;
    let mut app = app_with(
        "1
2
3
4
5
6
7
8",
    );
    inject_diagnostics(&mut app, &[(2, Severity::Error), (5, Severity::Warning)]);

    press(&mut app, "]d");
    assert_eq!(cursor(&app), (2, 0));
    assert!(app.message.as_ref().unwrap().text.contains("line 3"));
    press(&mut app, "]d");
    assert_eq!(cursor(&app), (5, 0));
    press(&mut app, "]d");
    assert_eq!(cursor(&app), (2, 0), "should wrap");
    press(&mut app, "[d");
    assert_eq!(cursor(&app), (5, 0), "should wrap backwards");
}

#[test]
fn diagnostic_navigation_says_so_when_there_are_none() {
    let mut app = app_with(
        "clean
",
    );
    press(&mut app, "]d");
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .text
        .contains("no diagnostics"));
    assert_eq!(cursor(&app), (0, 0));
}

#[test]
fn hunk_navigation_walks_the_changes_against_head() {
    use crate::tools::vcs::LineStatus;
    let mut app = app_with(
        "1
2
3
4
5
6
7
8",
    );
    {
        let statuses = &mut app.buffer_mut().line_statuses;
        statuses.insert(1, LineStatus::Added);
        statuses.insert(2, LineStatus::Added);
        statuses.insert(6, LineStatus::Modified);
    }
    press(&mut app, "]h");
    assert_eq!(cursor(&app), (1, 0));
    press(&mut app, "]h");
    assert_eq!(cursor(&app), (6, 0));
    press(&mut app, "]c");
    assert_eq!(cursor(&app), (1, 0), "]c is an alias and should wrap");
    press(&mut app, "[h");
    assert_eq!(cursor(&app), (6, 0));
}

#[test]
fn hunk_navigation_says_so_when_the_file_matches_head() {
    let mut app = app_with(
        "unchanged
",
    );
    press(&mut app, "]h");
    assert!(app.message.as_ref().unwrap().text.contains("no changes"));
}

#[test]
fn the_diagnostic_list_opens_a_picker_you_can_jump_from() {
    use crate::tools::diagnostics::Severity;
    let mut app = app_with("1\n2\n3\n4");
    press(&mut app, ":diag<CR>");
    assert!(app.picker.is_none());
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .text
        .contains("no diagnostics"));

    inject_diagnostics(&mut app, &[(0, Severity::Error), (3, Severity::Warning)]);
    press(&mut app, ":diag<CR>");
    let picker = app.picker.as_ref().expect("a picker");
    assert_eq!(picker.total(), 2);
    assert!(picker.item(0).unwrap().detail.contains("error"));
    assert!(picker.item(1).unwrap().detail.contains("warning"));

    // Selecting the second one jumps there.
    press(&mut app, "<Down><CR>");
    assert!(app.picker.is_none());
    assert_eq!(cursor(&app), (3, 0));
}

#[test]
fn the_sign_column_takes_room_only_when_it_is_on() {
    let mut app = app_with(
        "a
",
    );
    let with_signs = app.gutter_width();
    assert_eq!(app.sign_width(), 2);

    app.config.signs.enabled = false;
    assert_eq!(app.sign_width(), 0);
    assert_eq!(app.gutter_width(), with_signs - 2);

    // Line numbers and signs are independent.
    app.config.signs.enabled = true;
    app.config.editor.line_numbers = crate::config::LineNumbers::None;
    assert_eq!(app.number_width(), 0);
    assert_eq!(app.gutter_width(), 2);
}

#[test]
fn set_toggles_the_new_features_at_runtime() {
    use crate::tools::diagnostics::Severity;
    let mut app = app_with(
        "a
",
    );
    inject_diagnostics(&mut app, &[(0, Severity::Error)]);

    press(&mut app, ":set nodiagnostics<CR>");
    assert!(!app.config.diagnostics.enabled);
    assert!(
        app.buffer().diagnostics.is_empty(),
        "turning diagnostics off should drop stale findings"
    );

    press(&mut app, ":set nogitsigns nosigns nvt<CR>");
    assert!(!app.config.signs.git);
    assert!(!app.config.signs.enabled);

    press(&mut app, ":set fos<CR>");
    assert!(app.config.format.on_save);
    press(&mut app, ":set formatonsave?<CR>");
    assert!(app.message.as_ref().unwrap().text.contains("true"));

    press(&mut app, ":set noswatches<CR>");
    assert!(!app.config.appearance.color_swatches);
}

#[test]
fn a_configured_checker_runs_and_its_findings_reach_the_buffer() {
    let directory = scratch_dir("checker");
    let script = fake_checker(
        &directory,
        "echo \"$1:2:5: [error] deliberately broken\"\necho \"$1:4:1: [warning] also this\"",
    );
    let path = directory.join("deploy.yaml");
    std::fs::write(&path, "replicas: 1\ncontainers: 2\nimages: 3\nports: 4\n").unwrap();

    let mut configuration = Config::default();
    configuration.diagnostics.use_builtin = false;
    configuration.diagnostics.checker = vec![crate::config::CheckerConfig {
        name: Some("fake".to_string()),
        command: vec![script.display().to_string(), "$FILE".to_string()],
        pattern: r"^[^:]*:(?<line>\d+):(?<col>\d+):\s*\[(?<severity>\w+)\]\s*(?<message>.*)$"
            .to_string(),
        ..Default::default()
    }];

    let mut app = App::new(configuration, &[path], None).unwrap();
    app.viewport = Viewport {
        height: 20,
        text_width: 80,
    };
    settle(&mut app);

    let found = app.buffer().diagnostics.sorted();
    assert_eq!(found.len(), 2, "got {found:?}");
    assert_eq!(found[0].line, 1);
    assert_eq!(found[0].col, Some(4));
    assert_eq!(
        found[0].severity,
        crate::tools::diagnostics::Severity::Error
    );
    assert_eq!(found[0].message, "deliberately broken");
    assert_eq!(found[0].source, "fake");
    assert_eq!(
        app.buffer().diagnostics.worst_on_line(1),
        Some(crate::tools::diagnostics::Severity::Error)
    );

    // And navigation reaches them, column included.
    press(&mut app, "]d");
    assert_eq!(cursor(&app), (1, 4));
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn results_for_text_that_has_since_changed_are_discarded() {
    let directory = scratch_dir("stale");
    // Slow enough that the buffer can change before it answers.
    let script = fake_checker(
        &directory,
        "sleep 0.4
echo \"$1:1:1: [error] stale\"",
    );
    let path = directory.join("slow.yaml");
    std::fs::write(&path, "a: 1\n").unwrap();

    let mut configuration = Config::default();
    configuration.diagnostics.use_builtin = false;
    configuration.diagnostics.checker = vec![crate::config::CheckerConfig {
        name: Some("slow".to_string()),
        command: vec![script.display().to_string(), "$FILE".to_string()],
        pattern: r"^[^:]*:(?<line>\d+):(?<col>\d+):\s*\[(?<severity>\w+)\]\s*(?<message>.*)$"
            .to_string(),
        ..Default::default()
    }];

    let mut app = App::new(configuration, &[path], None).unwrap();
    app.viewport = Viewport {
        height: 20,
        text_width: 80,
    };

    // Change the text while the checker is still thinking.
    press(&mut app, "ox<Esc>");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        app.poll_background();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        app.buffer().diagnostics.is_empty(),
        "a result from before the edit must not be shown against the new lines"
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_missing_tool_is_mentioned_once_and_then_left_alone() {
    let mut configuration = Config::default();
    configuration.diagnostics.use_builtin = false;
    configuration.diagnostics.checker = vec![crate::config::CheckerConfig {
        name: Some("ghost".to_string()),
        command: vec!["miv-no-such-checker".to_string()],
        pattern: r"(?<line>\d+)".to_string(),
        ..Default::default()
    }];

    let mut app = App::new(configuration, &[], None).unwrap();
    app.viewport = Viewport {
        height: 20,
        text_width: 80,
    };
    app.refresh_buffer(0, crate::app::Trigger::Manual);
    settle(&mut app);

    let message = app
        .message
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(message.contains("not installed"), "{message}");
    assert_eq!(
        app.message.as_ref().unwrap().kind,
        crate::app::MessageKind::Info,
        "a tool you have not installed is not an error"
    );

    // A second run says nothing more about it.
    app.message = None;
    app.refresh_buffer(0, crate::app::Trigger::Manual);
    settle(&mut app);
    assert!(app.message.is_none(), "should not complain twice");
}

// -- plugins ----------------------------------------------------------------

fn plugin_dir(label: &str, name: &str, manifest: &str) -> std::path::PathBuf {
    let root = scratch_dir(label);
    let directory = root.join(name);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("plugin.toml"), manifest).unwrap();
    root
}

#[test]
fn a_manifest_parses_and_contributes_commands() {
    use crate::plugin::{Capability, Kind, Plugins, Target};
    let root = plugin_dir(
        "plug-ok",
        "shout",
        r#"
name = "shout"
version = "0.1.0"
description = "Make it loud"
capabilities = ["read_buffer", "write_buffer"]

[[command]]
name = "shout"
description = "Uppercase the selection"
kind = "filter"
target = "selection"
command = ["tr", "a-z", "A-Z"]
"#,
    );
    let plugins = Plugins::load(&root);
    assert!(plugins.problems.is_empty(), "{:?}", plugins.problems);
    assert_eq!(plugins.loaded.len(), 1);
    let plugin = &plugins.loaded[0];
    assert_eq!(plugin.manifest.name, "shout");
    assert!(plugin.grants(Capability::WriteBuffer));
    assert!(!plugin.grants(Capability::Network));

    let command = plugins.command("shout").expect("a contributed command");
    assert_eq!(command.kind, Kind::Filter);
    assert_eq!(command.target, Target::Selection);
    assert_eq!(command.plugin, "shout");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_command_that_writes_without_saying_so_is_refused() {
    use crate::plugin::Plugins;
    let root = plugin_dir(
        "plug-caps",
        "sneaky",
        r#"
name = "sneaky"
capabilities = ["read_buffer"]

[[command]]
name = "rewrite"
kind = "filter"
command = ["cat"]
"#,
    );
    let plugins = Plugins::load(&root);
    assert!(plugins.command("rewrite").is_none(), "must not be offered");
    assert!(
        plugins.problems.iter().any(|p| p.contains("write_buffer")),
        "{:?}",
        plugins.problems
    );
    // The plugin still loads; only the command it was not allowed is dropped.
    assert_eq!(plugins.loaded.len(), 1);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_broken_manifest_is_reported_and_the_others_still_load() {
    use crate::plugin::Plugins;
    let root = scratch_dir("plug-broken");
    for (name, body) in [
        ("good", "name = \"good\"\n"),
        ("nameless", "version = \"1\"\n"),
        ("garbage", "name = = =\n"),
    ] {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("plugin.toml"), body).unwrap();
    }
    let plugins = Plugins::load(&root);
    assert_eq!(plugins.loaded.len(), 1, "the good one should still load");
    assert_eq!(plugins.loaded[0].manifest.name, "good");
    assert_eq!(plugins.problems.len(), 2, "{:?}", plugins.problems);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn two_plugins_cannot_claim_the_same_command() {
    use crate::plugin::Plugins;
    let root = scratch_dir("plug-clash");
    for name in ["aaa", "bbb"] {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("plugin.toml"),
            format!(
                "name = \"{name}\"\ncapabilities = [\"write_buffer\"]\n\n\
                 [[command]]\nname = \"same\"\nkind = \"filter\"\ncommand = [\"cat\"]\n"
            ),
        )
        .unwrap();
    }
    let plugins = Plugins::load(&root);
    assert!(plugins.command("same").is_some());
    assert!(
        plugins.problems.iter().any(|p| p.contains("already")),
        "{:?}",
        plugins.problems
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_plugin_command_transforms_the_buffer_and_undoes_in_one_step() {
    use crate::plugin::Plugins;
    let root = plugin_dir(
        "plug-filter",
        "shout",
        r#"
name = "shout"
capabilities = ["read_buffer", "write_buffer"]

[[command]]
name = "shout"
kind = "filter"
target = "line"
command = ["tr", "a-z", "A-Z"]
"#,
    );
    let mut app = app_with(
        "quiet line
second line",
    );
    app.plugins = Plugins::load(&root);

    press(&mut app, ":shout<CR>");
    assert_eq!(
        content(&app),
        "QUIET LINE
second line
"
    );
    press(&mut app, "u");
    assert_eq!(
        content(&app),
        "quiet line
second line
"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_plugin_command_can_report_instead_of_editing() {
    use crate::plugin::Plugins;
    let root = plugin_dir(
        "plug-report",
        "counter",
        r#"
name = "counter"
capabilities = ["read_buffer", "show_ui"]

[[command]]
name = "count"
kind = "report"
target = "buffer"
command = ["wc", "-l"]
"#,
    );
    let mut app = app_with(
        "a
b
c",
    );
    app.plugins = Plugins::load(&root);
    press(&mut app, ":count<CR>");
    let message = &app.message.as_ref().unwrap().text;
    assert!(message.contains("count:"), "{message}");
    assert_eq!(
        content(&app),
        "a
b
c
",
        "a report must not edit"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_unknown_command_still_reports_itself_as_unknown() {
    let mut app = app_with(
        "text
",
    );
    press(&mut app, ":nosuchthing<CR>");
    assert!(app.message.as_ref().unwrap().text.contains("E492"));
}

#[test]
fn events_are_dispatched_from_where_they_happen() {
    use crate::plugin::Event;
    let mut app = app_with(
        "text
",
    );

    // A mode change is noticed once, wherever it came from.
    press(&mut app, "i");
    assert!(app
        .plugins
        .recent()
        .iter()
        .any(|event| matches!(event, Event::ModeChanged { mode } if *mode == "INSERT")));

    press(&mut app, "x<Esc>");
    assert!(app
        .plugins
        .recent()
        .iter()
        .any(|event| matches!(event, Event::BufferChanged { .. })));

    // And they show up in the overlay.
    press(&mut app, ":events<CR>");
    let overlay = app.overlay.as_ref().expect("an overlay");
    assert!(overlay.lines.iter().any(|line| line.contains("mode:")));
}

#[test]
fn the_plugin_list_says_what_is_loaded_and_what_it_may_do() {
    use crate::plugin::Plugins;
    let root = plugin_dir(
        "plug-list",
        "risky",
        r#"
name = "risky"
version = "2.0"
description = "Talks to the internet"
capabilities = ["read_buffer", "network"]
"#,
    );
    let mut app = app_with(
        "text
",
    );
    app.plugins = Plugins::load(&root);
    press(&mut app, ":plugins<CR>");
    let overlay = app.overlay.as_ref().expect("an overlay");
    let text = overlay.lines.join(
        "
",
    );
    assert!(text.contains("risky"), "{text}");
    assert!(
        text.contains("network"),
        "capabilities must be visible: {text}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn with_no_plugins_the_list_says_where_to_put_one() {
    let mut app = app_with(
        "text
",
    );
    press(&mut app, ":plugins<CR>");
    let overlay = app.overlay.as_ref().unwrap();
    assert!(overlay
        .lines
        .join(
            "
"
        )
        .contains("plugin.toml"));
}

// -- ai ---------------------------------------------------------------------

/// A provider that answers with `reply`, and proves it saw the prompt.
fn fake_provider(directory: &std::path::Path, reply: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = directory.join("provider");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh
prompt=$(cat)
case \"$prompt\" in
  *INSTRUCTION*) printf '%s' '{reply}' ;;
  *) echo 'NO INSTRUCTION IN PROMPT' ;;
esac
"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn app_with_provider(text: &str, reply: &str) -> (App, std::path::PathBuf) {
    let directory = scratch_dir("ai");
    let script = fake_provider(&directory, reply);
    let mut app = app_with(text);
    app.config.ai.enabled = true;
    app.config.ai.command = vec![script.display().to_string()];
    (app, directory)
}

#[test]
fn asking_without_a_provider_says_so() {
    let mut app = app_with(
        "text
",
    );
    press(&mut app, ":ai make it better<CR>");
    let message = &app.message.as_ref().unwrap().text;
    assert!(message.contains("ai is off"), "{message}");
    assert!(app.proposal.is_none());
}

#[test]
fn an_answer_becomes_a_proposal_rather_than_an_edit() {
    let (mut app, directory) = app_with_provider(
        "quiet line
",
        "LOUD LINE
",
    );
    press(&mut app, ":ai shout it<CR>");
    assert!(app.asking, "the request should be in flight");
    settle_ai(&mut app);

    // Nothing has touched the buffer yet.
    assert_eq!(content(&app), "quiet line\n");
    let proposal = app.proposal.as_ref().expect("a proposal");
    assert_eq!(proposal.instruction, "shout it");
    // Reshaped to the region, which is a line's text without its newline.
    assert_eq!(proposal.replacement, "LOUD LINE");
    assert!(proposal
        .diff()
        .iter()
        .any(|line| line.starts_with("+ LOUD")));

    // The prompt carried the instruction: the fake provider checked.
    assert!(!proposal.replacement.contains("NO INSTRUCTION"));

    // And the diff is shown for review.
    let overlay = app.overlay.as_ref().expect("the diff");
    assert_eq!(overlay.title, "AI proposal");
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn applying_a_proposal_is_attributed_and_undoes_in_one_step() {
    let (mut app, directory) = app_with_provider(
        "quiet line
",
        "LOUD LINE
",
    );
    press(&mut app, ":ai shout it<CR>");
    settle_ai(&mut app);

    press(&mut app, ":apply<CR>");
    assert_eq!(
        content(&app),
        "LOUD LINE
"
    );
    assert!(app.proposal.is_none());
    assert_eq!(app.buffer().history.last_author(), Some("ai"));

    // Undo says whose change it was taking back.
    press(&mut app, "u");
    assert_eq!(content(&app), "quiet line\n");
    let message = &app.message.as_ref().unwrap().text;
    assert!(message.contains("ai's change"), "{message}");
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_proposal_can_be_thrown_away() {
    let (mut app, directory) = app_with_provider(
        "quiet line
",
        "LOUD LINE
",
    );
    press(&mut app, ":ai shout it<CR>");
    settle_ai(&mut app);
    press(&mut app, ":discard<CR>");
    assert!(app.proposal.is_none());
    assert_eq!(content(&app), "quiet line\n");
    press(&mut app, ":apply<CR>");
    assert!(app.message.as_ref().unwrap().text.contains("no proposal"));
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_proposal_is_refused_if_the_text_moved_while_it_was_thinking() {
    let (mut app, directory) = app_with_provider(
        "quiet line
",
        "LOUD LINE
",
    );
    press(&mut app, ":ai shout it<CR>");
    settle_ai(&mut app);

    // Dismiss the diff, then edit the region before accepting.
    press(&mut app, "<Esc>");
    press(&mut app, "x");
    press(&mut app, ":apply<CR>");
    let message = &app.message.as_ref().unwrap().text;
    assert!(message.contains("changed since"), "{message}");
    assert!(!content(&app).contains("LOUD"), "it must not be applied");
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_provider_that_fails_is_reported_and_leaves_the_buffer_alone() {
    let directory = scratch_dir("ai-fail");
    let mut app = app_with(
        "untouched
",
    );
    app.config.ai.enabled = true;
    app.config.ai.command = vec!["miv-no-such-provider".to_string()];
    press(&mut app, ":ai anything<CR>");
    settle_ai(&mut app);
    let message = &app.message.as_ref().unwrap().text;
    assert!(message.contains("not installed"), "{message}");
    assert_eq!(
        content(&app),
        "untouched
"
    );
    assert!(app.proposal.is_none());
    assert!(!app.asking, "the request should not be left hanging");
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn the_transcript_records_both_sides() {
    let (mut app, directory) = app_with_provider(
        "quiet
", "LOUD
",
    );
    press(&mut app, ":ai shout<CR>");
    settle_ai(&mut app);
    press(&mut app, ":apply<CR>");

    let turns: Vec<String> = app
        .conversation
        .turns()
        .iter()
        .map(|turn| format!("{}: {}", turn.speaker(), turn.text()))
        .collect();
    assert!(
        turns.iter().any(|turn| turn.contains("you: shout")),
        "{turns:?}"
    );
    assert!(
        turns.iter().any(|turn| turn.starts_with("ai:")),
        "{turns:?}"
    );
    assert!(
        turns.iter().any(|turn| turn.contains("applied")),
        "{turns:?}"
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn the_chat_panel_toggles_without_taking_the_keyboard() {
    use crate::view::sidebar::View;
    let mut app = app_with(
        "text
",
    );
    press(&mut app, ":chat<CR>");
    assert!(app.workspace.sidebar.visible);
    assert_eq!(app.workspace.sidebar.view, View::Chat);
    // The transcript is read-only, so typing still goes to the buffer.
    assert!(!app.sidebar_focused());
    press(&mut app, "x");
    assert_eq!(
        content(&app),
        "ext
"
    );

    press(&mut app, ":chat<CR>");
    assert!(!app.workspace.sidebar.visible, "toggles off");
}

#[test]
fn asking_on_a_visual_selection_uses_the_selection() {
    let (mut app, directory) = app_with_provider(
        "one
two
three
",
        "REPLACED
",
    );
    press(&mut app, "Vj");
    press(&mut app, ":ai shout<CR>");
    settle_ai(&mut app);
    let proposal = app.proposal.as_ref().expect("a proposal");
    assert_eq!(
        proposal.original,
        "one
two
",
        "the selected lines"
    );
    press(&mut app, ":apply<CR>");
    assert_eq!(
        content(&app),
        "REPLACED
three
"
    );
    std::fs::remove_dir_all(&directory).ok();
}

// -- configuration ----------------------------------------------------------

fn write_config(body: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "miv-config-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("config.toml");
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn a_full_configuration_file_parses() {
    let path = write_config(
        r#"
[editor]
tab_width = 2
expand_tab = false
shift_width = 8
scrolloff = 5
line_numbers = "hybrid"
cursorline = false
ignore_case = false
smart_case = false
wrap_search = false

[appearance]
theme = "InspiredGitHub"
syntax_highlighting = false
max_highlight_lines = 1000
theme_background = true

[session]
bind = "127.0.0.1"
port = 7420
name = "arhun"
default_access = "write"
announce = false
max_participants = 3
show_remote_cursors = false
guest_commands = true
"#,
    );
    let config = crate::config::Config::load(&path).expect("should parse");
    assert_eq!(config.editor.tab_width, 2);
    assert!(!config.editor.expand_tab);
    assert_eq!(config.editor.shift_width, 8);
    assert_eq!(
        config.editor.line_numbers,
        crate::config::LineNumbers::Hybrid
    );
    assert_eq!(config.appearance.theme, "InspiredGitHub");
    assert!(config.appearance.theme_background);
    assert_eq!(config.session.port, 7420);
    assert_eq!(config.session.name, "arhun");
    assert_eq!(
        config.session.default_access,
        crate::session::protocol::Access::Write
    );
    assert!(config.session.guest_commands);
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_partial_configuration_keeps_the_defaults() {
    let path = write_config(
        "[editor]
tab_width = 3
",
    );
    let config = crate::config::Config::load(&path).unwrap();
    let defaults = crate::config::Config::default();
    assert_eq!(config.editor.tab_width, 3);
    assert_eq!(config.editor.shift_width, defaults.editor.shift_width);
    assert_eq!(config.appearance.theme, defaults.appearance.theme);
    assert_eq!(config.session.bind, defaults.session.bind);
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_mistyped_option_is_an_error_rather_than_a_silent_default() {
    let path = write_config(
        "[editor]
tab_widht = 2
",
    );
    let error = crate::config::Config::load(&path).unwrap_err();
    let text = format!("{error:#}");
    assert!(text.contains("tab_widht"), "{text}");

    let path = write_config("[editor]\ntab_width = \"four\"\n");
    assert!(crate::config::Config::load(&path).is_err());

    let path = write_config("[editor]\nline_numbers = \"sometimes\"\n");
    assert!(crate::config::Config::load(&path).is_err());

    let path = write_config(
        "[nonsense]
x = 1
",
    );
    assert!(crate::config::Config::load(&path).is_err());
    std::fs::remove_file(&path).ok();
}

#[test]
fn impossible_values_are_rejected() {
    let path = write_config(
        "[editor]
tab_width = 0
",
    );
    let error = format!("{:#}", crate::config::Config::load(&path).unwrap_err());
    assert!(error.contains("tab_width"), "{error}");

    let path = write_config(
        "[session]
max_participants = 0
",
    );
    let error = format!("{:#}", crate::config::Config::load(&path).unwrap_err());
    assert!(error.contains("max_participants"), "{error}");
    std::fs::remove_file(&path).ok();
}

#[test]
fn checkers_and_formatters_parse_from_configuration() {
    let path = write_config(
        r#"
[diagnostics]
enabled = true
on_save = true
on_change = true
debounce_ms = 250
timeout_ms = 1500
virtual_text = false
use_builtin = false

[[diagnostics.checker]]
name = "yamllint"
command = ["yamllint", "-f", "parsable", "$FILE"]
pattern = "^[^:]*:(?<line>\\d+):(?<col>\\d+): \\[(?<severity>\\w+)\\] (?<message>.*)$"
severity = "warning"
extensions = ["yml", "yaml"]

[[diagnostics.checker]]
command = ["mycheck"]
pattern = "(?<line>\\d+)"
filetypes = ["Rust"]

[format]
on_save = true
timeout_ms = 900

[[format.formatter]]
command = ["rustfmt", "--emit", "stdout"]
extensions = ["rs"]

[signs]
enabled = true
diagnostics = false
git = true
"#,
    );
    let config = crate::config::Config::load(&path).expect("should parse");
    assert_eq!(config.diagnostics.checker.len(), 2);
    assert_eq!(config.diagnostics.checker[0].display_name(), "yamllint");
    // A checker with no explicit name is known by its command.
    assert_eq!(config.diagnostics.checker[1].display_name(), "mycheck");
    assert!(config.diagnostics.on_change);
    assert_eq!(config.diagnostics.debounce_ms, 250);
    assert!(!config.diagnostics.virtual_text);
    assert!(!config.diagnostics.use_builtin);
    assert!(config.format.on_save);
    assert_eq!(config.format.formatter.len(), 1);
    assert!(!config.signs.diagnostics);
    assert!(config.signs.git);
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_broken_checker_is_rejected_when_the_file_loads() {
    // Better to be told at startup than to wonder why nothing happens.
    let path = write_config(
        r#"
[[diagnostics.checker]]
command = ["x"]
pattern = "no line group here"
"#,
    );
    let error = format!("{:#}", crate::config::Config::load(&path).unwrap_err());
    assert!(error.contains("line"), "{error}");

    let path = write_config(
        r#"
[[diagnostics.checker]]
pattern = "(?<line>[0-9]+)"
"#,
    );
    let error = format!("{:#}", crate::config::Config::load(&path).unwrap_err());
    assert!(error.contains("command"), "{error}");

    let path = write_config(
        r#"
[[diagnostics.checker]]
command = ["x"]
pattern = "(?<line>[0-9]+"
"#,
    );
    assert!(
        crate::config::Config::load(&path).is_err(),
        "an unparsable regex must be caught at load"
    );

    let path = write_config(
        r#"
[[format.formatter]]
name = "empty"
"#,
    );
    let error = format!("{:#}", crate::config::Config::load(&path).unwrap_err());
    assert!(error.contains("command"), "{error}");

    let path = write_config("[diagnostics]\nenabledd = true\n");
    assert!(crate::config::Config::load(&path).is_err());

    let path = write_config("[signs]\ngti = true\n");
    assert!(crate::config::Config::load(&path).is_err());
    std::fs::remove_file(&path).ok();
}

#[test]
fn the_example_plugins_load() {
    // They are the documentation for the manifest format, so they must keep
    // parsing — including the capability checks, which run at load.
    use crate::plugin::Plugins;
    let plugins = Plugins::load(std::path::Path::new("examples/plugins"));
    assert!(
        plugins.problems.is_empty(),
        "the shipped examples should load cleanly: {:?}",
        plugins.problems
    );
    assert_eq!(plugins.loaded.len(), 2, "secrets and json");
    assert!(plugins.command("b64decode").is_some());
    assert!(plugins.command("jsonfmt").is_some());
    // And the reporting command is there without needing write access.
    let keys = plugins.command("jsonkeys").expect("jsonkeys");
    assert_eq!(keys.kind, crate::plugin::Kind::Report);
}

#[test]
fn the_documented_example_configuration_is_valid() {
    // The example file is the documentation for every option, so it must keep
    // parsing — including the checker patterns, which are validated on load.
    let path = std::path::Path::new("example-config.toml");
    let config = crate::config::Config::load(path)
        .unwrap_or_else(|e| panic!("example-config.toml does not parse: {e:#}"));

    // It documents the defaults, so it should agree with them.
    let defaults = crate::config::Config::default();
    assert_eq!(config.editor.line_numbers, defaults.editor.line_numbers);
    assert_eq!(config.session.bind, defaults.session.bind);
    assert_eq!(config.diagnostics.on_save, defaults.diagnostics.on_save);
    assert_eq!(config.format.on_save, defaults.format.on_save);
    assert_eq!(config.signs.git, defaults.signs.git);

    // The sample checker and formatter blocks are commented out, so the file
    // does not quietly configure tools nobody asked for.
    assert!(config.diagnostics.checker.is_empty());
    assert!(config.format.formatter.is_empty());
}

#[test]
fn the_default_tooling_settings_are_unsurprising() {
    let config = crate::config::Config::default();
    // Formatting on save is on by default, so a save rewrites the file
    // whenever a formatter matches its type.
    assert!(config.format.on_save);
    // Whitespace trimming stays opt-in: unlike a formatter, it is not tied to
    // a filetype, so it would touch every file you save.
    assert!(!config.editor.trim_trailing_whitespace);
    assert!(!config.editor.ensure_final_newline);
    // Checking on save is useful and cheap; checking on every keystroke is not.
    assert!(config.diagnostics.on_save);
    assert!(!config.diagnostics.on_change);
    assert!(config.signs.enabled);
}

#[test]
fn the_default_session_settings_are_the_safe_ones() {
    // These defaults are a security posture, not a preference: a listener that
    // reached the network, or guests who could edit or run commands the moment
    // they connected, would all be surprising.
    let session = crate::config::Config::default().session;
    assert_eq!(session.bind, "127.0.0.1");
    assert_eq!(
        session.default_access,
        crate::session::protocol::Access::Read
    );
    assert!(!session.guest_commands);
    assert!(session.max_participants <= 16);
}

// -- unicode ----------------------------------------------------------------

#[test]
fn editing_multi_byte_text_stays_on_character_boundaries() {
    let mut app = app_with("日本語テキスト");
    press(&mut app, "x");
    assert_eq!(content(&app), "本語テキスト\n");

    let mut app = app_with("日本語 テキスト");
    press(&mut app, "dw");
    assert_eq!(content(&app), "テキスト\n");

    let mut app = app_with("héllo wörld");
    press(&mut app, "dw");
    assert_eq!(content(&app), "wörld\n");
}

#[test]
fn combining_characters_are_not_split_by_word_motions() {
    let mut app = app_with("café bar");
    press(&mut app, "dw");
    assert_eq!(content(&app), "bar\n");
}
#[test]
fn undoing_a_delete_of_the_last_line_leaves_no_blank_line() {
    // The buffer guarantees a trailing newline. Re-adding it after a deletion
    // used to bypass the transaction, so undo could not take it back.
    let mut app = app_with("untouched");
    press(&mut app, "dd");
    assert_eq!(content(&app), "\n");
    press(&mut app, "u");
    assert_eq!(content(&app), "untouched\n");
    press(&mut app, "<C-r>");
    assert_eq!(content(&app), "\n");

    let mut app = app_with("one\ntwo");
    press(&mut app, "Gdd");
    assert_eq!(content(&app), "one\n");
    press(&mut app, "u");
    assert_eq!(content(&app), "one\ntwo\n");
}
