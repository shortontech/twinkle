#!/usr/bin/env python3
"""Summarize five interleaved GPU text-cache attribution pairs."""
import argparse
import itertools
import json
import math
from pathlib import Path
import statistics

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('root', type=Path)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()

def percentile(values, fraction):
    values = sorted(values)
    return values[math.ceil(len(values) * fraction) - 1]

def read_run(path):
    invocation = json.loads((path / 'invocation.json').read_text())
    replay = json.loads((path / 'trace/replay.json').read_text())
    if invocation['status'] != 'complete' or replay['status'] != 'complete':
        raise ValueError(f'incomplete run: {path}')
    rows = [json.loads(line) for line in (path / 'pipeline.jsonl').read_text().splitlines()]
    frames = [row for row in rows if row['event'] == 'gpu_frame']
    lookups = [row for row in rows if row['event'] == 'text_lookup']
    rasters = [row for row in rows if row['event'] == 'text_raster']
    uploads = [row for row in rows if row['event'] == 'texture_upload']
    cache = [row for row in rows if row['event'] == 'texture_cache']
    presenter = [row['presenter_call_us'] for row in frames]
    preparation = [row['preparation_us'] for row in frames]
    return {
        'binarySha256': invocation['binarySha256'], 'finalState': replay['finalState'],
        'window': replay['nativeWindow'], 'frames': len(frames),
        'presenterCallUs': {'p50': statistics.median(presenter), 'p95': percentile(presenter, .95)},
        'preparationUs': {'p50': statistics.median(preparation), 'p95': percentile(preparation, .95)},
        'textHits': sum(bool(row['hit']) for row in lookups),
        'textMisses': sum(not row['hit'] for row in lookups),
        'keyBuildUs': sum(row.get('key_build_us') or 0 for row in lookups),
        'keyStringBytes': sum(row.get('key_string_bytes') or 0 for row in lookups),
        'paintCommandClones': sum(row.get('paint_command_clones') or 0 for row in lookups),
        'lookupKeyAllocations': sum(row.get('lookup_key_allocations', row.get('key_allocations', 0)) or 0
                                    for row in lookups),
        'bytesHashed': sum(row.get('bytes_hashed') or 0 for row in lookups),
        'bytesCompared': sum(row.get('bytes_compared') or 0 for row in lookups),
        'rasterizedPixels': sum(row['rasterized_pixels'] for row in rasters),
        'uploadedBytes': sum(row['uploaded_bytes'] for row in uploads),
        'cacheAtEnd': cache[-1] if cache else None,
    }

pairs = []
for number in range(1, 6):
    baseline = read_run(args.root / f'{number}-baseline')
    candidate = read_run(args.root / f'{number}-candidate')
    if baseline['finalState'] != candidate['finalState'] or baseline['window'] != candidate['window']:
        raise ValueError(f'pair {number} state/window mismatch')
    pairs.append({'pair': number, 'baseline': baseline, 'candidate': candidate})

metrics = {}
for category in ['presenterCallUs', 'preparationUs']:
    for quantile, limit in [('p50', 1.03), ('p95', 1.05)]:
        ratios = [pair['candidate'][category][quantile] / pair['baseline'][category][quantile]
                  for pair in pairs]
        resamples = sorted(statistics.median(sample) for sample in itertools.product(ratios, repeat=5))
        metrics[f'{category}.{quantile}'] = {
            'baseline': [pair['baseline'][category][quantile] for pair in pairs],
            'candidate': [pair['candidate'][category][quantile] for pair in pairs],
            'pairedCandidateOverBaseline': ratios,
            'medianPairedRatio': statistics.median(ratios),
            'empiricalBootstrapMedian95PercentInterval': [
                resamples[int(.025 * (len(resamples) - 1))],
                resamples[int(.975 * (len(resamples) - 1))],
            ],
            'nonRegressionLimit': limit,
        }
result = {
    'status': 'complete_measurement', 'pairs': pairs, 'metrics': metrics,
    'scope': 'Opt-in attribution release builds over the frozen 47-step Blocks trace; clocks, allocator counters, JSON serialization and file writes perturb timings; use normal timing for performance authority.',
    'limits': {'median': 1.03, 'p95': 1.05},
}
args.output.write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps({key: value['medianPairedRatio'] for key, value in metrics.items()}))
