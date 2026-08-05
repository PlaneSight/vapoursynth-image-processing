//! Scalar reference implementations of explicit-PSF deconvolution operations.
//!
//! [`wiener`] uses direct two-dimensional DFT and inverse DFT, with
//! `O((width * height)^2)` work and caller-provided reusable spectra. The
//! iterative [`richardson_lucy`] and [`regularized`] references use direct
//! circular convolution, costing `O(iterations * (width * height)^2)`. These
//! intentionally simple algorithms establish deterministic behavior, not a
//! practical large-frame implementation.

use core::fmt;

use vsip_core::{Extent, GeometryError, Plane, PlaneMut};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

#[derive(Clone, Copy, Debug, Default)]
struct Complex {
    real: f64,
    imaginary: f64,
}

impl Complex {
    fn multiply(self, right: Self) -> Self {
        Self {
            real: self.real * right.real - self.imaginary * right.imaginary,
            imaginary: self.real * right.imaginary + self.imaginary * right.real,
        }
    }

    fn conjugate(self) -> Self {
        Self {
            real: self.real,
            imaginary: -self.imaginary,
        }
    }

    fn magnitude_squared(self) -> f64 {
        self.real * self.real + self.imaginary * self.imaginary
    }
}

/// Validated configuration for frequency-domain Wiener deconvolution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WienerConfig {
    noise_to_signal: f32,
}

impl WienerConfig {
    /// Creates a Wiener configuration with a finite, non-negative noise ratio.
    pub fn new(noise_to_signal: f32) -> Result<Self, DeconvolveError> {
        if !noise_to_signal.is_finite() || noise_to_signal < 0.0 {
            return Err(DeconvolveError::InvalidConfiguration);
        }
        Ok(Self { noise_to_signal })
    }

    /// Returns the additive frequency-domain noise-to-signal regularizer.
    pub const fn noise_to_signal(self) -> f32 {
        self.noise_to_signal
    }
}

/// Validated configuration for Richardson-Lucy iterations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RichardsonLucyConfig {
    iterations: usize,
    epsilon: f32,
}

impl RichardsonLucyConfig {
    /// Creates a configuration with positive iteration count and denominator floor.
    pub fn new(iterations: usize, epsilon: f32) -> Result<Self, DeconvolveError> {
        if iterations == 0 || !epsilon.is_finite() || epsilon <= 0.0 {
            return Err(DeconvolveError::InvalidConfiguration);
        }
        Ok(Self {
            iterations,
            epsilon,
        })
    }

    /// Returns the number of multiplicative iterations.
    pub const fn iterations(self) -> usize {
        self.iterations
    }

    /// Returns the positive blurred-image denominator floor.
    pub const fn epsilon(self) -> f32 {
        self.epsilon
    }
}

/// Validated configuration for edge-aware regularized iterations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegularizedConfig {
    iterations: usize,
    data_step: f32,
    regularization: f32,
    edge_epsilon: f32,
}

impl RegularizedConfig {
    /// Creates a regularized configuration.
    ///
    /// `data_step` and `edge_epsilon` must be finite and positive;
    /// `regularization` must be finite and non-negative.
    pub fn new(
        iterations: usize,
        data_step: f32,
        regularization: f32,
        edge_epsilon: f32,
    ) -> Result<Self, DeconvolveError> {
        if iterations == 0
            || !data_step.is_finite()
            || data_step <= 0.0
            || !regularization.is_finite()
            || regularization < 0.0
            || !edge_epsilon.is_finite()
            || edge_epsilon <= 0.0
        {
            return Err(DeconvolveError::InvalidConfiguration);
        }
        Ok(Self {
            iterations,
            data_step,
            regularization,
            edge_epsilon,
        })
    }

    /// Returns the number of gradient iterations.
    pub const fn iterations(self) -> usize {
        self.iterations
    }

    /// Returns the data-gradient step size.
    pub const fn data_step(self) -> f32 {
        self.data_step
    }

    /// Returns the edge-aware smoothness multiplier.
    pub const fn regularization(self) -> f32 {
        self.regularization
    }

    /// Returns the positive edge-weight denominator floor.
    pub const fn edge_epsilon(self) -> f32 {
        self.edge_epsilon
    }
}

/// Validated configuration for constrained impulse point-spread estimation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PsfEstimateConfig {
    max_shift: usize,
}

