#!/usr/bin/env python3
"""Keep controlled dispatch cadence, CPU service, and API presentation distinct."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import statistics


def distribution(values):
    values = sorted(values)
    return {'samples': len(values), 'min': min(values), 'median': statistics.median(values),
            'p95': values[math.ceil(.95*len(values))-1], 'max': max(values)} if values else None


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--pairs', type=Path, required=True)
parser.add_argument('--native-pipeline', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
protocol = json.loads((args.pairs/'protocol.json').read_text())
if protocol['status'] != 'complete':
    raise ValueError('Paired series incomplete')
runs = []
for run in protocol['runs']:
    replay_path = args.pairs/run['run']/('trace/replay.json' if run['implementation'] == 'native' else 'replay.json')
    replay = json.loads(replay_path.read_text())
    steps = replay['steps']
    if len(steps) != len(replay['trace']['steps']):
        raise ValueError('Missing steps')
    actual_intervals = [b['actualMs']-a['actualMs'] for a, b in zip(steps, steps[1:])]
    runs.append({'pair': run['pair'], 'implementation': run['implementation'],
        'replaySha256': hashlib.sha256(replay_path.read_bytes()).hexdigest(), 'steps': len(steps),
        'scriptedDurationMs': replay['trace']['durationMs'],
        'scriptedStepsPerSecond': len(steps)/(replay['trace']['durationMs']/1000),
        'scheduleOffsetMs': distribution([s['actualMs']-s['atMs'] for s in steps]),
        'actualDispatchIntervalsMs': distribution(actual_intervals),
        'cpuServiceUs': distribution([s['serviceUs'] for s in steps]),
        'cpuServiceBoundary': replay['timingBoundary'],
        'polls': sum(s['poll'] for s in steps),
        'publicationAttempts': sum(s['publicationAttempts'] for s in steps),
        'observableChanges': {'knownChanged': sum(s['observableChanged'] is True for s in steps),
                              'unknownSteps': sum(s['observableChanged'] is None for s in steps),
                              'boundary': 'Normal native timing disables state observer; unknown is not zero.'},
        'reactPaintOpportunityMs': distribution([s['paintOpportunityMs'] for s in steps if s.get('paintOpportunityMs') is not None]),
        'completeUpdateToPhysicalDisplayMs': None})
events = [json.loads(line) for line in args.native_pipeline.read_text().splitlines()]
ends = [e for e in events if e['event'] == 'gpu_present_end']
frames = [e for e in events if e['event'] == 'gpu_frame']
result = {'status': 'passed', 'normalRuns': runs,
    'separateNativeAttributionApiCadence': {'pipelineSha256': hashlib.sha256(args.native_pipeline.read_bytes()).hexdigest(),
        'apiPresentEndCount': len(ends), 'gpuFrameCount': len(frames),
        'apiEndIntervalsMs': distribution([(b['elapsed_us']-a['elapsed_us'])/1000 for a, b in zip(ends, ends[1:])]),
        'presentCallUs': distribution([e['present_call_us'] for e in frames]),
        'physicalDisplayedFrameCadenceHz': None,
        'scope': 'Entire attribution invocation includes startup and warmup. CPU API call completion; logging overhead included. GPU completion, scanout and physical compositor display timing unavailable.'},
    'boundaries': ['Scripted47/10s rate is workload cadence, not maximum throughput. Idle/no-op steps remain.',
                   'Schedule offset records actual dispatch start minus fixed trace timestamp; can be slightly negative from browser timer/clock precision. Never clamp or silently remove it.',
                   'Native service ends at resolved frame; React service ends at DOM commit. These distributions do not compare complete update-to-present latency.',
                   'React requestAnimationFrame paint opportunity is neither raster completion nor physical display.',
                   'Attribution API event intervals are separate from normal trials; do not infer matched refresh or sum nested stage medians.']}
args.output.write_text(json.dumps(result, indent=2)+'\n')
print(json.dumps({'status': result['status'], 'normalRuns': len(runs), 'apiPresentEnds': len(ends)}))
