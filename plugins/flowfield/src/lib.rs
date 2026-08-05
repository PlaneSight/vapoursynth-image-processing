//! FlowField planned plugin contract.

use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-flowfield",
    namespace: "flowfield",
    summary: "Dense motion fields, confidence and warping",
    filters: &[
        Filter {
            name: "Estimate",
            summary: "Estimate forward or backward dense motion",
            maturity: Maturity::Planned,
            execution: Execution::Temporal,
        },
        Filter {
            name: "Warp",
            summary: "Warp a clip with a motion field",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Confidence",
            summary: "Compute forward-backward confidence",
            maturity: Maturity::Planned,
            execution: Execution::Analysis,
        },
        Filter {
            name: "Compose",
            summary: "Compose two motion fields",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Visualize",
            summary: "Render direction and magnitude",
            maturity: Maturity::Planned,
            execution: Execution::Analysis,
        },
    ],
};

