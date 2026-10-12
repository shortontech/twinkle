#!/usr/bin/env python3
"""Five serial pairs of each frozen scene on an existing private GPU display."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--wayland-socket', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
socket = args.wayland_socket.resolve()
if not socket.is_socket():
    parser.error('Private compositor unavailable')
binary = args.binary.resolve()
output = args.output.resolve(); output.mkdir()
sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
trace = root / 'benchmarks/0012/traces/blocks-behavior.json'
protocol = {'status': 'running', 'pairsPerFixture': 5, 'runs': [],
    'binary': str(binary), 'binarySha256': sha(binary), 'traceSha256': sha(trace),
    'privateSocket': str(socket), 'warmupSeconds': 3, 'samplesPerRun': 11,
    'scope': 'Unchanged empty, 50/200/800 cells and full accessible form. Static admitted scenes, not game replay. Normal releasejsx binary; endpoint native frame/V8 inspection happens after sample window. Fresh dedicated Chrome process tree; controller and shared compositor excluded. Natural collection; no forced GC.'}

def wait_for(path, proc):
    deadline = time.monotonic() + 15
    while not path.exists():
        if proc.poll() is not None or time.monotonic() > deadline:
            raise RuntimeError(f'Missing {path}')
        time.sleep(.05)

try:
    for pair in range(1, 6):
        # Rotate scene order; all measurements are serial.
        scenes = ['empty', 'cells-50', 'cells-200', 'cells-800', 'rich-form']
        scenes = scenes[pair-1:] + scenes[:pair-1]
        for fixture in scenes:
            for implementation in (['native', 'react'] if pair % 2 else ['react', 'native']):
                run = output / f'{pair:02}-{fixture}-{implementation}'
                print(f'Pair{pair} {fixture} {implementation}', flush=True)
                env = os.environ.copy()
                for key in ['DISPLAY', 'WAYLAND_SOCKET', 'TWINKLE_PIPELINE_TRACE', 'TWINKLE_JSX_TRACE', 'TWINKLE_BENCH_JSX_STAGES', 'TWINKLE_BENCH_OWNERSHIP', 'TWINKLE_RENDERER']:
                    env.pop(key, None)
                env.update(XDG_RUNTIME_DIR=str(socket.parent), WAYLAND_DISPLAY=socket.name)
                if implementation == 'native':
                    run.mkdir()
                    env.update(TWINKLE_BACKEND_REPORT=str(run/'backend.txt'), TWINKLE_BENCH_SAMPLER_READY=str(run/'process/sampling-ready.json'))
                    with (run/'stdout.log').open('w') as stdout, (run/'stderr.log').open('w') as stderr:
                        proc = subprocess.Popen([str(binary), '--fixture', fixture, '--output', str(run/'trace')], cwd=root, env=env, start_new_session=True, stdout=stdout, stderr=stderr)
                        try:
                            wait_for(run/'trace/ready.json', proc)
                            command = ['python3', 'scripts/live_pipeline_sample.py', '--attach', str(proc.pid), '--output', str(run/'process'), '--source', str(root), '--label', f'native-static-{fixture}', '--warmup', '3', '--seconds', '11']
                            result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True)
                            (run/'sampler.log').write_text(result.stdout + result.stderr)
                            if result.returncode:
                                raise RuntimeError('Native sampling failed')
                            wait_for(run/'trace/fixture.json', proc)
                        finally:
                            if proc.poll() is None:
                                os.killpg(proc.pid, signal.SIGTERM)
                                try:
                                    proc.wait(timeout=5)
                                except subprocess.TimeoutExpired:
                                    os.killpg(proc.pid, signal.SIGKILL); proc.wait()
                    report = json.loads((run/'trace/fixture.json').read_text())
                    window = json.loads((run/'trace/ready.json').read_text())
                    backend = (run/'backend.txt').read_text()
                    if 'backend_gpu' not in backend or 'backend_software' in backend:
                        raise RuntimeError('Native fallback')
                    if 'surface_synchronization' not in backend:
                        raise RuntimeError('Missing actual surface synchronization record')
                else:
                    command = ['node', 'scripts/run_react_replay.mjs', '--trace', str(trace), '--output', str(run), '--sample', '--fixture', fixture, '--wayland-socket', str(socket), '--window-size', '832,784']
                    result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True)
                    (output/f'{run.name}.log').write_text(result.stdout + result.stderr)
                    if result.returncode:
                        raise RuntimeError('React sampling failed')
                    report = json.loads((run/'replay.json').read_text())
                    manifest = json.loads((run/'manifest.json').read_text())
                    if 'presentationSynchronization' not in manifest:
                        raise RuntimeError('Missing browser synchronization boundary')
                    window = manifest['actualPage']
                    features = manifest['gpuAtCompletion']['gpu']['featureStatus']
                    if not manifest['assetsUnchanged'] or manifest['gpuFeatureStatusChanged'] or any(features[k] != 'enabled' for k in ['gpu_compositing', 'rasterization']):
                        raise RuntimeError('React backend/assets mismatch')
                summary = json.loads((run/'process/summary.json').read_text())
                if summary['status'] != 'complete' or summary['samples'] != 11 or summary['background_interference_samples']:
                    raise RuntimeError('Incomplete/disturbed sampling')
                if window.get('physicalViewport', window.get('viewport')) != [800, 720] or window['scale'] != 1:
                    raise RuntimeError('Actual viewport mismatch')
                if fixture.startswith('cells-') and implementation == 'react' and report['cellCount'] != int(fixture[6:]):
                    raise RuntimeError('Cells reduced')
                protocol['runs'].append({'pair': pair, 'fixture': fixture, 'implementation': implementation, 'run': run.name,
                    'process': summary['metrics'], 'ambientCpu': summary['system_cpu_percent_all_cores'], 'window': window,
                    'nodeCount': report.get('nodeCount', report.get('hostNodeCount')), 'report': report})
                if sha(binary) != protocol['binarySha256']:
                    raise RuntimeError('Binary changed')
                (output/'protocol.json').write_text(json.dumps(protocol, indent=2)+'\n')
    protocol['status'] = 'complete'
except Exception as error:
    protocol.update(status='incomplete', error=str(error)); raise
finally:
    (output/'protocol.json').write_text(json.dumps(protocol, indent=2)+'\n')
