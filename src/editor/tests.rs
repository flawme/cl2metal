//! Tests for the editor core: buffers, motions, operators, search and the
//! key-to-command path.

use super::buffer::{Buffer, Eol, MAX_EDIT_BYTES};
use super::command::Command;
use super::keymap::Keymap;
use super::mode::Mode;
use super::motion::{self, Motion};
use super::operator::{self, Operator, TextObject};
use super::pos::{Cursor, Pos, Span, VisualKind};
use super::search::{self, Query};
use super::{Editor, Settings};

/// An editor holding `text` in a scratch buffer.
fn editor(text: &str) -> Editor {
    let mut ed = Editor::new(
        Settings::default(),
        Keymap::from_config(&Default::default()),
        std::env::temp_dir(),
    );
    ed.buffers[0] = Buffer::from_text(text, None);
    ed
}

fn text(ed: &Editor) -> String {
    ed.buffer().text()
}

fn cursor(ed: &Editor) -> Pos {
    ed.buffer().cursors[0].pos
}

/// A pane geometry for the scroll tests: `rows` visible rows of 20 px.
fn pane(rows: usize) -> super::view::PaneGeometry {
    let rect =
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, rows as f32 * 20.0));
    super::view::PaneGeometry {
        rect,
        text: rect,
        metrics: super::view::Metrics {
            cell_w: 8.0,
            cell_h: 20.0,
        },
        rows,
        cols: 100,
        gutter: 40.0,
    }
}

fn press(ed: &mut Editor, key: egui::Key) {
    ed.key(key, egui::Modifiers::default(), None);
}

/// The egui key for a character, for tests that type text.
fn key_for(c: char) -> egui::Key {
    use egui::Key as K;
    match c {
        'a' => K::A,
        'b' => K::B,
        'c' => K::C,
        'd' => K::D,
        'e' => K::E,
        'f' => K::F,
        'g' => K::G,
        'h' => K::H,
        'i' => K::I,
        'j' => K::J,
        'k' => K::K,
        'l' => K::L,
        'm' => K::M,
        'n' => K::N,
        'o' => K::O,
        'p' => K::P,
        'q' => K::Q,
        'r' => K::R,
        's' => K::S,
        't' => K::T,
        'u' => K::U,
        'v' => K::V,
        'w' => K::W,
        'x' => K::X,
        'y' => K::Y,
        'z' => K::Z,
        ' ' => K::Space,
        '\n' => K::Enter,
        ':' => K::Colon,
        '/' => K::Slash,
        '0' => K::Num0,
        '1' => K::Num1,
        '2' => K::Num2,
        '3' => K::Num3,
        '4' => K::Num4,
        '5' => K::Num5,
        '6' => K::Num6,
        '7' => K::Num7,
        '8' => K::Num8,
        '9' => K::Num9,
        _ => K::A,
    }
}

fn type_text(ed: &mut Editor, s: &str) {
    for c in s.chars() {
        ed.key(key_for(c), egui::Modifiers::default(), Some(&c.to_string()));
    }
}

// ---------------------------------------------------------------------------
// Buffer
// ---------------------------------------------------------------------------

#[test]
fn text_round_trips_including_the_final_newline() {
    for source in ["", "\n", "a", "a\n", "a\nb", "a\nb\n", "a\n\nb\n"] {
        let buf = Buffer::from_text(source, None);
        assert_eq!(buf.text(), source, "round trip failed for {source:?}");
    }
}

#[test]
fn crlf_files_keep_their_line_endings() {
    let buf = Buffer::from_text("one\r\ntwo\r\n", None);
    assert_eq!(buf.eol, Eol::Crlf);
    assert_eq!(buf.line_count(), 2);
    assert_eq!(buf.line(0), "one", "the carriage return is not content");
    assert_eq!(buf.text(), "one\r\ntwo\r\n");
}

#[test]
fn positions_count_characters_not_bytes() {
    let buf = Buffer::from_text("héllo wörld\n", None);
    assert_eq!(buf.line_len(0), 11);
    // The byte offset of column 2 is past the two-byte é.
    assert_eq!(buf.byte_col(0, 2), 3);
    assert_eq!(buf.col_of_byte(0, 3), 2);
    assert_eq!(buf.char_at(Pos::new(0, 1)), Some('é'));
}

#[test]
fn inserting_multiline_text_splits_the_line() {
    let mut buf = Buffer::from_text("ab\n", None);
    let end = buf.insert(Pos::new(0, 1), "X\nY", false);
    assert_eq!(buf.text(), "aX\nYb\n");
    assert_eq!(end, Pos::new(1, 1));
}

#[test]
fn deleting_across_lines_joins_them() {
    let mut buf = Buffer::from_text("one\ntwo\n", None);
    let removed = buf.delete(Span::new(Pos::new(0, 1), Pos::new(1, 1)), false);
    assert_eq!(removed, "ne\nt");
    assert_eq!(buf.text(), "owo\n");
}

#[test]
fn undo_and_redo_restore_text_and_cursors() {
    let mut buf = Buffer::from_text("hello\n", None);
    buf.insert(Pos::new(0, 5), " world", false);
    assert_eq!(buf.text(), "hello world\n");
    assert!(buf.undo());
    assert_eq!(buf.text(), "hello\n");
    assert!(buf.redo());
    assert_eq!(buf.text(), "hello world\n");
}

