//! The file tree: a lazily expanded view of a directory.
//!
//! Reading happens on demand and the whole tree is rebuilt from the expanded
//! set, which keeps it correct when files appear or vanish underneath it
//! without needing a watcher.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One visible row of the tree.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct TreeRow {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) dir: bool,
    pub(crate) depth: usize,
    pub(crate) expanded: bool,
}

/// How deep the tree will read before giving up.
const MAX_DEPTH: usize = 24;

/// A question the tree is asking the user.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum TreePrompt {
    /// Create a file or a folder in this directory.
    New {
        dir: PathBuf,
        name: String,
        is_dir: bool,
    },
    /// Rename this entry.
    Rename { path: PathBuf, name: String },
    /// Delete this entry (a folder needs confirming).
    Delete { path: PathBuf, dir: bool },
}

/// The file tree.
pub(crate) struct FileTree {
    pub(crate) root: PathBuf,
    pub(crate) open: bool,
    pub(crate) show_hidden: bool,
    /// The row the user last clicked, for the per-row actions.
    pub(crate) selected: Option<PathBuf>,
    /// A question the tree is asking.
    pub(crate) prompt: Option<TreePrompt>,
    /// A row being dragged onto a folder: its path.
    pub(crate) drag: Option<PathBuf>,
    /// The folder row under the pointer while a drag is in flight.
    pub(crate) drag_target: Option<PathBuf>,
    /// Whether the root exists. A change either way rebuilds the rows, so a
    /// deleted root does not keep listing ghost entries and a recreated one
    /// comes back on its own.
    pub(crate) root_missing: bool,
    /// The column's width; the grip on its edge drags it wider or narrower.
    pub(crate) width: f32,
    rows: Vec<TreeRow>,
    expanded: HashSet<PathBuf>,
    /// Set when the rows need rebuilding.
    dirty: bool,
}

impl FileTree {
    pub(crate) fn new(root: PathBuf) -> Self {
        let mut tree = Self {
            root,
            open: true,
            show_hidden: false,
            selected: None,
            prompt: None,
            drag: None,
            drag_target: None,
            root_missing: false,
            width: 220.0,
            rows: Vec::new(),
            expanded: HashSet::new(),
            dirty: true,
        };
        tree.refresh();
        tree
    }

    /// Point the tree at another directory.
    pub(crate) fn set_root(&mut self, root: PathBuf) {
        if self.root != root {
            self.root = root;
            self.expanded.clear();
            self.dirty = true;
            self.refresh();
        }
    }

    /// The visible rows.
    pub(crate) fn rows(&self) -> &[TreeRow] {
        &self.rows
    }

    /// Rebuild the rows from the expanded set.
    pub(crate) fn refresh(&mut self) {
        self.dirty = false;
        let mut rows = Vec::new();
        self.walk(&self.root.clone(), 0, &mut rows);
        self.rows = rows;
    }

    /// Rebuild if something asked for it, or if the root appeared or vanished
    /// since the last rebuild: the cached rows would otherwise keep listing a
    /// folder that is no longer there.
    pub(crate) fn refresh_if_dirty(&mut self) {
        let missing = !self.root.exists();
        if self.dirty || missing != self.root_missing {
            self.root_missing = missing;
            self.refresh();
        }
    }

