//! Unit tests for the application layer.

use super::background::{
    bg_alpha_for, load_image, panel_alpha_for, read_cache, ui_alpha_for, write_cache,
};
use super::eggs::{EasterEgg, hidden_eggs, hue_rotate, matrix_char};
use super::events::shell_quote;
use super::*;

#[test]
fn dropped_paths_are_quoted_for_the_shell() {
    use std::path::Path;
    assert_eq!(shell_quote(Path::new("/tmp/plain.png")), "/tmp/plain.png");
    assert_eq!(
        shell_quote(Path::new("/tmp/my file.png")),
        "'/tmp/my file.png'"
    );
    assert_eq!(
        shell_quote(Path::new("/tmp/it's.png")),
        "'/tmp/it'\\''s.png'"
    );
}

#[test]
fn ease_out_cubic_is_normalised_and_monotonic() {
    assert_eq!(ease_out_cubic(0.0), 0.0);
    assert_eq!(ease_out_cubic(1.0), 1.0);
    // Clamped outside the unit range.
    assert_eq!(ease_out_cubic(-1.0), 0.0);
    assert_eq!(ease_out_cubic(2.0), 1.0);
    let mut previous = 0.0;
    for step in 0..=10 {
        let value = ease_out_cubic(step as f32 / 10.0);
        assert!(value >= previous, "must not go backwards");
        previous = value;
    }
    // Ease-out front-loads the motion.
    assert!(ease_out_cubic(0.5) > 0.5);
}

#[test]
fn truncate_label_keeps_short_labels_and_elides_long_ones() {
    assert_eq!(truncate_label("bash", 24), "bash");
    assert_eq!(truncate_label("abcdef", 4), "abc…");
    // Counts characters, not bytes.
    assert_eq!(truncate_label("héllo wörld", 5), "héll…");
}

#[test]
fn shell_label_uses_the_file_name() {
    assert_eq!(shell_label("/bin/bash"), "bash");
    assert_eq!(shell_label("/usr/bin/zsh"), "zsh");
    assert_eq!(shell_label("fish"), "fish");
}

#[test]
fn expand_tilde_resolves_the_home_directory() {
    let expanded = expand_tilde("~/.ssh/id_ed25519");
    if let Some(home) = dirs::home_dir() {
        assert!(expanded.starts_with(&home.to_string_lossy().to_string()));
        assert!(expanded.ends_with(".ssh/id_ed25519"));
    }
    // Absolute and relative paths are untouched.
    assert_eq!(expand_tilde("/etc/hosts"), "/etc/hosts");
    assert_eq!(expand_tilde("keys/id"), "keys/id");
}

#[test]
fn cursor_style_follows_the_config() {
    let mut config = Config {
        cursor_shape: CursorShape::Beam,
        cursor_blink: false,
        ..Config::default()
    };
    let style = cursor_style(&config);
    assert_eq!(
        style.shape,
        alacritty_terminal::vte::ansi::CursorShape::Beam
    );
    assert!(!style.blinking);

    config.cursor_shape = CursorShape::Underline;
    config.cursor_blink = true;
    let style = cursor_style(&config);
    assert_eq!(
        style.shape,
        alacritty_terminal::vte::ansi::CursorShape::Underline
    );
    assert!(style.blinking);
}

#[test]
fn font_zoom_steps_and_clamps() {
    // Ctrl+scroll and Ctrl+=/- step by one point.
    assert_eq!(zoomed_font_size(14.0, 1.0), 15.0);
    assert_eq!(zoomed_font_size(14.0, -1.0), 13.0);
    // The range stays legible at both ends.
    assert_eq!(zoomed_font_size(48.0, 1.0), 48.0);
    assert_eq!(zoomed_font_size(6.0, -1.0), 6.0);
    assert_eq!(zoomed_font_size(47.5, 1.0), 48.0);
    // Ctrl+0 resets to the default regardless of the current size.
    assert_eq!(
        zoomed_font_size(31.0, 0.0),
        crate::config::DEFAULT_FONT_SIZE
    );
    assert_eq!(zoomed_font_size(6.0, 0.0), crate::config::DEFAULT_FONT_SIZE);
}

