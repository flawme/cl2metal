# cl2metal — the full feature reference

Everything the app does, in detail. The [README](README.md) keeps the summary;
this is the complete list — features, keybindings, configuration, internal
commands, platform notes and source layout.

## Terminal

- GPU-instanced rendering (glyph atlas, truecolor, wide chars)
- Scrollback with a draggable scrollbar, `Shift+PageUp/PageDown` paging
- Scrollback **search** (`Ctrl+Shift+F`) with match highlighting
- Selection: drag, double-click word, triple-click line, `Ctrl+Shift+A` select-all
- Copy-on-select to the X11 primary selection, middle-click paste
- Cursor: block/beam/underline, blinking (DECSCUSR-aware), hollow when unfocused
- Visual bell, terminal font zoom, fullscreen
- Background image behind the terminal (cover/contain/stretch, adjustable
  opacity) — drop an image on the window to set it
- OSC 8 hyperlinks (Ctrl+click), OSC 52 clipboard (writes always, reads opt-in)
- Kitty keyboard protocol for modern TUI apps
- Shell integration (OSC 133): prompt marks, exit status, jump-to-prompt

## SSH

- Password, public-key, **ssh-agent** and **keyboard-interactive** (MFA) auth
- Password auth retries (three tries, OpenSSH-style): a stored password that
  the server rejects prompts for a fresh one instead of ending the session
- Host key verification against `~/.ssh/known_hosts` (TOFU prompt, MITM refusal)
- **Jump host** (`ProxyJump`), **local forwards** (`-L`), **remote forwards**
  (`-R`) and **dynamic SOCKS5 forwards** (`-D`)
- **Agent forwarding** (`-A`) per profile
- `~/.ssh/config` import (including `LocalForward`, `RemoteForward`,
  `DynamicForward`, `ForwardAgent`), ad-hoc `user@host` connect, reconnect in place
- Keepalive, `nodelay`, per-profile settings; secrets in the OS keyring

## SSH mode — the workspace around those sessions

- A host sidebar beside the terminal: search, groups, favourites, recents and a
  live connection dot per host
- Profile editor for name, host, port, user, auth, key file, group, jump host
  and all three forward kinds; right-click a host to duplicate, star or delete
  it, or use Remove host in the editor; renaming edits the host in place and
  moves its keyring secret, and changing the auth method drops the secret that
  no longer applies
- **SFTP browser** (`Ctrl+Shift+S` opens the workspace, then the SFTP entry in
  the palette): directory listing with permissions and sizes, navigation,
  download, upload, mkdir, rename, delete, progress and cancel
- Open a remote file in the editor and save it back over the same session
- The sidebar is a view, not a mode: the terminal keeps running behind it

## Editor

A modal editor built into the application:

- Vim-style **normal / insert / replace / visual / command** modes with motions,
  counts, operators (`d c y > < ~`), text objects (`iw aw i" a( i{ ip`),
  registers, undo/redo and `:` commands
- **Multiple cursors**: `Ctrl+D` select-next-occurrence, `Ctrl+Shift+L` all
  occurrences, `Ctrl+Alt+Up/Down` add a cursor, block selection with `Ctrl+V`
- Syntax highlighting (syntect, on a worker thread), line numbers, relative
  numbers, current-line highlight, bracket matching, whitespace markers
- File tree, buffer tabs, split panes, find/replace, go-to-line, command palette
- **Editable keybindings** with conflict detection and reset to defaults
- Local and remote files in the same editor, with the location in the status line
- Unsaved-change protection, external-change detection, large-file handling

## Keybindings

