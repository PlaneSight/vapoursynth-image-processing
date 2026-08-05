//! VapourSynth registration for GrainLab scalar reference filters.
//!
//! `Analyze` writes residual mean, residual variance, and temporal-difference
//! variance to the red, green, and blue planes of an RGB f32 clip.

#[macro_use]
extern crate vapoursynth;

use vapoursynth::{
    anyhow::Error,
    core::CoreRef,
    format::{ColorFamily, SampleType},
    frame::{FrameRef, FrameRefMut},
    node::Node,
    plugins::{Filter, FilterArgument, Metadata},
    prelude::API,
};
use vsip_core::{Extent, Plane, PlaneMut};
use vsip_vapoursynth::{
    BinaryFilter, BinaryFrameProcessor, FrameBuffers, ScratchPool, UnaryFilter, UnaryFrameProcessor,
};

const PLUGIN_IDENTIFIER: &str = "com.planesight.vsip.grainlab";
const RGB_WEIGHTS: [f32; 3] = [0.2126, 0.7152, 0.0722];

#[derive(Debug)]
struct Buffers {
    frames: FrameBuffers,
    floats: ScratchPool<f32>,
}

impl Buffers {
    fn new() -> Self {
        Self {
            frames: FrameBuffers::new(16).expect("positive retention"),
            floats: ScratchPool::new(16).expect("positive retention"),
        }
    }
}

fn kernel_error(error: impl core::fmt::Display) -> Error {
    Error::msg(error.to_string())
}

fn require_supported(clip: &Node<'_>) -> Result<(), Error> {
    let format = clip.info().format;
    match (format.sample_type(), format.bytes_per_sample()) {
        (SampleType::Integer, 1 | 2) | (SampleType::Float, 4) => Ok(()),
        _ => Err(Error::msg(
            "GrainLab adapters support only u8, u16, and f32 clips",
        )),
    }
}

fn require_analysis_format(clip: &Node<'_>) -> Result<(), Error> {
    let format = clip.info().format;
    if format.color_family() == ColorFamily::RGB
        && format.plane_count() == 3
        && format.sample_type() == SampleType::Float
        && format.bytes_per_sample() == 4
    {
        Ok(())
    } else {
        Err(Error::msg(
            "GrainLab Analyze requires planar RGB 32-bit floating-point clips",
        ))
    }
}

fn area(frame: &FrameRef<'_>, plane: usize) -> Result<usize, Error> {
    Ok(FrameBuffers::plane_area(frame, plane)?)
}

fn read(
    buffers: &Buffers,
    frame: &FrameRef<'_>,
    plane: usize,
    output: &mut [f32],
) -> Result<Extent, Error> {
    Ok(buffers.frames.read_f32(frame, plane, output)?)
}

fn write(
    buffers: &Buffers,
    input: &[f32],
    frame: &mut FrameRefMut<'_>,
    plane: usize,
) -> Result<(), Error> {
    buffers.frames.write_f32(input, frame, plane)?;
    Ok(())
}

fn read_luma(buffers: &Buffers, frame: &FrameRef<'_>, output: &mut [f32]) -> Result<Extent, Error> {
    output.fill(0.0);
    let mut expected_extent = None;
    for (plane, weight) in RGB_WEIGHTS.into_iter().enumerate() {
        let mut component = buffers.floats.acquire(output.len(), 0.0);
        let extent = read(buffers, frame, plane, &mut component)?;
        if expected_extent.is_some_and(|expected| expected != extent) {
            return Err(Error::msg("RGB plane extents differ"));
        }
        expected_extent = Some(extent);
        for (luma, &sample) in output.iter_mut().zip(component.iter()) {
            *luma += sample * weight;
        }
    }
    expected_extent.ok_or_else(|| Error::msg("RGB clip has no planes"))
}

#[derive(Debug)]
struct AnalyzeProcessor {
    buffers: Buffers,
    config: vs_grainlab::AnalysisConfig,
}