/// Regression: a wallpaper wider than the GPU's maximum texture side made
/// `egui::Context::load_texture` panic on startup.
#[test]
fn oversized_background_images_are_downscaled_to_fit() {
    let dir = std::env::temp_dir().join("cl2metal-test-bg");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("wide.png");
    let img = image::RgbaImage::from_pixel(300, 100, image::Rgba([10, 20, 30, 255]));
    img.save(&path).expect("write png");

    let (pixels, w, h) = load_image(path.to_str().unwrap(), 100).expect("load");
    assert_eq!((w, h), (100, 33), "the long edge must be clamped");
    assert_eq!(pixels.len(), (w * h * 4) as usize);

    // An image that already fits is left at its own size.
    let (_, w, h) = load_image(path.to_str().unwrap(), 4096).expect("load");
    assert_eq!((w, h), (300, 100));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn internal_commands_parse_at_the_end_of_the_prompt_line() {
    use super::eggs::{InternalCmd, parse_internal};
    // Bare commands, and the same typed after a shell prompt.
    assert_eq!(parse_internal(":info"), Some(InternalCmd::Info));
    assert_eq!(
        parse_internal("mehul@box:~$ :info"),
        Some(InternalCmd::Info)
    );
    assert_eq!(parse_internal(":matrix"), Some(InternalCmd::Matrix));
    assert_eq!(parse_internal(":party"), Some(InternalCmd::Party));
    assert_eq!(parse_internal(":42"), Some(InternalCmd::Answer));
    assert_eq!(parse_internal(":help"), Some(InternalCmd::Help));
    assert_eq!(parse_internal(":?"), Some(InternalCmd::Help));
    // Trailing whitespace is fine.
    assert_eq!(parse_internal(":info  "), Some(InternalCmd::Info));
}

#[test]
fn internal_commands_reject_lookalikes() {
    use super::eggs::parse_internal;
    // Not a command at all.
    assert_eq!(parse_internal("ls -la"), None);
    assert_eq!(parse_internal(""), None);
    // A suffix of another word must not match.
    assert_eq!(parse_internal("foo:info"), None);
    assert_eq!(parse_internal("http://x/:info"), None);
    // Case matters, like every other shell command.
    assert_eq!(parse_internal(":INFO"), None);
    assert_eq!(parse_internal(":Info"), None);
    // The long forms are not internal commands; they are handled separately.
    assert_eq!(parse_internal("cl2metal --info"), None);
}

#[test]
fn internal_commands_with_arguments_parse() {
    use super::eggs::{InternalCmd, parse_internal};
    assert_eq!(
        parse_internal(":theme paper"),
        Some(InternalCmd::Theme("paper".to_owned()))
    );
    assert_eq!(
        parse_internal("mehul@box:~$ :theme graphite"),
        Some(InternalCmd::Theme("graphite".to_owned()))
    );
    assert_eq!(parse_internal(":font 18"), Some(InternalCmd::Font(18.0)));
    assert_eq!(
        parse_internal(":bg /tmp/wall.png"),
        Some(InternalCmd::Background(Some("/tmp/wall.png".to_owned())))
    );
    assert_eq!(
        parse_internal(":bg off"),
        Some(InternalCmd::Background(None))
    );
    assert_eq!(parse_internal(":clear"), Some(InternalCmd::Clear));
    assert_eq!(parse_internal(":reload"), Some(InternalCmd::Reload));
    assert_eq!(
        parse_internal(":ssh mehul@host:2222"),
        Some(InternalCmd::Ssh("mehul@host:2222".to_owned()))
    );
    // Missing or malformed arguments explain themselves instead of reaching
    // the shell.
    assert!(matches!(
        parse_internal(":theme"),
        Some(InternalCmd::Usage(_))
    ));
    assert!(matches!(
        parse_internal(":font abc"),
        Some(InternalCmd::Usage(_))
    ));
    assert!(matches!(parse_internal(":bg"), Some(InternalCmd::Usage(_))));
    assert!(matches!(
        parse_internal(":ssh"),
        Some(InternalCmd::Usage(_))
    ));
}

#[test]
fn the_shell_erasure_covers_exactly_the_typed_command() {
    use super::eggs::{alias_erase_len, erase_bytes, internal_command_at};
    // The offset is the ':' of the last `:word` on the prompt line.
    let line = "PS C:\\Users\\me> :bench 8";
    assert_eq!(internal_command_at(line), line.find(":bench"));
    assert_eq!(internal_command_at(":info"), Some(0));
    assert_eq!(internal_command_at("foo:info"), None);
    assert_eq!(internal_command_at("http://x/:info"), None);
    // Aliases count as the whole token, path prefix included.
    assert_eq!(
        alias_erase_len("./cl2metal --party", "cl2metal --party"),
        18
    );
    assert_eq!(
        alias_erase_len("mehul@box:~$ cl2metal --matrix", "cl2metal --matrix"),
        17
    );
    // Ctrl+U where readline understands it; backspaces where it does not.
    #[cfg(not(windows))]
    assert_eq!(erase_bytes(8), vec![0x15]);
    #[cfg(windows)]
    {
        assert_eq!(erase_bytes(8), vec![0x08; 8]);
        assert_eq!(erase_bytes(0), Vec::<u8>::new());
    }
}

#[test]
fn typed_command_finds_the_word_being_typed() {
    use super::eggs::typed_command;
    assert_eq!(typed_command(":th"), Some("th"));
    assert_eq!(typed_command("mehul@box:~$ :inf"), Some("inf"));
    assert_eq!(typed_command(":"), Some(""));
    // Arguments already started: no completion.
    assert_eq!(typed_command(":theme pa"), None);
    assert_eq!(typed_command("ls -la"), None);
    assert_eq!(typed_command("foo:info"), None);
    assert_eq!(typed_command(""), None);
}

#[test]
fn pane_tree_splits_removes_and_renumbers() {
    use super::{PaneNode, SplitDir};
    let mut tree = PaneNode::Leaf(0);
    assert_eq!(tree.leaves(), vec![0]);
    // Split the only pane to the right: 0 | 1.
    assert!(tree.split_leaf(0, 1, SplitDir::Right));
    assert_eq!(tree.leaves(), vec![0, 1]);
    // Split pane 1 downwards: 0 | (1 / 2).
    assert!(tree.split_leaf(1, 2, SplitDir::Down));
    assert_eq!(tree.leaves(), vec![0, 1, 2]);
    // Splitting a pane that is not in the tree changes nothing.
    assert!(!tree.split_leaf(9, 3, SplitDir::Right));
    // Removing pane 1 collapses its split, leaving 0 | 2.
    assert!(tree.remove_leaf(1));
    assert_eq!(tree.leaves(), vec![0, 2]);
    // Renumber after the removal: 0 -> 0, 2 -> 1.
    tree.remap(&[Some(0), None, Some(1)]);
    assert_eq!(tree.leaves(), vec![0, 1]);
    // A bare leaf has nothing to remove.
    let mut single = PaneNode::Leaf(0);
    assert!(!single.remove_leaf(0));
}

#[test]
fn pane_rects_tile_the_area_and_the_divider_sits_between() {
    use super::view::{pane_dividers, pane_rects, set_ratio};
    use super::{PaneNode, SplitDir};
    let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
    let mut tree = PaneNode::Leaf(0);
    tree.split_leaf(0, 1, SplitDir::Right);
    let rects = pane_rects(&tree, area);
    assert_eq!(rects.len(), 2);
    assert_eq!(rects[0].0, 0);
    assert_eq!(rects[1].0, 1);
    // Both panes stay inside the area.
    for (_, r) in &rects {
        assert!(r.min.x >= area.min.x - 0.01 && r.max.x <= area.max.x + 0.01);
        assert!(r.min.y >= area.min.y - 0.01 && r.max.y <= area.max.y + 0.01);
    }
    // The first pane ends where the second begins.
    assert!(rects[0].1.max.x <= rects[1].1.min.x + 0.01);
    // One divider, sitting between them.
    let dividers = pane_dividers(&tree, area);
    assert_eq!(dividers.len(), 1);
    assert!(dividers[0].rect.min.x >= rects[0].1.max.x - 0.01);
    assert!(dividers[0].rect.max.x <= rects[1].1.min.x + 0.01);

    // Dragging the divider moves the split ratio, and the clamp holds.
    set_ratio(&mut tree, &[], 0.25);
    let rects = pane_rects(&tree, area);
    assert!(rects[0].1.width() < 300.0, "left pane shrank");
    set_ratio(&mut tree, &[], 5.0);
    let rects = pane_rects(&tree, area);
    assert!(
        rects[0].1.width() <= 800.0 * 0.85 + 0.01,
        "ratio is clamped"
    );
}

#[test]
fn even_out_gives_every_pane_an_equal_share() {
    use super::view::pane_rects;
    use super::{EvenMode, PaneNode, SplitDir};
    let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 600.0));
    // The layout from the screenshot: a column of three, plus two more.
    let build = || {
        let mut tree = PaneNode::Leaf(0);
        tree.split_leaf(0, 1, SplitDir::Right);
        tree.split_leaf(0, 2, SplitDir::Down);
        tree.split_leaf(2, 3, SplitDir::Down);
        tree.split_leaf(1, 4, SplitDir::Right);
        tree
    };
    assert_eq!(build().leaves().len(), 5);

    // Columns: five equal widths, each full height.
    let mut tree = build();
    tree.even_out(EvenMode::Columns);
    let rects = pane_rects(&tree, area);
    assert_eq!(rects.len(), 5);
    let widths: Vec<f32> = rects.iter().map(|(_, r)| r.width()).collect();
    let mean = widths.iter().sum::<f32>() / widths.len() as f32;
    for w in &widths {
        // Dividers eat a few pixels unevenly down the chain, so "equal" means
        // equal to within a divider.
        assert!((w - mean).abs() < 5.0, "columns must be equal: {widths:?}");
    }
    assert!(rects[0].1.min.x <= area.min.x + 0.01);
    assert!(rects[4].1.max.x >= area.max.x - 0.01);

    // Rows: five equal heights, each full width.
    let mut tree = build();
    tree.even_out(EvenMode::Rows);
    let rects = pane_rects(&tree, area);
    let heights: Vec<f32> = rects.iter().map(|(_, r)| r.height()).collect();
    let mean = heights.iter().sum::<f32>() / heights.len() as f32;
    for h in &heights {
        assert!((h - mean).abs() < 5.0, "rows must be equal: {heights:?}");
    }
    assert!(rects[0].1.min.y <= area.min.y + 0.01);
    assert!(rects[4].1.max.y >= area.max.y - 0.01);

    // Grid: three columns over two rows for five panes.
    let mut tree = build();
    tree.even_out(EvenMode::Grid);
    let rects = pane_rects(&tree, area);
    assert_eq!(rects.len(), 5);
    // The first three share a row.
    assert!((rects[0].1.min.y - rects[1].1.min.y).abs() < 0.01);
    assert!((rects[0].1.height() - rects[1].1.height()).abs() < 0.01);
    assert!((rects[0].1.width() - rects[1].1.width()).abs() < 5.0);
    // The last row sits below the first, at the same height.
    assert!(rects[3].1.min.y > rects[0].1.min.y);
    assert!((rects[0].1.height() - rects[3].1.height()).abs() < 5.0);

    // A single pane is left alone.
    let mut single = PaneNode::Leaf(0);
    single.even_out(EvenMode::Grid);
    assert_eq!(single.leaves(), vec![0]);
}