impl PsfEstimateConfig {
    /// Creates an exhaustive circular impulse-shift search configuration.
    ///
    /// A zero shift is valid and estimates an identity PSF only.
    pub fn new(max_shift: usize) -> Result<Self, DeconvolveError> {
        isize::try_from(max_shift).map_err(|_| DeconvolveError::CoordinatesTooLarge)?;
        Ok(Self { max_shift })
    }

    /// Returns the greatest tested impulse displacement in either direction.
    pub const fn max_shift(self) -> usize {
        self.max_shift
    }
}

/// Reusable caller-owned working storage for [`wiener`].
#[derive(Debug)]
pub struct WienerScratch {
    extent: Extent,
    input_spectrum: Vec<Complex>,
    psf_spectrum: Vec<Complex>,
    estimate_spectrum: Vec<Complex>,
}

impl WienerScratch {
    /// Allocates reusable storage exactly sized for `extent`.
    ///
    /// Allocation occurs only here, never inside [`wiener`].
    pub fn try_new(extent: Extent) -> Result<Self, DeconvolveError> {
        let area = area_for_scratch(extent)?;
        Ok(Self {
            extent,
            input_spectrum: zeroed_complex(area)?,
            psf_spectrum: zeroed_complex(area)?,
            estimate_spectrum: zeroed_complex(area)?,
        })
    }

    /// Returns the only extent accepted by this scratch allocation.
    pub const fn extent(&self) -> Extent {
        self.extent
    }
}

/// Reusable caller-owned working storage for [`richardson_lucy`].
#[derive(Debug)]
pub struct RichardsonLucyScratch {
    extent: Extent,
    estimate: Vec<f32>,
    blurred: Vec<f32>,
    ratio: Vec<f32>,
    correction: Vec<f32>,
}

impl RichardsonLucyScratch {
    /// Allocates reusable storage exactly sized for `extent`.
    pub fn try_new(extent: Extent) -> Result<Self, DeconvolveError> {
        let area = area_for_scratch(extent)?;
        Ok(Self {
            extent,
            estimate: zeroed_f32(area)?,
            blurred: zeroed_f32(area)?,
            ratio: zeroed_f32(area)?,
            correction: zeroed_f32(area)?,
        })
    }

    /// Returns the only extent accepted by this scratch allocation.
    pub const fn extent(&self) -> Extent {
        self.extent
    }
}

/// Reusable caller-owned working storage for [`regularized`].
#[derive(Debug)]
pub struct RegularizedScratch {
    extent: Extent,
    estimate: Vec<f32>,
    blurred: Vec<f32>,
    residual: Vec<f32>,
    backprojection: Vec<f32>,
    next: Vec<f32>,
}

impl RegularizedScratch {
    /// Allocates reusable storage exactly sized for `extent`.
    pub fn try_new(extent: Extent) -> Result<Self, DeconvolveError> {
        let area = area_for_scratch(extent)?;
        Ok(Self {
            extent,
            estimate: zeroed_f32(area)?,
            blurred: zeroed_f32(area)?,
            residual: zeroed_f32(area)?,
            backprojection: zeroed_f32(area)?,
            next: zeroed_f32(area)?,
        })
    }

    /// Returns the only extent accepted by this scratch allocation.
    pub const fn extent(&self) -> Extent {
        self.extent
    }
}

/// Failure returned by a deconvolution reference operation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeconvolveError {
    /// An input plane contains NaN or infinity.
    NonFiniteSample,
    /// Richardson-Lucy requires non-negative observed samples.
    NegativeObservation,
    /// A PSF has a negative value, non-finite sample, or zero total mass.
    InvalidPsf,
    /// A configuration violates a finite or positive scalar requirement.
    InvalidConfiguration,
    /// Corresponding planes do not have the same visible extent.
    ExtentMismatch,
    /// Scratch was allocated for a different extent.
    ScratchExtentMismatch,
    /// Scratch size arithmetic overflowed.
    ScratchSizeOverflow,
    /// Scratch allocation failed before a transform started.
    AllocationFailure,
    /// A finite input and configuration produced an unrepresentable f32 result.
    NumericalOverflow,
    /// Scalar coordinate conversion could not represent plane geometry.
    CoordinatesTooLarge,
    /// A plane failed its stride or backing-buffer validation.
    Geometry(GeometryError),
}

