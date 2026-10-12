#!/usr/bin/env python3
"""Summarize paired cached-getter trials without hiding environmental variation."""
import argparse
import itertools
import json
from pathlib import Path
import statistics


def summarize(root):
    protocol = json.loads((root / 'protocol.json').read_text())
    if protocol['status'] != 'complete' or protocol['pairs'] < 5:
        raise ValueError('Five complete pairs required')
    pairs = []
    for number in range(1, protocol['pairs'] + 1):
        group = [r for r in protocol['runs'] if r['pair'] == number]
        if len(group) != 2 or {r['variant'] for r in group} != {'before', 'after'}:
            raise ValueError('Missing variant')
        before = next(r for r in group if r['variant'] == 'before')
        after = next(r for r in group if r['variant'] == 'after')
        if before['finalStateSha256'] != after['finalStateSha256'] or before['nativeWindow'] != after['nativeWindow']:
            raise ValueError('Paired state/window mismatch')
        for run in group:
            summary = json.loads((root / run['run'] / 'summary.json').read_text())
            replay = json.loads((root / run['run'] / 'trace/replay.json').read_text())
            if summary['status'] != 'complete' or summary['background_interference_samples'] or len(replay['steps']) != 47:
                raise ValueError('Invalid raw trial')
            if replay['nativeResources'] is not None or replay['jsxRuntime'] is not None:
                raise ValueError('Endpoint observer enabled')
            if any(run['process'][key]['samples'] != 11 for key in ['rss_mib', 'pss_mib', 'cpu_percent_one_core']):
                raise ValueError('Incomplete process samples')
        pairs.append((before, after))
    metrics = {}
    for category, key, limit in [('serviceUs', 'p50', 1.03), ('serviceUs', 'p95', 1.05), ('serviceUs', 'sum', None),
                                 ('pss_mib', 'mean', None), ('rss_mib', 'mean', None), ('cpu_percent_one_core', 'mean', None)]:
        read = lambda r: r[category][key] if category == 'serviceUs' else r['process'][category][key]
        ratios = [read(after) / read(before) for before, after in pairs]
        resamples = sorted(statistics.median(sample) for sample in itertools.product(ratios, repeat=len(ratios)))
        interval = [resamples[int(.025 * (len(resamples)-1))], resamples[int(.975 * (len(resamples)-1))]]
        metrics[f'{category}.{key}'] = {'before': [read(a) for a, _ in pairs], 'after': [read(b) for _, b in pairs],
            'pairedAfterOverBefore': ratios, 'medianPairedRatio': statistics.median(ratios),
            'empiricalBootstrapMedian95PercentInterval': interval, 'nonRegressionLimit': limit,
            'pointEstimateWithinLimit': statistics.median(ratios) <= limit if limit else None,
            'upperIntervalWithinLimit': interval[1] <= limit if limit else None}
    gates = [m for m in metrics.values() if m['nonRegressionLimit']]
    return {'status': 'complete_measurement', 'pairs': len(pairs), 'metrics': metrics,
        'performanceAcceptance': 'supported' if all(m['upperIntervalWithinLimit'] for m in gates) else 'not_demonstrated',
        'scope': protocol['scope'], 'binarySha256': protocol['binarySha256'],
        'limitations': ['Five pairs cannot separate all machine/driver variation. Ambient CPU retained per run; no subtraction or outlier removal.',
                       'Bootstrap resamples paired run ratios; not within-run observations. No speedup or memory reduction claim unless supported.',
                       'Service boundary is canonical dispatch through resolved native frame; GPU presentation excluded.'],
        'followUp': 'If limits are not demonstrated, repeat the same predeclared protocol with a larger quiet sample; preserve this series.'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = summarize(args.root)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(result['performanceAcceptance'])
