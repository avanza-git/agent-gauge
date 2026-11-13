#!/usr/bin/env bash
# Install into the current user's KDE session. Re-running updates existing files.
set -euo pipefail
project_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
package_id=com.github.agentusage.panel
config_root=${XDG_CONFIG_HOME:-$HOME/.config}
data_root=${XDG_DATA_HOME:-$HOME/.local/share}

export PATH="$HOME/.cargo/bin:$PATH"
for tool in cargo python3 systemctl kpackagetool6; do
    command -v "$tool" >/dev/null || { echo "Missing required tool: $tool" >&2; exit 1; }
done
# Check session access before making any installation changes.
systemctl --user show-environment >/dev/null
cargo build --release --manifest-path "$project_dir/Cargo.toml" --offline

# Retire the old service without removing its unit or user settings.
systemctl --user disable --now claude-usage.service 2>/dev/null || true
systemctl --user stop agent-gauge.service 2>/dev/null || true
# The desktop mode and service share an advisory lock. Stop only our own executable.
python3 - "$config_root" <<'PY'
import fcntl, json, os, pathlib, shutil, signal, sys, time
root = pathlib.Path(sys.argv[1])
cache_root = pathlib.Path(os.environ.get('XDG_CACHE_HOME') or pathlib.Path.home() / '.cache')
for name in ('claude-usage', 'agent-gauge'):
    lock = pathlib.Path(os.environ.get('XDG_RUNTIME_DIR') or cache_root / name) / (name + '.lock')
    if not lock.exists():
        continue
    with lock.open('r+') as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            pid = int(handle.read().strip())
            process = pathlib.Path('/proc') / str(pid)
            executable = pathlib.Path(os.readlink(process / 'exe').removesuffix(' (deleted)')).name
            if process.stat().st_uid != os.getuid() or executable not in ('claude-usage', 'agent-gauge'):
                raise SystemExit('Instance lock belongs to an unexpected process; refusing to stop it.')
            os.kill(pid, signal.SIGTERM)
            for _ in range(50):
                try:
                    fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except BlockingIOError:
                    time.sleep(.1)
            else:
                raise SystemExit('Desktop instance did not stop; installation halted.')
PY

# Export only reads local history; startup performs the shared data migration.
# Run before creating the new config so existing settings are preserved.
"$project_dir/target/release/agent-gauge" --export json --days 0 >/dev/null
python3 - "$config_root" <<'PY'
import json, os, pathlib, shutil, sys
root = pathlib.Path(sys.argv[1])
path = root / 'agent-gauge/config.json'
path.parent.mkdir(parents=True, exist_ok=True)
config = json.loads(path.read_text()) if path.exists() else {}
if not isinstance(config, dict):
    raise SystemExit('Invalid config.json; installation halted.')
providers = config.get('providers', ['claude'])
if not isinstance(providers, list):
    raise SystemExit('Invalid config.json; installation halted.')
for provider in ('claude', 'codex'):
    if provider not in providers:
        providers.append(provider)
config['providers'] = providers
if path.exists():
    backup = path.with_name('config.json.before-panel')
    if not backup.exists():
        shutil.copy2(path, backup)
temp = path.with_suffix('.json.tmp')
temp.write_text(json.dumps(config, indent=4) + '\n')
os.chmod(temp, 0o600)
os.replace(temp, path)
PY

install -Dm755 "$project_dir/target/release/agent-gauge" "$HOME/.local/bin/agent-gauge"
if [[ -d "$data_root/plasma/plasmoids/$package_id" ]]; then
    kpackagetool6 --type Plasma/Applet --upgrade "$project_dir/plasma/$package_id"
else
    kpackagetool6 --type Plasma/Applet --install "$project_dir/plasma/$package_id"
fi

mkdir -p "$config_root/systemd/user"
unit_path="$config_root/systemd/user/agent-gauge.service"
if [[ -f "$unit_path" && ! -f "$unit_path.before-panel" ]]; then
    cp -p -- "$unit_path" "$unit_path.before-panel"
fi
cat > "$unit_path" <<'UNIT'
[Unit]
Description=AgentGauge usage for the KDE panel
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart="%h/.local/bin/agent-gauge" --panel-service
Restart=on-failure
RestartSec=10

[Install]
WantedBy=graphical-session.target
UNIT
systemctl --user daemon-reload
systemctl --user enable --now agent-gauge.service

if command -v qdbus-qt6 >/dev/null; then
    qdbus-qt6 org.kde.plasmashell /PlasmaShell org.kde.PlasmaShell.evaluateScript "$(cat -- "$project_dir/scripts/add-panel-widget.js")"
else
    echo 'Installed. Add “AgentGauge” from the top panel’s Add Widgets menu.'
fi
systemctl --user --no-pager --lines=3 status agent-gauge.service