    /// Mark the tree stale, so the next frame re-reads it.
    pub(crate) fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn walk(&self, dir: &Path, depth: usize, out: &mut Vec<TreeRow>) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<(PathBuf, String, bool)> = Vec::new();
        for entry in read.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !self.show_hidden && name.starts_with('.') {
                continue;
            }
            let dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push((path, name, dir));
        }
        // Directories first, then names, case-insensitively. The lowercase key
        // is built once per entry: doing it inside the comparison allocates two
        // strings per comparison and dominates the walk on a large directory.
        entries.sort_by_cached_key(|(_, name, dir)| (!*dir, name.to_lowercase()));
        for (path, name, dir) in entries {
            let expanded = dir && self.expanded.contains(&path);
            out.push(TreeRow {
                path: path.clone(),
                name,
                dir,
                depth,
                expanded,
            });
            if expanded {
                self.walk(&path, depth + 1, out);
            }
        }
    }

    /// Expand or collapse a directory.
    ///
    /// Only the toggled directory is read. A full rebuild re-reads every
    /// expanded directory, which is tens of milliseconds on a large tree and
    /// is paid on every click; `refresh` still does the full walk, and the
    /// refresh button and the file operations use it.
    pub(crate) fn toggle(&mut self, path: &Path) {
        if self.expanded.contains(path) {
            self.expanded.remove(path);
            self.collapse(path);
        } else {
            self.expanded.insert(path.to_path_buf());
            self.expand(path);
        }
    }

    /// Drop the rows under a collapsed directory.
    fn collapse(&mut self, path: &Path) {
        let Some(at) = self.row_of(path) else {
            // Not visible: its parent is collapsed, so nothing to drop.
            self.refresh();
            return;
        };
        let depth = self.rows[at].depth;
        let end = self.rows[at + 1..]
            .iter()
            .position(|r| r.depth <= depth)
            .map_or(self.rows.len(), |i| at + 1 + i);
        self.rows.drain(at + 1..end);
        self.rows[at].expanded = false;
    }

    /// Read one directory and splice its rows in under its own row.
    fn expand(&mut self, path: &Path) {
        let Some(at) = self.row_of(path) else {
            self.refresh();
            return;
        };
        let depth = self.rows[at].depth;
        let mut children = Vec::new();
        self.walk(path, depth + 1, &mut children);
        self.rows.splice(at + 1..at + 1, children);
        self.rows[at].expanded = true;
    }

    /// Expand every ancestor of a path, so it becomes visible.
    ///
    /// Returns true when the expanded set changed, which is the only thing that
    /// makes the rows stale: a path that is already visible needs no rebuild,
    /// and opening a file that is already on screen is the common case.
    fn expand_ancestors(&mut self, path: &Path) -> bool {
        let mut changed = false;
        let mut current = path.parent();
        while let Some(dir) = current {
            if dir.starts_with(&self.root) {
                changed |= self.expanded.insert(dir.to_path_buf());
            }
            if dir == self.root {
                break;
            }
            current = dir.parent();
        }
        changed
    }

    /// Expand every ancestor of a path, so it becomes visible.
    pub(crate) fn reveal(&mut self, path: &Path) {
        if self.expand_ancestors(path) {
            self.refresh();
        }
    }

    /// The index of the row for a path, if it is visible.
    pub(crate) fn row_of(&self, path: &Path) -> Option<usize> {
        self.rows.iter().position(|r| r.path == path)
    }

    /// The directory a new entry should go in: the selected folder, the
    /// selected file's folder, or the root.
    pub(crate) fn target_dir(&self) -> PathBuf {
        match self.selected.as_deref() {
            Some(path) if path.is_dir() => path.to_path_buf(),
            Some(path) => path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        }
    }

    /// Create a file or a folder, and reveal it.
    pub(crate) fn create(
        &mut self,
        dir: &Path,
        name: &str,
        is_dir: bool,
    ) -> std::io::Result<PathBuf> {
        let name = name.trim();
        if name.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the name cannot be empty",
            ));
        }
        if name.contains('/') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the name cannot contain a slash",
            ));
        }
        let path = dir.join(name);
        if path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{name} already exists"),
            ));
        }
        if is_dir {
            std::fs::create_dir_all(&path)?;
        } else {
            std::fs::File::create(&path)?;
        }
        self.expanded.insert(dir.to_path_buf());
        self.selected = Some(path.clone());
        // The new entry must show even when nothing had to be expanded, so
        // this rebuilds rather than going through `reveal`, which skips the
        // rebuild when the path is already visible.
        self.expand_ancestors(&path);
        self.refresh();
        Ok(path)
    }

    /// Rename an entry in place.
    pub(crate) fn rename(&mut self, path: &Path, name: &str) -> std::io::Result<PathBuf> {
        let name = name.trim();
        if name.is_empty() || name.contains('/') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the name cannot be empty or contain a slash",
            ));
        }
        let parent = path.parent().unwrap_or(&self.root);
        let target = parent.join(name);
        if target.exists() && target != path {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{name} already exists"),
            ));
        }
        std::fs::rename(path, &target)?;
        self.selected = Some(target.clone());
        self.refresh();
        Ok(target)
    }

    /// Move an entry into another folder.
    ///
    /// Refuses to overwrite anything at the destination, and refuses to move a
    /// folder into itself or one of its own descendants: both can only lose
    /// data. Moving into the folder the entry already lives in is a no-op.
    pub(crate) fn mv(&mut self, from: &Path, to_dir: &Path) -> std::io::Result<PathBuf> {
        let Some(name) = from.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the entry has no name",
            ));
        };
        let target = to_dir.join(&name);
        if target == from {
            return Ok(target); // already there: nothing to do
        }
        if !from.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} is gone", from.display()),
            ));
        }
        if !to_dir.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the destination is not a folder",
            ));
        }
        if target.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{name} already exists there"),
            ));
        }
        if from.is_dir() && to_dir.starts_with(from) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a folder cannot move into itself",
            ));
        }
        std::fs::rename(from, &target)?;
        // Keep the destination folder and the moved entry in view.
        self.expanded.insert(to_dir.to_path_buf());
        self.selected = Some(target.clone());
        self.refresh();
        Ok(target)
    }

    /// Delete a file, or an empty-or-not folder, for good.
    ///
    /// This is the Shift+Delete path: it goes through the are-you-sure
    /// prompt first, because unlike a trash move there is no taking it
    /// back. Plain deletion goes through `Editor::trash_from_tree`.
    pub(crate) fn delete(&mut self, path: &Path, dir: bool) -> std::io::Result<()> {
        if dir {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
        if self.selected.as_deref() == Some(path) {
            self.selected = None;
        }
        self.refresh();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cl2metal-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("sub/inner.txt"), "i").unwrap();
        std::fs::write(dir.join(".hidden"), "h").unwrap();
        dir
    }

    #[test]
    fn directories_come_first_then_names() {
        let dir = scratch_dir("order");
        let tree = FileTree::new(dir.clone());
        let names: Vec<&str> = tree.rows().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["sub", "a.txt", "b.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hidden_files_are_left_out_until_asked_for() {
        let dir = scratch_dir("hidden");
        let mut tree = FileTree::new(dir.clone());
        assert!(!tree.rows().iter().any(|r| r.name == ".hidden"));
        tree.show_hidden = true;
        tree.refresh();
        assert!(tree.rows().iter().any(|r| r.name == ".hidden"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expanding_a_directory_shows_its_children() {
        let dir = scratch_dir("expand");
        let mut tree = FileTree::new(dir.clone());
        assert_eq!(tree.rows().len(), 3);
        tree.toggle(&dir.join("sub"));
        let names: Vec<&str> = tree.rows().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["sub", "inner.txt", "a.txt", "b.txt"]);
        let inner = tree.rows().iter().find(|r| r.name == "inner.txt").unwrap();
        assert_eq!(inner.depth, 1);
        assert!(!inner.dir);
        tree.toggle(&dir.join("sub"));
        assert_eq!(tree.rows().len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn toggling_incrementally_matches_a_full_rebuild() {
        let dir = scratch_dir("incremental");
        std::fs::create_dir_all(dir.join("sub/deep")).unwrap();
        std::fs::write(dir.join("sub/deep/deepest.txt"), "d").unwrap();
        let mut tree = FileTree::new(dir.clone());

        // Expanding reads only the toggled directory, so the rows it produces
        // must be exactly what a full walk would have produced.
        tree.toggle(&dir.join("sub"));
        tree.toggle(&dir.join("sub/deep"));
        let expanded = tree.rows().to_vec();
        assert!(expanded.iter().any(|r| r.name == "deepest.txt"));
        tree.refresh();
        assert_eq!(tree.rows(), expanded.as_slice(), "expand matches a rebuild");

        // Collapsing drops the whole subtree, nested rows included.
        tree.toggle(&dir.join("sub"));
        let collapsed = tree.rows().to_vec();
        assert!(!collapsed.iter().any(|r| r.name == "inner.txt"));
        tree.refresh();
        assert_eq!(
            tree.rows(),
            collapsed.as_slice(),
            "collapse matches a rebuild"
        );

        // Re-expanding restores the nested rows, because the inner directory
        // is still in the expanded set.
        tree.toggle(&dir.join("sub"));
        tree.refresh();
        assert_eq!(
            tree.rows(),
            expanded.as_slice(),
            "re-expanding restores the subtree"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revealing_a_visible_file_does_not_rebuild() {
        let dir = scratch_dir("reveal-noop");
        let mut tree = FileTree::new(dir.clone());
        tree.reveal(&dir.join("sub/inner.txt"));
        assert!(tree.row_of(&dir.join("sub/inner.txt")).is_some());
        // A file that is already visible needs no rebuild: the rows must not
        // change, and the expanded set must not grow.
        let before = tree.rows().to_vec();
        tree.reveal(&dir.join("sub/inner.txt"));
        assert_eq!(tree.rows(), before.as_slice());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_expands_the_ancestors_of_a_file() {
        let dir = scratch_dir("reveal");
        let mut tree = FileTree::new(dir.clone());
        tree.reveal(&dir.join("sub/inner.txt"));
        assert!(tree.row_of(&dir.join("sub/inner.txt")).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_root_yields_no_rows_instead_of_panicking() {
        let tree = FileTree::new(PathBuf::from("/definitely/not/here"));
        assert!(tree.rows().is_empty());
    }

    #[test]
    fn creating_a_file_and_a_folder_shows_them() {
        let dir = scratch_dir("create");
        let mut tree = FileTree::new(dir.clone());
        let file = tree.create(&dir, "new.txt", false).unwrap();
        assert!(file.is_file());
        let folder = tree.create(&dir, "newdir", true).unwrap();
        assert!(folder.is_dir());
        // The new entries are visible without a manual refresh.
        assert!(tree.row_of(&file).is_some());
        assert!(tree.row_of(&folder).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn creating_refuses_bad_names_and_existing_entries() {
        let dir = scratch_dir("create-bad");
        let mut tree = FileTree::new(dir.clone());
        assert!(tree.create(&dir, "", false).is_err(), "empty name");
        assert!(tree.create(&dir, "   ", false).is_err(), "blank name");
        assert!(tree.create(&dir, "a/b", false).is_err(), "a slash");
        assert!(tree.create(&dir, "fresh.txt", false).is_ok());
        assert!(
            tree.create(&dir, "fresh.txt", false).is_err(),
            "it must not clobber an existing file"
        );
        assert!(
            tree.create(&dir, "a.txt", false).is_err(),
            "a file from the fixture is taken too"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renaming_moves_the_entry() {
        let dir = scratch_dir("rename");
        let mut tree = FileTree::new(dir.clone());
        let renamed = tree.rename(&dir.join("a.txt"), "z.txt").unwrap();
        assert!(renamed.ends_with("z.txt"));
        assert!(renamed.exists());
        assert!(!dir.join("a.txt").exists());
        assert!(tree.rename(&dir.join("z.txt"), "b.txt").is_err(), "taken");
        assert!(tree.rename(&dir.join("z.txt"), "").is_err(), "empty");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_removes_files_and_folders() {
        let dir = scratch_dir("delete");
        let mut tree = FileTree::new(dir.clone());
        tree.delete(&dir.join("a.txt"), false).unwrap();
        assert!(!dir.join("a.txt").exists());
        tree.delete(&dir.join("sub"), true).unwrap();
        assert!(!dir.join("sub").exists());
        assert!(tree.delete(&dir.join("gone.txt"), false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_entries_land_in_the_selected_folder() {
        let dir = scratch_dir("target");
        let mut tree = FileTree::new(dir.clone());
        // Nothing selected: the root.
        assert_eq!(tree.target_dir(), dir);
        // A folder selected: inside it.
        tree.selected = Some(dir.join("sub"));
        assert_eq!(tree.target_dir(), dir.join("sub"));
        // A file selected: beside it.
        tree.selected = Some(dir.join("a.txt"));
        assert_eq!(tree.target_dir(), dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_root_points_the_tree_at_another_directory() {
        // The editor follows the terminal's `cd`, and this is the call that
        // moves the tree when the shell's directory changes.
        let a = scratch_dir("follow-a");
        let b = scratch_dir("follow-b");
        std::fs::write(b.join("only-in-b.txt"), "").unwrap();
        let mut tree = FileTree::new(a.clone());
        // `toggle` is what keeps the expanded set, the rebuild's source of
        // truth, in step with the rows.
        tree.toggle(&a.join("sub"));
        assert!(
            tree.expanded.contains(&a.join("sub")),
            "the fixture started expanded"
        );
        tree.set_root(b.clone());
        assert_eq!(tree.root, b, "the tree points at the new root");
        assert!(tree.expanded.is_empty(), "expansion does not carry over");
        let names: Vec<&str> = tree.rows().iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"only-in-b.txt"),
            "the new root's files are listed: {names:?}"
        );
        // Pointing at the same place again changes nothing.
        let rows = tree.rows().to_vec();
        tree.set_root(b.clone());
        assert_eq!(tree.rows(), rows.as_slice(), "a repeated root is a no-op");
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn moving_an_entry_into_a_folder_moves_it() {
        let dir = scratch_dir("mv");
        std::fs::create_dir_all(dir.join("nest")).unwrap();
        let mut tree = FileTree::new(dir.clone());
        let to = tree.mv(&dir.join("a.txt"), &dir.join("nest")).unwrap();
        assert_eq!(to, dir.join("nest").join("a.txt"));
        assert!(to.exists(), "the file is on the disk in its new place");
        assert!(!dir.join("a.txt").exists(), "the old entry is gone");
        assert!(
            tree.expanded.contains(&dir.join("nest")),
            "the destination is expanded so the move is visible"
        );
        let names: Vec<&str> = tree.rows().iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"a.txt"),
            "the moved file is listed: {names:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moving_refuses_to_overwrite_or_lose_data() {
        let dir = scratch_dir("mv-refuse");
        std::fs::create_dir_all(dir.join("nest")).unwrap();
        std::fs::write(dir.join("nest").join("a.txt"), "in the way").unwrap();
        std::fs::create_dir_all(dir.join("sub").join("deep")).unwrap();
        let mut tree = FileTree::new(dir.clone());
        // A name already taken at the destination.
        assert!(tree.mv(&dir.join("a.txt"), &dir.join("nest")).is_err());
        // A folder into itself.
        assert!(tree.mv(&dir.join("sub"), &dir.join("sub")).is_err());
        // A folder into its own descendant.
        assert!(
            tree.mv(&dir.join("sub"), &dir.join("sub").join("deep"))
                .is_err()
        );
        // Into the folder it already lives in is a no-op, not an error.
        assert_eq!(
            tree.mv(&dir.join("a.txt"), &dir).unwrap(),
            dir.join("a.txt")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deleted_root_leaves_and_returns() {
        let dir = scratch_dir("root-gone");
        let mut tree = FileTree::new(dir.clone());
        assert!(!tree.rows().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
        tree.refresh_if_dirty();
        assert!(tree.rows().is_empty(), "the ghost rows are dropped");
        assert!(tree.root_missing, "the tree says the root is gone");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "back").unwrap();
        tree.refresh_if_dirty();
        assert!(!tree.rows().is_empty(), "a recreated root is listed again");
        assert!(!tree.root_missing);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
