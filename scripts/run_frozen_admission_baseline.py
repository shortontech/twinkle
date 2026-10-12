#!/usr/bin/env python3
"""Collect five corrected frozen admission baselines, preserving successful logs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--resume-parser-failure', action='store_true')
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
out = args.output.resolve()
command = ['cargo', 'test', '--release', '-p', 'twinkle-workbench', '--features', 'jsx', '--test', 'blocks_admission', '--', '--ignored', '--nocapture', '--test-threads=1']
if args.resume_parser_failure:
    protocol = json.loads((out/'protocol.json').read_text())
    if protocol['status'] != 'incomplete' or protocol.get('error') != 'Expectedboth frozen fixtures' or protocol['command'] != command:
        raise ValueError('Only the known report-prefix parser failure can be resumed')
    (out/'parser-failure.json').write_text(json.dumps(protocol, indent=2)+'\n')
    protocol.update(status='running', runs=[], resumedParserFailure=True)
    protocol.pop('error', None)
else:
    out.mkdir()
    protocol = {'status': 'running', 'runs': [], 'command': command,
        'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(),
        'trackedPatchSha256': hashlib.sha256(subprocess.check_output(['git', 'diff', '--binary', 'HEAD'], cwd=root)).hexdigest(),
        'scope': 'Five serial frozen200cell/fullaccessibleform1000update admittednativeframe runs,10warmup each. Normaljsx; detailed optionaltraces absent. SeparatefromliveGPU/software/React. Snapshotconstruction included in completeCPU tick; nested medians never summed. Nooptimizationbeforeafter claim.'}
try:
    for number in range(1, 6):
        log = out/f'{number:02}.log'
        if not log.exists():
            print(f'Frozen admission run{number}', flush=True)
            env = os.environ.copy()
            for key in ['TWINKLE_PIPELINE_TRACE', 'TWINKLE_JSX_TRACE', 'TWINKLE_BENCH_JSX_STAGES', 'TWINKLE_BENCH_OWNERSHIP']:
                env.pop(key, None)
            result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True)
            log.write_text(result.stdout+result.stderr)
            if result.returncode:
                raise RuntimeError('Admission fixture failed')
        text = log.read_text()
        if 'test result: ok. 2 passed' not in text:
            raise ValueError('Successful full fixture log required')
        reports = [json.loads(line.split('twinkle_release_admission=', 1)[1]) for line in text.splitlines() if line.startswith('twinkle_release_admission=')]
        if len(reports) != 2 or {r['suite'] for r in reports} != {'blocks_end_to_end', 'rich_accessible_form_end_to_end'}:
            raise ValueError('Both report envelopes required')
        for report in reports:
            m = report['metadata']
            if m['iterations'] != 1000 or m['warmupIterations'] != 10 or report['summary']['ticks'] != 1000:
                raise ValueError('Iteration mismatch')
            expected = 428 if report['suite'] == 'blocks_end_to_end' else 67
            if m['lastFrameResources']['nodes'] != expected or m['accessibilitySnapshotNodes'] != expected:
                raise ValueError('Fixture nodes reduced')
            if report['suite'] == 'rich_accessible_form_end_to_end' and (m['fields'] != 8 or m['semanticNodes'] < 20):
                raise ValueError('Form semantics reduced')
        protocol['runs'].append({'run': number, 'reports': reports, 'logSha256': hashlib.sha256(log.read_bytes()).hexdigest()})
        (out/'protocol.json').write_text(json.dumps(protocol, indent=2)+'\n')
    protocol['status'] = 'complete'
except Exception as error:
    protocol.update(status='incomplete', error=str(error)); raise
finally:
    (out/'protocol.json').write_text(json.dumps(protocol, indent=2)+'\n')
