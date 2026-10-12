#!/usr/bin/env python3
"""Validate complete paired trials and retain complete-run distributions."""
import argparse
import itertools
import json
from pathlib import Path
import statistics


def summarize(root):
    protocol = json.loads((root / 'protocol.json').read_text())
    if protocol['status'] != 'complete' or protocol['pairs'] < 5:
        raise ValueError('At least five complete pairs required')
    runs = protocol['runs']
    if len(runs) != protocol['pairs'] * 2:
        raise ValueError('Missing runs')
    pairs = []
    for number in range(1, protocol['pairs'] + 1):
        group = [r for r in runs if r['pair'] == number]
        if len(group) != 2 or {r['implementation'] for r in group} != {'native', 'react'}:
            raise ValueError(f'Invalid pair{number}')
        native = next(r for r in group if r['implementation'] == 'native')
        react = next(r for r in group if r['implementation'] == 'react')
        if native['finalStateSha256'] != react['finalStateSha256']:
            raise ValueError('Final state mismatch')
        if native['nativeWindowOrReactPage']['focused'] != react['nativeWindowOrReactPage']['focused']:
            raise ValueError('Actual focus mismatch')
        for run in group:
            evidence = run['quietMachineEvidence']
            if evidence['backgroundBuildSamples'] or evidence['backgroundInterferenceSamples']:
                raise ValueError('Interference recorded')
            if run['serviceUs']['samples'] != 47:
                raise ValueError('Missing service observations')
            if run['process']['cpu_percent_one_core']['samples'] != 11:
                raise ValueError('Incomplete CPU window')
        pairs.append((native, react))
    distributions = {}
    for implementation in ['native', 'react']:
        selected = [r for r in runs if r['implementation'] == implementation]
        distributions[implementation] = {}
        for key in ['rss_mib', 'pss_mib', 'cpu_percent_one_core']:
            means = [r['process'][key]['mean'] for r in selected]
            distributions[implementation][key] = {
                'runMeans': means, 'medianRunMean': statistics.median(means),
                'minRunMean': min(means), 'maxRunMean': max(means),
                'perRunP50': [r['process'][key]['p50'] for r in selected],
                'perRunP95': [r['process'][key]['p95'] for r in selected],
                'perRunValidSamples': [r['process'][key]['samples'] for r in selected]}
        distributions[implementation]['serviceUs'] = {
            'boundary': selected[0]['serviceBoundary'],
            'perRunSum': [r['serviceUs']['sum'] for r in selected],
            'perRunP50': [r['serviceUs']['p50'] for r in selected],
            'perRunP95': [r['serviceUs']['p95'] for r in selected]}
    # All trials retain the complete11-row window. Compare memory on the
    # common first10 samples covering the replay; three Chrome final-tail
    # samples in this dataset were explicitly unavailable, never counted zero.
    replay_memory = {}
    for run in runs:
        directory = root / run['run']
        sample_file = (directory / 'process' if run['implementation']=='react' else directory) / 'samples.jsonl'
        rows = [json.loads(line) for line in sample_file.read_text().splitlines()]
        if len(rows) != 11 or not all(row['memory_complete'] for row in rows[:10]):
            raise ValueError('Common replay memory window incomplete')
        replay_memory[(run['pair'], run['implementation'])] = {
            key: statistics.mean(row[key] for row in rows[:10]) for key in ['rss_mib','pss_mib']}
    ratios = {}
    for key in ['rss_mib', 'pss_mib', 'cpu_percent_one_core']:
        values = [(replay_memory[(react['pair'],'react')][key] / replay_memory[(native['pair'],'native')][key]) if key in ['rss_mib','pss_mib'] else react['process'][key]['mean'] / native['process'][key]['mean'] for native, react in pairs]
        # Exhaustive empirical bootstrap for five pairs; no independence claim
        # for within-run samples. For larger series retain the observed range.
        bootstrap = sorted(statistics.median(sample) for sample in itertools.product(values, repeat=len(values))) if len(values) == 5 else None
        ratios[key] = {'perPairReactOverNative': values, 'median': statistics.median(values),
                       'observedRange': [min(values), max(values)],
                       'empiricalBootstrapMedian95PercentInterval': [bootstrap[int(.025*(len(bootstrap)-1))], bootstrap[int(.975*(len(bootstrap)-1))]] if bootstrap else None}
    return {'status': 'passed', 'pairs': len(pairs), 'runs': len(runs), 'distributions': distributions,
            'commonReplayMemoryWindow': {'samplesPerRun':10,'scope':'First10post-warmup snapshots, approximately replayseconds1..10; final idle-tail remains in raw and full-window distributions, unavailable samples excluded only from those means. No trace steps or no-ops removed.',
                'runs':[{'pair':pair,'implementation':implementation,**values} for (pair,implementation),values in replay_memory.items()]},
            'pairedProcessRatios': ratios,
            'uncertainty': 'Five paired runs are the resampling unit. Bootstrap describes this small observed sample, not environmental bias or physical display latency.',
            'scope': 'Total dedicated app process RSS/PSS and CPU; private shared compositor/controller excluded. No empty subtraction. Native resolved-frame and React DOM-commit service boundaries differ: no service speed ratio or complete-update-to-display claim. Ambient CPU remains disclosed in protocol, even when no named build/crash-reporter was observed.13x/half-RAM historical observation remains unverified.'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = summarize(args.root)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'status': result['status'], 'pairs': result['pairs']}))