#[test]
fn undoing_back_to_the_saved_state_clears_the_modified_marker() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-dirty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dirty.txt");
    std::fs::write(&path, "one\ntwo\n").unwrap();
    let mut ed = editor("one\ntwo\n");
    ed.buffers[0].path = Some(path.clone());
    ed.buffers[0].save().unwrap();
    assert!(!ed.buffers[0].dirty, "a fresh save is clean");

    ed.insert_text("x");
    assert!(ed.buffers[0].dirty, "an edit marks it modified");
    assert!(ed.buffers[0].undo());
    assert!(
        !ed.buffers[0].dirty,
        "undoing back to the saved state clears the marker"
    );
    assert!(ed.buffers[0].redo());
    assert!(ed.buffers[0].dirty, "redoing marks it again");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn typing_after_a_save_marks_the_buffer_modified() {
    let dir =
        std::env::temp_dir().join(format!("cl2metal-editor-dirty-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run.txt");
    std::fs::write(&path, "one\n").unwrap();
    let mut ed = editor("one\n");
    ed.buffers[0].path = Some(path.clone());
    ed.buffers[0].save().unwrap();

    ed.enter_insert(super::exec::InsertAt::Cursor);
    type_text(&mut ed, "a");
    ed.buffers[0].save().unwrap();
    assert!(!ed.buffers[0].dirty, "a save clears the marker");
    type_text(&mut ed, "b");
    assert_eq!(
        ed.buffers[0].undo.depth(),
        (1, 0),
        "the keystroke merges into the step that was just saved"
    );
    assert!(
        ed.buffers[0].dirty,
        "typing after a save marks it modified again"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "aone\n",
        "the file still holds the text as of the save"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undoing_past_a_save_keeps_the_buffer_modified() {
    let mut buf = Buffer::from_text("one\n", None);
    buf.insert(Pos::new(0, 3), "a", true);
    buf.mark_saved();
    buf.insert(Pos::new(0, 4), "b", true);
    assert!(buf.dirty);
    assert!(buf.undo());
    assert!(
        buf.dirty,
        "the written text is the later one, so stepping back is a change"
    );
}

#[test]
fn a_multi_cursor_edit_is_one_undo_step() {
    let mut ed = editor("aaa\nbbb\n");
    // A cursor on each line, then insert at both.
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(0, 0)), Cursor::new(Pos::new(1, 0))];
    ed.insert_text(">");
    assert_eq!(text(&ed), ">aaa\n>bbb\n");
    assert_eq!(ed.buffer().cursors.len(), 2);
    assert!(ed.buffer().dirty, "a multi-cursor edit marks it modified");
    assert!(ed.buffer_mut().undo());
    assert_eq!(text(&ed), "aaa\nbbb\n", "one undo removes both inserts");
    assert!(
        !ed.buffer().dirty,
        "undoing back to the loaded state clears the marker"
    );
}

#[test]
fn typing_merges_into_a_single_undo_step() {
    let mut ed = editor("");
    ed.enter_insert(super::exec::InsertAt::Cursor);
    type_text(&mut ed, "hello");
    assert_eq!(text(&ed), "hello");
    assert!(ed.buffer_mut().undo());
    assert_eq!(text(&ed), "", "the whole word is one step");
}

// ---------------------------------------------------------------------------
// Motions
// ---------------------------------------------------------------------------

#[test]
fn word_motions_walk_words_and_punctuation() {
    let buf = Buffer::from_text("foo bar_baz (qux)\n", None);
    let start = Pos::new(0, 0);
    let next = motion::resolve(&buf, start, Motion::WordNext, 1);
    assert_eq!(next, Pos::new(0, 4), "w lands on the next word");
    let end = motion::resolve(&buf, start, Motion::WordEnd, 1);
    assert_eq!(end, Pos::new(0, 2), "e lands on the last character");
    let back = motion::resolve(&buf, Pos::new(0, 4), Motion::WordBack, 1);
    assert_eq!(back, Pos::new(0, 0));
    // Punctuation is its own word.
    let paren = motion::resolve(&buf, Pos::new(0, 8), Motion::WordNext, 1);
    assert_eq!(paren, Pos::new(0, 12), "the `(` after the space");
}

#[test]
fn word_motions_cross_line_boundaries() {
    let buf = Buffer::from_text("one\ntwo\n", None);
    let next = motion::resolve(&buf, Pos::new(0, 0), Motion::WordNext, 1);
    assert_eq!(next, Pos::new(1, 0));
}

#[test]
fn counts_repeat_a_motion() {
    let buf = Buffer::from_text("a b c d\n", None);
    let third = motion::resolve(&buf, Pos::new(0, 0), Motion::WordNext, 3);
    assert_eq!(third, Pos::new(0, 6));
}

#[test]
fn bracket_matching_finds_the_pair() {
    let buf = Buffer::from_text("fn f(a, (b)) {}\n", None);
    let open = Pos::new(0, 4);
    assert_eq!(motion::match_bracket(&buf, open), Some(Pos::new(0, 11)));
    let close = Pos::new(0, 11);
    assert_eq!(motion::match_bracket(&buf, close), Some(open));
    // A character that is not a bracket has no match.
    assert_eq!(motion::match_bracket(&buf, Pos::new(0, 0)), None);
}

#[test]
fn line_end_stops_on_the_last_character() {
    let buf = Buffer::from_text("abc\n", None);
    assert_eq!(
        motion::resolve(&buf, Pos::new(0, 0), Motion::LineEnd, 1),
        Pos::new(0, 2)
    );
    // An empty line has nowhere to go.
    let empty = Buffer::from_text("\n", None);
    assert_eq!(
        motion::resolve(&empty, Pos::new(0, 0), Motion::LineEnd, 1),
        Pos::new(0, 0)
    );
}

// ---------------------------------------------------------------------------
// Operators and text objects
// ---------------------------------------------------------------------------

#[test]
fn text_objects_select_words_quotes_and_brackets() {
    let buf = Buffer::from_text("let s = \"hello\";\n", None);
    let in_quote = operator::text_object(&buf, Pos::new(0, 10), TextObject::InnerQuote('"'));
    assert_eq!(in_quote, Some(Span::new(Pos::new(0, 9), Pos::new(0, 14))));
    let a_quote = operator::text_object(&buf, Pos::new(0, 10), TextObject::AQuote('"'));
    assert_eq!(a_quote, Some(Span::new(Pos::new(0, 8), Pos::new(0, 15))));

    let buf = Buffer::from_text("call(inner, other)\n", None);
    let inner = operator::text_object(&buf, Pos::new(0, 6), TextObject::InnerParen);
    assert_eq!(inner, Some(Span::new(Pos::new(0, 5), Pos::new(0, 17))));

    let word = operator::text_object(&buf, Pos::new(0, 6), TextObject::InnerWord);
    assert_eq!(word, Some(Span::new(Pos::new(0, 5), Pos::new(0, 10))));
}

#[test]
fn nested_brackets_are_matched_by_depth() {
    let buf = Buffer::from_text("f(a, g(b), c)\n", None);
    // The cursor on the inner `)` selects the inner pair.
    let inner = operator::text_object(&buf, Pos::new(0, 8), TextObject::InnerParen);
    assert_eq!(inner, Some(Span::new(Pos::new(0, 7), Pos::new(0, 8))));
    // The cursor on `a` selects the outer pair, not the inner one.
    let outer = operator::text_object(&buf, Pos::new(0, 2), TextObject::InnerParen);
    assert_eq!(outer, Some(Span::new(Pos::new(0, 2), Pos::new(0, 12))));
    // `a(` includes the brackets themselves.
    let around = operator::text_object(&buf, Pos::new(0, 2), TextObject::AParen);
    assert_eq!(around, Some(Span::new(Pos::new(0, 1), Pos::new(0, 13))));
}

#[test]
fn delete_word_removes_the_word_and_fills_the_register() {
    let mut ed = editor("foo bar baz\n");
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(0, 4))];
    ed.execute(Command::Operate(Operator::Delete));
    ed.execute(Command::Move(Motion::WordNext));
    // `dw` takes the word and the whitespace after it, as Vim does.
    assert_eq!(text(&ed), "foo baz\n");
    assert_eq!(ed.registers.unnamed().text(), "bar ");
}

