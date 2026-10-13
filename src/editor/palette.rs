//! The editor's command palette: one list of everything runnable.

use super::command::Command;
use super::keymap::format_sequence;
use std::path::PathBuf;

/// What choosing an item does.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum PaletteAction {
    /// Run an editor command.
    Command(Command),
    /// Open a file.
    OpenPath(PathBuf),
    /// Jump to a line.
    GotoLine(usize),
    /// Switch to a buffer.
    SwitchBuffer(usize),
    /// Open the keybinding editor.
    Keybindings,
    /// Open the settings.
    Settings,
    /// Split the focused pane.
    Split(super::SplitDir),
}

/// One row of the palette.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct PaletteItem {
    pub(crate) label: String,
    /// The key sequence, or a short explanation.
    pub(crate) detail: String,
    pub(crate) action: PaletteAction,
}

/// The palette's state.
#[derive(Clone, Debug)]
pub(crate) struct Palette {
    pub(crate) query: String,
    pub(crate) items: Vec<PaletteItem>,
    pub(crate) selected: usize,
}

impl Palette {
    /// A palette listing every bound command, plus the file actions.
    pub(crate) fn new(editor: &super::Editor) -> Self {
        let mut items: Vec<PaletteItem> = editor
            .keymap
            .bindings()
            .iter()
            .filter_map(|binding| {
                let command = Command::from_name(&binding.command)?;
                Some(PaletteItem {
                    label: humanise(&binding.command),
                    detail: format_sequence(&binding.keys),
                    action: PaletteAction::Command(command),
                })
            })
            .collect();
        // Commands that are useful but need not be bound.
        for (name, detail) in [
            ("save_as", "write to a new path"),
            ("new_scratch", "an empty buffer"),
            ("reopen_from_disk", "discard changes and reload"),
            ("find_in_files", "search the tree"),
            ("format_document", "ask the language server"),
            ("hover", "show the symbol under the cursor"),
            ("completion", "complete at the cursor"),
        ] {
            if let Some(command) = Command::from_name(name) {
                items.push(PaletteItem {
                    label: humanise(name),
                    detail: detail.to_owned(),
                    action: PaletteAction::Command(command),
                });
            }
        }
        items.push(PaletteItem {
            label: "Split right".to_owned(),
            detail: "Ctrl+Shift+D".to_owned(),
            action: PaletteAction::Split(super::SplitDir::Right),
        });
        items.push(PaletteItem {
            label: "Split down".to_owned(),
            detail: "Ctrl+Shift+E".to_owned(),
            action: PaletteAction::Split(super::SplitDir::Down),
        });
        items.push(PaletteItem {
            label: "Keybindings".to_owned(),
            detail: "edit shortcuts".to_owned(),
            action: PaletteAction::Keybindings,
        });
        items.push(PaletteItem {
            label: "Settings".to_owned(),
            detail: "editor and terminal".to_owned(),
            action: PaletteAction::Settings,
        });
        // Open buffers.
        for (index, buffer) in editor.buffers.iter().enumerate() {
            items.push(PaletteItem {
                label: format!("Buffer: {}", buffer.title()),
                detail: buffer.location(),
                action: PaletteAction::SwitchBuffer(index),
            });
        }
        // Files in the tree, so the palette can open them too.
        for row in editor.tree.rows().iter().filter(|r| !r.dir).take(400) {
            items.push(PaletteItem {
                label: format!("Open: {}", row.name),
                detail: row.path.display().to_string(),
                action: PaletteAction::OpenPath(row.path.clone()),
            });
        }
        Self {
            query: String::new(),
            items,
            selected: 0,
        }
    }

    /// The items matching the query, best first.
    pub(crate) fn filtered(&self) -> Vec<&PaletteItem> {
        let query = self.query.trim().to_lowercase();
        if query.is_empty() {
            return self.items.iter().collect();
        }
        let mut scored: Vec<(i32, &PaletteItem)> = self
            .items
            .iter()
            .filter_map(|item| score(item, &query).map(|s| (s, item)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
        scored.into_iter().map(|(_, item)| item).collect()
    }

    /// The item the selection is on.
    pub(crate) fn current(&self) -> Option<PaletteItem> {
        let filtered = self.filtered();
        filtered
            .get(self.selected.min(filtered.len().saturating_sub(1)))
            .map(|i| (*i).clone())
    }

    /// Move the selection, clamped to the list.
    pub(crate) fn move_selection(&mut self, delta: i32) {
        let len = self.filtered().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let next = self.selected as i32 + delta;
        self.selected = next.clamp(0, len as i32 - 1) as usize;
    }
}

/// A crude relevance score: prefix beats substring beats subsequence.
fn score(item: &PaletteItem, query: &str) -> Option<i32> {
    let label = item.label.to_lowercase();
    let detail = item.detail.to_lowercase();
    if label.starts_with(query) {
        return Some(1000 - label.len() as i32);
    }
    if label.contains(query) {
        return Some(500 - label.len() as i32);
    }
    if detail.contains(query) {
        return Some(200);
    }
    // Subsequence match, so "gtl" finds "Go to line".
    let mut chars = query.chars();
    let mut wanted = chars.next();
    let mut hits = 0;
    for c in label.chars() {
        match wanted {
            Some(w) if w == c => {
                hits += 1;
                wanted = chars.next();
            }
            _ => {}
        }
    }
    if wanted.is_none() && hits > 0 {
        Some(100 + hits)
    } else {
        None
    }
}

/// `move_word_next` -> `Move word next`.
fn humanise(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut first = true;
    for word in name.split('_') {
        if !first {
            out.push(' ');
        }
        if first {
            let mut chars = word.chars();
            if let Some(c) = chars.next() {
                out.extend(c.to_uppercase());
                out.push_str(chars.as_str());
            }
            first = false;
        } else {
            out.push_str(word);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, detail: &str) -> PaletteItem {
        PaletteItem {
            label: label.to_owned(),
            detail: detail.to_owned(),
            action: PaletteAction::GotoLine(1),
        }
    }

    #[test]
    fn humanise_splits_snake_case() {
        assert_eq!(humanise("move_word_next"), "Move word next");
        assert_eq!(humanise("save"), "Save");
    }

    #[test]
    fn scoring_prefers_prefixes_then_substrings() {
        let prefix = item("Save file", "");
        let substring = item("Reopen from disk", "");
        assert!(score(&prefix, "save").unwrap() > score(&substring, "save").unwrap_or(0));
        assert!(score(&item("Go to line", ""), "line").is_some());
        assert!(score(&item("Go to line", ""), "zzz").is_none());
    }

    #[test]
    fn a_subsequence_still_matches() {
        let it = item("Go to line", "");
        assert!(score(&it, "gtl").is_some());
    }

    #[test]
    fn selection_is_clamped_to_the_list() {
        let mut palette = Palette {
            query: String::new(),
            items: vec![item("one", ""), item("two", "")],
            selected: 0,
        };
        palette.move_selection(-5);
        assert_eq!(palette.selected, 0);
        palette.move_selection(99);
        assert_eq!(palette.selected, 1);
    }
}
