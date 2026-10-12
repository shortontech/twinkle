#!/usr/bin/env python3
"""Summarize opt-in pipeline evidence with explicit overlap and unknown fields."""
import argparse
from collections import Counter
import hashlib
import json
import math
from pathlib import Path
import statistics


def stats(values):
    values = sorted(values)
    return {'samples': len(values), 'min': min(values), 'median': statistics.median(values),
            'p95': values[math.ceil(.95*len(values))-1], 'max': max(values)} if values else None


def summarize(root):
    files = [root/'pipeline.jsonl', root/'bridge.jsonl']
    rows = []
    for path in files:
        rows.extend(json.loads(line) for line in path.read_text().splitlines())
    replay = json.loads((root/'trace/replay.json').read_text())
    if replay['status'] != 'complete':
        raise ValueError('Replay incomplete')
    scopes = {}
    scope_key = lambda r: r['event']+'.'+r['scope'] if r['event'] == 'native_emission' else r['event']
    for event in sorted({scope_key(r) for r in rows}):
        group = [r for r in rows if scope_key(r) == event]
        scopes[event] = {'events': len(group), 'cpuCallTimes': {}}
        keys = set().union(*(r.keys() for r in group))
        for key in sorted(keys):
            if key.endswith('_us') or key.endswith('Us'):
                if key == 'elapsed_us':
                    continue
                values = [r[key] for r in group if isinstance(r.get(key), (int, float))]
                scopes[event]['cpuCallTimes'][key] = {'distributionUs': stats(values),
                    'unavailableRows': sum(r.get(key) is None for r in group)}
    bridge = [r for r in rows if r['event'] == 'v8_call']
    serial = [r for r in rows if r['event'] == 'json_serialization']
    parse = [r for r in rows if r['event'] == 'json_parse']
    frame_work = [r for r in rows if r['event'] == 'native_frame_work']
    gpu = [r for r in rows if r['event'] == 'gpu_frame']
    texture_cache = [r for r in rows if r['event'] == 'texture_cache']
    textures = [r for r in rows if r['event'] == 'text_lookup']
    capacities = {}
    for key in ['framebuffer_live_bytes', 'framebuffer_peak_bytes', 'texture_cache_resident_bytes',
                'texture_cache_peak_bytes', 'vertex_buffer_capacity_bytes']:
        values = [r[key] for r in gpu if isinstance(r.get(key), (int, float))]
        capacities[key] = {'observations': len(values), 'last': values[-1] if values else None,
                           'highWater': max(values) if values else None}
    for key in ['resident_bytes', 'texture_bytes', 'key_and_metadata_bytes', 'peak_bytes']:
        values = [r[key] for r in texture_cache if isinstance(r.get(key), (int, float))]
        capacities['texture_cache.' + key] = {'observations': len(values),
            'last': values[-1] if values else None, 'highWater': max(values) if values else None}
    input_bytes_available = all('argumentKeyUtf8Bytes' in r and 'argumentStringUtf8Bytes' in r for r in bridge)
    return {'status': 'passed', 'scope': 'Opt-in attribution invocation, including initialization and warmup. CPU clocks, serialization, callback and log overhead present; not normal performance evidence.',
        'inputSha256': {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in files+[root/'trace/replay.json']},
        'traceSteps': replay['totalTraceSteps'], 'eventCounts': dict(Counter(r['event'] for r in rows)),
        'stageDistributions': scopes,
        'bridgeWork': {'argumentValues': sum(r['argumentValues'] for r in bridge),
            'argumentKeyUtf8Bytes': sum(r['argumentKeyUtf8Bytes'] for r in bridge) if input_bytes_available else None,
            'argumentStringUtf8Bytes': sum(r['argumentStringUtf8Bytes'] for r in bridge) if input_bytes_available else None,
            'resultUtf8BytesCopied': sum(r['resultUtf8Bytes'] for r in bridge),
            'jsonInputUtf8BytesParsed': sum(r['inputUtf8Bytes'] for r in parse),
            'serializedEnvelopeUtf16Units': sum(r['outputUtf16Units'] for r in serial),
            'microtaskCheckpointsSkipped': sum(r['checkpointSkipped'] for r in bridge),
            'boundary': 'Direct argument value conversion has no serialized JSON input wire. String/key byte content is distinct from V8 allocation. UTF16 serialized length differs from UTF8 copied/parsed bytes. Copy and parse bytes describe the same returned text and are not independent ownership.'},
        'nativeFrameWork': {'scopes': frame_work,
            'boundary': 'Function entries during frame resolution only. Includes repeats; nested scopes overlap. App-view IDs and other traversal families excluded. Normal timing disables these counters.'},
        'gpuWork': {'frames': len(gpu), 'commandsPrepared': sum(r['commands_prepared'] for r in gpu),
            'sceneVertexBytesGenerated': sum(r['scene_vertex_bytes'] for r in gpu),
            'totalVertexBytesUploaded': sum(r['total_vertex_upload_bytes'] for r in gpu),
            'drawRecordsPrepared': sum(r['prepared_draw_records'] for r in gpu)
                if all('prepared_draw_records' in r for r in gpu) else None,
            'sceneDrawsEncoded': sum(r['scene_draws_encoded'] for r in gpu) if all('scene_draws_encoded' in r for r in gpu) else None,
            'sceneDamageCoveragePixels': sum(r['scene_damage_coverage_pixels'] for r in gpu) if all('scene_damage_coverage_pixels' in r for r in gpu) else None,
            'textLookupHits': sum(r['hit'] for r in textures), 'textLookupMisses': sum(not r['hit'] for r in textures),
            'textRasterPixels': sum(r['rasterized_pixels'] for r in rows if r['event'] == 'text_raster'),
            'textureUploadBytes': sum(r['uploaded_bytes'] for r in rows if r['event'] == 'texture_upload'),
            'textureCacheHitsAtEnd': texture_cache[-1].get('hits') if texture_cache else None,
            'textureCacheMissesAtEnd': texture_cache[-1].get('misses') if texture_cache else None,
            'textureCacheInsertionsAtEnd': texture_cache[-1].get('insertions') if texture_cache else None,
            'textureCacheEvictionsAtEnd': texture_cache[-1].get('evictions') if texture_cache else None,
            'textureCacheEvictionScanEntriesAtEnd': texture_cache[-1].get('eviction_scan_entries') if texture_cache else None,
            'textureCacheOversizedUncachedAtEnd': texture_cache[-1].get('oversized_uncached') if texture_cache else None,
            'textureCacheWholeResetsAtEnd': texture_cache[-1].get('whole_resets') if texture_cache else None,
            'textureCacheBytesHashedAtEnd': texture_cache[-1].get('bytes_hashed') if texture_cache else None,
            'textureCacheBytesComparedAtEnd': texture_cache[-1].get('bytes_compared') if texture_cache else None,
            'redrawModes': dict(Counter(r['redraw'] for r in gpu)),
            'boundary': 'Preparation walks command list even on retained redraw reuse. Generated scene/auxiliary vertices are subsets of uploaded bytes, not additional ownership. Damage redraw mode is not physical pixels actually scanned out.'},
        'gpuCapacities': capacities, 'nativeEndpointOwnership': replay['nativeResources'], 'v8Endpoint': replay['jsxRuntime'],
        'unavailable': {'physicalCompositorDisplayTime': None, 'gpuExecutionUs': None,
            'externallyHeldAndInFlightGpuBytes': None, 'driverAllocationMetadataBytes': None},
        'overlapRules': ['Native view/layout are nested in host service; text key/raster/upload are nested in GPU preparation, which is nested in presenter call. Never sum stage medians.',
            'JS serialization is nested in JavaScript call; checkpoint/patch stages may enclose additional calls and output I/O.',
            'Expanded frame includes legacy subtotal, arena ID index, hash approximations and scratch. Publication overlap is transient, not an extra steady-state category.',
            'Rust allocator requests, V8 heap, frame ownership, GPU capacities and process RSS/PSS use different overlapping denominators. Never add them to invent RAM.',
            'Externally held/in-flight GPU allocation and physical display timing remain unavailable; submit/present API completion does not prove them. Driver fences/resource tracing would be needed for exact lifetime bytes.'],
        'residualFollowUp': 'App process PSS includes mapped code/libraries, stacks, allocator reserves/metadata/fragmentation, foreign V8 storage and driver allocations. Ownership capacities/request sizes cannot be subtracted as resident pages. Detailed mapping and driver lifetime attribution is the boundary for any future culprit or leak claim.'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = summarize(args.root)
    args.output.write_text(json.dumps(result, indent=2)+'\n')
    print(json.dumps({'status': result['status'], 'traceSteps': result['traceSteps']}))
