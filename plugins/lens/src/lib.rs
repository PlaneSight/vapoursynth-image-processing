//! Lens planned plugin contract.

use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-lens",
    namespace: "lens",
    summary: "Lens, chromatic aberration and rolling-shutter correction",
    filters: &[
        Filter {
            name: "Undistort",
            summary: "Correct radial and tangential distortion",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "Chromatic",
            summary: "Correct wavelength-dependent displacement",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "Vignette",
            summary: "Correct or apply radial light falloff",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "RollingShutter",
            summary: "Correct row-dependent camera motion",
            maturity: Maturity::Planned,
            execution: Execution::Temporal,
        },
    ],
};