#[test]
fn even_command_parses_with_modes() {
    use super::EvenMode;
    use super::eggs::{InternalCmd, parse_internal};
    assert_eq!(
        parse_internal(":even"),
        Some(InternalCmd::Even(EvenMode::Columns))
    );
    assert_eq!(
        parse_internal(":even rows"),
        Some(InternalCmd::Even(EvenMode::Rows))
    );
    assert_eq!(
        parse_internal(":even grid"),
        Some(InternalCmd::Even(EvenMode::Grid))
    );
    assert!(matches!(
        parse_internal(":even sideways"),
        Some(InternalCmd::Usage(_))
    ));
}

#[test]
fn bench_command_parses_with_and_without_seconds() {
    use super::eggs::{InternalCmd, parse_internal};
    assert_eq!(parse_internal(":bench"), Some(InternalCmd::Bench(10.0)));
    assert_eq!(parse_internal(":bench 30"), Some(InternalCmd::Bench(30.0)));
    // Clamped to a sane window.
    assert_eq!(
        parse_internal(":bench 999"),
        Some(InternalCmd::Bench(120.0))
    );
    assert_eq!(parse_internal(":bench 0.2"), Some(InternalCmd::Bench(1.0)));
    // A bad argument explains itself instead of running.
    assert!(matches!(
        parse_internal(":bench soon"),
        Some(InternalCmd::Usage(_))
    ));
}