impl fmt::Display for DeconvolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteSample => formatter.write_str("input contains a non-finite sample"),
            Self::NegativeObservation => {
                formatter.write_str("Richardson-Lucy observed samples must be non-negative")
            }
            Self::InvalidPsf => formatter.write_str("point-spread function is invalid"),
            Self::InvalidConfiguration => formatter.write_str("configuration is invalid"),
            Self::ExtentMismatch => formatter.write_str("all planes must have the same extent"),
            Self::ScratchExtentMismatch => {
                formatter.write_str("scratch allocation does not match the plane extent")
            }
            Self::ScratchSizeOverflow => formatter.write_str("scratch size overflowed"),
            Self::AllocationFailure => formatter.write_str("scratch allocation failed"),
            Self::NumericalOverflow => formatter.write_str("calculation exceeded f32 range"),
            Self::CoordinatesTooLarge => {
                formatter.write_str("plane coordinates exceed the scalar reference range")
            }
            Self::Geometry(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for DeconvolveError {}

impl From<GeometryError> for DeconvolveError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry(error)
    }
}

/// Applies circular Wiener deconvolution with an origin-at-zero full-frame PSF.
///
/// `psf(0, 0)` is the zero-lag impulse coefficient; centered PSFs must be
/// circularly shifted by the caller. The non-negative PSF is normalized by its
/// positive total mass. All planes and `scratch` must have the same extent.
/// The function allocates nothing after scratch creation.
pub fn wiener(
    observed: Plane<'_, f32>,
    psf: Plane<'_, f32>,
    output: PlaneMut<'_, f32>,
    scratch: &mut WienerScratch,
    config: WienerConfig,
) -> Result<(), DeconvolveError> {
    ensure_wiener_geometry(
        observed.extent(),
        psf.extent(),
        output.extent(),
        scratch.extent,
    )?;
    ensure_finite(observed)?;
    let mass = validate_psf(psf)?;
    dft(observed, &mut scratch.input_spectrum)?;
    dft(psf, &mut scratch.psf_spectrum)?;
    for frequency in &mut scratch.psf_spectrum {
        frequency.real /= mass;
        frequency.imaginary /= mass;
    }
    for ((estimate, input), psf) in scratch
        .estimate_spectrum
        .iter_mut()
        .zip(&scratch.input_spectrum)
        .zip(&scratch.psf_spectrum)
    {
        let denominator = psf.magnitude_squared() + f64::from(config.noise_to_signal);
        *estimate = if denominator == 0.0 {
            Complex::default()
        } else {
            let numerator = psf.conjugate().multiply(*input);
            Complex {
                real: numerator.real / denominator,
                imaginary: numerator.imaginary / denominator,
            }
        };
    }
    inverse_dft(&scratch.estimate_spectrum, output)
}

/// Applies multiplicative Richardson-Lucy deconvolution with circular borders.
///
/// Observed samples and PSF samples must be non-negative. The PSF is normalized
/// by its positive total mass internally, so its input coefficients need not
/// already sum to one. Scratch and output are owned by the caller.
pub fn richardson_lucy(
    observed: Plane<'_, f32>,
    psf: Plane<'_, f32>,
    output: PlaneMut<'_, f32>,
    scratch: &mut RichardsonLucyScratch,
    config: RichardsonLucyConfig,
) -> Result<(), DeconvolveError> {
    ensure_iterative_geometry(
        observed.extent(),
        psf.extent(),
        output.extent(),
        scratch.extent,
    )?;
    ensure_finite(observed)?;
    if observed.rows().flatten().any(|sample| *sample < 0.0) {
        return Err(DeconvolveError::NegativeObservation);
    }
    let mass = validate_psf(psf)?;
    copy_plane(observed, &mut scratch.estimate)?;
    for value in &mut scratch.estimate {
        *value = value.max(config.epsilon);
    }
    let scale = 1.0 / mass;
    for _ in 0..config.iterations {
        circular_convolve(
            &scratch.estimate,
            psf,
            observed.extent(),
            scale,
            &mut scratch.blurred,
        )?;
        let width = observed.extent().width();
        for y in 0..observed.extent().height() {
            let observed_row = observed.row(y).ok_or(DeconvolveError::ExtentMismatch)?;
            for (x, &input) in observed_row.iter().enumerate() {
                let index = y * width + x;
                scratch.ratio[index] = finite_f32(
                    f64::from(input) / f64::from(scratch.blurred[index].max(config.epsilon)),
                )?;
            }
        }
        circular_adjoint(
            &scratch.ratio,
            psf,
            observed.extent(),
            scale,
            &mut scratch.correction,
        )?;
        for (estimate, &correction) in scratch.estimate.iter_mut().zip(&scratch.correction) {
            *estimate = finite_f32(f64::from(*estimate) * f64::from(correction))?;
        }
    }
    copy_slice_to_plane(&scratch.estimate, output)
}

