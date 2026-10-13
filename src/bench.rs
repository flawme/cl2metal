//! The `:bench` spinning torus: a real end-to-end stress test.
//!
//! The frames are produced by a child process (this same binary, run with
//! `--bench-frames`), so a run exercises the whole path a real program takes:
//! child → PTY → reader thread → VT parser → grid → frame build → GPU. The app
//! measures its own frame intervals while the child runs and reports them when
//! it exits.

/// Luminance ramp, darkest to brightest (the classic donut.c ramp).
const RAMP: &[u8] = b".,-~:;=!*#$@";

/// One frame of the spinning torus as ANSI text: cursor home, then the grid.
///
/// `a` rotates around the x axis, `b` around the z axis.
pub fn frame(a: f32, b: f32, cols: usize, rows: usize) -> String {
    use std::fmt::Write as _;
    let cols = cols.max(8);
    let rows = rows.max(4);
    let mut zbuf = vec![0.0f32; cols * rows];
    let mut cells = vec![b' '; cols * rows];
    let (cx, cy) = (cols as f32 / 2.0, rows as f32 / 2.0);
    // The classic projection is tuned for 80x22; scale it to the grid.
    let (sx, sy) = (cols as f32 / 80.0, rows as f32 / 22.0);
    let (sa, ca) = a.sin_cos();
    let (sb, cb) = b.sin_cos();

    let mut j = 0.0f32;
    while j < std::f32::consts::TAU {
        let (sj, cj) = j.sin_cos();
        let mut i = 0.0f32;
        while i < std::f32::consts::TAU {
            let (si, ci) = i.sin_cos();
            let h = cj + 2.0;
            let d = 1.0 / (si * h * sa + sj * ca + 5.0);
            let t = si * h * ca - sj * sa;
            let x = cx + 30.0 * sx * d * (ci * h * cb - t * sb);
            let y = cy + 15.0 * sy * d * (ci * h * sb + t * cb);
            let n = 8.0 * ((sj * sa - si * cj * ca) * cb - si * cj * sa - sj * ca - ci * cj * sb);
            let (xi, yi) = (x as isize, y as isize);
            if xi >= 0 && (xi as usize) < cols && yi >= 0 && (yi as usize) < rows {
                let idx = yi as usize * cols + xi as usize;
                if d > zbuf[idx] {
                    zbuf[idx] = d;
                    let lum = (n.max(0.0) as usize).min(RAMP.len() - 1);
                    cells[idx] = RAMP[lum];
                }
            }
            i += 0.02;
        }
        j += 0.07;
    }

    let mut out = String::with_capacity(cols * rows * 14);
    out.push_str("\x1b[H");
    for y in 0..rows {
        for x in 0..cols {
            let ch = cells[y * cols + x];
            if ch == b' ' {
                out.push(' ');
                continue;
            }
            // A 256-colour ramp, so the frame is dense with escape sequences:
            // that is the point of a benchmark.
            let lum = RAMP.iter().position(|c| *c == ch).unwrap_or(0);
            let color = 17 + (lum * 12) as u8;
            let _ = write!(out, "\x1b[38;5;{color}m{}", ch as char);
        }
        out.push_str("\x1b[0m\r\n");
    }
    out
}

/// Print the animation to stdout for `secs` seconds, paced to ~60 fps.
pub fn run_stdout(secs: f32, cols: usize, rows: usize) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    // Hide the cursor and clear the screen.
    out.write_all(b"\x1b[?25l\x1b[2J")?;
    let start = std::time::Instant::now();
    let mut frames = 0u64;
    let (mut a, mut b) = (0.0f32, 0.0f32);
    let target = std::time::Duration::from_micros(16_666);
    while start.elapsed().as_secs_f32() < secs {
        let frame_start = std::time::Instant::now();
        out.write_all(frame(a, b, cols, rows).as_bytes())?;
        out.flush()?;
        frames += 1;
        a += 0.09;
        b += 0.045;
        let spent = frame_start.elapsed();
        if spent < target {
            std::thread::sleep(target - spent);
        }
    }
    let elapsed = start.elapsed().as_secs_f32();
    let _ = write!(
        out,
        "\x1b[?25h\x1b[0m\r\nbench: {frames} frames in {elapsed:.1}s ({:.1} fps) · {cols}x{rows}\r\n",
        frames as f32 / elapsed.max(0.001)
    );
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_fills_the_grid_and_homes_the_cursor() {
        let f = frame(0.5, 1.0, 40, 12);
        assert!(f.starts_with("\x1b[H"), "must home the cursor first");
        // One row per grid row.
        assert_eq!(f.matches("\x1b[0m\r\n").count(), 12);
        // The torus is drawn: the ramp appears.
        assert!(f.contains('@') || f.contains('#') || f.contains('*'), "{f}");
        // Every line is at most the grid width plus its escapes.
        for line in f.split("\r\n") {
            let visible = line.matches(|c: char| c != '\x1b').count();
            assert!(visible <= 40 * 12, "line too long");
        }
    }

    #[test]
    fn rotation_changes_the_frame() {
        let a = frame(0.0, 0.0, 40, 12);
        let b = frame(1.2, 0.4, 40, 12);
        assert_ne!(a, b, "the shape must move");
    }

    #[test]
    fn tiny_grids_do_not_panic() {
        // A pane can be very small; the generator must cope.
        for (cols, rows) in [(1, 1), (8, 4), (200, 60)] {
            let f = frame(0.3, 0.7, cols, rows);
            assert!(!f.is_empty());
        }
    }
}
