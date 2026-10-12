#!/usr/bin/env python3
"""Temporarily suppress a verified looping launcher and restore its socket."""
import argparse
import json
from pathlib import Path
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--receipt', type=Path, required=True)
parser.add_argument('command', nargs=argparse.REMAINDER)
args = parser.parse_args()
command = args.command[1:] if args.command[:1] == ['--'] else args.command
if not command:
    parser.error('Command required')
unit = 'drkonqi-coredump-launcher.socket'
original = subprocess.check_output(['systemctl', '--user', 'is-active', unit], text=True).strip()
if original != 'active':
    raise RuntimeError('Expected previously recorded active socket')
subprocess.run(['systemctl', '--user', 'stop', unit], check=True)
receipt = {'originalState': original, 'startedEpochSeconds': time.time(), 'command': command,
    'reason': 'Known private Chrome post-sampling shutdown SIGSEGV triggers self-crashing KDE launcher. Temporary socket stop only, no permanent mask/enablement change.'}
args.receipt.write_text(json.dumps(receipt, indent=2)+'\n')
try:
    result = subprocess.run(command)
    receipt['commandExitCode'] = result.returncode
finally:
    try:
        quiet, deadline = None, time.monotonic()+30
        while time.monotonic() < deadline:
            names = subprocess.check_output(['ps', '-eo', 'comm='], text=True).splitlines()
            if any(n.strip() in {'apport', 'drkonqi-coredum', 'systemd-coredum'} for n in names):
                quiet = None
            else:
                quiet = quiet or time.monotonic()
                if time.monotonic()-quiet >= 2:
                    break
            time.sleep(.5)
        receipt['coreProcessingDrained'] = quiet is not None and time.monotonic()-quiet >= 2
    finally:
        restored = subprocess.run(['systemctl', '--user', 'start', unit])
        receipt.update(restoredEpochSeconds=time.time(), restoreExitCode=restored.returncode,
            state=subprocess.check_output(['systemctl', '--user', 'is-active', unit], text=True).strip())
        args.receipt.write_text(json.dumps(receipt, indent=2)+'\n')
raise SystemExit(result.returncode)