/// Applies edge-aware gradient regularized circular deconvolution.
///
/// The observed plane supplies the fixed edge guide. A four-neighbour weighted
/// Laplacian is added to the PSF data gradient, where weights are
/// `1 / (edge_epsilon + guide difference)`. This is a reference method with no
/// convergence guarantee; callers must choose a stable `data_step`.
pub fn regularized(
    observed: Plane<'_, f32>,
    psf: Plane<'_, f32>,
    output: PlaneMut<'_, f32>,
    scratch: &mut RegularizedScratch,
    config: RegularizedConfig,
) -> Result<(), DeconvolveError> {
    ensure_iterative_geometry(
        observed.extent(),
        psf.extent(),
        output.extent(),
        scratch.extent,
    )?;
    ensure_finite(observed)?;
    let mass = validate_psf(psf)?;
    copy_plane(observed, &mut scratch.estimate)?;
    let extent = observed.extent();
    let scale = 1.0 / mass;
    for _ in 0..config.iterations {
        circular_convolve(&scratch.estimate, psf, extent, scale, &mut scratch.blurred)?;
        let width = extent.width();
        for y in 0..extent.height() {
            let observed_row = observed.row(y).ok_or(DeconvolveError::ExtentMismatch)?;
            for (x, &input) in observed_row.iter().enumerate() {
                let index = y * width + x;
                scratch.residual[index] =
                    finite_f32(f64::from(scratch.blurred[index]) - f64::from(input))?;
            }
        }
        circular_adjoint(
            &scratch.residual,
            psf,
            extent,
            scale,
            &mut scratch.backprojection,
        )?;
        update_regularized(
            &scratch.estimate,
            &scratch.backprojection,
            observed,
            extent,
            &mut scratch.next,
            config,
        )?;
        core::mem::swap(&mut scratch.estimate, &mut scratch.next);
    }
    copy_slice_to_plane(&scratch.estimate, output)
}

/// Estimates a normalized non-negative circular impulse PSF from two planes.
///
/// This constrained reference supports only a single shifted impulse. It finds
/// the shift whose circularly shifted `reference` minimizes full-frame squared
/// error to `observed`, writes zero elsewhere, and writes `1.0` at the selected
/// PSF coordinate. Cost is `O(width * height * (2 * max_shift + 1)^2)`.
pub fn estimate_psf(
    observed: Plane<'_, f32>,
    reference: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    config: PsfEstimateConfig,
) -> Result<(), DeconvolveError> {
    ensure_same_extent(observed.extent(), reference.extent())?;
    ensure_same_extent(observed.extent(), output.extent())?;
    ensure_finite(observed)?;
    ensure_finite(reference)?;
    let radius =
        isize::try_from(config.max_shift).map_err(|_| DeconvolveError::CoordinatesTooLarge)?;
    ensure_coordinate_range(observed.extent())?;
    let extent = observed.extent();
    let horizontal_radius = radius.min(
        isize::try_from(extent.width() - 1).map_err(|_| DeconvolveError::CoordinatesTooLarge)?,
    );
    let vertical_radius = radius.min(
        isize::try_from(extent.height() - 1).map_err(|_| DeconvolveError::CoordinatesTooLarge)?,
    );
    let mut best_cost = f64::INFINITY;
    let mut best_x = 0_isize;
    let mut best_y = 0_isize;
    for shift_y in -vertical_radius..=vertical_radius {
        for shift_x in -horizontal_radius..=horizontal_radius {
            let cost = shifted_difference_cost(observed, reference, shift_x, shift_y)?;
            if is_better_shift(cost, shift_x, shift_y, best_cost, best_x, best_y) {
                best_cost = cost;
                best_x = shift_x;
                best_y = shift_y;
            }
        }
    }
    for row in output.rows_mut() {
        row.fill(0.0);
    }
    let output_x = best_x.rem_euclid(extent.width() as isize) as usize;
    let output_y = best_y.rem_euclid(extent.height() as isize) as usize;
    let row = output
        .row_mut(output_y)
        .ok_or(DeconvolveError::ExtentMismatch)?;
    row[output_x] = 1.0;
    Ok(())
}

fn area_for_scratch(extent: Extent) -> Result<usize, DeconvolveError> {
    extent.area().ok_or(DeconvolveError::ScratchSizeOverflow)
}

