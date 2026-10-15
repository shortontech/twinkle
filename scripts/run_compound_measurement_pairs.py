#!/usr/bin/env python3
"""Serialized production-library admission for compound measurement classification."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--suite', choices=['primary', 'guards'], default='primary')
    parser.add_argument('--fixture', action='append', help='Restrict the selected suite to family-mode-scenario; repeatable')
    args = parser.parse_args()
    args.output.mkdir(exist_ok=False)
    binaries = {'before': args.before.resolve(), 'after': args.after.resolve()}
    fixtures = [(family, mode, 'paint') for family in ['slider', 'dropdown'] for mode in ['cold', 'retained']]
    if args.suite == 'guards':
        fixtures = [('dropdown', mode, scenario) for scenario in ['font', 'inline'] for mode in ['cold', 'retained']]
        fixtures += [('dropdown', 'retained', 'many-options')]
        fixtures += [(family, 'retained', scenario) for scenario in ['dense', 'tiny'] for family in ['slider', 'dropdown']]
        fixtures += [(family, 'cold', 'structural') for family in ['slider', 'dropdown']]
    if args.fixture:
        selected = set(args.fixture)
        available = {'-'.join(f) for f in fixtures}
        if not selected <= available:
            parser.error(f'Unknown fixtures: {selected - available}')
        fixtures = [f for f in fixtures if '-'.join(f) in selected]
    root = Path(__file__).resolve().parents[1]
    source = root / 'crates/twinkle-workbench/src/bin/twinkle_compound_admission.rs'
    protocol = {'status': 'running', 'suite': args.suite, 'pairs': 5, 'iterations': 200, 'warmups': 10, 'binarySha256': {v: hashlib.sha256(p.read_bytes()).hexdigest() for v, p in binaries.items()}, 'fixtureSourceSha256': hashlib.sha256(source.read_bytes()).hexdigest(), 'candidateRevision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(), 'candidateTrackedPatchSha256': hashlib.sha256(subprocess.check_output(['git', 'diff', '--binary', 'HEAD'], cwd=root)).hexdigest(), 'scope': 'Ordinary UI dependency (cfg(test) disabled). Separate normal timing and attribution builds. Five serial alternating pairs.', 'runs': []}
    env = os.environ.copy()
    for key in ['TWINKLE_PIPELINE_TRACE', 'TWINKLE_JSX_TRACE']:
        env.pop(key, None)
    try:
        for family, mode, scenario in fixtures:
            fixture = f'{family}-{mode}-{scenario}'
            for pair in range(1, 6):
                for variant in ['before', 'after'] if pair % 2 else ['after', 'before']:
                    command = [str(binaries[variant]), '--family', family, '--mode', mode, '--scenario', scenario, '--iterations', '200']
                    result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True)
                    log = result.stdout + result.stderr
                    name = f'{fixture}-{pair:02}-{variant}.log'
                    (args.output / name).write_text(log)
                    if result.returncode:
                        raise RuntimeError(f'{name}: {log[-1500:]}')
                    reports = [json.loads(line.split('=', 1)[1]) for line in log.splitlines() if line.startswith('compound_admission=')]
                    if len(reports) != 1:
                        raise ValueError(f'Missing report: {name}')
                    report = reports[0]
                    assert report['family'] == family and report['mode'] == mode and (report['scenario'] == scenario)
                    assert report['iterations'] == 200 and report['warmups'] == 10
                    assert not report['harnessCfgTest'] and (not report['allocatorEnabled'])
                    assert report['activeFrames'] == (0 if scenario == 'structural' else 100)
                    assert report['nodeCount'] >= (2 if scenario == 'tiny' else 802)
                    for stage in ['nativeFrame', 'completeUpdate', 'layout']:
                        assert len(report['rawSamplesUs'][stage]) == 200
                    protocol['runs'].append({'fixture': fixture, 'pair': pair, 'variant': variant, 'log': name, 'report': report})
                    print(f"{fixture} pair={pair} {variant}: {report['nativeFrame']}", flush=True)
        protocol['status'] = 'complete'
    finally:
        (args.output / 'protocol.json').write_text(json.dumps(protocol, indent=2) + '\n')
if __name__ == '__main__':
    main()
