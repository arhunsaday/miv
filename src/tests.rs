//! Tests.
//!
//! The editing tests drive the real key handler rather than calling internals,
//! so they cover the whole path a keystroke actually takes: pending state,
//! motion resolution, operator application, history and cursor placement.

use crate::app::{App, Viewport};
use crate::config::Config;
use crate::keymap;
use crate::keys;
use crate::mode::Mode;
use crate::search::{self, Direction};
use crate::text::{self, Position};
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
    let mut buffer = crate::buffer::Buffer::open(1, &crlf_path).unwrap();
    buffer.write(&crlf_path).unwrap();
    assert_eq!(std::fs::read(&crlf_path).unwrap(), b"one\r\ntwo\r\n");

    let bare_path = directory.join("bare.txt");
    std::fs::write(&bare_path, b"no newline").unwrap();
    let mut buffer = crate::buffer::Buffer::open(2, &bare_path).unwrap();
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

    let mut buffer = crate::buffer::Buffer::open(1, &path).unwrap();
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
    app.registers
        .yank(Some('a'), crate::register::RegisterContent::charwise("@a"));
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
fn key_notation_round_trips() {
    let input = "iabc<Esc>3dd<C-r><CR>";
    let parsed = keys::parse(input);
    assert_eq!(keys::encode_all(&parsed), input);
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
