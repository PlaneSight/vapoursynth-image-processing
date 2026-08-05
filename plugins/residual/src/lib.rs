//! Residual planned plugin contract.

use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-residual",
    namespace: "residual",
    summary: "Structure, texture and noise decomposition",
    filters: &[
        Filter {
            name: "Decompose",
            summary: "Separate structure, texture and residual",
            maturity: Maturity::Planned,
            execution: Execution::Analysis,
        },
        Filter {
            name: "Temporal",
            summary: "Measure motion-aligned temporal residual",
            maturity: Maturity::Planned,
            execution: Execution::Temporal,
        },
        Filter {
            name: "Spectrum",
            summary: "Measure residual power spectrum",
            maturity: Maturity::Planned,
            execution: Execution::Analysis,
        },
        Filter {
            name: "Compare",
            summary: "Compare residual fields between clips",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
    ],
};

