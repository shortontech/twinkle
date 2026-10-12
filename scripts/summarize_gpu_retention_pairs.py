#!/usr/bin/env python3
"""Summarize same-binary retained GPU sparse-update pairs."""

from __future__ import annotations

import argparse
import json
import statistics
from pathlib import Path


def percentile(values: list[float], quantile: float) -> float:
    ordered = sorted(values)
    if not ordered:
        raise ValueError("empty sample")
    position = (len(ordered) - 1) * quantile
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = position - lower
    return ordered[lower] * (1.0 - fraction) + ordered[upper] * fraction


def samples(path: Path, event: str, skip: int, redraw_prefix: str) -> list[dict]:
    rows = [json.loads(line) for line in path.read_text().splitlines()]
    if event == "gpu_frame":
        rows = [
            row
            for row in rows
            if row.get("event") == event
            and row.get("redraw", "").startswith(redraw_prefix)
        ]
    else:
        rows = [row for row in rows if row.get("event") == event]
        # Align companion events with partial frames by excluding initial full frame.
        rows = rows[1:]
    return rows[skip:]


def stats(values: list[float]) -> dict[str, float | int | None]:
    if not values:
        return {"count": 0, "median": None, "p95": None}
    return {
        "count": len(values),
        "median": statistics.median(values),
        "p95": percentile(values, 0.95),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    parser.add_argument("--skip", type=int, default=2)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--redraw-prefix", default="Partial")
    args = parser.parse_args()
    keys = [
        "presenter_call_us",
        "preparation_us",
        "vertex_upload_call_us",
        "encode_submit_us",
        "present_call_us",
        "commands_prepared",
        "scene_vertices",
        "scene_vertex_upload_bytes",
        "total_vertex_upload_bytes",
    ]
    allocation_keys = ["allocation_calls", "allocation_requested_bytes"]
    pairs: list[dict] = []
    for pair in range(1, 6):
        result: dict = {"pair": pair}
        for mode in ("legacy", "candidate"):
            path = args.root / f"pair-{pair}-{mode}" / "pipeline.jsonl"
            frames = samples(path, "gpu_frame", args.skip, args.redraw_prefix)
            damage = samples(path, "gpu_damage", args.skip, args.redraw_prefix)
            result[mode] = {
                key: stats([row[key] for row in frames if row.get(key) is not None])
                for key in keys
            }
            result[mode]["allocator"] = {
                key: stats([row[key] for row in damage if row.get(key) is not None])
                for key in allocation_keys
            }
        pairs.append(result)

    metrics = keys + [f"allocator.{key}" for key in allocation_keys]
    aggregate: dict[str, dict] = {}
    for key in metrics:
        section, _, leaf = key.partition(".")
        medians: dict[str, list[float]] = {"legacy": [], "candidate": []}
        p95s: dict[str, list[float]] = {"legacy": [], "candidate": []}
        for pair in pairs:
            for mode in medians:
                entry = pair[mode][section][leaf] if leaf else pair[mode][section]
                if entry["median"] is not None:
                    medians[mode].append(entry["median"])
                if entry["p95"] is not None:
                    p95s[mode].append(entry["p95"])
        if not medians["legacy"] or not medians["candidate"]:
            aggregate[key] = {"available": False}
            continue
        legacy = statistics.median(medians["legacy"])
        candidate = statistics.median(medians["candidate"])
        aggregate[key] = {
            "medianOfPairMedians": {"legacy": legacy, "candidate": candidate},
            "candidateReductionPercent": (legacy - candidate) / legacy * 100.0
            if legacy
            else None,
            "medianOfPairP95": {
                "legacy": statistics.median(p95s["legacy"]),
                "candidate": statistics.median(p95s["candidate"]),
            },
        }

    output = {
        "schema": 1,
        "comparison": "same release binary; candidate versus TWINKLE_GPU_DISABLE_INCREMENTAL_PREPARATION=1",
        "fixture": "rich-form with one visible retained hover probe",
        "sequence": f"102 fixture updates; first two {args.redraw_prefix} frames excluded as warmup",
        "timingBoundary": "CPU API service through queue present; not GPU completion or compositor display",
        "allocatorBoundary": "process-wide Rust allocator deltas; foreign driver/C++ allocations excluded",
        "pairs": pairs,
        "aggregate": aggregate,
    }
    encoded = json.dumps(output, indent=2) + "\n"
    if args.output:
        args.output.write_text(encoded)
    else:
        print(encoded, end="")


if __name__ == "__main__":
    main()
