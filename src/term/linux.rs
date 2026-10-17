//! Linux process inspection: child process enumeration and shell working directory via `/proc`.

use std::collections::HashMap;
use std::path::PathBuf;

/// The names of the processes still running under a shell, if any.
pub fn running_children(pid: u32) -> Vec<String> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut names: HashMap<u32, String> = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(child_pid) = name.parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{child_pid}/stat")) else {
            continue;
        };
        let Some((head, tail)) = stat.rsplit_once(')') else {
            continue;
        };
        let mut fields = tail.split_whitespace();
        let state = fields.next().unwrap_or("");
        let Some(ppid) = fields.next().and_then(|p| p.parse::<u32>().ok()) else {
            continue;
        };
        if state == "Z" {
            continue;
        }
        let comm = head
            .split_once('(')
            .map(|(_, c)| c.to_owned())
            .unwrap_or_default();
        names.insert(child_pid, comm);
        children.entry(ppid).or_default().push(child_pid);
    }
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![pid];
    while let Some(p) = stack.pop() {
        let Some(kids) = children.get(&p) else {
            continue;
        };
        for &kid in kids {
            if let Some(name) = names.get(&kid)
                && !out.iter().any(|n| n == name)
            {
                out.push(name.clone());
            }
            stack.push(kid);
        }
    }
    out
}

/// The working directory of a shell process, when it can be read from `/proc`.
pub fn shell_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}
