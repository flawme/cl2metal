//! Windows: a PowerShell balloon for notifications, Start Menu shortcut
//! installation, and Consolas as the default font.

use std::io;

/// The font to use when the config does not name one.
///
/// Consolas ships with every Windows since Vista; Cascadia Mono is nicer but
/// only present with Windows Terminal, and the font loader would have to fall
/// back anyway.
pub(crate) fn default_font() -> String {
    "Consolas".to_owned()
}

/// The command that shows a desktop notification.
///
/// Windows has no `notify-send`; a balloon in the notification area is the
/// closest thing that needs no extra crate. The process lives for a few
/// seconds so the balloon is actually seen, then disposes itself.
pub(crate) fn notification_command(title: &str, body: &str) -> std::process::Command {
    // Single quotes are the only escaping PowerShell needs here.
    let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         Add-Type -AssemblyName System.Drawing; \
         $n = New-Object System.Windows.Forms.NotifyIcon; \
         $n.Icon = [System.Drawing.SystemIcons]::Information; \
         $n.Visible = $true; \
         $n.ShowBalloonTip(5000, {title}, {body}, 'Info'); \
         Start-Sleep -Seconds 6; \
         $n.Dispose()",
        title = quote(title),
        body = quote(body),
    );
    let mut cmd = std::process::Command::new("powershell");
    cmd.arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-WindowStyle")
        .arg("Hidden")
        .arg("-Command")
        .arg(script);
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

/// Install a Windows Start Menu shortcut (`cl2metal.lnk`) so the application
/// appears in the Start Menu and Windows search.
pub(crate) fn install_desktop() -> io::Result<()> {
    let data_dir = dirs::data_dir()
        .ok_or_else(|| io::Error::other("cannot locate the Windows AppData directory"))?;
    let programs_dir = data_dir.join(r"Microsoft\Windows\Start Menu\Programs");
    std::fs::create_dir_all(&programs_dir)?;

    let exe = std::env::current_exe()?;
    let shortcut_path = programs_dir.join("cl2metal.lnk");

    let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let script = format!(
        "$ws = New-Object -ComObject WScript.Shell; \
         $s = $ws.CreateShortcut({shortcut}); \
         $s.TargetPath = {target}; \
         $s.WorkingDirectory = {workdir}; \
         $s.Description = 'cl2metal - sleek terminal and SSH client'; \
         $s.Save()",
        shortcut = quote(&shortcut_path.display().to_string()),
        target = quote(&exe.display().to_string()),
        workdir = quote(
            &exe.parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ),
    );

    let status = std::process::Command::new("powershell")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(&script)
        .status()?;

    if !status.success() {
        return Err(io::Error::other(format!(
            "failed to create shortcut: powershell exited with {status}"
        )));
    }

    println!("{}", shortcut_path.display());
    Ok(())
}

/// Open a console window on `command`.
///
/// The command goes into a temporary `.cmd` file: `cmd.exe` has its own
/// quoting rules, and a PowerShell one-liner full of quotes does not survive
/// being passed through `start`. `cmd /k` keeps the window open after the
/// installer finishes, so the result stays readable.
pub(crate) fn open_terminal(command: &str) -> Result<(), String> {
    let path = std::env::temp_dir().join(format!("cl2metal-update-{}.cmd", std::process::id()));
    std::fs::write(&path, format!("@echo off\r\n{command}\r\n"))
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
    let status = std::process::Command::new("cmd")
        .args(["/C", "start", "", "cmd", "/k"])
        .arg(&path)
        .status()
        .map_err(|e| format!("cmd start: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("could not open a terminal".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_font_is_consolas() {
        assert_eq!(default_font(), "Consolas");
    }

    #[test]
    fn notification_command_formats_powershell() {
        let cmd = notification_command("test title", "test body");
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|a| a.contains("ShowBalloonTip")));
        assert!(args.iter().any(|a| a.contains("'test title'")));
        assert!(args.iter().any(|a| a.contains("'test body'")));
    }
}
