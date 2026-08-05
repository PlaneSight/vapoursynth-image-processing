//! Phase planned plugin contract.

use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-phase",
    namespace: "phase",
    summary: "Phase motion, vibration and transient event analysis",
    filters: &[
        Filter {
            name: "Correlate",
            summary: "Measure global subpixel translation",
            maturity: Maturity::Planned,
            execution: Execution::Temporal,
        },
        Filter {
            name: "LocalMotion",
            summary: "Measure tiled phase displacement",
            maturity: Maturity::Planned,
            execution: Execution::Temporal,
        },
        Filter {
            name: "Magnify",
            summary: "Amplify motion in a temporal frequency band",
            maturity: Maturity::Planned,
            execution: Execution::Temporal,
        },
        Filter {
            name: "EventEnergy",
            summary: "Score coherent transient motion",
            maturity: Maturity::Planned,
            execution: Execution::Analysis,
        },
    ],
};

