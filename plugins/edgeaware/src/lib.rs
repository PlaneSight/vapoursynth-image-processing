//! EdgeAware planned plugin contract.

use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-edgeaware",
    namespace: "edgeaware",
    summary: "Guided and edge-preserving smoothing",
    filters: &[
        Filter {
            name: "Guided",
            summary: "Apply self-guided smoothing",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "JointGuided",
            summary: "Filter using another guide clip",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "DomainTransform",
            summary: "Apply domain-transform filtering",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "RollingGuidance",
            summary: "Remove small structures iteratively",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "GlobalSmooth",
            summary: "Apply edge-aware global smoothing",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
    ],
};