#[test]
fn delete_line_is_linewise_and_pastes_back() {
    let mut ed = editor("one\ntwo\nthree\n");
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(1, 0))];
    ed.execute(Command::Operate(Operator::Delete));
    ed.execute(Command::Operate(Operator::Delete));
    assert_eq!(text(&ed), "one\nthree\n");
    assert!(ed.registers.unnamed().linewise);
    ed.execute(Command::PasteAfter);
    assert_eq!(text(&ed), "one\nthree\ntwo\n");
}

#[test]
fn change_inner_word_enters_insert_mode() {
    let mut ed = editor("hello world\n");
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(0, 1))];
    ed.execute(Command::Operate(Operator::Change));
    ed.operate_object(Operator::Change, TextObject::InnerWord, None);
    assert_eq!(text(&ed), " world\n");
    assert_eq!(ed.mode, Mode::Insert);
}

#[test]
fn yank_leaves_the_buffer_alone() {
    let mut ed = editor("keep me\n");
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(0, 0))];
    ed.execute(Command::Operate(Operator::Yank));
    ed.execute(Command::Operate(Operator::Yank));
    assert_eq!(text(&ed), "keep me\n");
    assert_eq!(ed.registers.unnamed().text(), "keep me");
}

#[test]
fn indent_and_dedent_shift_whole_lines() {
    let mut ed = editor("one\ntwo\n");
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(0, 0)), Cursor::new(Pos::new(1, 0))];
    ed.execute(Command::Indent);
    assert_eq!(text(&ed), "    one\n    two\n");
    ed.execute(Command::Dedent);
    assert_eq!(text(&ed), "one\ntwo\n");
}

#[test]
fn toggle_comment_adds_and_removes_the_prefix() {
    let mut ed = editor("let a = 1;\nlet b = 2;\n");
    ed.buffers[0].path = Some(std::path::PathBuf::from("x.rs"));
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(0, 0)), Cursor::new(Pos::new(1, 0))];
    ed.execute(Command::ToggleComment);
    assert_eq!(text(&ed), "//let a = 1;\n//let b = 2;\n");
    ed.execute(Command::ToggleComment);
    assert_eq!(text(&ed), "let a = 1;\nlet b = 2;\n");
}

#[test]
fn join_lines_pulls_the_next_line_up() {
    let mut ed = editor("one\n    two\n");
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(0, 0))];
    ed.execute(Command::JoinLines);
    assert_eq!(text(&ed), "one two\n");
}

// ---------------------------------------------------------------------------
// Visual mode and multiple cursors
// ---------------------------------------------------------------------------

#[test]
fn block_selection_covers_a_rectangle() {
    let buf = Buffer::from_text("abcd\nefgh\nijkl\n", None);
    let span = Span::new(Pos::new(0, 1), Pos::new(2, 2));
    let runs = super::view::selection_runs(&buf, span, VisualKind::Block, 1, 2);
    assert_eq!(
        runs,
        vec![(0, 1, 3), (1, 1, 3), (2, 1, 3)],
        "every line in the block gets the same columns"
    );
}

#[test]
fn line_selection_covers_whole_lines() {
    let buf = Buffer::from_text("abcd\nefgh\n", None);
    let span = Span::new(Pos::new(0, 2), Pos::new(1, 1));
    let runs = super::view::selection_runs(&buf, span, VisualKind::Line, 2, 1);
    assert_eq!(runs, vec![(0, 0, 4), (1, 0, 4)]);
}

#[test]
fn select_all_occurrences_puts_a_cursor_on_each() {
    let mut ed = editor("let x = 1;\nlet y = x + x;\n");
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(0, 4))];
    ed.execute(Command::SelectAllOccurrences);
    assert_eq!(ed.buffer().cursors.len(), 3, "x appears three times");
    ed.insert_text("value");
    assert_eq!(text(&ed), "let value = 1;\nlet y = value + value;\n");
}

#[test]
fn select_next_occurrence_adds_one_cursor_at_a_time() {
    let mut ed = editor("a a a\n");
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(0, 0))];
    ed.execute(Command::SelectNextOccurrence);
    assert_eq!(
        ed.buffer().cursors.len(),
        1,
        "the first press selects the word"
    );
    ed.execute(Command::SelectNextOccurrence);
    assert_eq!(ed.buffer().cursors.len(), 2);
    ed.execute(Command::SelectNextOccurrence);
    assert_eq!(ed.buffer().cursors.len(), 3);
}

