#!/usr/bin/env python3
"""Serial controlled before/after trials for the three cached native getters."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
for name in ['before-binary','after-binary','before-source','after-source','wayland-socket','output']:
    parser.add_argument('--'+name, type=Path, required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
socket = args.wayland_socket.resolve()
if not socket.is_socket():
    parser.error('private compositor socket unavailable')
output = args.output.resolve(); output.mkdir()
trace = root / 'benchmarks/0012/traces/blocks-behavior.json'
sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
binaries = {'before':args.before_binary.resolve(),'after':args.after_binary.resolve()}
sources = {'before':args.before_source.resolve(),'after':args.after_source.resolve()}
protocol = {'status':'running','pairs':5,'runs':[], 'binarySha256':{k:sha(p) for k,p in binaries.items()},
            'traceSha256':sha(trace),'warmupSeconds':3,'processSamplesPerRun':11,'traceSteps':47,
            'scope':'Normal releasejsx builds; only three full-inspection native getters reverted in before source. Detailed endpoint ownership walks disabled in both. Shared private compositor excluded; no concurrent builds/benchmarks.',
            'privateSocket':str(socket)}
try:
    for pair in range(1,6):
        for variant in (['before','after'] if pair%2 else ['after','before']):
            run = output / f'{pair:02}-{variant}'
            env = os.environ.copy()
            for key in ['DISPLAY','WAYLAND_SOCKET','TWINKLE_PIPELINE_TRACE','TWINKLE_JSX_TRACE','TWINKLE_BENCH_JSX_STAGES','TWINKLE_BENCH_OWNERSHIP','TWINKLE_RENDERER']:
                env.pop(key,None)
            env.update(XDG_RUNTIME_DIR=str(socket.parent),WAYLAND_DISPLAY=socket.name,
                       TWINKLE_BACKEND_REPORT=str(run/'backend.txt'),TWINKLE_BENCH_SAMPLER_READY=str(run/'sampling-ready.json'))
            command = ['python3','scripts/live_pipeline_sample.py','--output',str(run),'--source',str(sources[variant]),
                       '--label',f'cached-native-getters-{variant}','--warmup','3','--seconds','11','--',
                       str(binaries[variant]),'--replay',str(trace),'--output',str(run/'trace')]
            print(f'Pair{pair}: {variant}',flush=True)
            result = subprocess.run(command,cwd=root,env=env,capture_output=True,text=True)
            (output/f'{pair:02}-{variant}.log').write_text(result.stdout+result.stderr)
            if result.returncode:
                raise RuntimeError(f'{variant} failed')
            replay = json.loads((run/'trace/replay.json').read_text())
            summary = json.loads((run/'summary.json').read_text())
            backend = (run/'backend.txt').read_text()
            if summary['status']!='complete' or summary['samples']!=11 or summary['background_interference_samples']:
                raise RuntimeError('Incomplete or externally disturbed trial')
            if replay['nativeResources'] is not None or replay['jsxRuntime'] is not None:
                raise RuntimeError('Expensive endpoint observers unexpectedly enabled')
            if len(replay['steps'])!=47 or replay['nativeWindow']['physicalViewport']!=[800,720] or replay['scale']!=1:
                raise RuntimeError('Trace/window mismatch')
            if 'backend_gpu' not in backend or 'backend_software' in backend:
                raise RuntimeError('Backend mismatch')
            service = sorted(row['serviceUs'] for row in replay['steps'])
            protocol['runs'].append({'pair':pair,'variant':variant,'run':run.name,'process':summary['metrics'],
                'serviceUs':{'sum':sum(service),'p50':service[23],'p95':service[math.ceil(len(service)*.95)-1]},
                'nativeWindow':replay['nativeWindow'],'ambientCpu':summary['system_cpu_percent_all_cores'],
                'finalStateSha256':hashlib.sha256(json.dumps(replay['finalState'],sort_keys=True).encode()).hexdigest()})
            if sha(binaries[variant])!=protocol['binarySha256'][variant]:
                raise RuntimeError('Binary changed')
            (output/'protocol.json').write_text(json.dumps(protocol,indent=2)+'\n')
        a,b=protocol['runs'][-2:]
        if a['finalStateSha256']!=b['finalStateSha256'] or a['nativeWindow']['focused']!=b['nativeWindow']['focused']:
            raise RuntimeError('Paired state/focus mismatch')
    protocol['status']='complete'
except Exception as error:
    protocol.update(status='incomplete',error=str(error));raise
finally:
    (output/'protocol.json').write_text(json.dumps(protocol,indent=2)+'\n')