| Shortcut | Action |
|---|---|
| `Ctrl+Shift+T` | New terminal tab |
| `Ctrl+Shift+W` | Close the focused pane (the tab when it is the last pane) |
| `Ctrl+Shift+R` | Reopen the most recently closed tab |
| `Ctrl+Shift+D` / `Ctrl+Shift+E` | Split the focused pane right / down |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab |
| `Ctrl+Shift+P` or `Ctrl+Shift+K` | Command palette |
| `Ctrl+Shift+N` | Palette filtered to SSH profiles |
| `Ctrl+Shift+F` | Search scrollback (`Enter` next, `Shift+Enter` previous, `Esc` close) |
| `Ctrl+Shift+A` | Select all |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste |
| `Ctrl+Shift+Up` / `Ctrl+Shift+Down` | Jump to previous / next prompt |
| `Shift+PageUp` / `Shift+PageDown` | Scroll a page |
| `Ctrl+Shift+Home` / `Ctrl+Shift+End` | Scroll to top / bottom |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | Terminal font size up / down / reset |
| `Ctrl+scroll` | Terminal font size up / down |
| `Ctrl+Shift+,` | Settings |
| `F11` | Fullscreen |
| `Ctrl+click` | Open a hyperlink |
| Right-click | Context menu (copy, paste, select all, search, clear, rename, new/close tab) |
| Double-click a tab | Rename it |
| Drag a tab | Reorder tabs |
| `Enter` / `R` on a dead tab | Close / reconnect |
| `Ctrl+Shift+O` | Switch between the terminal and the editor |
| `Ctrl+Shift+S` | Toggle the SSH workspace sidebar |

### Editor keys

The editor is modal: in normal mode the letters are commands, in insert mode
they are text. The defaults are Vim's, and every one of them can be changed in
**Keybindings** (palette, or `Ctrl+Shift+K` while the editor is focused).

| Keys | Action |
|---|---|
| `h j k l` | Move left / down / up / right |
| `w e b`, `W E B` | Word and big-word motions |
| `0 ^ $`, `gg G` | Line start / first non-blank / line end, file start / end |
| `i a I A o O` | Insert before / after / line start / line end / below / above |
| `d c y` + motion or object | Delete, change, yank (`dw`, `dd`, `diw`, `ci"`, `ya(`) |
| `x X D J p P` | Delete char, delete back, delete to end, join, paste after / before |
| `u`, `Ctrl+R` | Undo, redo |
| `v V Ctrl+V` | Visual char / line / block |
| `Ctrl+D`, `Ctrl+Shift+L` | Select next occurrence, select all occurrences |
| `Ctrl+Alt+Down` / `Ctrl+Alt+Up` | Add a cursor below / above |
| `/ ? n N` | Search forward / back, next, previous |
| `:` | Command line (`:w`, `:q`, `:e FILE`, `:3`, `:s/a/b/g`, `:%s/a/b/g`, `:set …`) |
| `g c`, `>`, `<`, `~` | Comment, indent, dedent, toggle case |
| `%` | Matching bracket |
| `Ctrl+S` | Save |
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+B` | File tree |
| `Ctrl+Shift+D` / `Ctrl+Shift+E` | Split right / down |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous buffer |
| `Ctrl+Shift+F` | Find in files |
| `Ctrl+H` | Replace |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste with the system clipboard |
| `Ctrl+Shift+T` | Back to the terminal |

## Panes

A tab can hold several sessions. `Ctrl+Shift+D` splits the focused pane side by
side, `Ctrl+Shift+E` stacks a new one below; each pane runs its own shell or SSH
session with its own scrollback. Click a pane to focus it, drag the divider
between two panes to resize them, and `Ctrl+Shift+W` closes the focused pane
(the tab itself when it is the last one). Scrolling follows the pointer.

## Notifications and paste safety

- A background tab that produced output gets a small dot in the tab bar.
- When a command that ran for more than five seconds finishes while the window
  is unfocused, cl2metal sends a desktop notification (`notify-send`). This
  needs shell integration for the timing.
- Pasting several lines that end in a newline asks first, because the last line
  would run immediately. Single commands with their trailing newline paste
  straight through.
- Selecting text publishes it to the X11 primary selection, and (with
  `copy_on_select`, on by default) to the clipboard as well.

## Configuration

`~/.config/cl2metal/config.toml` (created on first save; no secrets are stored here).
The settings modal (`Ctrl+Shift+,`) previews every change live; Save writes the
file, Cancel or Esc reverts.

```toml
font = "DejaVu Sans Mono"
font_size = 14.0
theme = "midnight"          # midnight | graphite | paper
shell = ""                  # "" = $SHELL
scrollback = 10000
padding = [8.0, 12.0]       # [x, y] points
cursor_shape = "block"      # block | beam | underline
cursor_blink = true
visual_bell = true
ssh_keepalive = 30          # seconds, 0 disables
allow_osc52_read = false    # let programs read the clipboard
log_sessions = false        # write transcripts to the state dir
auto_reconnect = false      # retry dropped SSH sessions
opacity = 1.0               # window opacity (needs a compositor)
background_image = ""       # path to a png/jpeg/bmp behind the terminal
background_image_opacity = 0.28  # 0 = hidden, 1 = strongest ambient blend
background_fit = "cover"    # cover | contain | stretch
remember_secrets = true     # store SSH secrets in the OS keyring

