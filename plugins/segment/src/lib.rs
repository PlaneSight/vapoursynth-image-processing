//! Segment planned plugin contract.

use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-segment",
    namespace: "segment",
    summary: "Watershed, superpixels and region graphs",
    filters: &[
        Filter {
            name: "Watershed",
            summary: "Segment from image gradients and markers",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Superpixels",
            summary: "Generate compact perceptual regions",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "RegionGraph",
            summary: "Measure adjacency between labeled regions",
            maturity: Maturity::Planned,
            execution: Execution::Analysis,
        },
        Filter {
            name: "Merge",
            summary: "Merge regions under a typed criterion",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
    ],
};

