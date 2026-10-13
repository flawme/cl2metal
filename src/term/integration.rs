//! Shell integration: OSC 133 semantic prompt marks.
//!
//! The terminal side lives in [`super::TermCore::feed`]; this module ships the
//! shell-side hooks and wires them in without touching the user's dotfiles
//! (bash gets `--rcfile`, zsh gets a private `ZDOTDIR`).

use std::path::{Path, PathBuf};

const BASH_SNIPPET: &str = r#"# cl2metal shell integration (OSC 133 semantic prompts).
# Sourced automatically by cl2metal; safe to source from your own ~/.bashrc too.
if [ -f "$HOME/.bashrc" ] && [ -z "$CL2METAL_BASH_RC" ]; then
    CL2METAL_BASH_RC=1 . "$HOME/.bashrc"
fi

# The terminal owns no prompt text: preserve the user's existing prompt
# exactly, except that it normalises the suffix to one deliberate space after
# `$`/`#`. This keeps input comfortably separated without a wide gap.
PS1="${PS1% } "

# Start the first prompt at the first terminal row. A shell can inherit blank
# rows while the PTY is being established; clear them before Bash renders its
# first prompt, without changing the prompt text or terminal padding.
printf '\033[H\033[2J'

__cl2metal_prompt() {
    local __cl2metal_code=$?
    printf '\033]133;D;%s\007' "$__cl2metal_code"
    printf '\033]133;A\007'
}

__cl2metal_preexec() {
    case "$BASH_COMMAND" in
        __cl2metal_*|*PROMPT_COMMAND*) return ;;
    esac
    printf '\033]133;C\007'
}

trap '__cl2metal_preexec' DEBUG
PROMPT_COMMAND="__cl2metal_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
"#;

const ZSH_SNIPPET: &str = r#"# cl2metal shell integration (OSC 133 semantic prompts).
if [ -f "$HOME/.zshrc" ] && [ -z "$CL2METAL_ZSH_RC" ]; then
    CL2METAL_ZSH_RC=1 . "$HOME/.zshrc"
fi

# Match the local Bash startup: the initial prompt begins at the terminal's
# first row instead of below inherited blank rows.
printf '\033[H\033[2J'

__cl2metal_precmd() {
    local __cl2metal_code=$?
    printf '\033]133;D;%s\007' "$__cl2metal_code"
    printf '\033]133;A\007'
}
__cl2metal_preexec() { printf '\033]133;C\007'; }

autoload -Uz add-zsh-hook
add-zsh-hook precmd __cl2metal_precmd
add-zsh-hook preexec __cl2metal_preexec
"#;

const ZSH_ENV_SNIPPET: &str = r#"# cl2metal: keep the user's own .zshenv working under our ZDOTDIR.
if [ -f "$HOME/.zshenv" ] && [ -z "$CL2METAL_ZSH_ENV" ]; then
    CL2METAL_ZSH_ENV=1 . "$HOME/.zshenv"
fi
"#;

/// Directory holding the generated integration scripts.
pub fn dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(crate::config::APP_NAME)
        .join("shell")
}

/// Write the integration scripts (idempotent) and return the directory.
pub fn ensure_scripts() -> std::io::Result<PathBuf> {
    let dir = dir();
    std::fs::create_dir_all(&dir)?;
    let zsh = dir.join("zsh");
    std::fs::create_dir_all(&zsh)?;
    write_if_changed(&dir.join("bash.sh"), BASH_SNIPPET)?;
    write_if_changed(&zsh.join(".zshrc"), ZSH_SNIPPET)?;
    write_if_changed(&zsh.join(".zshenv"), ZSH_ENV_SNIPPET)?;
    Ok(dir)
}

fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path)
        .map(|c| c == contents)
        .unwrap_or(false)
    {
        return Ok(());
    }
    std::fs::write(path, contents)
}

/// `--rcfile` argument for bash, when integration is available.
pub fn bash_rcfile(program: &str) -> Option<PathBuf> {
    if !is_shell(program, "bash") {
        return None;
    }
    let path = dir().join("bash.sh");
    path.is_file().then_some(path)
}

/// `ZDOTDIR` for zsh, when integration is available.
pub fn zsh_dotdir(program: &str) -> Option<PathBuf> {
    if !is_shell(program, "zsh") {
        return None;
    }
    let path = dir().join("zsh");
    path.is_dir().then_some(path)
}

fn is_shell(program: &str, name: &str) -> bool {
    Path::new(program)
        .file_name()
        .map(|f| f.to_string_lossy() == name)
        .unwrap_or(false)
}
