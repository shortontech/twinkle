#!/usr/bin/env python3
"""Publish matched empty-subtracted static footprints and scaling residuals."""
import argparse
import json
from pathlib import Path
import statistics


def fit(points):
    xs, ys = zip(*points)
    mx, my = statistics.mean(xs), statistics.mean(ys)
    slope = sum((x-mx)*(y-my) for x, y in points) / sum((x-mx)**2 for x in xs)
    intercept = my-slope*mx
    return {'slopePerCell': slope, 'intercept': intercept,
            'points': [{'cells': x, 'increment': y, 'residual': y-(intercept+slope*x)} for x, y in points]}


def summarize(root):
    protocol = json.loads((root/'protocol.json').read_text())
    if protocol['status'] != 'complete' or protocol['pairsPerFixture'] < 5:
        raise ValueError('Five complete pairs per scene required')
    count = protocol['pairsPerFixture']
    scenes = ['empty', 'cells-50', 'cells-200', 'cells-800', 'rich-form']
    runs = protocol['runs']
    if len(runs) != count*len(scenes)*2:
        raise ValueError('Missing runs')
    lookup = {(r['pair'], r['fixture'], r['implementation']): r for r in runs}
    if len(lookup) != len(runs):
        raise ValueError('Duplicate runs')
    distributions, scaling = {}, {}
    for implementation in ['native', 'react']:
        distributions[implementation] = {}
        scaling[implementation] = []
        for scene in scenes:
            selected = [lookup[(pair, scene, implementation)] for pair in range(1, count+1)]
            for run in selected:
                summary = json.loads((root/run['run']/'process/summary.json').read_text())
                rows = [json.loads(line) for line in (root/run['run']/'process/samples.jsonl').read_text().splitlines()]
                if summary['status'] != 'complete' or summary['background_interference_samples'] or len(rows) != 11:
                    raise ValueError('Incomplete/disturbed run')
                # Use complete common replay-length window, retain raw idle tail.
                if not all(row['memory_complete'] for row in rows[:10]):
                    raise ValueError('Common memory window unavailable')
                run['commonMemory'] = {key: statistics.mean(row[key] for row in rows[:10]) for key in ['rss_mib', 'pss_mib']}
            distributions[implementation][scene] = {'nodeCounts': [r['nodeCount'] for r in selected], 'metrics': {}}
            for key in ['rss_mib', 'pss_mib', 'cpu_percent_one_core']:
                full = [r['process'][key] for r in selected]
                values = [r['commonMemory'][key] if key != 'cpu_percent_one_core' else r['process'][key]['mean'] for r in selected]
                distributions[implementation][scene]['metrics'][key] = {'runMeans': values, 'medianRunMean': statistics.median(values),
                    'fullWindowP50': [m['p50'] for m in full], 'fullWindowP95': [m['p95'] for m in full],
                    'fullWindowValidSamples': [m['samples'] for m in full]}
        for pair in range(1, count+1):
            empty = lookup[(pair, 'empty', implementation)]
            entry = {'pair': pair, 'processMemory': {}, 'nativeExpandedFrameBytes': None}
            for key in ['rss_mib', 'pss_mib']:
                points = [(cells, lookup[(pair, f'cells-{cells}', implementation)]['commonMemory'][key]-empty['commonMemory'][key]) for cells in [50, 200, 800]]
                entry['processMemory'][key] = fit(points)
            if implementation == 'native':
                points = [(cells, lookup[(pair, f'cells-{cells}', implementation)]['report']['expandedOwnedBytes']-empty['report']['expandedOwnedBytes']) for cells in [50, 200, 800]]
                entry['nativeExpandedFrameBytes'] = fit(points)
            scaling[implementation].append(entry)
    increments = []
    for pair in range(1, count+1):
        for scene in scenes[1:]:
            for implementation in ['native', 'react']:
                run, empty = lookup[(pair, scene, implementation)], lookup[(pair, 'empty', implementation)]
                increments.append({'pair': pair, 'fixture': scene, 'implementation': implementation,
                    **{key: run['commonMemory'][key]-empty['commonMemory'][key] for key in ['rss_mib', 'pss_mib']}})
    return {'status': 'passed', 'pairsPerFixture': count, 'distributions': distributions,
        'incrementsAboveMatchedEmpty': increments, 'scalingPerPair': scaling,
        'memoryWindow': 'First10 of11post-warmup samples, common to every scene/run; full raw11 and per-run full-window quantiles retained. CPU uses full11.',
        'scope': protocol['scope'],
        'limitations': ['Static scene process increments include startup/cache/driver differences, not only per-cell allocation.',
                       'Each pair has its own empty baseline. Report slopes and intercept/residuals, never divide whole process footprint by cell count.',
                       'GPU/compositor physical display latency unavailable. Shared compositor excluded; app totals include dedicated Chrome overhead.',
                       'Ambient activity remains in protocol. No outlier removal, forced GC, noise subtraction or memory-leak conclusion.']}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = summarize(args.root)
    args.output.write_text(json.dumps(result, indent=2)+'\n')
    print(result['status'])
