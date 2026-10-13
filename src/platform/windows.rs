//! Windows: a PowerShell balloon for notifications, no application-menu
//! install (that is a `.desktop` concept), and Consolas as the default font.

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

/// Windows has no `.desktop` entry to install.
///
/// A Start Menu shortcut would need COM (`IShellLink`); until then the honest
/// answer is to say so rather than write something that does nothing.
pub(crate) fn install_desktop() -> io::Result<()> {
    Err(io::Error::other(
        "--install-desktop is a Linux feature; on Windows, pin cl2metal.exe to the Start Menu or taskbar",
    ))
}
