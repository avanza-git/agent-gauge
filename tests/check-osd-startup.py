"""KDE/X11 integration check. Opens temporary OSDs; never contacts provider APIs."""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time

binary = str(Path('target/release/agent-gauge').resolve())


def state_for(pid):
    clients = subprocess.check_output(['xprop', '-root', '_NET_CLIENT_LIST'], text=True)
    for xid in re.findall(r'0x[0-9a-fA-F]+', clients):
        info = subprocess.run(['xprop', '-id', xid, '_NET_WM_PID', '_NET_WM_STATE'],
                              capture_output=True, text=True).stdout
        if re.search(r'_NET_WM_PID\(CARDINAL\) = ' + str(pid) + r'\b', info):
            return info
    return None


for enabled in (True, True, False):
    with tempfile.TemporaryDirectory(prefix='osd-startup-') as directory:
        root = Path(directory)
        runtime = root / 'runtime'
        runtime.mkdir(mode=0o700)
        config = root / 'config.json'
        config.write_text(json.dumps({
            'providers': [], 'claude_dir': str(root / 'claude'),
            'notifications_enabled': False, 'force_x11': True,
            'osd_always_on_top': enabled,
        }))
        env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), XDG_CACHE_HOME=str(root / 'cache'))
        process = subprocess.Popen([binary, '--config', str(config)], env=env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 8
            state = None
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise AssertionError(process.communicate()[1].decode())
                state = state_for(process.pid)
                if state is not None and ('_NET_WM_STATE_ABOVE' in state) == enabled:
                    break
                time.sleep(.1)
            assert state is not None and ('_NET_WM_STATE_ABOVE' in state) == enabled, state
            # Remain correct after the full startup retry schedule has finished.
            time.sleep(5)
            state = state_for(process.pid)
            assert state is not None and ('_NET_WM_STATE_ABOVE' in state) == enabled, state
            assert '_NET_WM_STATE_SKIP_TASKBAR' in state, state
            print(f'Always on top enabled={enabled}: correct at launch and after startup settles.', flush=True)
        finally:
            process.terminate()
            process.communicate(timeout=5)
