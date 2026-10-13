//! Scrollback search: compiling the query, collecting matches, and
//! mapping them onto the visible grid.

use super::*;

impl App {
    pub(crate) fn toggle_search(&mut self) {
        self.search = match self.search.take() {
            Some(_) => None,
            None => Some(SearchState {
                query: String::new(),
                compiled_for: String::new(),
                regex: None,
                error: None,
                matches: Vec::new(),
                current: 0,
            }),
        };
    }

    /// Recompile the query and collect matches when it changed.
    pub(crate) fn refresh_search(&mut self) {
        let Some(mut state) = self.search.take() else {
            return;
        };
        if state.compiled_for != state.query {
            state.compiled_for = state.query.clone();
            state.matches.clear();
            state.current = 0;
            state.error = None;
            state.regex = None;
            if !state.query.is_empty() {
                match RegexSearch::new(&state.query) {
                    Ok(mut regex) => {
                        let matches = self
                            .tabs
                            .get(self.active)
                            .map(|t| t.core.search_collect(&mut regex, SEARCH_CAP))
                            .unwrap_or_default();
                        state.matches = matches;
                        state.regex = Some(regex);
                    }
                    Err(e) => state.error = Some(e.to_string()),
                }
            }
            // Jump to the first match of a fresh query.
            if let Some(first) = state.matches.first().map(|m| *m.start())
                && let Some(tab) = self.tabs.get_mut(self.active)
            {
                tab.core.scroll_to(first);
            }
        }
        self.search = Some(state);
    }

    /// Move to the next (or previous) match and scroll it into view.
    pub(crate) fn search_step(&mut self, forward: bool) {
        self.refresh_search();
        let Some(mut state) = self.search.take() else {
            return;
        };
        let count = state.matches.len();
        if count > 0 {
            state.current = if forward {
                (state.current + 1) % count
            } else {
                (state.current + count - 1) % count
            };
            let start = *state.matches[state.current].start();
            if let Some(tab) = self.tabs.get_mut(self.active) {
                tab.core.scroll_to(start);
            }
        }
        self.search = Some(state);
    }

    /// Cells to highlight for the active search, limited to the viewport.
    pub(crate) fn search_cells(&self) -> Option<render::SearchCells> {
        let state = self.search.as_ref()?;
        if state.matches.is_empty() {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
        let offset = tab.core.term.grid().display_offset() as i32;
        let rows = tab.core.rows() as i32;
        let last_col = tab.core.cols().saturating_sub(1);
        let mut cells = render::SearchCells::new();
        for (i, m) in state.matches.iter().enumerate() {
            let current = i == state.current;
            let (start, end) = (*m.start(), *m.end());
            for line in start.line.0..=end.line.0 {
                let view_line = line + offset;
                if view_line < 0 || view_line >= rows {
                    continue;
                }
                let from = if line == start.line.0 {
                    start.column.0
                } else {
                    0
                };
                let to = if line == end.line.0 {
                    end.column.0.min(last_col)
                } else {
                    last_col
                };
                for col in from..=to {
                    cells
                        .entry((line, col))
                        .and_modify(|v| *v |= current)
                        .or_insert(current);
                }
            }
        }
        Some(cells)
    }
}