#[test]
fn bench_command_line_quotes_and_leaves_a_row_for_the_cursor() {
    use super::eggs::bench_command_line;
    assert_eq!(
        bench_command_line("/usr/bin/cl2metal", 8.0, 100, 30),
        "/usr/bin/cl2metal --bench-frames 8 100 29"
    );
    // A path with spaces is quoted so the shell sees one word.
    let line = bench_command_line("/home/me/my apps/cl2metal", 5.0, 80, 24);
    assert!(
        line.starts_with("\"/home/me/my apps/cl2metal\" --bench-frames 5 80 23"),
        "{line}"
    );
    // A one-row pane still gets a drawable grid.
    let line = bench_command_line("cl2metal", 1.0, 20, 1);
    assert!(line.ends_with(" 20 1"), "{line}");
}

#[test]
fn bench_refuses_remote_and_dead_panes() {
    use super::eggs::bench_refusal;
    // A local, live pane is the only place the child exists.
    assert_eq!(bench_refusal(TabKind::Local, false), None);
    assert!(
        bench_refusal(TabKind::Ssh, false).is_some_and(|r| r.contains("local pane")),
        "an SSH pane must be refused"
    );
    assert!(
        bench_refusal(TabKind::Local, true).is_some_and(|r| r.contains("no live session")),
        "a dead pane must be refused"
    );
}

