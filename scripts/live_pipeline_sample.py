#!/usr/bin/env python3
"""Linux whole-process-tree sampler; production binary startup is excluded by warmup.

This is external attribution, not update-to-present instrumentation. GPU VRAM,
allocator ownership and JavaScript heaps are separate ledgers, not RSS additions.
"""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import statistics
import subprocess
import time


def command_output(command):
    try:
        result = subprocess.run(command, text=True, capture_output=True, timeout=10)
        return {"command": command, "exit_code": result.returncode,
                "stdout": result.stdout.strip(), "stderr": result.stderr.strip()}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"command": command, "error": str(error)}


def source_manifest(path):
    path = Path(path).resolve()
    entries = {}
    if path.is_file():
        entries[path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
    else:
        for directory, children, files in os.walk(path):
            children[:] = sorted(child for child in children if child not in {'.git', 'node_modules', 'target', 'dist', '__pycache__', 'benchmark-artifacts'})
            for name in sorted(files):
                entry = Path(directory) / name
                if entry.is_file():
                    entries[str(entry.relative_to(path))] = hashlib.sha256(entry.read_bytes()).hexdigest()
    encoded = json.dumps(entries, sort_keys=True).encode()
    return {"path": str(path), "git_revision": command_output(['git', '-C', str(path), 'rev-parse', 'HEAD']),
            "git_status": command_output(['git', '-C', str(path), 'status', '--porcelain']),
            "source_files_sha256": entries, "source_manifest_sha256": hashlib.sha256(encoded).hexdigest()}


def system_cpu_times(proc_root=Path('/proc')):
    try:
        fields = (proc_root / 'stat').read_text().splitlines()[0].split()
        if fields[0] != 'cpu' or len(fields) < 9:
            return None
        # Guest ticks are already included in user/nice; exclude their columns.
        ticks = [int(value) for value in fields[1:9]]
        return sum(ticks), sum(ticks) - ticks[3] - ticks[4]
    except (OSError, ValueError, IndexError):
        return None


def snapshot(root, proc_root=Path('/proc'), interference_processes=None):
    processes = {}
    for entry in proc_root.iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / 'stat').read_text()
            fields = stat.rsplit(')', 1)[1].split()
            processes[int(entry.name)] = dict(parent=int(fields[1]), ticks=int(fields[11]) + int(fields[12]),
                                             start_ticks=int(fields[19]), name=stat[stat.find('(')+1:stat.rfind(')')])
        except (OSError, ValueError, IndexError):
            pass
    selected = {root}
    while True:
        expanded = selected | {pid for pid, data in processes.items() if data['parent'] in selected}
        if expanded == selected:
            break
        selected = expanded
    members, missing = [], []
    if interference_processes is not None:
        compilers = {'cargo', 'rustc', 'gcc', 'g++', 'cc1', 'cc1plus', 'clang', 'clang++', 'ninja', 'make'}
        reporters = {'apport', 'drkonqi-coredum', 'systemd-coredum'}
        interference_processes.extend({'pid': pid, 'name': data['name'], 'kind': 'build' if data['name'] in compilers else 'crash_reporter'}
                                      for pid, data in processes.items() if pid not in selected and data['name'] in compilers | reporters)
    for pid in sorted(selected):
        try:
            entry = proc_root / str(pid)
            fields = dict(line.split(':', 1) for line in (entry / 'smaps_rollup').read_text().splitlines() if ':' in line)
            record = dict(pid=pid, **processes[pid],
                          executable=str((entry / 'exe').readlink()),
                          command=(entry / 'cmdline').read_bytes().decode(errors='replace').replace('\0', ' ').strip(),
                          rss_kib=int(fields['Rss'].split()[0]), pss_kib=int(fields['Pss'].split()[0]))
            members.append(record)
        except (OSError, KeyError, ValueError):
            # Enumeration and smaps reads are not atomic. A child that has
            # demonstrably exited owns no process address space at this point;
            # distinguish it from a live but unreadable process.
            if pid != root and pid in processes and not (proc_root / str(pid)).exists():
                members.append(dict(pid=pid, **processes[pid], executable=None, command=None,
                                    rss_kib=0, pss_kib=0, memory_status='confirmed_exited_during_snapshot'))
            else:
                missing.append(pid)
    return members, missing


