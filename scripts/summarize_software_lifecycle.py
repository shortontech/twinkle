#!/usr/bin/env python3
"""Verify integrated app cycles, queue generations, and clean worker release."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('root', type=Path)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
root = args.root
invocation = json.loads((root/'invocation.json').read_text())
replay = json.loads((root/'trace/replay.json').read_text())
steps = [json.loads(l) for l in (root/'trace/steps.jsonl').read_text().splitlines()]
owners = [json.loads(l) for l in (root/'trace/ownership.jsonl').read_text().splitlines()]
events = [json.loads(l) for l in (root/'pipeline.jsonl').read_text().splitlines()]
if invocation['status'] != 'complete' or invocation['exitCode'] != 0 or invocation['mode'] != 'forced-software':
    raise ValueError('Verified clean software invocation required')
if len(steps) != 447 or replay['totalTraceSteps'] != 447 or not replay['streamedSteps'] or replay['steps']:
    raise ValueError('Full streamed lifecycle required')
if len(owners) != 102 or any(r['harnessStoredSteps'] for r in owners):
    raise ValueError('102 history-free ownership checkpoints required')
cycles = [r for r in owners if r['label'] == 'cycle-restart']
if len(cycles) != 100 or len({r['cycle'] for r in cycles}) != 100:
    raise ValueError('100 distinct cycles required')
queue = [r for r in events if r['event'] == 'software_queue_lifetime']
stops = [r for r in queue if r['transition'] == 'stop']
exits = [r for r in events if r['event'] == 'software_worker_exit']
released = [r for r in events if r['event'] == 'software_renderer_release']
if len(stops) != 1 or len(exits) != 1 or len(released) != 1:
    raise ValueError('One worker/surface retirement and normal stop required')
stop = stops[0]
if not stop['stopped'] or stop['active_generation'] is not None or stop['pending_generations'] or stop['completed_generations']:
    raise ValueError('Queue not empty at stop')
if stop['pending_command_vector_capacity_bytes'] or stop['completed_pixel_capacity_bytes'] or exits[0]['retained_renderers_released']:
    raise ValueError('Queue/renderer ownership not released')
for row in queue:
    if len(row['pending_generations']) > 1 or len(row['completed_generations']) > 1:
        raise ValueError('Single-surface queue exceeded one pending/completed slot')
    for generation in row['pending_generations']+row['completed_generations']+([row['active_generation']] if row['active_generation'] else []):
        if generation[0] != 1 or generation[1] < 1:
            raise ValueError('Invalid generation identity')
last = cycles[-20:]
stats = lambda values: {'min': min(values), 'max': max(values), 'last': values[-1]}
result = {'status': 'passed', 'traceSteps': len(steps), 'restartCycles': len(cycles), 'ownershipCheckpoints': len(owners),
    'normalExitCode': invocation['exitCode'], 'eventCounts': dict(Counter(r['event'] for r in events)),
    'queueTransitionCounts': dict(Counter(r['transition'] for r in queue)),
    'queueHighWater': {'pendingSlots': max(len(r['pending_generations']) for r in queue),
        'activeSlots': max(int(r['active_generation'] is not None) for r in queue),
        'completedSlots': max(len(r['completed_generations']) for r in queue),
        'pendingCommandVectorCapacityBytes': max(r['pending_command_vector_capacity_bytes'] for r in queue),
        'completedPixelCapacityBytes': max(r['completed_pixel_capacity_bytes'] for r in queue)},
    'retiredFramebufferCapacityBytes': released[0]['framebuffer_capacity_bytes'], 'queueAtStop': stop, 'workerAtExit': exits[0],
    'last20RestartAllocator': {key: stats([r['allocator'][key] for r in last]) for key in ['liveRequestedBytes', 'peakRequestedBytes', 'cumulativeRequestedBytes']},
    'last20RetainedJsx': {key: sorted({r['retainedJsx'][key] for r in last}) for key in last[0]['retainedJsx']},
    'returnToIdle': owners[-1],
    'inputSha256': {str(root/name): hashlib.sha256((root/name).read_bytes()).hexdigest() for name in ['pipeline.jsonl', 'trace/steps.jsonl', 'trace/ownership.jsonl', 'invocation.json']},
    'scope': 'Live CPU-raster software client on private GPU compositor; same447step controlled clock. Natural collection, zero retained harness steps. Attribution includes clocks/logging and is excluded from normal timing. App waits for25sample window then exits normally, retaining actual worker retire/stop/join events.',
    'overlapAndExclusions': ['Pending command vectors, active requests, completed pixels, caller-taken results and renderer framebuffer/cache are distinct owners that can coexist.',
        'Queue capacities count vector/pixel storage only. Active/nested string/image/damage/map payloads excluded; renderer raster events record framebuffer and cache diagnostics separately.',
        'Capacity/request bytes overlap Rust allocator/process totals. Queue-zero and renderer-owner release prove these owners retired, not allocator RSS decommit or absence of foreign driver retention.',
        'Rust live/peak, cumulative traffic and retained JSX identities distinguish known stable ownership from churn; V8 used/committed reflects natural GC. No global leak conclusion.',
        'Retained acknowledgment/stale-result safety has separate default/feature regressions in software-lifetime-headless-v3.']}
args.output.write_text(json.dumps(result, indent=2)+'\n')
print(json.dumps({'status': result['status'], 'cycles': len(cycles), 'queueEvents': len(queue)}))