#[test]
fn internal_commands_are_refused_outside_local_panes() {
    use super::eggs::internal_refusal;
    // The internal namespace drives this app, so only local panes run it.
    assert_eq!(internal_refusal(TabKind::Local), None);
    assert!(
        internal_refusal(TabKind::Ssh).is_some_and(|r| r.contains("local pane")),
        "an SSH pane must be refused the way `:bench` is"
    );
}

#[test]
fn bench_summary_reports_fps_and_the_worst_frame() {
    use super::eggs::{Bench, bench_summary};
    let bench = Bench {
        started: 0.0,
        until: 10.0,
        last: 10.0,
        frames: 600,
        worst_ms: 41.5,
        sum_ms: 16.6 * 600.0,
        worst_logic_ms: 3.2,
        sum_logic_ms: 1.1 * 600.0,
        worst_ui_ms: 9.4,
        sum_ui_ms: 4.2 * 600.0,
    };
    let line = bench_summary(&bench, 10.0);
    assert!(line.contains("600 frames in 10.0s (60.0 fps)"), "{line}");
    assert!(line.contains("frame 16.6/41.5 ms"), "{line}");
    assert!(line.contains("logic 1.1/3.2 ms"), "{line}");
    assert!(line.contains("ui 4.2/9.4 ms"), "{line}");
    // A zero-length window must not divide by zero.
    let empty = Bench { frames: 0, ..bench };
    let line = bench_summary(&empty, 0.0);
    assert!(line.contains("0 frames"), "{line}");
}

#[test]
fn cpu_percent_math() {
    use super::sysinfo::cpu_percent;
    // 100 ticks (one second of CPU at USER_HZ) over one second of wall clock.
    assert_eq!(cpu_percent((1000, 0.0), (1100, 1.0)), Some(100.0));
    // Half a second of CPU over a second.
    assert_eq!(cpu_percent((0, 0.0), (50, 1.0)), Some(50.0));
    // Idle.
    assert_eq!(cpu_percent((100, 1.0), (100, 2.0)), Some(0.0));
    // Too short an interval to measure.
    assert_eq!(cpu_percent((100, 0.0), (100, 0.01)), None);
    // A counter that went backwards (process restart) must not go negative.
    assert_eq!(cpu_percent((500, 0.0), (100, 1.0)), Some(0.0));
}

#[test]
fn process_readers_see_this_process() {
    use super::sysinfo::{mib, process_cpu_ticks, process_memory_line, process_rss_kib};
    // The test binary is a process too, so these must all resolve.
    assert!(process_rss_kib().is_some_and(|rss| rss > 0));
    // A thread count needs a Toolhelp32 snapshot on Windows.
    #[cfg(unix)]
    {
        use super::sysinfo::process_threads;
        assert!(process_threads().is_some_and(|t| t >= 1));
    }
    assert!(process_cpu_ticks().is_some());
    let line = process_memory_line();
    assert!(line.contains("MiB"), "memory line: {line}");
    assert_eq!(mib(1024), "1 MiB");
    assert_eq!(mib(0), "0 MiB");
}

