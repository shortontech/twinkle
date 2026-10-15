#!/usr/bin/env python3
"""Run serialized, alternating release pairs for spec 0021's native fixtures.

Build and freeze both normal release test executables first. Attribution builds
must run separately. Each executable must contain the same fixture definitions.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--guards", action="store_true", help="Run the frozen Blocks/rich-form integration executables")
    parser.add_argument("--matrix", action="store_true", help="Run the native-frame sparse/tiny/dense/structural fixture executables")
    args = parser.parse_args()
    if args.guards and args.matrix:
        parser.error("Choose either --guards or --matrix")
    args.output.mkdir(exist_ok=False)
    binaries = {"before": args.before.resolve(), "after": args.after.resolve()}
    protocol = {
        "status": "running",
        "pairs": 5,
        "binarySha256": {key: hashlib.sha256(path.read_bytes()).hexdigest()
                         for key, path in binaries.items()},
        "runs": [],
        "candidateRevision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "candidateTrackedPatchSha256": hashlib.sha256(subprocess.check_output(["git", "diff", "--binary", "HEAD"])).hexdigest(),
        "fixtureSourceSha256": hashlib.sha256((Path(__file__).resolve().parents[1] / "crates/twinkle/src/ui/tree/measurement_admission.rs").read_bytes()).hexdigest(),
        "scope": "Complete native frame including declaration construction; 801 nodes, 800x720, 10 warmups, 200 samples. No JSX producer/update timing claim.",
    }
    env = os.environ.copy()
    for key in ("TWINKLE_PIPELINE_TRACE", "TWINKLE_JSX_TRACE"):
        env.pop(key, None)
    fixtures = ["sparse_active_overlay_dependency_elision_release_workload",
                "retained_sparse_active_overlay_identity_release_workload"]
    if args.guards:
        fixtures = ["blocks_shaped_jsx_updates_emit_end_to_end_release_distribution",
                    "rich_accessible_form_updates_emit_end_to_end_release_distribution"]
        protocol["scope"] = "Frozen 200-cell Blocks and eight-field rich form; 10 warmups, 1000 complete updates with native-frame/layout distributions."
    if args.matrix:
        fixtures = ["sparse-cold", "sparse-retained", "tiny", "dense", "structural"]
        protocol["scope"] = "Native-frame fixture matrix; 10 warmups, 200 samples; complete update includes declaration construction. Separate attribution/allocator builds."
    try:
        for fixture in fixtures:
            for pair in range(1, 6):
                order = ["before", "after"] if pair % 2 else ["after", "before"]
                for variant in order:
                    if args.matrix:
                        env["TWINKLE_MEASUREMENT_CASE"] = fixture
                    test = "measurement_boundary_native_frame_release_admission" if args.matrix else fixture
                    result = subprocess.run(
                        [str(binaries[variant]), test, "--ignored", "--nocapture", "--test-threads=1"],
                        env=env, capture_output=True, text=True, check=True)
                    log = result.stdout + result.stderr
                    name = f"{fixture}-{pair:02}-{variant}.log"
                    (args.output / name).write_text(log)
                    if "test result: ok. 1 passed" not in log:
                        raise ValueError(f"Missing successful fixture: {name}")
                    run = {"fixture": fixture, "pair": pair, "variant": variant, "log": name}
                    if args.matrix:
                        reports = [json.loads(line.split("measurement_boundary_admission=", 1)[1])
                                   for line in log.splitlines() if line.startswith("measurement_boundary_admission=")]
                        if len(reports) != 1 or reports[0]["case"] != fixture:
                            raise ValueError(f"Missing native report: {name}")
                        run["report"] = reports[0]
                        print(f"{fixture} pair={pair} {variant}: {reports[0]['nativeFrame']}", flush=True)
                    elif args.guards:
                        reports = [json.loads(line.split("twinkle_release_admission=", 1)[1])
                                   for line in log.splitlines() if line.startswith("twinkle_release_admission=")]
                        if len(reports) != 1:
                            raise ValueError(f"Missing frozen report: {name}")
                        run["report"] = reports[0]
                        print(f"{fixture} pair={pair} {variant}: complete", flush=True)
                    else:
                        metrics = re.search(r"p50_us=([\d.]+) p95_us=([\d.]+) total_us=([\d.]+)", log)
                        if metrics is None:
                            raise ValueError(f"Missing distribution: {name}")
                        run.update(zip(("p50_us", "p95_us", "total_us"), map(float, metrics.groups())))
                        print(f"{fixture} pair={pair} {variant}: {metrics.group(0)}", flush=True)
                    protocol["runs"].append(run)
        protocol["status"] = "complete"
    finally:
        (args.output / "protocol.json").write_text(json.dumps(protocol, indent=2) + "\n")


if __name__ == "__main__":
    main()