#[test]
fn adding_a_cursor_below_keeps_the_column() {
    let mut ed = editor("one\ntwo\nthree\n");
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(0, 2))];
    ed.execute(Command::AddCursorBelow);
    assert_eq!(ed.buffer().cursors.len(), 2);
    assert_eq!(ed.buffer().cursors[1].pos, Pos::new(1, 2));
    ed.execute(Command::ClearExtraCursors);
    assert_eq!(ed.buffer().cursors.len(), 1);
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[test]
fn search_finds_every_match_in_reading_order() {
    let buf = Buffer::from_text("one two one\nthree one\n", None);
    let matches = search::find_all(&buf, &Query::new("one"));
    assert_eq!(matches.len(), 3);
    assert_eq!(matches[0].start, Pos::new(0, 0));
    assert_eq!(matches[1].start, Pos::new(0, 8));
    assert_eq!(matches[2].start, Pos::new(1, 6));
}

#[test]
fn case_insensitive_search_matches_both_cases() {
    let buf = Buffer::from_text("One ONE one\n", None);
    let mut query = Query::new("one");
    query.case_sensitive = false;
    assert_eq!(search::find_all(&buf, &query).len(), 3);
    assert_eq!(search::find_all(&buf, &Query::new("one")).len(), 1);
}

#[test]
fn whole_word_search_skips_partial_matches() {
    let buf = Buffer::from_text("one ones one\n", None);
    let mut query = Query::new("one");
    query.whole_word = true;
    assert_eq!(search::find_all(&buf, &query).len(), 2);
}

#[test]
fn regex_search_works() {
    let buf = Buffer::from_text("a1 b2 c3\n", None);
    let mut query = Query::new(r"[a-z]\d");
    query.regex = true;
    assert_eq!(search::find_all(&buf, &query).len(), 3);
}

#[test]
fn replace_all_rewrites_every_match() {
    let mut buf = Buffer::from_text("x x x\n", None);
    let count = search::replace_all(&mut buf, &Query::new("x"), "y");
    assert_eq!(count, 3);
    assert_eq!(buf.text(), "y y y\n");
}

#[test]
fn search_next_wraps_around() {
    let buf = Buffer::from_text("one\ntwo\none\n", None);
    let query = Query::new("one");
    let from = Pos::new(2, 0);
    let next = search::find_next(&buf, &query, from, false).unwrap();
    assert_eq!(next.start, Pos::new(0, 0), "it wraps to the first match");
}

// ---------------------------------------------------------------------------
// Keys to commands
// ---------------------------------------------------------------------------

#[test]
fn normal_mode_letters_run_commands_not_text() {
    let mut ed = editor("hello\n");
    press(&mut ed, egui::Key::L);
    assert_eq!(cursor(&ed), Pos::new(0, 1), "l moves right");
    assert_eq!(text(&ed), "hello\n", "and inserts nothing");
    press(&mut ed, egui::Key::X);
    assert_eq!(text(&ed), "hllo\n", "x deletes a character");
}

#[test]
fn insert_mode_types_text_and_escape_returns() {
    let mut ed = editor("");
    press(&mut ed, egui::Key::I);
    assert_eq!(ed.mode, Mode::Insert);
    type_text(&mut ed, "hi there");
    assert_eq!(text(&ed), "hi there");
    press(&mut ed, egui::Key::Escape);
    assert_eq!(ed.mode, Mode::Normal);
    // In normal mode the same letters are motions again.
    press(&mut ed, egui::Key::H);
    assert_eq!(cursor(&ed).col, 6);
}

#[test]
fn a_count_before_a_motion_repeats_it() {
    let mut ed = editor("a b c d e\n");
    press(&mut ed, egui::Key::Num3);
    press(&mut ed, egui::Key::W);
    assert_eq!(cursor(&ed), Pos::new(0, 6), "3w");
}

#[test]
fn g_then_g_jumps_to_the_first_line() {
    let mut ed = editor("one\ntwo\nthree\n");
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(2, 0))];
    press(&mut ed, egui::Key::G);
    press(&mut ed, egui::Key::G);
    assert_eq!(cursor(&ed).line, 0);
}

#[test]
fn d_then_w_deletes_a_word_and_d_then_d_a_line() {
    let mut ed = editor("alpha beta\ngamma\n");
    press(&mut ed, egui::Key::D);
    press(&mut ed, egui::Key::W);
    assert_eq!(text(&ed), "beta\ngamma\n");
    press(&mut ed, egui::Key::D);
    press(&mut ed, egui::Key::D);
    assert_eq!(text(&ed), "gamma\n");
}

#[test]
fn d_then_i_then_w_deletes_the_inner_word() {
    let mut ed = editor("hello world\n");
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(0, 7))];
    press(&mut ed, egui::Key::D);
    press(&mut ed, egui::Key::I);
    press(&mut ed, egui::Key::W);
    assert_eq!(text(&ed), "hello \n");
}

#[test]
fn colon_opens_the_command_line_and_runs_ex_commands() {
    let mut ed = editor("one\ntwo\nthree\n");
    press(&mut ed, egui::Key::Colon);
    assert!(ed.mode.is_command());
    type_text(&mut ed, "3");
    press(&mut ed, egui::Key::Enter);
    assert_eq!(cursor(&ed).line, 2, ":3 goes to line three");
    assert_eq!(ed.mode, Mode::Normal);
}

#[test]
fn ex_substitute_replaces_on_the_current_line() {
    let mut ed = editor("aaa\nbbb\n");
    ed.run_ex_command("s/a/X/g");
    assert_eq!(text(&ed), "XXX\nbbb\n");
    ed.run_ex_command("%s/b/Y/g");
    assert_eq!(text(&ed), "XXX\nYYY\n");
}

