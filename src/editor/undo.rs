//! Undo and redo: line-range replacements with mergeable runs.
//!
//! Every mutation is expressed as one or more line-range replacements, so undo
//! is a splice and needs no inverse-operation logic. A multi-cursor edit is a
//! single step with several parts, which is what makes `Ctrl+Z` after editing
//! ten cursors behave the way people expect.

use super::pos::Cursor;

/// One line-range replacement: `before` lines become `after` lines at `at`.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Part {
    pub(crate) at: usize,
    pub(crate) before: Vec<String>,
    pub(crate) after: Vec<String>,
}

/// One undo step: one or more replacements, plus the cursor state around them.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Edit {
    /// Replacements, ordered by ascending `at`.
    pub(crate) parts: Vec<Part>,
    pub(crate) cursors_before: Vec<Cursor>,
    pub(crate) cursors_after: Vec<Cursor>,
}

impl Edit {
    /// Whether `next` continues this edit: both are single-part edits on the
    /// same line, and the new one starts from the previous one's result.
    fn continues(&self, next: &Edit) -> bool {
        if self.parts.len() != 1 || next.parts.len() != 1 {
            return false;
        }
        let (a, b) = (&self.parts[0], &next.parts[0]);
        b.at == a.at && b.before == a.after
    }
}

/// The undo history: a stack with a redo tail.
pub(crate) struct UndoStack {
    edits: Vec<Edit>,
    /// One past the last applied edit; anything after it is the redo tail.
    at: usize,
    /// Whether the next push may merge into the previous edit.
    merge: bool,
    /// Pushes collected into one step by `begin_group`.
    group: Option<Edit>,
    /// Identity of each content state: `states[i]` is the state reached by
    /// applying `edits[..i]`. A fresh id is minted whenever the content at a
    /// position changes, so an edit that merges into an existing step still
    /// counts as a change even though the position does not move.
    states: Vec<u64>,
    /// The next state id to mint.
    next_state: u64,
    /// The state the file was last written at, so undoing back to it can
    /// clear the modified marker.
    saved: u64,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self {
            edits: Vec::new(),
            at: 0,
            merge: false,
            group: None,
            states: vec![0],
            next_state: 1,
            saved: 0,
        }
    }
}

impl UndoStack {
    /// Remember the current content as the saved one.
    pub(crate) fn mark_saved(&mut self) {
        self.saved = self.states[self.at];
    }

    /// Whether the buffer differs from what was last written.
    pub(crate) fn is_dirty(&self) -> bool {
        self.states[self.at] != self.saved
    }

    /// An id for a state whose content just changed.
    fn fresh_state(&mut self) -> u64 {
        let id = self.next_state;
        self.next_state += 1;
        id
    }

    /// Start collecting pushes into a single undo step.
    ///
    /// A multi-cursor edit is applied one cursor at a time (so each edit sees
    /// the previous one's result), and this is what makes the whole run undo
    /// together.
    pub(crate) fn begin_group(&mut self) {
        self.group = Some(Edit {
            parts: Vec::new(),
            cursors_before: Vec::new(),
            cursors_after: Vec::new(),
        });
    }

    /// Close the group and push it as one step.
    pub(crate) fn end_group(&mut self) {
        let Some(group) = self.group.take() else {
            return;
        };
        if group.parts.is_empty() {
            return;
        }
        if group.parts.len() == 1 {
            // A single edit: go through the normal path so the merge rules
            // still apply and a run of typing stays one undo step.
            let edit = Edit {
                parts: group.parts,
                cursors_before: group.cursors_before,
                cursors_after: group.cursors_after,
            };
            self.push(edit, true);
            return;
        }
        self.edits.truncate(self.at);
        self.states.truncate(self.at + 1);
        self.edits.push(group);
        self.at = self.edits.len();
        let id = self.fresh_state();
        self.states.push(id);
        self.merge = false;
    }

    /// Record an edit. `merge` allows it to join the previous step.
    pub(crate) fn push(&mut self, edit: Edit, merge: bool) {
        if let Some(group) = &mut self.group {
            if group.parts.is_empty() {
                group.cursors_before = edit.cursors_before.clone();
            }
            group.parts.extend(edit.parts);
            group.cursors_after = edit.cursors_after;
            return;
        }
        // A fresh edit invalidates the redo tail.
        self.edits.truncate(self.at);
        self.states.truncate(self.at + 1);
        if merge
            && self.merge
            && let Some(last) = self.edits.last_mut()
            && last.continues(&edit)
        {
            last.parts[0].after = edit.parts[0].after.clone();
            last.cursors_after = edit.cursors_after;
            self.at = self.edits.len();
            // The step at this position now holds different content, so the
            // state it leads to is a new one.
            let id = self.fresh_state();
            self.states[self.at] = id;
            return;
        }
        self.edits.push(edit);
        self.at = self.edits.len();
        let id = self.fresh_state();
        self.states.push(id);
        self.merge = merge;
    }