fn zeroed_f32(length: usize) -> Result<Vec<f32>, DeconvolveError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| DeconvolveError::AllocationFailure)?;
    values.resize(length, 0.0);
    Ok(values)
}

fn zeroed_complex(length: usize) -> Result<Vec<Complex>, DeconvolveError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| DeconvolveError::AllocationFailure)?;
    values.resize(length, Complex::default());
    Ok(values)
}

fn ensure_wiener_geometry(
    observed: Extent,
    psf: Extent,
    output: Extent,
    scratch: Extent,
) -> Result<(), DeconvolveError> {
    ensure_same_extent(observed, psf)?;
    ensure_same_extent(observed, output)?;
    if observed == scratch {
        Ok(())
    } else {
        Err(DeconvolveError::ScratchExtentMismatch)
    }
}

fn ensure_iterative_geometry(
    observed: Extent,
    psf: Extent,
    output: Extent,
    scratch: Extent,
) -> Result<(), DeconvolveError> {
    ensure_wiener_geometry(observed, psf, output, scratch)
}

fn ensure_same_extent(left: Extent, right: Extent) -> Result<(), DeconvolveError> {
    if left == right {
        Ok(())
    } else {
        Err(DeconvolveError::ExtentMismatch)
    }
}

fn ensure_finite(plane: Plane<'_, f32>) -> Result<(), DeconvolveError> {
    if plane.rows().flatten().all(|sample| sample.is_finite()) {
        Ok(())
    } else {
        Err(DeconvolveError::NonFiniteSample)
    }
}

fn validate_psf(psf: Plane<'_, f32>) -> Result<f64, DeconvolveError> {
    let mut mass = 0.0_f64;
    for &sample in psf.rows().flatten() {
        if !sample.is_finite() || sample < 0.0 {
            return Err(DeconvolveError::InvalidPsf);
        }
        mass += f64::from(sample);
    }
    if mass <= 0.0 || !mass.is_finite() {
        Err(DeconvolveError::InvalidPsf)
    } else {
        Ok(mass)
    }
}

fn dft(input: Plane<'_, f32>, output: &mut [Complex]) -> Result<(), DeconvolveError> {
    let extent = input.extent();
    let area = area_for_scratch(extent)?;
    if output.len() != area {
        return Err(DeconvolveError::ScratchExtentMismatch);
    }
    let width = extent.width();
    let height = extent.height();
    for frequency_y in 0..height {
        for frequency_x in 0..width {
            let mut sum = Complex::default();
            for y in 0..height {
                let row = input.row(y).ok_or(DeconvolveError::ExtentMismatch)?;
                for (x, &sample) in row.iter().enumerate() {
                    let phase = -core::f64::consts::TAU
                        * (frequency_x as f64 * x as f64 / width as f64
                            + frequency_y as f64 * y as f64 / height as f64);
                    let sample = f64::from(sample);
                    sum.real += sample * phase.cos();
                    sum.imaginary += sample * phase.sin();
                }
            }
            output[frequency_y * width + frequency_x] = sum;
        }
    }
    Ok(())
}

fn inverse_dft(input: &[Complex], mut output: PlaneMut<'_, f32>) -> Result<(), DeconvolveError> {
    let extent = output.extent();
    let area = area_for_scratch(extent)?;
    if input.len() != area {
        return Err(DeconvolveError::ScratchExtentMismatch);
    }
    let width = extent.width();
    let height = extent.height();
    let scale = 1.0 / area as f64;
    for (y, row) in output.rows_mut().enumerate() {
        for (x, destination) in row.iter_mut().enumerate() {
            let mut real = 0.0_f64;
            for frequency_y in 0..height {
                for frequency_x in 0..width {
                    let phase = core::f64::consts::TAU
                        * (frequency_x as f64 * x as f64 / width as f64
                            + frequency_y as f64 * y as f64 / height as f64);
                    let frequency = input[frequency_y * width + frequency_x];
                    real += frequency.real * phase.cos() - frequency.imaginary * phase.sin();
                }
            }
            *destination = finite_f32(real * scale)?;
        }
    }
    Ok(())
}

fn copy_plane(input: Plane<'_, f32>, output: &mut [f32]) -> Result<(), DeconvolveError> {
    let area = area_for_scratch(input.extent())?;
    if output.len() != area {
        return Err(DeconvolveError::ScratchExtentMismatch);
    }
    let width = input.extent().width();
    for (y, row) in input.rows().enumerate() {
        output[y * width..(y + 1) * width].copy_from_slice(row);
    }
    Ok(())
}