# Optional colour overrides on top of the selected theme:
# [colors]
# accent = "#ff8800"
# bg = "#000000"
# ansi4 = "#123456"

[[profiles]]
name = "prod"
host = "prod.example.com"
port = 22
user = "deploy"
auth = "agent"              # password | key | agent
key_path = "~/.ssh/id_ed25519"
proxy_jump = "user@bastion:22"
forwards = ["8080:localhost:5432"]        # -L
remote_forwards = ["9000:localhost:3000"] # -R
socks_forwards = ["1080"]                 # -D
agent_forwarding = false                  # -A
```

The window size and position are remembered between launches.

The agent broker has its own block:

```toml
[agent_api]
enabled = true             # the Settings -> Security master switch
confirm_commands = true     # screen commands: read-only ones run silently
# allow_profiles = ["prod"] # when set, only these profiles may be used
```

## Command line

```sh
cl2metal                     # local shell
cl2metal prod                # connect to the "prod" profile
cl2metal user@host:2222      # ad-hoc SSH connection
cl2metal -e htop             # run a command in the first tab
cl2metal --edit src/main.rs  # open a file in the editor
cl2metal --help
```

SSH passwords and key passphrases live in the system keyring
(Secret Service / GNOME Keyring / KWallet), never on disk.

## Shell integration

On first launch cl2metal writes OSC 133 hooks to
`~/.config/cl2metal/shell/` and wires them in without touching your dotfiles:
bash is started with `--rcfile`, zsh with a private `ZDOTDIR`. Both source your
real rc file first. This is what powers the prompt marks, the exit-status dot in
the tab bar, and `Ctrl+Shift+Up/Down`.

## Internal commands

Type these at the prompt: the line is intercepted and erased from the shell's
own editor (Ctrl+U on unix, backspaces on Windows), so the shell never sees it.
They are a local-pane feature: in an SSH pane the line is cleared and refused
with a notice, the way `:bench` is — the command drives this app on this
machine, not the remote host.

| Command | Effect |
| --- | --- |
| `:info` | system info panel (OS, kernel, CPU, GPU, memory, load, uptime, session) |
| `:bench [secs]` | spinning-torus benchmark: measures the app's frame rate under a full-screen animation |
| `:matrix` | digital rain over the terminal |
| `:party` | party mode: the accent hue-cycles for a few seconds |
| `:42` | the answer |
| `:theme <id>` | switch theme (`midnight`, `graphite`, `paper`) |
| `:font <size>` | set the font size |
| `:bg <path>` / `:bg off` | set or clear the wallpaper |
| `:clear` | clear the scrollback |
| `:reload` | re-read `config.toml` |
| `:ssh <user@host[:port]>` | connect to a host |
| `:even [columns\|rows\|grid]` | even out the panes |
| `:help` | list these commands |

`Esc` closes the info panel. "System info" is also in the command palette
(`Ctrl+Shift+P`).

`:bench` is a real end-to-end benchmark, not a synthetic overlay: the frames are
produced by a child process (this same binary, run with `--bench-frames`), so a
run exercises the whole path a program takes — child, PTY, reader thread, VT
parser, grid, frame build, GPU. When it finishes, the app logs and toasts the
result:

```
bench: 1176 frames in 8.5s (138.3 fps), frame 7.2/39.4 ms, logic 1.2/7.3 ms, ui 3.5/11.9 ms
```

`frame` is the interval between frames (mean/worst), `logic` the per-frame
session and input pass, `ui` the per-frame build pass. The child can also be run
directly in any terminal: `cl2metal --bench-frames [secs] [cols] [rows]`.

## Easter eggs

Three hidden effects, none of them in the way:

- **Konami code** (`↑↑↓↓←→←→BA`) — party mode: the accent hue-cycles for a few seconds.
- **Palette**: type `matrix` for digital rain over the terminal, `party` for party
  mode, `42` for the answer.
- **At the prompt**: `:matrix`, `:party`, `:42` (the long `cl2metal --matrix`
  forms still work as aliases).

## Platforms

Linux is the primary target; Windows and macOS build and run from the same tree. The
OS-specific code lives in one module per platform (a file, or a folder when
it needs helpers), so a new platform is a new module plus the wiring:

Building on Windows needs **CMake** on `PATH`: the SSH crypto backend
(`aws-lc-sys`) builds C with it. NASM is optional — the repo pins
`AWS_LC_SYS_PREBUILT_NASM=1` (`.cargo/config.toml`), so the backend uses its
prebuilt assembly objects.

| | Linux | Windows | macOS |
| --- | --- | --- | --- |
| Terminal, panes, tabs, renderer, SSH | yes | yes | yes |
| SSH agent auth | the `SSH_AUTH_SOCK` socket | the OpenSSH agent's named pipe (`\\.\pipe\openssh-ssh-agent`) | the `SSH_AUTH_SOCK` socket |
| `:info` panel | `/proc` | Win32 counters | `sysctl`, `vm_stat`, and `ps` |
| Default shell | `$SHELL`, else `/bin/bash` | `pwsh.exe`, else `powershell.exe`, else `cmd.exe` | `$SHELL`, else `/bin/bash` |
| Desktop notifications | `notify-send` | a PowerShell balloon | `osascript` notification |
| Shell integration (OSC 133 marks, prompt jumping, exit-status dot) | bash/zsh hooks | not yet | bash/zsh hooks |
| `--install-desktop` | `.desktop` entry + icon | a Start Menu shortcut | a user-local `~/Applications/cl2metal.app` bundle |
| Primary selection (middle-click paste) | X11 | not applicable | not applicable |

Where the code lives:

```
src/platform/{linux.rs,macos.rs,windows/}       notifications, app install, default font
src/app/sysinfo/{linux.rs,macos.rs,windows/}     the :info panel's readers
src/term/{linux.rs,macos.rs,windows/}            process inspection and shell cwd
```

CI runs `clippy` and the test suite on Linux, Windows, and macOS. **Windows and
Linux have been tested on real hardware**; macOS is verified by CI
(Apple-silicon runners) but has not yet been driven hands-on.

## Desktop entry

`cl2metal --install-desktop` writes the platform's launcher entry, pointing at
the absolute path of the binary that ran the command:

- **Linux**: `cl2metal.desktop` into `$XDG_DATA_HOME/applications/` (default
  `~/.local/share/applications/`), the icon into
  `$XDG_DATA_HOME/icons/hicolor/scalable/apps/cl2metal.svg`, and refreshes the
  menu database with `update-desktop-database` when it is installed.
- **Windows**: `cl2metal.lnk` into the Start Menu
  (`%APPDATA%\Microsoft\Windows\Start Menu\Programs`).
- **macOS**: a user-local `~/Applications/cl2metal.app` bundle.

Every path written is printed to stdout. `cl2metal --version` (also `-V`)
prints the version and exits.

## Source layout

```
src/
  main.rs          window setup, CLI, logging
  bench.rs         the `:bench` spinning torus (frame generator + child mode)
  app.rs           application state, startup, eframe glue
  app/session.rs   tab lifecycle and the session message pump
  app/events.rs    keyboard and pointer routing
  app/view.rs      startup overlay, tab strip, terminal surface, modals
  app/search.rs    scrollback search
  app/clipboard.rs clipboard and X11 primary selection
  app/background.rs wallpaper loading (off-thread, cached) and alpha values
  app/eggs.rs      internal commands (`:info`, `:matrix`, …) and easter eggs
  app/sysinfo.rs   the `:info` panel: per-OS facts (/proc, sysctl, Win32 counters) and session facts
  app/ssh_mode.rs  the SSH workspace sidebar: hosts, groups, favourites
  app/sftp_panel.rs the SFTP browser view
  app/editor_view.rs hosting the editor: theme colours, key routing, remote saves
  editor/          the modal editor (buffer, modes, motions, operators, view, …)
  agent/           the MCP broker: server, protocol, consent guard, :mcp-on registration
  ui.rs            egui chrome: tab bar widgets, palette, settings, prompts
  render/          wgpu terminal renderer (instanced quads + glyph atlas)
  term/            terminal core, PTY, SSH, SFTP, input encoding, shell integration
  config.rs        config.toml, SSH profiles, known_hosts, keyring secrets
  theme.rs         built-in themes and colour overrides
```

The wallpaper is decoded on a worker thread and cached in the state directory
(`background-<hash>.rgba`), so only the first launch after changing the image
pays for the decode.
