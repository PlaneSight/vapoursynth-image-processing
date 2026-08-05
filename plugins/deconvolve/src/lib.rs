//! Deconvolve planned plugin contract.

use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-deconvolve",
    namespace: "deconvolve",
    summary: "Explicit-PSF and regularized deconvolution",
    filters: &[
        Filter {
            name: "Wiener",
            summary: "Frequency-domain Wiener deconvolution",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "RichardsonLucy",
            summary: "Iterative Richardson-Lucy deconvolution",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "Regularized",
            summary: "Edge-aware regularized deconvolution",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "EstimatePsf",
            summary: "Estimate a constrained point-spread function",
            maturity: Maturity::Planned,
            execution: Execution::Analysis,
        },
    ],
};

