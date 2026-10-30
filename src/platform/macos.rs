//! macOS desktop integration: notifications, a user-local app bundle, and Menlo.

use std::io;
use std::path::Path;

pub(crate) fn default_font() -> String {
    "Menlo".to_owned()
}

pub(crate) fn notification_command(title: &str, body: &str) -> std::process::Command {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        applescript_escape(body),
        applescript_escape(title)
    );
    let mut cmd = std::process::Command::new("osascript");
    cmd.arg("-e").arg(script);
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

fn applescript_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Open Terminal.app on `command`.
///
/// Terminal.app runs a script file, so the command goes into a temporary
/// script instead of being quoted through osascript.
pub(crate) fn open_terminal(command: &str) -> Result<(), String> {
    let path = std::env::temp_dir().join(format!("cl2metal-update-{}.sh", std::process::id()));
    std::fs::write(&path, format!("#!/bin/sh\n{command}\n"))
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("chmod {}: {e}", path.display()))?;
    let status = std::process::Command::new("open")
        .args(["-a", "Terminal"])
        .arg(&path)
        .status()
        .map_err(|e| format!("open -a Terminal: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("Terminal.app refused the script".to_owned())
    }
}

pub(crate) fn install_desktop() -> io::Result<()> {
    let app_dir = dirs::home_dir()
        .ok_or_else(|| io::Error::other("cannot locate the home directory"))?
        .join("Applications/cl2metal.app");
    let executable = std::env::current_exe()?;
    install_app_bundle(&app_dir, &executable)?;
    println!("{}", app_dir.display());
    Ok(())
}

fn install_app_bundle(app_dir: &Path, executable: &Path) -> io::Result<()> {
    let contents = app_dir.join("Contents");
    let macos = contents.join("MacOS");
    std::fs::create_dir_all(&macos)?;
    let target = macos.join("cl2metal");
    if std::fs::symlink_metadata(&target).is_ok() {
        std::fs::remove_file(&target)?;
    }
    std::os::unix::fs::symlink(executable, &target)?;
    std::fs::write(contents.join("Info.plist"), info_plist())?;
    Ok(())
}

fn info_plist() -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
     <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
     <plist version=\"1.0\"><dict>\n\
     <key>CFBundleExecutable</key><string>cl2metal</string>\n\
     <key>CFBundleIdentifier</key><string>com.cl2metal.app</string>\n\
     <key>CFBundleName</key><string>cl2metal</string>\n\
     <key>CFBundlePackageType</key><string>APPL</string>\n\
     <key>CFBundleShortVersionString</key><string>{}</string>\n\
     <key>LSMinimumSystemVersion</key><string>12.0</string>\n\
     </dict></plist>\n",
        env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_font_is_menlo() {
        assert_eq!(default_font(), "Menlo");
    }

    #[test]
    fn notification_command_escapes_applescript_strings() {
        let command = notification_command("a \"title\"", "line 1\nline 2\\");
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args[1].contains("a \\\"title\\\""));
        assert!(args[1].contains("line 1\\nline 2\\\\"));
    }

    #[test]
    fn app_bundle_points_to_the_executable() {
        let root = std::env::temp_dir().join(format!("cl2metal-app-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let executable = root.with_extension("binary");
        std::fs::write(&executable, b"binary").expect("create test executable");
        let app = root.with_extension("app");
        install_app_bundle(&app, &executable).expect("install app bundle");
        assert_eq!(
            std::fs::read_link(app.join("Contents/MacOS/cl2metal")).unwrap(),
            executable
        );
        assert!(
            std::fs::read_to_string(app.join("Contents/Info.plist"))
                .unwrap()
                .contains("com.cl2metal.app")
        );
        let _ = std::fs::remove_dir_all(root.with_extension("app"));
        let _ = std::fs::remove_file(executable);
    }
}
