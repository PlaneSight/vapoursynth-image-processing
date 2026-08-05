//! Register planned plugin contract.

use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-register",
    namespace: "register",
    summary: "Image registration, alignment and robust stacking",
    filters: &[
        Filter {
            name: "Estimate",
            summary: "Estimate a geometric transform",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Warp",
            summary: "Apply a registration transform",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Stack",
            summary: "Robustly combine aligned clips",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Stabilize",
            summary: "Smooth and compensate camera motion",
            maturity: Maturity::Planned,
            execution: Execution::Temporal,
        },
    ],
};

