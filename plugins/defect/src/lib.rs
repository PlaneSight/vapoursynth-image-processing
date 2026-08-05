//! Defect planned plugin contract.

use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-defect",
    namespace: "defect",
    summary: "Dust, scratches, dropouts and temporal restoration",
    filters: &[
        Filter {
            name: "TemporalOutliers",
            summary: "Detect isolated temporal defects",
            maturity: Maturity::Planned,
            execution: Execution::Temporal,
        },
        Filter {
            name: "ScratchDetect",
            summary: "Detect persistent line-shaped defects",
            maturity: Maturity::Planned,
            execution: Execution::Analysis,
        },
        Filter {
            name: "DropoutRepair",
            summary: "Repair tape and digital dropouts",
            maturity: Maturity::Planned,
            execution: Execution::Temporal,
        },
        Filter {
            name: "DeadPixels",
            summary: "Detect spatially fixed sensor defects",
            maturity: Maturity::Planned,
            execution: Execution::Analysis,
        },
        Filter {
            name: "InpaintTemporal",
            summary: "Repair masks from motion-aligned neighbours",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
    ],
};

