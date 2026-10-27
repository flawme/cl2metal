#!/bin/sh
# cl2metal installer — downloads the latest release binary for this platform.
#
# Run it straight from the web:
#
#   curl -fsSL https://raw.githubusercontent.com/flawme/cl2metal/master/install.sh | sh
#   wget -qO-   https://raw.githubusercontent.com/flawme/cl2metal/master/install.sh | sh
#
# Installs to ~/.local/bin unless CL2METAL_INSTALL_DIR says otherwise.
set -eu

repo="flawme/cl2metal"
install_dir="${CL2METAL_INSTALL_DIR:-$HOME/.local/bin}"

log() { printf 'cl2metal: %s\n' "$*"; }
die() { printf 'cl2metal: error: %s\n' "$*" >&2; exit 1; }

os=$(uname -s)
machine=$(uname -m)
case "$os:$machine" in
    Linux:x86_64)  target="x86_64-unknown-linux-gnu" ;;
    # Only x86_64 Linux is released. Other machines (arm, riscv, ...) must
    # stop here with a clear reason, not fail on a download that cannot run.
    Linux:*) die "no Linux release for $machine yet — only x86_64 is built. Build from source:
  git clone https://github.com/$repo && cd cl2metal && cargo build --release" ;;
    Darwin:x86_64)               target="x86_64-apple-darwin" ;;
    Darwin:arm64|Darwin:aarch64) target="aarch64-apple-darwin" ;;
    *) die "no release binary for $os $machine — build from source:
  git clone https://github.com/$repo && cd cl2metal && cargo build --release" ;;
esac

asset="cl2metal-$target.tar.gz"
url="https://github.com/$repo/releases/latest/download/$asset"

fetch() {
    # fetch <url> <output-file>
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$2" "$1"
    else
        die "need curl or wget to download the release"
    fi
}

tmp=$(mktemp -d) || die "cannot create a temporary directory"
trap 'rm -rf "$tmp"' EXIT

log "downloading $asset"
fetch "$url" "$tmp/$asset" || die "download failed — is there a published release?
  see https://github.com/$repo/releases"

# Verify the published checksum when a checksum tool is available.
if fetch "$url.sha256" "$tmp/$asset.sha256" 2>/dev/null; then
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$tmp" && sha256sum -c "$asset.sha256") || die "checksum mismatch"
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$tmp" && shasum -a 256 -c "$asset.sha256") || die "checksum mismatch"
    else
        log "no sha256 tool found — skipping checksum verification"
    fi
fi

tar -xzf "$tmp/$asset" -C "$tmp"

mkdir -p "$install_dir"
mv "$tmp/cl2metal" "$install_dir/cl2metal"
chmod +x "$install_dir/cl2metal"

case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) log "installed to $install_dir, which is not on your PATH — add it with:
  export PATH=\"$install_dir:\$PATH\"" ;;
esac

log "installed $("$install_dir/cl2metal" --version 2>/dev/null || echo cl2metal)"

# Offer the desktop entry. The script itself arrives on stdin with
# curl | sh, so the answer must be read from the terminal; the default
# is yes when there is no terminal to ask (automation, pipes).
answer="y"
if [ -r /dev/tty ]; then
    printf 'cl2metal: add the application-menu entry? [Y/n] '
    read -r answer < /dev/tty || answer="y"
fi
case "$answer" in
    [nN]*)
        log "skipped - run 'cl2metal --install-desktop' any time"
        ;;
    *)
        if "$install_dir/cl2metal" --install-desktop; then
            log "the application-menu entry is in place"
        else
            log "the desktop entry failed - run 'cl2metal --install-desktop' to retry"
        fi
        ;;
esac