fn copy_slice_to_plane(
    input: &[f32],
    mut output: PlaneMut<'_, f32>,
) -> Result<(), DeconvolveError> {
    let area = area_for_scratch(output.extent())?;
    if input.len() != area {
        return Err(DeconvolveError::ScratchExtentMismatch);
    }
    let width = output.extent().width();
    for (y, row) in output.rows_mut().enumerate() {
        row.copy_from_slice(&input[y * width..(y + 1) * width]);
    }
    Ok(())
}

fn circular_convolve(
    input: &[f32],
    psf: Plane<'_, f32>,
    extent: Extent,
    scale: f64,
    output: &mut [f32],
) -> Result<(), DeconvolveError> {
    let area = area_for_scratch(extent)?;
    if input.len() != area || output.len() != area {
        return Err(DeconvolveError::ScratchExtentMismatch);
    }
    let width = extent.width();
    let height = extent.height();
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0.0_f64;
            for psf_y in 0..height {
                let psf_row = psf.row(psf_y).ok_or(DeconvolveError::ExtentMismatch)?;
                let source_y = modular_subtract(y, psf_y, height);
                for (psf_x, &coefficient) in psf_row.iter().enumerate() {
                    let source_x = modular_subtract(x, psf_x, width);
                    sum += f64::from(input[source_y * width + source_x]) * f64::from(coefficient);
                }
            }
            output[y * width + x] = finite_f32(sum * scale)?;
        }
    }
    Ok(())
}

fn circular_adjoint(
    input: &[f32],
    psf: Plane<'_, f32>,
    extent: Extent,
    scale: f64,
    output: &mut [f32],
) -> Result<(), DeconvolveError> {
    let area = area_for_scratch(extent)?;
    if input.len() != area || output.len() != area {
        return Err(DeconvolveError::ScratchExtentMismatch);
    }
    let width = extent.width();
    let height = extent.height();
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0.0_f64;
            for psf_y in 0..height {
                let psf_row = psf.row(psf_y).ok_or(DeconvolveError::ExtentMismatch)?;
                let source_y = modular_add(y, psf_y, height);
                for (psf_x, &coefficient) in psf_row.iter().enumerate() {
                    let source_x = modular_add(x, psf_x, width);
                    sum += f64::from(input[source_y * width + source_x]) * f64::from(coefficient);
                }
            }
            output[y * width + x] = finite_f32(sum * scale)?;
        }
    }
    Ok(())
}

fn update_regularized(
    estimate: &[f32],
    backprojection: &[f32],
    guide: Plane<'_, f32>,
    extent: Extent,
    next: &mut [f32],
    config: RegularizedConfig,
) -> Result<(), DeconvolveError> {
    let area = area_for_scratch(extent)?;
    if estimate.len() != area || backprojection.len() != area || next.len() != area {
        return Err(DeconvolveError::ScratchExtentMismatch);
    }
    let width = extent.width();
    let height = extent.height();
    for y in 0..height {
        let guide_row = guide.row(y).ok_or(DeconvolveError::ExtentMismatch)?;
        for x in 0..width {
            let index = y * width + x;
            let center = estimate[index];
            let mut smooth_gradient = 0.0_f64;
            if x > 0 {
                smooth_gradient += edge_gradient(
                    center,
                    estimate[index - 1],
                    guide_row[x],
                    guide_row[x - 1],
                    config.edge_epsilon,
                );
            }
            if x + 1 < width {
                smooth_gradient += edge_gradient(
                    center,
                    estimate[index + 1],
                    guide_row[x],
                    guide_row[x + 1],
                    config.edge_epsilon,
                );
            }
            if y > 0 {
                let above = guide.row(y - 1).ok_or(DeconvolveError::ExtentMismatch)?;
                smooth_gradient += edge_gradient(
                    center,
                    estimate[index - width],
                    guide_row[x],
                    above[x],
                    config.edge_epsilon,
                );
            }
            if y + 1 < height {
                let below = guide.row(y + 1).ok_or(DeconvolveError::ExtentMismatch)?;
                smooth_gradient += edge_gradient(
                    center,
                    estimate[index + width],
                    guide_row[x],
                    below[x],
                    config.edge_epsilon,
                );
            }
            let gradient = f64::from(backprojection[index])
                + f64::from(config.regularization) * smooth_gradient;
            next[index] = finite_f32(f64::from(center) - f64::from(config.data_step) * gradient)?;
        }
    }
    Ok(())
}

