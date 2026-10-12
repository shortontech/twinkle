#!/usr/bin/env python3
"""Serial, interleaved production trials; no build or expensive tracing work."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import time

p=argparse.ArgumentParser()
p.add_argument('--binary',type=Path,required=True)
p.add_argument('--output',type=Path,required=True)
p.add_argument('--pairs',type=int,default=5)
p.add_argument('--wayland-socket',type=Path)
p.add_argument('--react-window-size',default='832,784')
a=p.parse_args()
if a.pairs<5:p.error('at least five pairs required')
if a.wayland_socket and (not a.wayland_socket.is_absolute() or not a.wayland_socket.is_socket()):p.error('--wayland-socket must be an existing absolute socket')
root=Path(__file__).resolve().parents[1]
binary=a.binary.resolve();output=a.output.resolve();output.mkdir()
trace=root/'benchmarks/0012/traces/blocks-behavior.json'
sha=lambda path:hashlib.sha256(path.read_bytes()).hexdigest()
manifest={'status':'running','pairs':a.pairs,'binary':str(binary),'binarySha256':sha(binary),'traceSha256':sha(trace),
 'build':'release, twinkle-workbench features jsx; pipeline-attribution disabled (caller must provide preserved matching build)',
 'warmupSeconds':3,'sampleCountPerRun':11,'traceDurationMs':10000,
 'order':'native/react for even pair indices; react/native for odd indices; serial',
 'privateWaylandSocket':str(a.wayland_socket) if a.wayland_socket else None,
 'compositorIncluded':False,
 'limitations':['Service boundaries differ: native resolved frame versus React DOM commit. They are not update-to-display latency.',
 'Full process sampling window includes ten-second replay plus one-second end/idle tail; final report serialization is included. Detailed endpoint ownership/V8 walks must be disabled.',
 'Dedicated Chrome process tree; shared driver resources not included in PSS. Existing user apps remain open.'], 'runs':[]}
(output/'protocol.json').write_text(json.dumps(manifest,indent=2)+'\n')
try:
 for pair in range(a.pairs):
  for implementation in (['native','react'] if pair%2==0 else ['react','native']):
   run=output/f'{pair+1:02}-{implementation}'
   env=os.environ.copy()
   for key in ('TWINKLE_PIPELINE_TRACE','TWINKLE_JSX_TRACE','TWINKLE_BENCH_JSX_STAGES','TWINKLE_BENCH_OWNERSHIP','TWINKLE_RENDERER','TWINKLE_BENCH_SAMPLER_READY','TWINKLE_BACKEND_REPORT'):env.pop(key,None)
   if implementation=='native':
    env.pop('WAYLAND_SOCKET',None)
    if a.wayland_socket:
     env.pop('DISPLAY',None);env['XDG_RUNTIME_DIR']=str(a.wayland_socket.parent);env['WAYLAND_DISPLAY']=a.wayland_socket.name
    else:
     env.pop('WAYLAND_DISPLAY',None);env['WINIT_X11_SCALE_FACTOR']='1'
    env['TWINKLE_BENCH_SAMPLER_READY']=str(run/'sampling-ready.json');env['TWINKLE_BACKEND_REPORT']=str(run/'backend.txt')
    command=['python3','scripts/live_pipeline_sample.py','--output',str(run),'--label','native-normal-paired','--source',str(root),'--warmup','3','--seconds','11','--',str(binary),'--replay',str(trace),'--output',str(run/'trace')]
   else:
    command=['node','scripts/run_react_replay.mjs','--trace',str(trace),'--output',str(run),'--sample']
    if a.wayland_socket:command+=['--wayland-socket',str(a.wayland_socket),'--window-size',a.react_window_size]
   started=time.time();print(f'Pair {pair+1}: {implementation}',flush=True)
   result=subprocess.run(command,cwd=root,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
   (output/f'{pair+1:02}-{implementation}.log').write_text(result.stdout+result.stderr)
   if result.returncode:raise RuntimeError(f'{implementation} failed: {result.returncode}')
   process=run if implementation=='native' else run/'process'
   summary=json.loads((process/'summary.json').read_text());replay=json.loads((run/('trace/replay.json' if implementation=='native' else 'replay.json')).read_text())
   if summary['status']!='complete' or summary['samples']!=11 or len(replay['steps'])!=47:raise RuntimeError('incomplete trial')
   if summary.get('background_build_samples')!=0:raise RuntimeError('concurrent build detected or quiet-machine evidence unavailable')
   if summary.get('background_interference_samples')!=0:raise RuntimeError('external build/crash-reporting interference detected or unavailable')
   if implementation=='native':
    backend=(run/'backend.txt').read_text()
    if replay['nativeResources'] is not None or replay['jsxRuntime'] is not None:raise RuntimeError('Expensive endpoint observers enabled in normal timing run')
    if 'backend_gpu' not in backend or 'backend_software' in backend:raise RuntimeError('native backend mismatch or fallback')
    if 'surface_synchronization' not in backend:raise RuntimeError('Missing actual surface synchronization record')
    backend_identity={'report':'backend.txt','fallbackObserved':False}
   else:
    browser_manifest=json.loads((run/'manifest.json').read_text())
    if 'presentationSynchronization' not in browser_manifest:raise RuntimeError('Missing browser synchronization boundary')
    gpu=browser_manifest['gpuAtCompletion']['gpu']
    if browser_manifest['gpuFeatureStatusChanged'] or gpu['featureStatus']['gpu_compositing']!='enabled' or gpu['featureStatus']['rasterization']!='enabled':raise RuntimeError('React GPU backend changed or disabled')
    backend_identity={'featureStatus':gpu['featureStatus'],'devices':gpu['devices'],'glRenderer':gpu['auxAttributes'].get('glRenderer')}
   if replay['scale']!=1 or replay.get('logicalViewport',replay.get('viewport'))!=[800,720]:raise RuntimeError('viewport mismatch')
   window=replay.get('nativeWindow') if implementation=='native' else json.loads((run/'page-ready.json').read_text())
   if a.wayland_socket and (window is None or window.get('physicalViewport',window.get('viewport'))!=[800,720] or window.get('scale')!=1):raise RuntimeError('actual window metadata missing or mismatched')
   service=sorted(step['serviceUs'] for step in replay['steps'])
   record={'pair':pair+1,'implementation':implementation,'run':str(run.relative_to(output)),'startedEpochSeconds':started,'process':summary['metrics'],
     'serviceBoundary':replay['timingBoundary'],'serviceUs':{'sum':sum(service),'p50':service[len(service)//2],'p95':service[math.ceil(len(service)*.95)-1],'samples':len(service)},
     'nativeWindowOrReactPage':window,
     'backendIdentity':backend_identity,
     'quietMachineEvidence':{'backgroundBuildSamples':summary['background_build_samples'],'backgroundInterferenceSamples':summary['background_interference_samples'],'systemCpuPercentAllCores':summary['system_cpu_percent_all_cores'],'boundary':summary['quiet_machine_boundary']},
     'quantiles':'sorted nearest rank (median middle for odd sample count)', 'finalStateSha256':hashlib.sha256(json.dumps(replay['finalState'],sort_keys=True).encode()).hexdigest()}
   manifest['runs'].append(record)
   if sha(binary)!=manifest['binarySha256']:raise RuntimeError('native binary changed during trials')
   (output/'protocol.json').write_text(json.dumps(manifest,indent=2)+'\n')
  pair_runs=manifest['runs'][-2:]
  if pair_runs[0]['finalStateSha256']!=pair_runs[1]['finalStateSha256']:raise RuntimeError('paired final states differ')
  if a.wayland_socket and pair_runs[0]['nativeWindowOrReactPage']['focused']!=pair_runs[1]['nativeWindowOrReactPage']['focused']:raise RuntimeError('paired actual window focus differs')
 manifest['status']='complete'
except Exception as error:
 manifest['status']='incomplete';manifest['error']=str(error);raise
finally:(output/'protocol.json').write_text(json.dumps(manifest,indent=2)+'\n')
