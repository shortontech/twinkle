#!/usr/bin/env python3
"""Validate and summarize a chart dashboard GPU-admission trace."""

import argparse
import json
from pathlib import Path


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("trace", type=Path)
args = parser.parse_args()

frames = [
    row
    for line in args.trace.read_text().splitlines()
    if (row := json.loads(line)).get("event") == "gpu_frame"
]
if len(frames) < 4:
    parser.error(f"expected at least four GPU frames, found {len(frames)}")

reuse_index = next(
    (
        index
        for index, frame in enumerate(frames)
        if frame.get("redraw") == "Reuse"
        and frame.get("commands_prepared") == 0
        and frame.get("scene_draws_encoded") == 0
        and frame.get("vertex_upload_calls") == 0
        and frame.get("total_vertex_upload_bytes") == 0
    ),
    None,
)
if reuse_index is None:
    parser.error("trace has no zero-preparation, zero-upload retained reuse frame")
if reuse_index == 0 or frames[reuse_index - 1].get("commands_prepared", 0) < 100:
    parser.error("trace does not contain the expected 16-chart initial frame")

changed = [
    frame
    for frame in frames[reuse_index + 1 :]
    if frame.get("commands_prepared", 0) > 0
    and frame.get("total_vertex_upload_bytes", 0) > 0
]
if len(changed) < 2:
    parser.error("trace does not contain the expected post-reuse hover and zoom frames")

fields = (
    "redraw",
    "fallback_reason",
    "commands_prepared",
    "scene_draws_encoded",
    "scene_vertex_upload_bytes",
    "total_vertex_upload_bytes",
    "vertex_upload_calls",
    "command_finalize_and_submit_us",
    "presenter_call_us",
)
summary = {
    "status": "valid",
    "gpuFrames": len(frames),
    "initial": {field: frames[reuse_index - 1].get(field) for field in fields},
    "unchanged": {field: frames[reuse_index].get(field) for field in fields},
    "hover": {field: changed[0].get(field) for field in fields},
    "zoom": {field: changed[1].get(field) for field in fields},
    "boundary": (
        "GPU presenter CPU call and upload accounting; compositor display and "
        "GPU execution completion are unavailable"
    ),
}
print(json.dumps(summary, indent=2, sort_keys=True))