fn edge_gradient(
    center: f32,
    neighbour: f32,
    guide: f32,
    guide_neighbour: f32,
    epsilon: f32,
) -> f64 {
    let guide_difference = (f64::from(guide) - f64::from(guide_neighbour)).abs();
    let weight = 1.0 / (f64::from(epsilon) + guide_difference);
    weight * (f64::from(center) - f64::from(neighbour))
}

fn shifted_difference_cost(
    observed: Plane<'_, f32>,
    reference: Plane<'_, f32>,
    shift_x: isize,
    shift_y: isize,
) -> Result<f64, DeconvolveError> {
    let extent = observed.extent();
    let width = extent.width();
    let height = extent.height();
    let mut cost = 0.0_f64;
    for y in 0..height {
        let observed_row = observed.row(y).ok_or(DeconvolveError::ExtentMismatch)?;
        let reference_y = (y as isize - shift_y).rem_euclid(height as isize) as usize;
        let reference_row = reference
            .row(reference_y)
            .ok_or(DeconvolveError::ExtentMismatch)?;
        for (x, &sample) in observed_row.iter().enumerate() {
            let reference_x = (x as isize - shift_x).rem_euclid(width as isize) as usize;
            let difference = f64::from(sample) - f64::from(reference_row[reference_x]);
            cost += difference * difference;
        }
    }
    let area = width
        .checked_mul(height)
        .ok_or(DeconvolveError::CoordinatesTooLarge)?;
    Ok(cost / area as f64)
}

fn modular_add(left: usize, right: usize, modulus: usize) -> usize {
    let right = right % modulus;
    if left >= modulus - right {
        left - (modulus - right)
    } else {
        left + right
    }
}

fn modular_subtract(left: usize, right: usize, modulus: usize) -> usize {
    let right = right % modulus;
    if left >= right {
        left - right
    } else {
        modulus - (right - left)
    }
}

fn is_better_shift(
    cost: f64,
    x: isize,
    y: isize,
    best_cost: f64,
    best_x: isize,
    best_y: isize,
) -> bool {
    if cost < best_cost {
        return true;
    }
    if cost != best_cost {
        return false;
    }
    let distance = x.saturating_mul(x).saturating_add(y.saturating_mul(y));
    let best_distance = best_x
        .saturating_mul(best_x)
        .saturating_add(best_y.saturating_mul(best_y));
    distance < best_distance
}

fn ensure_coordinate_range(extent: Extent) -> Result<(), DeconvolveError> {
    isize::try_from(extent.width()).map_err(|_| DeconvolveError::CoordinatesTooLarge)?;
    isize::try_from(extent.height()).map_err(|_| DeconvolveError::CoordinatesTooLarge)?;
    Ok(())
}

