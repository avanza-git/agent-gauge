#!/usr/bin/env bash
# Install AgentGauge for the current macOS user. Re-running updates the install.
#
#   scripts/install-macos.sh                 build, install the app, start at login
#   scripts/install-macos.sh --no-autostart  install the app only (no LaunchAgent)
#   scripts/install-macos.sh --uninstall     stop it and remove the app, link and LaunchAgent
#
# Config, cache and usage history are never removed.
set -euo pipefail

project_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
bundle_id=com.github.agent-gauge
app_dir="$HOME/Applications/AgentGauge.app"
app_bin="$app_dir/Contents/MacOS/agent-gauge"
cli_link="$HOME/.local/bin/agent-gauge"
agent_plist="$HOME/Library/LaunchAgents/$bundle_id.plist"
log_dir="${XDG_CACHE_HOME:-$HOME/.cache}/agent-gauge"
domain="gui/$(id -u)"

autostart=1
case "${1:-}" in
    "") ;;
    --no-autostart) autostart=0 ;;
    --uninstall) autostart=-1 ;;
    -h | --help) sed -n '2,8p' "$0"; exit 0 ;;
    *) echo "Unknown option: $1 (try --help)" >&2; exit 2 ;;
esac

[ "$(uname -s)" = Darwin ] || { echo "This installer is for macOS." >&2; exit 1; }

stop_running() {
    launchctl bootout "$domain/$bundle_id" 2>/dev/null || true
    # A copy started by hand holds the single-instance lock; stop only our own binary.
    pkill -u "$(id -u)" -x agent-gauge 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        pgrep -u "$(id -u)" -x agent-gauge >/dev/null || return 0
        sleep 0.3
    done
}

if [ "$autostart" = -1 ]; then
    stop_running
    rm -f "$agent_plist"
    [ -L "$cli_link" ] && [ "$(readlink "$cli_link")" = "$app_bin" ] && rm -f "$cli_link"
    rm -rf "$app_dir"
    echo "AgentGauge removed. Config and cache in ~/.config/agent-gauge and $log_dir were kept."
    exit 0
fi

export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
command -v cargo >/dev/null || { echo "Missing required tool: cargo (install Rust first)" >&2; exit 1; }
cargo build --release --manifest-path "$project_dir/Cargo.toml"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$project_dir/Cargo.toml" | head -1)

stop_running

# --- App bundle ---------------------------------------------------------------
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources"
install -m 755 "$project_dir/target/release/agent-gauge" "$app_bin"
cat > "$app_dir/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>$bundle_id</string>
    <key>CFBundleName</key><string>AgentGauge</string>
    <key>CFBundleDisplayName</key><string>AgentGauge</string>
    <key>CFBundleExecutable</key><string>agent-gauge</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
codesign --force --sign - "$app_dir" >/dev/null 2>&1 || echo "warning: ad-hoc codesign failed; continuing" >&2
# Register the bundle so notifications can be attributed to it.
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app_dir" 2>/dev/null || true

# --- CLI link (same build for --statusline / --json) ---------------------------
mkdir -p "$(dirname "$cli_link")"
if [ -e "$cli_link" ] && [ ! -L "$cli_link" ] && [ ! -e "$cli_link.before-macos" ]; then
    mv "$cli_link" "$cli_link.before-macos"
fi
ln -sfn "$app_bin" "$cli_link"

# --- LaunchAgent ----------------------------------------------------------------
if [ "$autostart" = 1 ]; then
    mkdir -p "$(dirname "$agent_plist")" "$log_dir"
    cat > "$agent_plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>$bundle_id</string>
    <key>ProgramArguments</key>
    <array><string>$app_bin</string></array>
    <key>RunAtLoad</key><true/>
    <!-- Restart after a crash, but a Quit from the menu stays quit. -->
    <key>KeepAlive</key>
    <dict><key>SuccessfulExit</key><false/></dict>
    <key>ProcessType</key><string>Interactive</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:$HOME/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
    <key>StandardOutPath</key><string>$log_dir/widget.log</string>
    <key>StandardErrorPath</key><string>$log_dir/widget.log</string>
</dict>
</plist>
PLIST
    launchctl bootstrap "$domain" "$agent_plist"
    echo "AgentGauge installed to $app_dir and started (launches at login)."
else
    rm -f "$agent_plist"
    echo "AgentGauge installed to $app_dir. Start it with: open \"$app_dir\""
fi
echo "CLI: $cli_link  (make sure ~/.local/bin is on your PATH)"