def percentile(values, fraction):
    # Linear interpolation, specified explicitly so reports are reproducible.
    ordered = sorted(values)
    position = (len(ordered) - 1) * fraction
    low = int(position)
    high = min(low + 1, len(ordered) - 1)
    return ordered[low] + (ordered[high] - ordered[low]) * (position - low)


def summarize(rows):
    result = {}
    for key in ['rss_mib', 'pss_mib', 'cpu_percent_one_core']:
        values = [row[key] for row in rows if row[key] is not None]
        result[key] = ({'samples': len(values), 'mean': statistics.mean(values), 'p50': percentile(values, .5),
                        'p95': percentile(values, .95), 'peak': max(values)} if values else None)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, help='Run directory; must not already exist')
    parser.add_argument('--warmup', type=int, default=10)
    parser.add_argument('--seconds', type=int, default=60)
    parser.add_argument('--attach', type=int, help='Sample an existing process; leave it running')
    parser.add_argument('--source', action='append', default=[], help='Explicit source checkout or workload file')
    parser.add_argument('--label', required=True)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if args.seconds < 1 or args.warmup < 0 or bool(args.attach) == bool(command):
        parser.error('provide exactly one of --attach PID or a command, positive seconds and nonnegative warmup')
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=False)
    manifest = dict(label=args.label, started_utc=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
                    cwd=os.getcwd(), command=command, attach=args.attach,
                    warmup_seconds=args.warmup, requested_samples=args.seconds,
                    sample_interval_seconds=1, os=platform.platform(),
                    clock_ticks_per_second=os.sysconf('SC_CLK_TCK'),
                    environment={key: os.environ.get(key) for key in ['DISPLAY', 'WAYLAND_DISPLAY', 'XDG_SESSION_TYPE', 'WINIT_X11_SCALE_FACTOR', 'TWINKLE_RENDERER', 'TWINKLE_PIPELINE_TRACE']},
                    compiler=command_output(['rustc', '--version']), gpu=command_output(['nvidia-smi']),
                    sources=[source_manifest(source) for source in args.source],
                    boundaries={'memory': 'all descendants including zygote, renderer, GPU, utility processes; PSS apportions shared pages',
                                'cpu': 'process-tree user+system CPU delta; 100% is one logical core',
                                'presentation_latency': 'unavailable from external sampler',
                                'vram': 'not included in RSS/PSS; GPU snapshot in manifest is separate',
                                'limitations': ['one-second sampling can miss exited children and their CPU',
                                                'permission failures or exited processes make a sample incomplete',
                                                'normal allocator/font/GPU caches may retain capacity without being leaks']})
    proc = None
    rows = []
    executables = set()
    try:
        if not args.attach:
            proc = subprocess.Popen(command, stdout=(output / 'stdout.log').open('w'),
                                    stderr=(output / 'stderr.log').open('w'), start_new_session=True)
        root = args.attach or proc.pid
        manifest['root_pid'] = root
        manifest['running_executable'] = str(Path(f'/proc/{root}/exe').readlink())
        executable = Path(manifest['running_executable'])
        manifest['running_executable_sha256'] = hashlib.sha256(executable.read_bytes()).hexdigest()
        (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        previous, _ = snapshot(root)
        previous_ticks = {(p['pid'], p['start_ticks']): p['ticks'] for p in previous}
        start = previous_time = time.monotonic()
        previous_system = system_cpu_times()
        # Replays poll for this file; publish it only after the JSON is complete.
        ready_pending = output / 'sampling-ready.json.pending'
        ready_pending.write_text(json.dumps({'rootPid': root, 'samplingStartEpochMs': time.time()*1000, 'samplingStartMonotonicSeconds': start, 'warmupMs': args.warmup*1000}) + '\n')
        ready_pending.replace(output / 'sampling-ready.json')
        with (output / 'samples.jsonl').open('w') as details, (output / 'samples.csv').open('w', newline='') as csv_file:
            writer = csv.DictWriter(csv_file, fieldnames=['seconds', 'process_count', 'memory_complete', 'rss_mib', 'pss_mib', 'cpu_percent_one_core', 'system_cpu_percent_all_cores'])
            writer.writeheader()
            for index in range(args.warmup + args.seconds):
                time.sleep(max(0, start + index + 1 - time.monotonic()))
                if proc is not None and proc.poll() is not None:
                    raise RuntimeError(f'Application exited early: {proc.returncode}')
                interference_processes = []
                members, missing = snapshot(root, interference_processes=interference_processes)
                if not any(p['pid'] == root for p in members):
                    raise RuntimeError('Root process is no longer readable')
                executables.update(p["executable"] for p in members if p["executable"] is not None)
                now = time.monotonic()
                system = system_cpu_times()
                system_cpu = None
                if system is not None and previous_system is not None and system[0] > previous_system[0]:
                    system_cpu = (system[1] - previous_system[1]) / (system[0] - previous_system[0]) * 100
                previous_system = system
                current = {(p['pid'], p['start_ticks']): p['ticks'] for p in members}
                cpu = sum(max(0, ticks - previous_ticks.get(key, 0)) for key, ticks in current.items()) / os.sysconf('SC_CLK_TCK') / (now - previous_time) * 100
                previous_ticks, previous_time = current, now
                if index < args.warmup:
                    continue
                row = dict(seconds=now-start, process_count=len(members), memory_complete=not missing,
                           rss_mib=sum(p['rss_kib'] for p in members)/1024 if not missing else None,
                           pss_mib=sum(p['pss_kib'] for p in members)/1024 if not missing else None,
                           cpu_percent_one_core=cpu, system_cpu_percent_all_cores=system_cpu)
                writer.writerow(row)
                details.write(json.dumps(dict(**row, processes=members, unreadable_pids=missing, background_interference_processes=interference_processes)) + '\n')
                row['background_interference_processes'] = interference_processes
                rows.append(row)
        manifest['process_executables_sha256'] = {
            path: hashlib.sha256(Path(path).read_bytes()).hexdigest() for path in sorted(executables)
        }
        (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        summary = dict(status='complete', samples=len(rows), metrics=summarize(rows))
        summary['background_build_samples'] = sum(any(p['kind']=='build' for p in row['background_interference_processes']) for row in rows)
        summary['background_interference_samples'] = sum(bool(row['background_interference_processes']) for row in rows)
        ambient = [row['system_cpu_percent_all_cores'] for row in rows if row['system_cpu_percent_all_cores'] is not None]
        summary['system_cpu_percent_all_cores'] = {'samples': len(ambient), 'mean': statistics.mean(ambient), 'p95': percentile(ambient, .95)} if ambient else None
        summary['quiet_machine_boundary'] = 'External build/crash-reporter process names checked once per sample; transient/restricted processes may be missed. Aggregate machine CPU includes measured app and all other work; no noise subtraction.'
        (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
        print(json.dumps(summary, indent=2))
    except Exception as error:
        (output / 'summary.json').write_text(json.dumps(dict(status='incomplete', error=str(error), samples=len(rows)), indent=2) + '\n')
        raise
    finally:
        if proc is not None:
            try:
                os.killpg(proc.pid, signal.SIGTERM)
                proc.wait(timeout=5)
            except ProcessLookupError:
                pass
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait()


if __name__ == '__main__':
    main()