fn finite_f32(value: f64) -> Result<f32, DeconvolveError> {
    let value = value as f32;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(DeconvolveError::NumericalOverflow)
    }
}

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-deconvolve",
    namespace: "deconvolve",
    summary: "Explicit-PSF and regularized deconvolution",
    filters: &[
        Filter {
            name: "Wiener",
            summary: "Frequency-domain Wiener deconvolution",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "RichardsonLucy",
            summary: "Iterative Richardson-Lucy deconvolution",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Regularized",
            summary: "Edge-aware regularized deconvolution",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "EstimatePsf",
            summary: "Estimate a constrained point-spread function",
            maturity: Maturity::Experimental,
            execution: Execution::Analysis,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::{
        DeconvolveError, Plane, PlaneMut, PsfEstimateConfig, RegularizedConfig, RegularizedScratch,
        RichardsonLucyConfig, RichardsonLucyScratch, WienerConfig, WienerScratch, estimate_psf,
        modular_add, modular_subtract, regularized, richardson_lucy, wiener,
    };
    use vsip_core::Extent;

    fn assert_close(actual: &[f32], expected: &[f32]) {
        for (&actual, &expected) in actual.iter().zip(expected) {
            assert!((actual - expected).abs() < 1.0e-4, "{actual} != {expected}");
        }
    }

    #[test]
    fn identity_psf_preserves_odd_padded_wiener_input() {
        let extent = Extent::new(3, 1).expect("valid extent");
        let observed = [1.0, 2.0, 3.0, 99.0];
        let psf = [2.0, 0.0, 0.0, 88.0];
        let mut output = [-1.0; 4];
        let mut scratch = WienerScratch::try_new(extent).expect("scratch");
        wiener(
            Plane::new(&observed, extent, 4).expect("observed"),
            Plane::new(&psf, extent, 4).expect("psf"),
            PlaneMut::new(&mut output, extent, 4).expect("output"),
            &mut scratch,
            WienerConfig::new(0.0).expect("config"),
        )
        .expect("wiener");
        assert_close(&output[..3], &observed[..3]);
        assert_eq!(output[3], -1.0);
    }

    #[test]
    fn iterative_identity_psf_references_do_not_allocate_per_transform() {
        let extent = Extent::new(3, 1).expect("valid extent");
        let observed = [1.0, 2.0, 3.0];
        let psf = [1.0, 0.0, 0.0];
        let mut lucy_output = [0.0; 3];
        let mut lucy_scratch = RichardsonLucyScratch::try_new(extent).expect("scratch");
        richardson_lucy(
            Plane::new(&observed, extent, 3).expect("observed"),
            Plane::new(&psf, extent, 3).expect("psf"),
            PlaneMut::new(&mut lucy_output, extent, 3).expect("output"),
            &mut lucy_scratch,
            RichardsonLucyConfig::new(2, 1.0e-6).expect("config"),
        )
        .expect("lucy");
        assert_close(&lucy_output, &observed);

        let mut regularized_output = [0.0; 3];
        let mut regularized_scratch = RegularizedScratch::try_new(extent).expect("scratch");
        regularized(
            Plane::new(&observed, extent, 3).expect("observed"),
            Plane::new(&psf, extent, 3).expect("psf"),
            PlaneMut::new(&mut regularized_output, extent, 3).expect("output"),
            &mut regularized_scratch,
            RegularizedConfig::new(2, 0.25, 0.0, 1.0e-3).expect("config"),
        )
        .expect("regularized");
        assert_close(&regularized_output, &observed);
    }

    #[test]
    fn constrained_psf_estimate_selects_a_circular_impulse() {
        let extent = Extent::new(3, 1).expect("valid extent");
        let reference = [1.0, 0.0, 0.0];
        let observed = [0.0, 1.0, 0.0];
        let mut psf = [-1.0; 3];
        estimate_psf(
            Plane::new(&observed, extent, 3).expect("observed"),
            Plane::new(&reference, extent, 3).expect("reference"),
            PlaneMut::new(&mut psf, extent, 3).expect("output"),
            PsfEstimateConfig::new(1).expect("config"),
        )
        .expect("estimate psf");
        assert_eq!(psf, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn rejects_invalid_configuration_geometry_and_nan() {
        assert!(matches!(
            WienerConfig::new(-1.0),
            Err(DeconvolveError::InvalidConfiguration)
        ));
        assert!(matches!(
            RichardsonLucyConfig::new(0, 1.0),
            Err(DeconvolveError::InvalidConfiguration)
        ));
        assert!(matches!(
            PsfEstimateConfig::new(usize::MAX),
            Err(DeconvolveError::CoordinatesTooLarge)
        ));
        let extent = Extent::new(1, 1).expect("valid extent");
        let mut output = [0.0];
        let mut scratch = WienerScratch::try_new(extent).expect("scratch");
        assert!(matches!(
            wiener(
                Plane::new(&[f32::NAN], extent, 1).expect("observed"),
                Plane::new(&[1.0], extent, 1).expect("psf"),
                PlaneMut::new(&mut output, extent, 1).expect("output"),
                &mut scratch,
                WienerConfig::new(0.0).expect("config"),
            ),
            Err(DeconvolveError::NonFiniteSample)
        ));
        let wrong_extent = Extent::new(2, 1).expect("wrong extent");
        let mut wrong_output = [0.0; 2];
        assert!(matches!(
            wiener(
                Plane::new(&[1.0], extent, 1).expect("observed"),
                Plane::new(&[1.0], extent, 1).expect("psf"),
                PlaneMut::new(&mut wrong_output, wrong_extent, 2).expect("output"),
                &mut scratch,
                WienerConfig::new(0.0).expect("config"),
            ),
            Err(DeconvolveError::ExtentMismatch)
        ));
    }

    #[test]
    fn modular_coordinates_do_not_overflow_at_usize_boundaries() {
        let modulus = usize::MAX;
        assert_eq!(modular_add(modulus - 1, modulus - 1, modulus), modulus - 2);
        assert_eq!(modular_subtract(0, modulus - 1, modulus), 1);
    }
}