    /// End the current run, so the next edit starts a new undo step.
    pub(crate) fn break_run(&mut self) {
        self.merge = false;
    }

    /// Step back.
    pub(crate) fn undo(&mut self) -> Option<Edit> {
        if self.at == 0 {
            return None;
        }
        self.at -= 1;
        self.merge = false;
        Some(self.edits[self.at].clone())
    }

    /// Step forward again.
    pub(crate) fn redo(&mut self) -> Option<Edit> {
        if self.at >= self.edits.len() {
            return None;
        }
        let edit = self.edits[self.at].clone();
        self.at += 1;
        self.merge = false;
        Some(edit)
    }

    /// Steps available in each direction, for the status line.
    pub(crate) fn depth(&self) -> (usize, usize) {
        (self.at, self.edits.len() - self.at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::pos::Pos;

    fn part(at: usize, before: &[&str], after: &[&str]) -> Part {
        Part {
            at,
            before: before.iter().map(|s| (*s).to_owned()).collect(),
            after: after.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn edit(at: usize, before: &[&str], after: &[&str]) -> Edit {
        Edit {
            parts: vec![part(at, before, after)],
            cursors_before: vec![Cursor::new(Pos::new(at, 0))],
            cursors_after: vec![Cursor::new(Pos::new(at, 1))],
        }
    }

    #[test]
    fn typing_merges_into_one_step() {
        let mut stack = UndoStack::default();
        stack.push(edit(0, &[""], &["a"]), true);
        stack.push(edit(0, &["a"], &["ab"]), true);
        stack.push(edit(0, &["ab"], &["abc"]), true);
        assert_eq!(stack.depth(), (1, 0), "three keystrokes are one undo step");
        let undone = stack.undo().unwrap();
        assert_eq!(undone.parts[0].before, vec![""]);
        assert_eq!(undone.parts[0].after, vec!["abc"]);
    }

    #[test]
    fn a_break_starts_a_new_step() {
        let mut stack = UndoStack::default();
        stack.push(edit(0, &[""], &["a"]), true);
        stack.break_run();
        stack.push(edit(0, &["a"], &["ab"]), true);
        assert_eq!(stack.depth(), (2, 0));
    }

    #[test]
    fn merging_into_the_saved_step_is_still_a_change() {
        let mut stack = UndoStack::default();
        stack.push(edit(0, &[""], &["a"]), true);
        stack.mark_saved();
        assert!(!stack.is_dirty());
        stack.push(edit(0, &["a"], &["ab"]), true);
        assert_eq!(stack.depth(), (1, 0), "the push merged");
        assert!(stack.is_dirty(), "the merged step holds new content");
    }

    #[test]
    fn undo_then_redo_round_trips() {
        let mut stack = UndoStack::default();
        stack.push(edit(0, &["one"], &["two"]), false);
        assert_eq!(stack.undo().unwrap().parts[0].before, vec!["one"]);
        assert_eq!(stack.depth(), (0, 1));
        assert_eq!(stack.redo().unwrap().parts[0].after, vec!["two"]);
        assert_eq!(stack.depth(), (1, 0));
        assert!(stack.redo().is_none());
        assert!(stack.undo().is_some());
        assert!(stack.undo().is_none());
    }

    #[test]
    fn a_new_edit_drops_the_redo_tail() {
        let mut stack = UndoStack::default();
        stack.push(edit(0, &["one"], &["two"]), false);
        stack.undo();
        assert_eq!(stack.depth(), (0, 1));
        stack.push(edit(0, &["one"], &["three"]), false);
        assert_eq!(stack.depth(), (1, 0), "the redo tail is gone");
    }

    #[test]
    fn unrelated_edits_do_not_merge() {
        let mut stack = UndoStack::default();
        stack.push(edit(0, &["a"], &["ab"]), true);
        stack.push(edit(5, &["x"], &["xy"]), true);
        assert_eq!(stack.depth(), (2, 0));
    }

    #[test]
    fn a_multi_part_edit_is_one_step_and_never_merges() {
        let mut stack = UndoStack::default();
        let multi = Edit {
            parts: vec![part(0, &["a"], &["A"]), part(9, &["b"], &["B"])],
            cursors_before: vec![Cursor::new(Pos::new(0, 0)), Cursor::new(Pos::new(9, 0))],
            cursors_after: vec![Cursor::new(Pos::new(0, 1)), Cursor::new(Pos::new(9, 1))],
        };
        stack.push(multi.clone(), true);
        assert_eq!(stack.depth(), (1, 0));
        // A following single-part edit must not fold into it.
        stack.push(edit(0, &["A"], &["Ax"]), true);
        assert_eq!(stack.depth(), (2, 0));
        assert_eq!(stack.undo().unwrap().parts.len(), 1);
        assert_eq!(stack.undo().unwrap().parts.len(), 2);
    }
}
