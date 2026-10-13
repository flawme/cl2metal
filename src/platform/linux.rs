//! Linux: `notify-send` for notifications, a `.desktop` entry plus icon for
//! the application menu, and DejaVu Sans Mono as the default font.

use std::io;
use std::path::{Path, PathBuf};

/// The app icon, embedded so `--install-desktop` works from any directory.
const ICON_SVG: &str = include_str!("../../assets/cl2metal.svg");

/// Desktop-entry template; `@EXEC@` becomes the absolute binary path.
const DESKTOP_ENTRY: &str = include_str!("../../assets/cl2metal.desktop");

/// The font to use when the config does not name one.
pub(crate) fn default_font() -> String {
    "DejaVu Sans Mono".to_owned()
}

/// The command that shows a desktop notification.
pub(crate) fn notification_command(title: &str, body: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("notify-send");
    cmd.arg("-a")
        .arg("cl2metal")
        .arg("-i")
        .arg("utilities-terminal")
        .arg(title)
        .arg(body);
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

/// Install the desktop entry and icon so the app appears in the application
/// menu and in file managers' "Open with" lists.
pub(crate) fn install_desktop() -> io::Result<()> {
    let data_dir =
        dirs::data_dir().ok_or_else(|| io::Error::other("cannot locate the XDG data directory"))?;
    let exe = std::env::current_exe()?;
    for path in install_desktop_to(&data_dir, &exe)? {
        println!("{}", path.display());
    }
    refresh_desktop_database(&data_dir.join("applications"));
    Ok(())
}

/// Write the entry and the icon under `data_dir`; returns the paths written.
fn install_desktop_to(data_dir: &Path, exe: &Path) -> io::Result<Vec<PathBuf>> {
    let apps_dir = data_dir.join("applications");
    let icons_dir = data_dir.join("icons/hicolor/scalable/apps");
    std::fs::create_dir_all(&apps_dir)?;
    std::fs::create_dir_all(&icons_dir)?;

    let entry_path = apps_dir.join("cl2metal.desktop");
    std::fs::write(&entry_path, desktop_entry(exe))?;
    let icon_path = icons_dir.join("cl2metal.svg");
    std::fs::write(&icon_path, ICON_SVG)?;
    Ok(vec![entry_path, icon_path])
}

/// The desktop entry with `@EXEC@` replaced by the absolute binary path.
fn desktop_entry(exe: &Path) -> String {
    let exe = exe.display().to_string();
    // An Exec path containing spaces must be quoted.
    let exe = if exe.contains(' ') {
        format!("\"{exe}\"")
    } else {
        exe
    };
    DESKTOP_ENTRY.replace("@EXEC@", &exe)
}

/// Refresh the menu database when the tool is on `PATH`; failures are ignored.
fn refresh_desktop_database(apps_dir: &Path) {
    if !on_path("update-desktop-database") {
        return;
    }
    match std::process::Command::new("update-desktop-database")
        .arg(apps_dir)
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => log::warn!("update-desktop-database exited with {status}"),
        Err(e) => log::warn!("update-desktop-database could not run: {e}"),
    }
}

/// `true` when `name` names a file on `PATH`.
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_uses_the_absolute_binary_path() {
        let entry = desktop_entry(Path::new("/opt/cl2metal/cl2metal"));
        assert!(entry.contains("Exec=/opt/cl2metal/cl2metal %U"), "{entry}");
        assert!(entry.contains("[Desktop Entry]"), "{entry}");
        assert!(entry.contains("Icon=cl2metal"), "{entry}");
        assert!(entry.contains("StartupWMClass=cl2metal"), "{entry}");
    }

    #[test]
    fn desktop_entry_quotes_a_path_with_spaces() {
        let entry = desktop_entry(Path::new("/opt/my apps/cl2metal"));
        assert!(
            entry.contains("Exec=\"/opt/my apps/cl2metal\" %U"),
            "{entry}"
        );
    }

    #[test]
    fn icon_is_a_small_self_contained_svg() {
        assert!(ICON_SVG.starts_with("<svg"), "{ICON_SVG}");
        assert!(ICON_SVG.contains("viewBox=\"0 0 256 256\""), "{ICON_SVG}");
        assert!(ICON_SVG.contains("#1a1b26"), "{ICON_SVG}");
        assert!(ICON_SVG.contains("#7aa2f7"), "{ICON_SVG}");
        assert!(ICON_SVG.len() < 1024, "icon is {} bytes", ICON_SVG.len());
        for forbidden in ["<script", "<image", "xlink:href", "url("] {
            assert!(!ICON_SVG.contains(forbidden), "icon contains {forbidden}");
        }
    }

    #[test]
    fn install_writes_the_entry_and_the_icon() {
        let dir =
            std::env::temp_dir().join(format!("cl2metal-desktop-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = install_desktop_to(&dir, Path::new("/opt/cl2metal/cl2metal")).expect("install");
        assert_eq!(
            paths,
            vec![
                dir.join("applications/cl2metal.desktop"),
                dir.join("icons/hicolor/scalable/apps/cl2metal.svg"),
            ]
        );
        let entry = std::fs::read_to_string(&paths[0]).expect("read entry");
        assert!(entry.contains("Exec=/opt/cl2metal/cl2metal %U"), "{entry}");
        let icon = std::fs::read_to_string(&paths[1]).expect("read icon");
        assert_eq!(icon, ICON_SVG);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
