//! ToneLab planned plugin contract.

use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-tonelab",
    namespace: "tonelab",
    summary: "Local tone, contrast and exposure operations",
    filters: &[
        Filter {
            name: "Clahe",
            summary: "Apply contrast-limited adaptive equalization",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "LocalLaplacian",
            summary: "Perform edge-aware local tone adjustment",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "ExposureFusion",
            summary: "Fuse differently exposed inputs",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "NormalizeIllumination",
            summary: "Separate and normalize illumination",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
    ],
};

