#!/usr/bin/env python3
"""Run an isolated Weston headless compositor from locally extracted packages.

No physical display backend or desktop socket is used. Requires Weston14 and
libweston14 packages extracted into --root, without installing system packages.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--socket', default='bench-0012')
    parser.add_argument('--shell', choices=['desktop', 'kiosk'], default='desktop')
    parser.add_argument('--width', type=int, default=1200)
    parser.add_argument('--height', type=int, default=900)
    args = parser.parse_args()
    root, runtime, output = (p.resolve() for p in (args.root, args.runtime, args.output))
    if '/' in args.socket or not args.socket:
        parser.error('--socket must be a single filename')
    runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    if runtime.stat().st_uid != os.getuid():
        parser.error('runtime directory must belong to current user')
    runtime.chmod(0o700)
    if (runtime / args.socket).exists():
        parser.error('socket already exists; do not replace a running compositor')
    output.mkdir(parents=True, exist_ok=False)
    library = root / 'usr/lib/x86_64-linux-gnu'
    directories = [library / 'libweston-14', library / 'weston']
    modules = sorted(p for directory in directories for p in directory.glob('*.so'))
    executable = root / 'usr/bin/weston'
    env = os.environ.copy()
    for key in ['DISPLAY', 'WAYLAND_DISPLAY', 'WAYLAND_SOCKET']:
        env.pop(key, None)
    env.update(XDG_RUNTIME_DIR=str(runtime), LD_LIBRARY_PATH=f'{library}:{library / "weston"}',
               WESTON_MODULE_MAP=';'.join(f'{p.name}={p}' for p in modules),
               WESTON_DATA_DIR=str(root / 'usr/share/weston'))
    # Upstream module-map syntax:
    # https://cgit.freedesktop.org/wayland/weston/tree/libweston/compositor.c
    if args.width < 800 or args.height < 720:
        parser.error('virtual output must fit the800x720 comparison client')
    config = output / 'weston.ini'
    config.write_text(f'[shell]\nclient={root / "usr/libexec/weston-desktop-shell"}\npanel-position=none\nlocking=false\n[input-method]\npath={root / "usr/libexec/weston-keyboard"}\n')
    command = [str(executable), '--backend=headless', '--renderer=gl', f'--shell={args.shell}',
               f'--config={config}', f'--socket={args.socket}', f'--width={args.width}', f'--height={args.height}',
               '--scale=1', '--idle-time=0', f'--log={output / "weston.log"}']
    manifest = {'command': command, 'privateSocket': str(runtime / args.socket),
                'configurationSha256': hashlib.sha256(config.read_bytes()).hexdigest(),
                'binarySha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
                'moduleSha256': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in modules},
                'compositorIncludedInAppProcessTotals': False,
                'virtualOutput': [args.width, args.height],
                'scope': 'Virtual output only; independently verify each client viewport800x720. Inspect weston.log and each client backend before labeling GPU. Offscreen frame callbacks do not establish physical monitor presentation.'}
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    result = subprocess.run(command, env=env, check=False)
    (output / 'exit.json').write_text(json.dumps({'returncode': result.returncode}) + '\n')
    return result.returncode


if __name__ == '__main__':
    raise SystemExit(main())