#[test]
fn sysinfo_reads_the_machine() {
    use super::sysinfo::{cpu_line, gib, memory_line, os_line, uptime_line};
    // /proc is always there on Linux, so these must produce real values.
    let cpu = cpu_line();
    assert!(cpu.contains("threads"), "cpu line: {cpu}");
    assert!(cpu.contains('('), "cpu line: {cpu}");
    let mem = memory_line();
    assert!(
        mem.contains(" / ") && mem.ends_with("GiB"),
        "mem line: {mem}"
    );
    assert_ne!(uptime_line(), "unknown");
    assert!(!os_line().is_empty());
    // Byte maths: 1 GiB of KiB is 1.0 GiB.
    assert_eq!(gib(1024 * 1024), "1.0 GiB");
    assert_eq!(gib(0), "0.0 GiB");
}

#[test]
#[cfg(unix)]
fn cpu_model_strips_the_colon_after_the_tab() {
    use super::sysinfo::cpu_model;
    // Real /proc/cpuinfo separates the key and the value with a tab.
    let text = "processor\t: 0\nmodel name\t: AMD Ryzen 7 7445HS w/ Radeon 740M Graphics\n";
    assert_eq!(
        cpu_model(text),
        "AMD Ryzen 7 7445HS w/ Radeon 740M Graphics"
    );
    // No model line at all.
    assert_eq!(cpu_model("processor\t: 0\n"), "unknown CPU");
    assert_eq!(cpu_model(""), "unknown CPU");
}

#[test]
fn paste_guard_only_stops_multi_line_pastes_that_run() {
    use super::events::paste_needs_confirm;
    // A single command with its trailing newline is the normal case.
    assert!(!paste_needs_confirm("ls -la\n"));
    assert!(!paste_needs_confirm("ls -la"));
    assert!(!paste_needs_confirm(""));
    // Several lines that end in a newline would run the last one.
    assert!(paste_needs_confirm("cd /tmp\nls\n"));
    assert!(paste_needs_confirm("a\nb\n\n"));
    assert!(paste_needs_confirm("a\r\nb\r\n"));
    // Several lines without a trailing newline are safe to paste.
    assert!(!paste_needs_confirm("cd /tmp\nls"));
}

#[test]
fn hue_rotation_keeps_the_colour_identity() {
    let base = [122, 162, 247];
    // A full turn comes back to where it started.
    for spun in [hue_rotate(base, 0.0), hue_rotate(base, 360.0)] {
        for (a, b) in base.iter().zip(spun.iter()) {
            assert!((*a as i32 - *b as i32).abs() <= 1, "{base:?} -> {spun:?}");
        }
    }
    // Rotating keeps the value (the brightest channel) intact.
    let spun = hue_rotate(base, 120.0);
    assert_eq!(spun.iter().max(), base.iter().max());
    // Grey has no hue to rotate.
    assert_eq!(hue_rotate([128, 128, 128], 90.0), [128, 128, 128]);
}

#[test]
fn matrix_glyphs_are_deterministic_and_printable() {
    assert_eq!(matrix_char(3, 2, 1.5), matrix_char(3, 2, 1.5));
    for col in 0..64 {
        for step in 0..9 {
            let ch = matrix_char(col, step, 0.25);
            assert!(ch.is_ascii_graphic(), "unexpected glyph {ch:?}");
        }
    }
}

#[test]
fn palette_eggs_stay_hidden_until_named() {
    assert!(hidden_eggs("").is_empty());
    assert!(hidden_eggs("settings").is_empty());
    assert_eq!(hidden_eggs("matrix"), vec![EasterEgg::Matrix]);
    assert_eq!(hidden_eggs("42"), vec![EasterEgg::Answer]);
    assert_eq!(hidden_eggs("party"), vec![EasterEgg::Party]);
}

