//! macOS process inspection using the system `ps` and `lsof` utilities.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

/// Child and descendant command names for the pane-close confirmation.
pub fn running_children(pid: u32) -> Vec<String> {
    let Ok(output) = Command::new("ps")
        .args(["-axo", "pid=,ppid=,stat=,comm="])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut names = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(state)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let command = fields.collect::<Vec<_>>().join(" ");
        if command.is_empty() {
            continue;
        }
        let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) else {
            continue;
        };
        if state.contains('Z') {
            continue;
        }
        let name = std::path::Path::new(command)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        names.insert(pid, name);
        children.entry(ppid).or_default().push(pid);
    }

    let mut result = Vec::new();
    let mut stack = vec![pid];
    while let Some(parent) = stack.pop() {
        if let Some(kids) = children.get(&parent) {
            for &child in kids {
                if let Some(name) = names.get(&child)
                    && !result.contains(name)
                {
                    result.push(name.clone());
                }
                stack.push(child);
            }
        }
    }
    result
}

/// The process's current directory, read from `lsof`'s cwd descriptor entry.
pub fn shell_cwd(pid: u32) -> Option<PathBuf> {
    let output = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix('n'))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}