impl BinaryFrameProcessor for AnalyzeProcessor {
    fn process(
        &self,
        current: &FrameRef<'_>,
        previous: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let length = area(current, 0)?;
        let mut current_values = self.buffers.floats.acquire(length, 0.0);
        let extent = read_luma(&self.buffers, current, &mut current_values)?;
        let mut previous_values = self.buffers.floats.acquire(length, 0.0);
        if read_luma(&self.buffers, previous, &mut previous_values)? != extent {
            return Err(Error::msg("current and previous extents differ"));
        }
        let profile = vs_grainlab::analyze(
            Plane::new(&current_values, extent, extent.width())?,
            Some(Plane::new(&previous_values, extent, extent.width())?),
            self.config,
        )
        .map_err(kernel_error)?;
        let values = [
            profile.residual_mean(),
            profile.residual_variance(),
            profile.temporal_variance().unwrap_or(0.0),
        ];
        let mut output = self.buffers.floats.acquire(length, 0.0);
        for (plane, value) in values.into_iter().enumerate() {
            output.fill(value);
            write(&self.buffers, &output, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct SynthesizeProcessor {
    buffers: Buffers,
    profile: vs_grainlab::GrainProfile,
    seed: u64,
}

impl UnaryFrameProcessor for SynthesizeProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let extent = Extent::new(source.width(plane), source.height(plane))?;
            let mut output = self.buffers.floats.acquire(length, 0.0);
            vs_grainlab::synthesize(
                self.profile,
                self.seed ^ plane as u64,
                PlaneMut::new(&mut output, extent, extent.width())?,
            )
            .map_err(kernel_error)?;
            write(&self.buffers, &output, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct MatchProcessor {
    buffers: Buffers,
    profile: vs_grainlab::GrainProfile,
    seed: u64,
}

impl UnaryFrameProcessor for MatchProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut input = self.buffers.floats.acquire(length, 0.0);
            let extent = read(&self.buffers, source, plane, &mut input)?;
            let mut output = self.buffers.floats.acquire(length, 0.0);
            vs_grainlab::match_grain(
                Plane::new(&input, extent, extent.width())?,
                self.profile,
                self.seed ^ plane as u64,
                PlaneMut::new(&mut output, extent, extent.width())?,
            )
            .map_err(kernel_error)?;
            write(&self.buffers, &output, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ResidualProcessor {
    buffers: Buffers,
}

impl BinaryFrameProcessor for ResidualProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        reference: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut input = self.buffers.floats.acquire(length, 0.0);
            let extent = read(&self.buffers, source, plane, &mut input)?;
            let mut clean = self.buffers.floats.acquire(length, 0.0);
            if read(&self.buffers, reference, plane, &mut clean)? != extent {
                return Err(Error::msg("source and reference plane extents differ"));
            }
            let mut output = self.buffers.floats.acquire(length, 0.0);
            vs_grainlab::residual(
                Plane::new(&input, extent, extent.width())?,
                Plane::new(&clean, extent, extent.width())?,
                PlaneMut::new(&mut output, extent, extent.width())?,
            )
            .map_err(kernel_error)?;
            write(&self.buffers, &output, destination, plane)?;
        }
        Ok(())
    }
}

fn analysis_config(radius: i64) -> Result<vs_grainlab::AnalysisConfig, Error> {
    let radius =
        usize::try_from(radius).map_err(|_| Error::msg("local_radius must be positive"))?;
    if radius == 0 {
        return Err(Error::msg("local_radius must be positive"));
    }
    vs_grainlab::AnalysisConfig::new(radius).map_err(kernel_error)
}

fn profile(mean: f64, variance: f64) -> Result<vs_grainlab::GrainProfile, Error> {
    vs_grainlab::GrainProfile::new(mean as f32, variance as f32, None).map_err(kernel_error)
}

fn seed(seed: i64) -> Result<u64, Error> {
    u64::try_from(seed).map_err(|_| Error::msg("seed must be non-negative"))
}

make_filter_function! { AnalyzeFunction, "Analyze" fn create_analyze<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, previous: Node<'core>, local_radius: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> { require_analysis_format(&clip)?; require_analysis_format(&previous)?; Ok(Some(Box::new(BinaryFilter::new(clip, previous, AnalyzeProcessor { buffers: Buffers::new(), config: analysis_config(local_radius)? }).map_err(kernel_error)?))) } }
make_filter_function! { SynthesizeFunction, "Synthesize" fn create_synthesize<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, mean: f64, variance: f64, random_seed: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> { require_supported(&clip)?; Ok(Some(Box::new(UnaryFilter::new(clip, SynthesizeProcessor { buffers: Buffers::new(), profile: profile(mean, variance)?, seed: seed(random_seed)? })))) } }
make_filter_function! { MatchFunction, "Match" fn create_match<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, mean: f64, variance: f64, random_seed: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> { require_supported(&clip)?; Ok(Some(Box::new(UnaryFilter::new(clip, MatchProcessor { buffers: Buffers::new(), profile: profile(mean, variance)?, seed: seed(random_seed)? })))) } }
make_filter_function! { ResidualFunction, "Residual" fn create_residual<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, reference: Node<'core>) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> { require_supported(&clip)?; require_supported(&reference)?; Ok(Some(Box::new(BinaryFilter::new(clip, reference, ResidualProcessor { buffers: Buffers::new() }).map_err(kernel_error)?))) } }

mod plugin_abi {
    #![allow(missing_docs)]

    use super::*;

    export_vapoursynth_plugin! { Metadata { identifier: PLUGIN_IDENTIFIER, namespace: "grainlab", name: "PlaneSight GrainLab", read_only: true }, [AnalyzeFunction::new(), SynthesizeFunction::new(), MatchFunction::new(), ResidualFunction::new()] }
}
