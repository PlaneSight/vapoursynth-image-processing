//! Typed, allocation-free descriptions of plugin namespaces and filters.

/// Implementation maturity exposed to tooling and documentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Maturity {
    /// Contract only; not callable.
    Planned,
    /// Implemented but API and results may change.
    Experimental,
    /// Tested and supported public contract.
    Stable,
}

/// Dominant execution shape of a filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Execution {
    /// Operates on one frame without neighbouring frames.
    Spatial,
    /// Requests neighbouring frames.
    Temporal,
    /// Consumes more than one input clip or auxiliary plane.
    MultiInput,
    /// Produces measurements or frame properties.
    Analysis,
}

/// One public filter contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Filter {
    /// VapourSynth-visible function name.
    pub name: &'static str,
    /// Short purpose statement.
    pub summary: &'static str,
    /// Current implementation maturity.
    pub maturity: Maturity,
    /// Dominant execution shape.
    pub execution: Execution,
}

/// One independently releasable plugin namespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Plugin {
    /// Cargo package name.
    pub package: &'static str,
    /// VapourSynth namespace.
    pub namespace: &'static str,
    /// Plugin purpose.
    pub summary: &'static str,
    /// Public filter catalogue.
    pub filters: &'static [Filter],
}