#[test]
fn slash_searches_and_n_repeats() {
    let mut ed = editor("one\ntwo\none\n");
    press(&mut ed, egui::Key::Slash);
    type_text(&mut ed, "one");
    press(&mut ed, egui::Key::Enter);
    assert_eq!(cursor(&ed).line, 2, "the search starts after the cursor");
    press(&mut ed, egui::Key::N);
    assert_eq!(cursor(&ed).line, 0, "n wraps to the first match");
}

#[test]
fn visual_mode_then_an_operator_acts_on_the_selection() {
    let mut ed = editor("abcdef\n");
    press(&mut ed, egui::Key::V);
    assert!(ed.mode.is_visual());
    press(&mut ed, egui::Key::L);
    press(&mut ed, egui::Key::L);
    press(&mut ed, egui::Key::D);
    assert_eq!(text(&ed), "def\n");
    assert_eq!(ed.mode, Mode::Normal);
}

#[test]
fn unknown_keys_are_left_for_the_application() {
    let mut ed = editor("x\n");
    // F5 is not bound in normal mode.
    assert!(!ed.key(egui::Key::F5, egui::Modifiers::default(), None));
    assert_eq!(text(&ed), "x\n");
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

#[test]
fn saving_writes_the_file_and_clears_the_dirty_flag() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.txt");
    let mut ed = editor("hello\n");
    ed.buffers[0].path = Some(path.clone());
    ed.buffers[0].dirty = true;
    ed.save_active();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
    assert!(!ed.buffer().dirty);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_external_change_is_detected() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-ext-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("watched.txt");
    std::fs::write(&path, "one\n").unwrap();
    let buf = Buffer::open(&path).unwrap();
    assert_eq!(buf.check_external(), super::ExternalChange::None);
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&path, "one\ntwo\n").unwrap();
    assert_eq!(buf.check_external(), super::ExternalChange::Changed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn closing_a_dirty_buffer_asks_before_discarding_it() {
    let mut ed = editor("unsaved\n");
    ed.buffers[0].dirty = true;
    ed.close_buffer(0);
    assert_eq!(ed.buffers.len(), 1, "the buffer stays open");
    let prompt = ed.close_prompt.clone().expect("the editor asks first");
    assert_eq!(prompt.buffer, 0);
    assert!(
        !prompt.can_save,
        "a scratch buffer has nowhere to save to, so it offers discard and cancel only"
    );

    // Cancel keeps it, and so does asking to save something unsaveable.
    ed.close_prompt = None;
    ed.close_buffer(0);
    ed.answer_close(true);
    assert!(ed.close_prompt.is_some(), "the question stays up");
    assert_eq!(ed.buffers.len(), 1);

    // Discard closes it.
    ed.answer_close(false);
    assert!(ed.close_prompt.is_none());
    assert_eq!(
        ed.buffers.len(),
        1,
        "the last buffer is replaced, not removed"
    );
    assert_eq!(text(&ed), "");
}

#[test]
fn closing_a_dirty_file_saves_it_when_asked() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-close-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("work.txt");
    std::fs::write(&path, "one\n").unwrap();

    let mut ed = editor("one\n");
    ed.buffers[0].path = Some(path.clone());
    ed.buffers[0].save().unwrap();
    ed.insert_text("x");
    assert!(ed.buffers[0].dirty);

    ed.close_buffer(0);
    let prompt = ed.close_prompt.clone().expect("the editor asks first");
    assert!(prompt.can_save, "it has a file to write to");
    ed.answer_close(true);

    assert!(ed.close_prompt.is_none());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "xone\n",
        "the edit reached the file before the buffer went away"
    );
    assert_eq!(text(&ed), "", "and the buffer is gone");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn closing_a_dirty_file_discards_it_when_asked() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-discard-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("work.txt");
    std::fs::write(&path, "one\n").unwrap();

    let mut ed = editor("one\n");
    ed.buffers[0].path = Some(path.clone());
    ed.buffers[0].save().unwrap();
    ed.insert_text("x");

    ed.close_buffer(0);
    ed.answer_close(false);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "one\n",
        "discarding leaves the file alone"
    );
    assert_eq!(text(&ed), "");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_clean_buffer_closes_without_asking() {
    let mut ed = editor("clean\n");
    ed.close_buffer(0);
    assert!(ed.close_prompt.is_none(), "nothing to ask about");
    assert_eq!(text(&ed), "");
}

#[test]
fn typing_with_no_file_open_says_so() {
    // A freshly launched editor holds no file, only the stand-in buffer.
    let mut ed = Editor::new(
        Settings::default(),
        Keymap::from_config(&Default::default()),
        std::env::temp_dir(),
    );
    ed.enter_insert(super::exec::InsertAt::Cursor);
    ed.text_input("hi");
    assert_eq!(text(&ed), "", "nothing was written");
    assert_eq!(
        ed.message.as_ref().map(|(m, _)| m.as_str()),
        Some("open a file to write"),
        "the status line says what to do instead"
    );
}

#[test]
fn after_the_last_file_is_closed_typing_asks_for_a_file() {
    let mut ed = editor("one\n");
    ed.close_buffer(0);
    ed.enter_insert(super::exec::InsertAt::Cursor);
    ed.text_input("x");
    assert_eq!(text(&ed), "", "the stand-in buffer stays empty");
    assert_eq!(
        ed.message.as_ref().map(|(m, _)| m.as_str()),
        Some("open a file to write")
    );
}

#[test]
fn an_explicit_new_buffer_still_takes_text() {
    let mut ed = editor("");
    ed.new_scratch();
    ed.enter_insert(super::exec::InsertAt::Cursor);
    ed.text_input("hi");
    assert_eq!(text(&ed), "hi", "a buffer the user asked for is writable");
    assert!(ed.message.is_none(), "no complaint about it");
}