#[test]
fn background_alpha_keeps_the_chrome_readable() {
    // Wallpaper is intentionally capped below full transparency so the
    // terminal remains legible over a light or detailed photograph.
    assert_eq!(bg_alpha_for(0.0), 1.0);
    assert_eq!(bg_alpha_for(1.0), 0.4);
    assert!((bg_alpha_for(0.35) - 0.79).abs() < 1e-6);

    // The chrome has an even stronger contrast floor.
    assert_eq!(ui_alpha_for(0.0), 1.0);
    assert!(ui_alpha_for(1.0) >= 0.78);
    assert!(ui_alpha_for(0.35) > bg_alpha_for(0.35));
    for step in 0..=10 {
        let a = ui_alpha_for(step as f32 / 10.0);
        assert!((0.78..=1.0).contains(&a), "chrome alpha out of range: {a}");
    }
}

#[test]
fn side_panels_let_the_wallpaper_through() {
    // The large side surfaces are a wash over the shared backdrop, not a wall:
    // the backdrop already carries the surface tint, so a panel only has to
    // separate itself from the text area. A nearly opaque panel is what hid
    // the wallpaper behind a 240 px column when SSH mode or the editor came up.
    assert_eq!(panel_alpha_for(0.0), 0.30);
    assert_eq!(panel_alpha_for(1.0), 0.10);
    assert!((panel_alpha_for(0.35) - 0.195).abs() < 1e-6);
    for step in 0..=10 {
        let o = step as f32 / 10.0;
        let a = panel_alpha_for(o);
        assert!((0.10..=0.30).contains(&a), "panel alpha out of range: {a}");
        assert!(
            1.0 - a >= 0.70,
            "the wash must leave most of the backdrop showing: {a}"
        );
        assert!(
            a < ui_alpha_for(o),
            "a panel must be lighter than the chrome: {a}"
        );
    }
}

/// Tabby's tab transition: `flex-basis`/`width` animates 1px -> full size
/// on enter and full size -> 0 on leave.
#[test]
fn tab_animation_widths_match_tabbys_transition() {
    assert_eq!(tab_enter_width(200.0, 0.0), 1.0);
    assert_eq!(tab_enter_width(200.0, 1.0), 200.0);
    let mut last = 0.0;
    for step in 0..=10 {
        let w = tab_enter_width(200.0, step as f32 / 10.0);
        assert!(w >= last, "the tab must only grow: {w} < {last}");
        last = w;
    }

    assert_eq!(tab_leave_width(200.0, 0.0), 200.0);
    assert_eq!(tab_leave_width(200.0, 1.0), 0.0);
    let mut last = 200.0;
    for step in 0..=10 {
        let w = tab_leave_width(200.0, step as f32 / 10.0);
        assert!(w <= last, "the tab must only shrink: {w} > {last}");
        last = w;
    }

    // Progress outside 0..=1 is clamped, never negative or overshooting.
    assert_eq!(tab_enter_width(200.0, -1.0), 1.0);
    assert_eq!(tab_enter_width(200.0, 2.0), 200.0);
    assert_eq!(tab_leave_width(200.0, -1.0), 200.0);
    assert_eq!(tab_leave_width(200.0, 2.0), 0.0);
}

#[test]
fn tab_animation_easings_are_normalised_and_monotonic() {
    for f in [ease_out_cubic, ease_in_out_cubic] {
        assert_eq!(f(0.0), 0.0);
        assert_eq!(f(1.0), 1.0);
        assert_eq!(f(-1.0), 0.0);
        assert_eq!(f(2.0), 1.0);
        let mut last = 0.0;
        for step in 0..=20 {
            let v = f(step as f32 / 20.0);
            assert!(v >= last - 1e-6, "easing must be monotonic");
            last = v;
        }
    }
    // Ease-in-out is symmetric around the midpoint.
    assert!((ease_in_out_cubic(0.5) - 0.5).abs() < 1e-6);
}

#[test]
fn background_cache_round_trips_and_rejects_junk() {
    let dir = std::env::temp_dir().join("cl2metal-test-cache");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("cache.rgba");
    let pixels = vec![7u8; 4 * 3 * 2];

    write_cache(&path, 3, 2, &pixels);
    let (back, w, h) = read_cache(&path).expect("read");
    assert_eq!((w, h), (3, 2));
    assert_eq!(back, pixels);

    // A truncated or foreign file is rejected instead of trusted.
    std::fs::write(&path, b"nope").expect("write");
    assert!(read_cache(&path).is_err());

    let _ = std::fs::remove_dir_all(&dir);
}
