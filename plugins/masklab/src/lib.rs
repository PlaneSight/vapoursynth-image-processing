//! MaskLab plugin contract and implemented reference operations.

pub use vsip_kernels::distance::{DistanceError, l1_to_zero};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-masklab",
    namespace: "masklab",
    summary: "Morphology, distance fields and connected-region analysis",
    filters: &[
        Filter {
            name: "DistanceL1",
            summary: "Manhattan distance to the nearest zero-valued mask pixel",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "DistanceEuclidean",
            summary: "Exact Euclidean distance transform",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "Components",
            summary: "Connected-component labels and statistics",
            maturity: Maturity::Planned,
            execution: Execution::Analysis,
        },
        Filter {
            name: "Reconstruct",
            summary: "Geodesic morphological reconstruction",
            maturity: Maturity::Planned,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Thin",
            summary: "Topology-preserving binary thinning",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
        Filter {
            name: "Feather",
            summary: "Distance-based inner and outer mask feathering",
            maturity: Maturity::Planned,
            execution: Execution::Spatial,
        },
    ],
};

