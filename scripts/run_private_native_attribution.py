#!/usr/bin/env python3
"""Own a private attribution app until normal exit, preserving worker teardown."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--trace', type=Path, required=True)
parser.add_argument('--wayland-socket', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--software', action='store_true')
parser.add_argument('--ownership', action='store_true')
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
binary, trace, socket, output = [p.resolve() for p in (args.binary, args.trace, args.wayland_socket, args.output)]
if not socket.is_socket():
    parser.error('Private compositor unavailable')
output.mkdir()
env = os.environ.copy()
for key in ['DISPLAY', 'WAYLAND_SOCKET', 'TWINKLE_RENDERER', 'TWINKLE_BENCH_OWNERSHIP']:
    env.pop(key, None)
env.update(XDG_RUNTIME_DIR=str(socket.parent), WAYLAND_DISPLAY=socket.name,
    TWINKLE_PIPELINE_TRACE=str(output/'pipeline.jsonl'), TWINKLE_JSX_TRACE=str(output/'bridge.jsonl'),
    TWINKLE_BENCH_JSX_STAGES=str(output/'jsx-stages.jsonl'), TWINKLE_BACKEND_REPORT=str(output/'backend.txt'),
    TWINKLE_BENCH_SAMPLER_READY=str(output/'process/sampling-ready.json'), TWINKLE_BENCH_EXIT_AFTER_REPORT='1')
if args.software:
    env['TWINKLE_RENDERER'] = 'software'
if args.ownership:
    env['TWINKLE_BENCH_OWNERSHIP'] = '1'
command = [str(binary), '--replay', str(trace), '--output', str(output/'trace'), '--states']
receipt = {'status': 'running', 'binary': str(binary), 'binarySha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
    'traceSha256': hashlib.sha256(trace.read_bytes()).hexdigest(), 'command': command,
    'features': 'release jsx,pipeline-attribution,pipeline-allocator (caller must provide matching preserved build)',
    'mode': 'forced-software' if args.software else 'gpu',
    'scope': 'Opt-in pipeline/bridge/JSX/allocator attribution; clocks, counters, file writes and observers perturb CPU/GC. Attach sampler excludes controller. App exits via AdapterOutcome after trace report and complete sampler window, allowing worker join/cache retirement. Never use as normal timing series.'}
proc = None
try:
    with (output/'stdout.log').open('w') as stdout, (output/'stderr.log').open('w') as stderr:
        proc = subprocess.Popen(command, cwd=root, env=env, start_new_session=True, stdout=stdout, stderr=stderr)
        deadline = time.monotonic()+30
        while not (output/'trace/ready.json').exists():
            if proc.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError('Native startup failed')
            time.sleep(.05)
        duration = json.loads(trace.read_text())['durationMs']
        samples = math.ceil(duration/1000)+2
        sampler = ['python3', 'scripts/live_pipeline_sample.py', '--attach', str(proc.pid), '--source', str(root),
            '--output', str(output/'process'), '--label', f'native-private-{receipt["mode"]}-attribution', '--warmup', '3', '--seconds', str(samples)]
        sampled = subprocess.run(sampler, cwd=root, env=env, capture_output=True, text=True)
        (output/'sampler.log').write_text(sampled.stdout+sampled.stderr)
        if sampled.returncode:
            raise RuntimeError('Attribution sampler failed')
        code = proc.wait(timeout=15)
        receipt.update(rootPid=proc.pid, exitCode=code, samplerSamples=samples)
        if code:
            raise RuntimeError(f'Native app did not exit normally: {code}')
    report = json.loads((output/'trace/replay.json').read_text())
    summary = json.loads((output/'process/summary.json').read_text())
    backend = (output/'backend.txt').read_text()
    if report['status'] != 'complete' or summary['status'] != 'complete' or summary['samples'] != samples:
        raise RuntimeError('Incomplete attribution')
    if report['nativeWindow']['physicalViewport'] != [800, 720] or report['scale'] != 1:
        raise RuntimeError('Actual window mismatch')
    if args.software and ('backend_software' not in backend or r'requested_renderer=Some(\"software\")' not in backend or 'backend_gpu' in backend):
        raise RuntimeError('Forced software backend not verified')
    if not args.software and ('backend_gpu' not in backend or 'backend_software' in backend):
        raise RuntimeError('GPU backend mismatch')
    receipt.update(status='complete', rootPid=proc.pid, exitCode=code, samplerSamples=samples,
        nativeWindow=report['nativeWindow'], streamedSteps=report['streamedSteps'], traceSteps=report['totalTraceSteps'],
        backgroundInterferenceSamples=summary['background_interference_samples'])
except Exception as error:
    receipt.update(status='incomplete', error=str(error)); raise
finally:
    if proc is not None and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL); proc.wait()
    (output/'invocation.json').write_text(json.dumps(receipt, indent=2)+'\n')
