#!/usr/bin/env python3
"""Validate headless static frame counters and fit empty-subtracted work slopes."""
import argparse
import json
from pathlib import Path


def summarize(root):
    metrics = ['layoutElementVisits', 'recursiveMeasurements', 'emissionElementVisits',
               'transientStateVisits', 'idConstructions', 'idConstructedUtf8Bytes']
    scenes = {}
    for name in ['empty', 'cells-50', 'cells-200', 'cells-800', 'rich-form']:
        fixture = json.loads((root / name / 'fixture.json').read_text())
        rows = [json.loads(line) for line in (root / name / 'pipeline.jsonl').read_text().splitlines()]
        work = [row for row in rows if row['event'] == 'native_frame_work']
        if len(work) != 1:
            raise ValueError(f'{name}: expected one initial frame scope, got {len(work)}')
        work = work[0]
        for key in ['layoutElementVisits', 'emissionElementVisits', 'transientStateVisits']:
            if work[key] != fixture['nodeCount']:
                raise ValueError(f'{name}: {key} differs from actual resolved nodes')
        scenes[name] = {key: work[key] for key in metrics}
        scenes[name]['nodeCount'] = fixture['nodeCount']
        scenes[name]['expandedOwnedBytes'] = fixture['expandedOwnedBytes']
    slopes = {}
    cells = [50, 200, 800]
    xmean = sum(cells) / len(cells)
    for key in metrics + ['expandedOwnedBytes', 'nodeCount']:
        values = [scenes[f'cells-{n}'][key] - scenes['empty'][key] for n in cells]
        ymean = sum(values) / len(values)
        slope = sum((x - xmean) * (y - ymean) for x, y in zip(cells, values)) / sum((x - xmean)**2 for x in cells)
        intercept = ymean - slope * xmean
        slopes[key] = {'incrementalAboveEmpty': values, 'slopePerLogicalCell': slope,
                       'intercept': intercept,
                       'residuals': [y - (intercept + slope*x) for x, y in zip(cells, values)]}
    return {'status': 'passed', 'scenes': scenes, 'scalingCells': cells, 'fits': slopes,
            'scope': 'Initial unchanged native static admission at800x720, headless. Deterministic function-entry work counts and expanded frame estimates; excludes GPU/software presentation and process RSS/PSS. IDs outside frame resolution and other traversal families excluded. No timing or optimization claim.'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = summarize(args.root)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'status': result['status'], 'scenes': len(result['scenes'])}))
