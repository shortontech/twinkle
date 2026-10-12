#!/usr/bin/env python3
"""Compare saved live states; service time and presentation remain distinct."""
import argparse
import hashlib
import json
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('native', type=Path)
p.add_argument('react', type=Path)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
input_paths = [a.native, a.react]
def load_report(path):
    report = json.loads(path.read_text())
    if report.get('streamedSteps'):
        if report['steps']:
            raise ValueError('Streamed runtime unexpectedly retained report history')
        stream = path.parent / 'steps.jsonl'
        report['steps'] = [json.loads(line) for line in stream.read_text().splitlines()]
        input_paths.append(stream)
    return report
n, r = [load_report(x) for x in (a.native, a.react)]
errors = []
for name, report in [('native', n), ('react', r)]:
    if report.get('status') != 'complete': errors.append(f'{name}: incomplete')
    if report.get('scale') != 1: errors.append(f'{name}: scale mismatch')
    viewport = report.get('logicalViewport', report.get('viewport'))
    if viewport != [800, 720]: errors.append(f'{name}: viewport mismatch')
    if len(report['steps']) != len(report['trace']['steps']): errors.append(f'{name}: missing steps')
if n['trace'] != r['trace']: errors.append('different traces')
if len(n['steps']) != len(r['steps']): errors.append('different step counts')
for name, report in [('native', n), ('react', r)]:
    for i, step in enumerate(report['steps']):
        expected = report['trace']['steps'][i] if i < len(report['trace']['steps']) else None
        if expected is None or step.get('index') != i or any(step.get(key) != expected.get(key) for key in ('atMs', 'kind', 'label')):
            errors.append(f'{name} {i}: row does not match its frozen trace step')
        state = step.get('state')
        if not isinstance(state, dict) or not {'presentation', 'model', 'bookkeeping', 'focused', 'visible', 'held'} <= state.keys():
            errors.append(f'{name} {i}: incomplete canonical state')
        elif len(state.get('presentation', {}).get('board', [])) != 200:
            errors.append(f'{name} {i}: incomplete board')
        else:
            required = {
                'presentation': {'board', 'next', 'score', 'lines', 'level', 'paused', 'over', 'menuOpen', 'message'},
                'model': {'cells', 'shape', 'rotation', 'x', 'y', 'next', 'score', 'lines', 'over', 'paused'},
                'bookkeeping': {'axisX', 'gravityElapsedMs', 'horizontalDirection', 'horizontalElapsedMs'},
                'held': {'left', 'right', 'down'},
            }
            if any(not isinstance(state.get(key), dict) or not fields <= state[key].keys() for key, fields in required.items()) or len(state['model'].get('cells', [])) != 200:
                errors.append(f'{name} {i}: incomplete model/controller state')
for i, (ns, rs) in enumerate(zip(n['steps'], r['steps'])):
    for key in ('index', 'atMs', 'kind', 'label', 'state'):
        if key == 'state' and (ns.get(key) is None or rs.get(key) is None):
            errors.append(f'{i}: missing state')
        elif ns.get(key) != rs.get(key): errors.append(f'{i}: {key} differs')
if n['finalState'] != r['finalState']: errors.append('different final state')
result = {'status': 'failed' if errors else 'passed', 'steps': len(n['steps']), 'errors': errors,
          'inputSha256': {str(x): hashlib.sha256(x.read_bytes()).hexdigest() for x in input_paths},
          'scope': 'Live canonical native host and live React committed DOM/model states; includes no-op steps and controller bookkeeping. Controlled clock and synthetic focus/visibility flags; OS delivery and compositor timestamps are not proven.',
          'labels': [s.get('label') for s in n['steps']],
          'nativePublicationAttempts': sum(s['publicationAttempts'] for s in n['steps']),
          'reactPublicationAttempts': sum(s['publicationAttempts'] for s in r['steps'])}
a.output.write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps({'status': result['status'], 'steps': result['steps'], 'errors': len(errors)}))
raise SystemExit(bool(errors))
