#!/usr/bin/env bash
# Install peat — agent memory as a fold
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/flowerornament/peat/main/install.sh | bash
#
# Installs to ~/.local/bin by default. Set INSTALL_DIR to override:
#   curl -fsSL ... | INSTALL_DIR=/usr/local/bin bash
#   curl -fsSL ... | bash -s -- --install-dir "$HOME/bin"

set -euo pipefail

REPO="flowerornament/peat"
INSTALL_DIR="${INSTALL_DIR:-${BIN_DIR:-$HOME/.local/bin}}"
REQUESTED_TAG=""
DRY_RUN=false
SUPPORTED_RELEASE_TARGETS=(
    "aarch64-apple-darwin"
    "x86_64-unknown-linux-gnu"
    "aarch64-unknown-linux-gnu"
)

info()  { printf '\033[1;34m%s\033[0m\n' "$*"; }
error() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

print_help() {
    cat <<'HELP'
Install peat — agent memory as a fold

Usage:
  install.sh [OPTIONS]

Options:
  --install-dir PATH   Install to PATH instead of ~/.local/bin
  --tag TAG            Install a specific release tag (for example v0.1.0)
  --hooks-print        After installing, print the Claude Code hook snippet
  --print-target       Print the detected release target and exit
  --dry-run            Print the install plan without downloading or writing
  -h, --help           Show this help

Environment:
  INSTALL_DIR          Install directory override (BIN_DIR is an alias)
HELP
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || error "Missing required command: $1"
}

detect_target() {
    local os arch
    os="$(uname -s)"; arch="$(uname -m)"
    case "$os/$arch" in
        Darwin/arm64)   echo "aarch64-apple-darwin" ;;
        Linux/x86_64)   echo "x86_64-unknown-linux-gnu" ;;
        Linux/aarch64)  echo "aarch64-unknown-linux-gnu" ;;
        *) error "No prebuilt binary for $os/$arch — build from source: cargo install --git https://github.com/$REPO" ;;
    esac
}

print_hooks=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --install-dir) [ "$#" -ge 2 ] || error "--install-dir requires a path"; INSTALL_DIR="$2"; shift 2 ;;
        --tag)         [ "$#" -ge 2 ] || error "--tag requires a tag"; REQUESTED_TAG="$2"; shift 2 ;;
        --hooks-print) print_hooks=true; shift ;;
        --print-target) detect_target; exit 0 ;;
        --dry-run)     DRY_RUN=true; shift ;;
        -h|--help)     print_help; exit 0 ;;
        *) error "Unknown option: $1" ;;
    esac
done

require_cmd curl
require_cmd tar

TARGET="$(detect_target)"
ok=false
for t in "${SUPPORTED_RELEASE_TARGETS[@]}"; do [ "$TARGET" = "$t" ] && ok=true; done
$ok || error "Unsupported release target: $TARGET"

TAG="$REQUESTED_TAG"
if [ -z "$TAG" ]; then
    info "Finding latest release..."
    TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | head -1 | cut -d'"' -f4)
    [ -n "$TAG" ] || error "Could not resolve the latest release tag"
fi

URL="https://github.com/$REPO/releases/download/$TAG/peat-$TARGET.tar.gz"

info "Install plan:"
printf '  target:  %s\n' "$TARGET"
printf '  tag:     %s\n' "$TAG"
printf '  binary:  %s/peat\n' "$INSTALL_DIR"

if $DRY_RUN; then info "Dry run — nothing written."; exit 0; fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
info "Downloading $URL"
curl -fsSL "$URL" -o "$tmpdir/peat.tar.gz"
tar xzf "$tmpdir/peat.tar.gz" -C "$tmpdir"
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmpdir/peat" "$INSTALL_DIR/peat"
info "Installed $("$INSTALL_DIR/peat" --version) to $INSTALL_DIR/peat"

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) info "Note: $INSTALL_DIR is not on your PATH." ;;
esac

if $print_hooks; then
    cat <<'HOOKS'

Claude Code hooks (merge into .claude/settings.json — see the repo's
hooks/README.md for the full moment-coverage matrix and Codex forms):

  SessionStart : write .peat/current-session; `peat brief` (stdout -> context)
  Stop         : `peat capture <transcript> --final-msg <last message>`,
                 then a once-per-session observation prompt
  PreCompact   : salvage `peat capture` before the context window is replaced
  SessionEnd   : salvage `peat capture` for /clear and other non-Stop endings

  https://github.com/flowerornament/peat/blob/main/hooks/README.md
HOOKS
fi
