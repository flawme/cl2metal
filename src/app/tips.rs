//! Startup tips: one short hint per launch, picked at random.
//!
//! The first launch after an install always shows the drag-an-image tip,
//! because that is the feature nobody finds on their own. Every launch after
//! it picks from the pool, so the hints rotate instead of repeating.

/// Every tip the app can show. Keep them one line: the banner does not wrap.
pub(crate) const TIPS: &[&str] = &[
    "drag an image onto the window to change the background",
    "Ctrl+Shift+T opens a new tab in the current directory",
    "Ctrl+Shift+O switches between the terminal and the editor",
    "Ctrl+Shift+P opens the command palette",
    "Ctrl+Shift+F searches the scrollback",
    "Ctrl+Shift+R reopens the last closed tab",
    "Ctrl+Shift+D splits the pane side by side",
    "Ctrl+Shift+E splits the pane top and bottom",
    "Ctrl+= and Ctrl+- change the font size",
    "the editor's file tree follows the shell's cd",
    "type :info to see this machine's facts",
    "drop a file on the terminal to type its path",
];

/// The tip a fresh install sees first.
pub(crate) const FIRST_TIP: &str = TIPS[0];

/// Pick the tip for this launch.
///
/// `first_run` forces the drag-an-image tip; otherwise the seed (the clock,
/// in practice) chooses one, so two launches rarely show the same hint.
pub(crate) fn pick(first_run: bool, seed: u64) -> &'static str {
    if first_run {
        return FIRST_TIP;
    }
    TIPS[(seed % TIPS.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_install_gets_the_drag_image_tip() {
        assert_eq!(pick(true, 7), FIRST_TIP);
        assert!(
            FIRST_TIP.contains("drag an image"),
            "the first tip teaches the wallpaper drag: {FIRST_TIP:?}"
        );
    }

    #[test]
    fn every_seed_lands_on_a_real_tip() {
        for seed in 0..1000 {
            let tip = pick(false, seed);
            assert!(TIPS.contains(&tip), "seed {seed} picked {tip:?}");
            assert!(!tip.is_empty(), "seed {seed} picked an empty tip");
        }
    }

    #[test]
    fn consecutive_seeds_rotate_the_pool() {
        assert_ne!(
            pick(false, 0),
            pick(false, 1),
            "two launches in a row should not repeat the same tip"
        );
    }
}
