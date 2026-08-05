"""Load every VSIP library and verify its complete public catalogue."""

import os
from pathlib import Path
import sys

import vapoursynth as vs


CATALOGUES = {
    "masklab": (
        "DistanceL1",
        "DistanceEuclidean",
        "ComponentLabels",
        "Reconstruct",
        "Thin",
        "Feather",
    ),
    "defect": (
        "TemporalOutliers",
        "ScratchDetect",
        "DropoutRepair",
        "SpatialOutliers",
        "InpaintTemporal",
    ),
    "deconvolve": ("Wiener", "RichardsonLucy", "Regularized", "EstimatePsf"),
    "flowfield": ("Estimate", "Warp", "Confidence", "Compose", "Visualize"),
    "grainlab": ("Analyze", "Synthesize", "Match", "Residual"),
    "phase": ("Correlate", "LocalMotion", "Magnify", "EventEnergy"),
    "edgeaware": (
        "Guided",
        "JointGuided",
        "DomainTransform",
        "RollingGuidance",
        "GlobalSmooth",
    ),
    "register": ("Estimate", "Warp", "Stack", "Stabilize"),
    "lens": ("Undistort", "Chromatic", "Vignette", "RollingShutter"),
    "tonelab": (
        "Clahe",
        "LocalLaplacian",
        "ExposureFusion",
        "NormalizeIllumination",
    ),
    "residual": ("Decompose", "Temporal", "Spectrum", "Compare"),
    "segment": ("Watershed", "Superpixels", "RegionDegreeMap", "Merge"),
}


plugin_dir_value = os.environ.get("VSIP_PLUGIN_DIR")
if not plugin_dir_value:
    raise RuntimeError("VSIP_PLUGIN_DIR must name the absolute release directory")
plugin_dir = Path(plugin_dir_value)
if not plugin_dir.is_absolute():
    raise RuntimeError("VSIP_PLUGIN_DIR must be absolute")

if os.name == "nt":
    prefix, suffix = "", ".dll"
elif sys.platform == "darwin":
    prefix, suffix = "lib", ".dylib"
else:
    prefix, suffix = "lib", ".so"

core = vs.core
for namespace, filters in CATALOGUES.items():
    library = plugin_dir / f"{prefix}vs_{namespace}_vapoursynth{suffix}"
    if not library.is_file():
        raise RuntimeError(f"missing adapter library: {library}")
    core.std.LoadPlugin(path=str(library))
    plugin = getattr(core, namespace)
    missing = [name for name in filters if not hasattr(plugin, name)]
    if missing:
        raise RuntimeError(f"{namespace} did not register: {', '.join(missing)}")

core.std.BlankClip(width=2, height=2, length=1, format=vs.GRAY8).set_output()
