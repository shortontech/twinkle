#!/usr/bin/env python3
"""Serial controlled before/after trials for a native static GPU fixture."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
for name in ['before-binary', 'after-binary', 'before-source', 'after-source', 'wayland-socket', 'output']:
    parser.add_argument('--' + name, type=Path, required=True)
parser.add_argument('--fixture', required=True)
parser.add_argument('--expected-physical-viewport', required=True)
parser.add_argument('--scope', required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
socket = args.wayland_socket.resolve()
if not socket.is_socket():
    parser.error('Wayland socket unavailable')
viewport = [int(value) for value in args.expected_physical_viewport.split(',')]
if len(viewport) != 2 or any(value <= 0 for value in viewport):
    parser.error('--expected-physical-viewport must be WIDTH,HEIGHT')
output = args.output.resolve()
output.mkdir()
sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
binaries = {'before': args.before_binary.resolve(), 'after': args.after_binary.resolve()}
sources = {'before': args.before_source.resolve(), 'after': args.after_source.resolve()}
protocol = {
    'status': 'running', 'pairs': 5, 'fixture': args.fixture, 'runs': [],
    'binarySha256': {key: sha(path) for key, path in binaries.items()},
    'warmupSeconds': 3, 'processSamplesPerRun': 11, 'scope': args.scope,
    'waylandSocket': str(socket), 'expectedPhysicalViewport': viewport,
    'compositorIncludedInProcessTotals': False,
}

def wait_for(path, process):
    deadline = time.monotonic() + 15
    while not path.exists():
        if process.poll() is not None or time.monotonic() > deadline:
            raise RuntimeError(f'Missing {path}')
        time.sleep(.05)

try:
    for pair in range(1, 6):
        for variant in (['before', 'after'] if pair % 2 else ['after', 'before']):
            run = output / f'{pair:02}-{variant}'
            env = os.environ.copy()
            for key in ['DISPLAY', 'WAYLAND_SOCKET', 'TWINKLE_PIPELINE_TRACE', 'TWINKLE_JSX_TRACE',
                        'TWINKLE_BENCH_JSX_STAGES', 'TWINKLE_BENCH_OWNERSHIP', 'TWINKLE_RENDERER']:
                env.pop(key, None)
            env.update(XDG_RUNTIME_DIR=str(socket.parent), WAYLAND_DISPLAY=socket.name,
                       TWINKLE_BACKEND_REPORT=str(run / 'backend.txt'),
                       TWINKLE_BENCH_SAMPLER_READY=str(run / 'process/sampling-ready.json'))
            print(f'Pair{pair}: {variant}', flush=True)
            run.mkdir()
            with (run / 'stdout.log').open('w') as stdout, (run / 'stderr.log').open('w') as stderr:
                process = subprocess.Popen(
                    [str(binaries[variant]), '--fixture', args.fixture, '--output', str(run / 'trace')],
                    cwd=root, env=env, start_new_session=True, stdout=stdout, stderr=stderr)
                try:
                    wait_for(run / 'trace/ready.json', process)
                    command = ['python3', 'scripts/live_pipeline_sample.py', '--attach', str(process.pid),
                               '--output', str(run / 'process'), '--source', str(sources[variant]),
                               '--label', f'{args.fixture}-{variant}', '--warmup', '3', '--seconds', '11']
                    result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True)
                    (output / f'{pair:02}-{variant}.log').write_text(result.stdout + result.stderr)
                    if result.returncode:
                        raise RuntimeError(f'{variant} sampling failed')
                    wait_for(run / 'trace/fixture.json', process)
                finally:
                    if process.poll() is None:
                        os.killpg(process.pid, signal.SIGTERM)
                        try:
                            process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            os.killpg(process.pid, signal.SIGKILL)
                            process.wait()
            summary = json.loads((run / 'process/summary.json').read_text())
            fixture = json.loads((run / 'trace/fixture.json').read_text())
            window = json.loads((run / 'trace/ready.json').read_text())
            backend = (run / 'backend.txt').read_text()
            if summary['status'] != 'complete' or summary['samples'] != 11 or summary['background_interference_samples']:
                raise RuntimeError('Incomplete or externally disturbed trial')
            if window['physicalViewport'] != viewport or window['scale'] != 1:
                raise RuntimeError('Window mismatch')
            if 'backend_gpu' not in backend or 'backend_software' in backend:
                raise RuntimeError('Backend mismatch')
            protocol['runs'].append({
                'pair': pair, 'variant': variant, 'run': run.name, 'process': summary['metrics'],
                'window': window, 'nodeCount': fixture.get('nodeCount'),
                'fixtureShapeSha256': hashlib.sha256(json.dumps({key: fixture.get(key) for key in
                    ['fixture', 'nodeCount', 'retainedNodes', 'paintPrimitives', 'accessibilityNodes']},
                    sort_keys=True).encode()).hexdigest(),
            })
            if sha(binaries[variant]) != protocol['binarySha256'][variant]:
                raise RuntimeError('Binary changed')
            (output / 'protocol.json').write_text(json.dumps(protocol, indent=2) + '\n')
        before = next(run for run in protocol['runs'][-2:] if run['variant'] == 'before')
        after = next(run for run in protocol['runs'][-2:] if run['variant'] == 'after')
        if (before['fixtureShapeSha256'] != after['fixtureShapeSha256']
                or before['window'] != after['window']):
            raise RuntimeError('Paired fixture/window mismatch')
    protocol['status'] = 'complete'
except Exception as error:
    protocol.update(status='incomplete', error=str(error))
    raise
finally:
    (output / 'protocol.json').write_text(json.dumps(protocol, indent=2) + '\n')
