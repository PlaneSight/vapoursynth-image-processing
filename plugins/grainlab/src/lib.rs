//! GrainLab planned plugin contract.

use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-grainlab",
    namespace: "grainlab",
    summary: "Grain measurement, matching and synthesis",
    filters: &[
        Filter {
            name: "Analyze",
            summary: "Measure spatial and temporal grain statistics",
            maturity: Maturity::Planned,
            execution: Execution::Analysis,
        },
        Filter {
            name: "Synthesize",
            summary: "Generate grain from a measured profile",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Match",
            summary: "Transfer a grain profile to a clean clip",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Residual",
            summary: "Extract the candidate grain residual",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
    ],
};

