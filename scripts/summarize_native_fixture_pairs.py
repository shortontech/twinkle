#!/usr/bin/env python3
"""Summarize paired native static-fixture process samples."""
import argparse
import itertools
import json
from pathlib import Path
import statistics

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('root', type=Path)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
protocol = json.loads((args.root / 'protocol.json').read_text())
if protocol['status'] != 'complete' or protocol['pairs'] < 5:
    raise ValueError('Five complete pairs required')
pairs = []
for number in range(1, protocol['pairs'] + 1):
    group = [run for run in protocol['runs'] if run['pair'] == number]
    before = next(run for run in group if run['variant'] == 'before')
    after = next(run for run in group if run['variant'] == 'after')
    if before['fixtureShapeSha256'] != after['fixtureShapeSha256'] or before['window'] != after['window']:
        raise ValueError('Paired fixture/window mismatch')
    pairs.append((before, after))
metrics = {}
for category, key in [('pss_mib', 'mean'), ('rss_mib', 'mean'), ('cpu_percent_one_core', 'mean')]:
    read = lambda run: run['process'][category][key]
    ratios = [read(after) / read(before) for before, after in pairs]
    resamples = sorted(statistics.median(sample) for sample in itertools.product(ratios, repeat=len(ratios)))
    metrics[f'{category}.{key}'] = {
        'before': [read(before) for before, _ in pairs],
        'after': [read(after) for _, after in pairs],
        'pairedAfterOverBefore': ratios,
        'medianPairedRatio': statistics.median(ratios),
        'empiricalBootstrapMedian95PercentInterval': [
            resamples[int(.025 * (len(resamples) - 1))],
            resamples[int(.975 * (len(resamples) - 1))],
        ],
    }
result = {
    'status': 'complete_measurement', 'pairs': len(pairs), 'fixture': protocol['fixture'],
    'metrics': metrics, 'scope': protocol['scope'], 'binarySha256': protocol['binarySha256'],
    'limitations': [
        'Process samples include idle/window/event-loop work and are not complete-frame service time.',
        'Five pairs cannot separate all machine/driver variation; no outlier removal or noise subtraction.',
        'The shared compositor is outside app process totals and physical scanout/GPU execution are unavailable.',
    ],
}
args.output.write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps({key: value['medianPairedRatio'] for key, value in metrics.items()}))
