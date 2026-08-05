"""Deterministic MaskLab ABI and frame-processing smoke test."""

import os

import vapoursynth as vs


plugin_path = os.environ.get("VSIP_MASKLAB_PLUGIN")
if not plugin_path:
    raise RuntimeError("VSIP_MASKLAB_PLUGIN must name the absolute MaskLab library path")

core = vs.core
core.std.LoadPlugin(path=plugin_path)

source = core.std.BlankClip(width=5, height=3, length=1, format=vs.GRAY8, color=[255])
distance = core.masklab.DistanceL1(source)
frame = distance.get_frame(0)
plane = memoryview(frame[0])
first_sample = plane[0, 0] if plane.ndim == 2 else plane[0]
if first_sample != 8:
    raise RuntimeError(f"DistanceL1 returned {first_sample}; expected the no-seed sentinel 8")

distance.set_output()