#[test]
fn deleting_a_file_from_the_tree_closes_or_detaches_its_buffers() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-del-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let clean = dir.join("clean.txt");
    let dirty = dir.join("dirty.txt");
    std::fs::write(&clean, "clean\n").unwrap();
    std::fs::write(&dirty, "dirty\n").unwrap();

    let mut ed = editor("");
    ed.buffers[0] = Buffer::open(&clean).unwrap();
    ed.buffers.push(Buffer::open(&dirty).unwrap());
    ed.buffers[1].dirty = true;

    // The buffer of a deleted CLEAN file closes.
    ed.tree_deleted(&clean);
    assert_eq!(ed.buffers.len(), 1, "the clean buffer was closed");
    assert_eq!(
        ed.buffers[0].path.as_deref(),
        Some(dirty.as_path()),
        "the remaining buffer is the dirty one"
    );

    // The buffer of a deleted DIRTY file keeps its text but loses its path,
    // so saving it asks for a name instead of resurrecting the file.
    ed.tree_deleted(&dirty);
    assert!(
        ed.buffers[0].path.is_none(),
        "detached from the deleted path"
    );
    assert_eq!(ed.buffers[0].text(), "dirty\n", "the text survives");

    // Deleting a folder reaches the buffers of the files inside it.
    let inner = dir.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    let inside = inner.join("f.txt");
    std::fs::write(&inside, "inner\n").unwrap();
    let mut ed2 = editor("");
    ed2.buffers[0] = Buffer::open(&inside).unwrap();
    ed2.buffers[0].dirty = true;
    ed2.tree_deleted(&inner);
    assert!(
        ed2.buffers[0].path.is_none(),
        "a folder delete detaches too"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn deleting_from_the_tree_trashes_instead_of_erasing() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-trash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let name = "only-trash-me.txt";
    let path = dir.join(name);
    std::fs::write(&path, "gone\n").unwrap();
    let trash_files = dirs::home_dir().unwrap().join(".local/share/Trash/files");
    let trash_info = dirs::home_dir().unwrap().join(".local/share/Trash/info");
    // A previous run that died mid-test may have left its copy behind;
    // clear the way so the fresh one lands under its own plain name.
    for base in [&trash_files, &trash_info] {
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let file = entry.file_name().to_string_lossy().into_owned();
                if file.starts_with(name) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
    // The trashed copy must leave the user's recycle bin even if this test
    // panics on the way.
    struct Binned(std::path::PathBuf, std::path::PathBuf);
    impl Drop for Binned {
        fn drop(&mut self) {
            for base in [&self.0, &self.1] {
                if let Ok(entries) = std::fs::read_dir(base) {
                    for entry in entries.flatten() {
                        let file = entry.file_name().to_string_lossy().into_owned();
                        if file.starts_with("only-trash-me.txt") {
                            let _ = std::fs::remove_file(entry.path());
                        }
                    }
                }
            }
        }
    }
    let _binned = Binned(trash_files.clone(), trash_info.clone());

    let mut ed = editor("");
    ed.buffers[0] = Buffer::open(&path).unwrap();

    ed.trash_from_tree(&path);
    assert!(!path.exists(), "the file left its folder");
    assert!(ed.buffers[0].scratch, "its open buffer closed");
    let message = ed
        .message
        .as_ref()
        .map(|(m, _)| m.as_str())
        .unwrap_or_default();
    assert!(message.contains("moved to the trash"), "says so: {message}");
    // Recoverable, not erased: on Linux the file sits in the freedesktop trash.
    #[cfg(target_os = "linux")]
    assert!(
        trash_files.join(name).exists(),
        "the entry is in the trash, waiting to be restored"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_deleted_on_disk_stops_being_edited_in_real_time() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-gone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let clean = dir.join("clean.txt");
    let dirty = dir.join("dirty.txt");
    std::fs::write(&clean, "clean\n").unwrap();
    std::fs::write(&dirty, "dirty\n").unwrap();

    let mut ed = editor("");
    ed.buffers[0] = Buffer::open(&clean).unwrap();
    ed.buffers.push(Buffer::open(&dirty).unwrap());
    ed.buffers[1].dirty = true;

    // The files vanish outside the editor's knowledge.
    std::fs::remove_file(&clean).unwrap();
    std::fs::remove_file(&dirty).unwrap();
    ed.check_external_changes(1.0);
    ed.check_external_changes(2.0);

    // The clean buffer is closed; the dirty one keeps its text but is
    // detached from the path that no longer exists.
    assert_eq!(ed.buffers.len(), 1, "only the dirty buffer is left");
    assert_eq!(ed.buffers[0].text(), "dirty\n", "the unsaved text survives");
    assert!(
        ed.buffers[0].path.is_none(),
        "the survivor is the detached dirty one"
    );
    let message = ed
        .message
        .as_ref()
        .map(|(m, _)| m.as_str())
        .unwrap_or_default();
    assert!(
        message.contains("was deleted on disk"),
        "the status line says so: {message}"
    );

    // A later sweep reports nothing new and closes nothing else.
    ed.check_external_changes(3.0);
    assert_eq!(ed.buffers.len(), 1, "no double-closing on the next sweep");
    assert_eq!(ed.buffers[0].text(), "dirty\n");

    // When the deleted file's buffer is the only one, the editor lands on
    // the no-file state instead of an empty editor.
    let solo = dir.join("solo.txt");
    std::fs::write(&solo, "solo\n").unwrap();
    let mut ed3 = editor("");
    ed3.buffers[0] = Buffer::open(&solo).unwrap();
    std::fs::remove_file(&solo).unwrap();
    ed3.check_external_changes(1.0);
    ed3.check_external_changes(2.0);
    assert!(ed3.buffers[0].scratch, "landed on the no-file buffer");
    assert!(
        ed3.buffers[0].path.is_none(),
        "the closed file is not open again"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn moving_a_file_moves_its_open_buffer_with_it() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-mv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("nest")).unwrap();
    let file = dir.join("mvme.txt");
    std::fs::write(&file, "moving\n").unwrap();

    let mut ed = editor("");
    ed.buffers[0] = Buffer::open(&file).unwrap();
    ed.move_in_tree(&file, &dir.join("nest"));
    let new_path = dir.join("nest").join("mvme.txt");
    assert_eq!(
        ed.buffers[0].path.as_deref(),
        Some(new_path.as_path()),
        "the buffer follows the file"
    );
    assert!(new_path.exists(), "the file moved on the disk");
    // And saving writes to the new place, not the old one.
    ed.buffers[0].dirty = true;
    ed.save_active();
    assert_eq!(
        std::fs::read_to_string(&new_path).unwrap(),
        "moving\n",
        "saving writes to the new location"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn renaming_a_file_keeps_its_open_buffer_on_it() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-rename-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join("old.txt");
    std::fs::write(&old, "same text\n").unwrap();

    let mut ed = editor("");
    ed.buffers[0] = Buffer::open(&old).unwrap();
    let new = ed.tree.rename(&old, "new.txt").unwrap();
    // The editor renames and rewrites the buffer's path together, which is
    // what the tree prompt does.
    ed.buffers_follow(&old, &new);
    assert_eq!(
        ed.buffers[0].path.as_deref(),
        Some(new.as_path()),
        "the buffer points at the new name"
    );
    assert!(new.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saving_a_file_that_was_deleted_recreates_it_and_says_so() {
    let dir =
        std::env::temp_dir().join(format!("cl2metal-editor-resurrect-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("gone.txt");
    std::fs::write(&file, "still editing\n").unwrap();

    let mut ed = editor("");
    ed.buffers[0] = Buffer::open(&file).unwrap();
    std::fs::remove_file(&file).unwrap();
    ed.save_active();
    assert!(file.exists(), "the save recreates the file");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "still editing\n");
    assert_eq!(
        ed.message.as_ref().map(|(m, _)| m.as_str()),
        Some("recreated gone.txt — it had been deleted on disk"),
        "and the message says what happened"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_the_text_modes_take_text() {
    // egui sends a Text event after the Key event for the same press. The
    // application drops that text when the key was handled as a command, so
    // this is what decides whether `i` inserts an `i` or enters insert mode.
    assert!(!Mode::Normal.takes_text());
    assert!(!Mode::Visual(VisualKind::Char).takes_text());
    assert!(Mode::Insert.takes_text());
    assert!(Mode::Replace.takes_text());
    assert!(Mode::Command(super::mode::CommandKind::Ex).takes_text());
}

#[test]
fn colon_q_asks_before_closing_a_dirty_buffer() {
    let mut ed = editor("unsaved\n");
    ed.enter_insert(super::exec::InsertAt::Cursor);
    type_text(&mut ed, "x");
    assert!(ed.buffer().dirty, "the edit landed");
    press(&mut ed, egui::Key::Escape);
    // `:` opens the command line, `q` types the command, Enter runs it.
    ed.key(egui::Key::Colon, egui::Modifiers::default(), Some(":"));
    assert!(ed.mode.is_command(), "`:` opens the command line");
    ed.text_input("q");
    assert_eq!(ed.command_line, "q", "the command is typed");
    press(&mut ed, egui::Key::Enter);
    assert!(
        ed.close_prompt.is_some(),
        "`:q` on a dirty buffer asks instead of discarding"
    );
    assert_eq!(ed.buffers.len(), 1, "and the buffer is still there");
}

/// Wait for a file read started by `open_path` to land.
fn wait_for_load(ed: &mut Editor) {
    for _ in 0..5_000 {
        if ed.poll_loading() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("the file read never finished");
}

#[test]
fn opening_a_file_reads_it_off_the_ui_thread() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-async-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("long.txt");
    let mut source = String::new();
    for i in 0..200_000 {
        source.push_str(&format!("line {i}\n"));
    }
    std::fs::write(&path, &source).unwrap();

    let mut ed = editor("");
    assert!(ed.open_path(&path));
    // The buffer is only ever inserted by `poll_loading`, so this is what
    // proves the read did not happen on the calling thread.
    assert!(ed.loading.is_some(), "the read is handed to a worker");
    assert_eq!(ed.buffers.len(), 1, "and has not landed yet");

    wait_for_load(&mut ed);
    assert!(ed.loading.is_none());
    assert_eq!(ed.buffers.len(), 2, "the scratch buffer plus the file");
    assert_eq!(ed.buffer().path.as_deref(), Some(path.as_path()));
    assert_eq!(ed.buffer().line_count(), 200_000);
    assert_eq!(text(&ed), source, "the whole file arrived");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opening_the_same_file_twice_reuses_the_buffer() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.txt");
    std::fs::write(&path, "content\n").unwrap();
    let mut ed = editor("");
    assert!(ed.open_path(&path));
    // A second click while the first read is still running must not start
    // another one.
    assert!(ed.open_path(&path));
    wait_for_load(&mut ed);
    assert_eq!(ed.buffers.len(), 2, "the scratch buffer plus one file");
    // Once it is open, opening it again focuses it instead of reading it.
    assert!(ed.open_path(&path));
    assert!(ed.loading.is_none(), "no second read was started");
    assert_eq!(ed.buffers.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_too_large_to_edit_is_refused_before_it_is_read() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-huge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("huge.bin");
    // Sparse: the check reads the length, so this costs no disk.
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_EDIT_BYTES + 1).unwrap();
    drop(file);

    let err = match Buffer::open(&path) {
        Ok(_) => panic!("a file over the limit must not open"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("too large to edit"),
        "the refusal says why: {err}"
    );

    // And through the editor, where it lands as a status message.
    let mut ed = editor("");
    assert!(ed.open_path(&path));
    wait_for_load(&mut ed);
    assert_eq!(ed.buffers.len(), 1, "nothing was opened");
    let message = ed
        .message
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default();
    assert!(message.contains("too large to edit"), "{message:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opening_a_missing_file_reports_it_and_opens_nothing() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-gone-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut ed = editor("");
    assert!(ed.open_path(&dir.join("not-here.txt")));
    wait_for_load(&mut ed);
    assert_eq!(ed.buffers.len(), 1, "nothing was opened");
    let message = ed
        .message
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default();
    assert!(
        message.contains("cannot open"),
        "the user is told why: {message:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_remote_buffer_is_marked_and_saved_through_the_app() {
    let mut ed = editor("");
    ed.open_remote(
        "user@host:22".to_owned(),
        "/etc/hosts".to_owned(),
        "127.0.0.1\n".to_owned(),
    );
    let buf = ed.buffer();
    assert!(buf.is_remote());
    assert_eq!(buf.title(), "hosts");
    assert_eq!(buf.location(), "user@host:22:/etc/hosts");
    ed.buffer_mut().dirty = true;
    ed.save_active();
    let save = ed
        .pending_remote_save
        .as_ref()
        .expect("a remote save is handed to the app, not written locally");
    assert_eq!(save.buffer, ed.active);
    assert_eq!(save.host, "user@host:22");
    assert_eq!(save.path, "/etc/hosts");
    assert_eq!(
        save.text, "127.0.0.1\n",
        "the bytes travel with the request, so closing the buffer cannot lose them"
    );
}

// ---------------------------------------------------------------------------
// Layout and settings
// ---------------------------------------------------------------------------

#[test]
fn splitting_and_closing_panes_keeps_the_buffer_open() {
    let mut ed = editor("one\n");
    ed.new_scratch();
    assert_eq!(ed.buffers.len(), 2);
    ed.split(super::SplitDir::Right);
    assert_eq!(ed.layout.pane_count(), 2);
    ed.close_split();
    assert_eq!(ed.layout.pane_count(), 1);
    assert_eq!(
        ed.buffers.len(),
        2,
        "closing a pane does not close the buffer"
    );
}

#[test]
fn set_options_change_the_settings() {
    let mut ed = editor("");
    ed.run_ex_command("set tabstop=2");
    assert_eq!(ed.settings.tab_width, 2);
    ed.run_ex_command("set number");
    assert!(ed.settings.line_numbers);
    ed.run_ex_command("set nonumber");
    assert!(!ed.settings.line_numbers);
    ed.run_ex_command("set wrap");
    assert!(ed.settings.wrap);
}

#[test]
fn tab_inserts_to_the_next_stop() {
    let mut ed = editor("ab");
    ed.enter_insert(super::exec::InsertAt::LineEnd);
    ed.insert_tab();
    assert_eq!(text(&ed), "ab  ", "two spaces reach column four");
    ed.insert_tab();
    assert_eq!(text(&ed), "ab      ", "and four more reach eight");
}

#[test]
fn auto_indent_carries_the_indentation_to_the_new_line() {
    let mut ed = editor("    indented\n");
    ed.enter_insert(super::exec::InsertAt::LineEnd);
    ed.newline();
    // The file ends with a newline, so the new empty line is the last one.
    assert_eq!(text(&ed), "    indented\n    \n");
}

#[test]
fn opening_a_line_below_puts_the_cursor_there() {
    let mut ed = editor("one\ntwo\n");
    ed.buffer_mut().cursors = vec![Cursor::new(Pos::new(0, 1))];
    ed.execute(Command::OpenBelow);
    assert_eq!(text(&ed), "one\n\ntwo\n");
    assert_eq!(cursor(&ed), Pos::new(1, 0));
    assert_eq!(ed.mode, Mode::Insert);
}

#[test]
fn replace_mode_overwrites_characters() {
    let mut ed = editor("abcdef\n");
    ed.mode = Mode::Replace;
    ed.insert_text("XY");
    assert_eq!(text(&ed), "XYcdef\n");
}

#[test]
fn a_large_file_is_opened_without_highlighting() {
    let dir = std::env::temp_dir().join(format!("cl2metal-editor-large-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("big.txt");
    let big = "x".repeat(super::buffer::LARGE_FILE_BYTES as usize + 16);
    std::fs::write(&path, &big).unwrap();
    let buf = Buffer::open(&path).unwrap();
    assert!(buf.large, "a file over the limit is marked as large");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_wheel_is_not_yanked_back_to_the_cursor() {
    let mut ed = editor(&"line\n".repeat(200));
    let geometry = pane(20);
    // The first draw anchors on the cursor without moving the view.
    ed.follow_cursor(0, geometry);
    assert_eq!(ed.buffers[0].scroll[1], 0.0);
    // The wheel scrolls; the cursor has not moved, so the view stays put.
    ed.buffers[0].scroll[1] = 40.0;
    ed.follow_cursor(0, geometry);
    assert_eq!(
        ed.buffers[0].scroll[1], 40.0,
        "scrolling must not be undone by the cursor follow"
    );
    // Moving the cursor off-screen brings it back into view.
    ed.buffers[0].cursors[0].pos = Pos::new(120, 0);
    ed.follow_cursor(0, geometry);
    assert!(
        ed.buffers[0].scroll[1] > 40.0,
        "a moved cursor is followed again"
    );
}

#[test]
fn the_scrollbar_thumb_maps_back_to_its_offset() {
    let max_scroll = 400.0;
    let track_min_y = 10.0;
    let travel = 300.0;
    let thumb_h = 40.0;
    // Grabbing the thumb by its middle leaves the offset where it was.
    for scroll in [0.0, 100.0, 250.0, 400.0] {
        let offset = super::view::thumb_offset(scroll, max_scroll);
        let y = track_min_y + travel * offset + thumb_h * 0.5;
        let back = super::view::scroll_at_pointer(y, track_min_y, travel, thumb_h, max_scroll);
        assert!((back - scroll).abs() < 0.5, "{scroll} came back as {back}");
    }
    // Dragging past either end clamps instead of running away.
    assert_eq!(
        super::view::scroll_at_pointer(-500.0, track_min_y, travel, thumb_h, max_scroll),
        0.0
    );
    assert_eq!(
        super::view::scroll_at_pointer(9999.0, track_min_y, travel, thumb_h, max_scroll),
        max_scroll
    );
    // A buffer that fits has no travel at all.
    assert_eq!(super::view::thumb_offset(10.0, 0.0), 0.0);
}
