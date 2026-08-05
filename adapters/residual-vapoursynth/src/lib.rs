//! VapourSynth runtime registration for Residual reference filters.

#![allow(missing_docs)] // The upstream export macro generates the ABI entry point.

#[macro_use]
extern crate vapoursynth;

use vapoursynth::{
    anyhow::Error,
    core::CoreRef,
    format::{Format, SampleType},
    frame::{FrameRef, FrameRefMut},
    node::Node,
    plugins::{Filter, FilterArgument, Metadata},
    prelude::API,
};
use vsip_core::{Plane, PlaneMut};
use vsip_vapoursynth::{
    BinaryFilter, BinaryFrameProcessor, FrameBuffers, TemporalFilter, TemporalFrameProcessor,
    UnaryFilter, UnaryFrameProcessor,
};

const PLUGIN_IDENTIFIER: &str = "com.planesight.vsip.residual";

#[derive(Debug)]
struct Buffers {
    frames: FrameBuffers,
}

impl Buffers {
    fn new() -> Self {
        Self {
            frames: FrameBuffers::new(32).expect("positive scratch retention"),
        }
    }
}

fn area(frame: &FrameRef<'_>, plane: usize) -> Result<usize, Error> {
    FrameBuffers::plane_area(frame, plane).map_err(Into::into)
}

fn write_f32(
    buffers: &Buffers,
    input: &[f32],
    frame: &mut FrameRefMut<'_>,
    plane: usize,
) -> Result<(), Error> {
    buffers
        .frames
        .write_f32(input, frame, plane)
        .map(|_| ())
        .map_err(Into::into)
}

fn require_f32_format(format: Format<'_>) -> Result<(), Error> {
    if format.sample_type() == SampleType::Float && format.bytes_per_sample() == 4 {
        return Ok(());
    }
    Err(Error::msg(
        "Residual filters require single-precision f32 clips so signed and fractional results are preserved",
    ))
}

fn non_negative_f32(value: f64, name: &str) -> Result<f32, Error> {
    if !value.is_finite() || value < 0.0 || value > f64::from(f32::MAX) {
        return Err(Error::msg(format!(
            "{name} must be finite, non-negative, and representable as f32"
        )));
    }
    let narrowed = value as f32;
    if value != 0.0 && narrowed == 0.0 {
        return Err(Error::msg(format!("{name} underflows f32")));
    }
    Ok(narrowed)
}

fn triplet_indices(frame_count: usize, center: usize) -> Option<[usize; 3]> {
    let previous = center.checked_sub(1)?;
    let next = center.checked_add(1)?;
    (next < frame_count).then_some([previous, center, next])
}

fn triplet<'frames, 'core>(
    frames: &'frames [FrameRef<'core>],
    center: usize,
) -> Option<(
    &'frames FrameRef<'core>,
    &'frames FrameRef<'core>,
    &'frames FrameRef<'core>,
)> {
    let [previous, current, next] = triplet_indices(frames.len(), center)?;
    Some((&frames[previous], &frames[current], &frames[next]))
}

#[derive(Clone, Copy, Debug)]
enum DecomposeOutput {
    Structure,
    Texture,
    Residual,
}

#[derive(Debug)]
struct DecomposeProcessor {
    buffers: Buffers,
    config: vs_residual::DecomposeConfig,
    output: DecomposeOutput,
}

impl UnaryFrameProcessor for DecomposeProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut input = self.buffers.frames.floats(length);
            let extent = self.buffers.frames.read_f32(source, plane, &mut input)?;
            let mut structure = self.buffers.frames.floats(length);
            let mut texture = self.buffers.frames.floats(length);
            let mut residual = self.buffers.frames.floats(length);
            vs_residual::decompose(
                Plane::new(&input, extent, extent.width())?,
                PlaneMut::new(&mut structure, extent, extent.width())?,
                PlaneMut::new(&mut texture, extent, extent.width())?,
                PlaneMut::new(&mut residual, extent, extent.width())?,
                self.config,
            )?;
            let selected: &[f32] = match self.output {
                DecomposeOutput::Structure => &structure,
                DecomposeOutput::Texture => &texture,
                DecomposeOutput::Residual => &residual,
            };
            write_f32(&self.buffers, selected, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct TemporalProcessor {
    buffers: Buffers,
    config: vs_residual::TemporalResidualConfig,
}

impl TemporalFrameProcessor for TemporalProcessor {
    fn process(
        &self,
        frames: &[FrameRef<'_>],
        center: usize,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let current = frames
            .get(center)
            .ok_or_else(|| Error::msg("temporal window has no center frame"))?;
        let window = triplet(frames, center);
        for plane in 0..current.format().plane_count() {
            let length = area(current, plane)?;
            // The shared scheduler shortens endpoint windows. A residual has no
            // two-sided baseline there, so the defined endpoint output is zero.
            let mut output = self.buffers.frames.floats(length);
            if let Some((previous, current, next)) = window {
                let mut before = self.buffers.frames.floats(length);
                let extent = self.buffers.frames.read_f32(previous, plane, &mut before)?;
                let mut sample = self.buffers.frames.floats(length);
                let current_extent = self.buffers.frames.read_f32(current, plane, &mut sample)?;
                let mut after = self.buffers.frames.floats(length);
                let next_extent = self.buffers.frames.read_f32(next, plane, &mut after)?;
                if extent != current_extent || extent != next_extent {
                    return Err(Error::msg("temporal frame plane extents differ"));
                }
                vs_residual::temporal(
                    Plane::new(&before, extent, extent.width())?,
                    Plane::new(&sample, extent, extent.width())?,
                    Plane::new(&after, extent, extent.width())?,
                    PlaneMut::new(&mut output, extent, extent.width())?,
                    self.config,
                )?;
            }
            write_f32(&self.buffers, &output, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct SpectrumProcessor {
    buffers: Buffers,
    config: vs_residual::SpectrumConfig,
}

impl UnaryFrameProcessor for SpectrumProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut input = self.buffers.frames.floats(length);
            let extent = self.buffers.frames.read_f32(source, plane, &mut input)?;
            let result =
                vs_residual::spectrum(Plane::new(&input, extent, extent.width())?, self.config)?;
            write_f32(&self.buffers, &result.power, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct CompareProcessor {
    buffers: Buffers,
    config: vs_residual::CompareConfig,
}

impl BinaryFrameProcessor for CompareProcessor {
    fn process(
        &self,
        left: &FrameRef<'_>,
        right: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..left.format().plane_count() {
            let length = area(left, plane)?;
            let mut first = self.buffers.frames.floats(length);
            let extent = self.buffers.frames.read_f32(left, plane, &mut first)?;
            let mut second = self.buffers.frames.floats(length);
            let other_extent = self.buffers.frames.read_f32(right, plane, &mut second)?;
            if extent != other_extent {
                return Err(Error::msg("residual field plane extents differ"));
            }
            let mut output = self.buffers.frames.floats(length);
            vs_residual::compare(
                Plane::new(&first, extent, extent.width())?,
                Plane::new(&second, extent, extent.width())?,
                PlaneMut::new(&mut output, extent, extent.width())?,
                self.config,
            )?;
            write_f32(&self.buffers, &output, destination, plane)?;
        }
        Ok(())
    }
}

fn output_component(value: i64) -> Result<DecomposeOutput, Error> {
    match value {
        0 => Ok(DecomposeOutput::Structure),
        1 => Ok(DecomposeOutput::Texture),
        2 => Ok(DecomposeOutput::Residual),
        _ => Err(Error::msg(
            "output_component must be 0 (structure), 1 (texture), or 2 (residual)",
        )),
    }
}

fn normalization(value: i64) -> Result<vs_residual::SpectrumNormalization, Error> {
    match value {
        0 => Ok(vs_residual::SpectrumNormalization::None),
        1 => Ok(vs_residual::SpectrumNormalization::ByPixels),
        _ => Err(Error::msg(
            "normalization must be 0 (none) or 1 (by-pixels)",
        )),
    }
}

fn metric(value: i64) -> Result<vs_residual::CompareMetric, Error> {
    match value {
        0 => Ok(vs_residual::CompareMetric::Signed),
        1 => Ok(vs_residual::CompareMetric::Absolute),
        2 => Ok(vs_residual::CompareMetric::Squared),
        _ => Err(Error::msg(
            "metric must be 0 (signed), 1 (absolute), or 2 (squared)",
        )),
    }
}

make_filter_function! {
    DecomposeFunction, "Decompose"
    fn create_decompose<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, texture_radius: i64, structure_radius: i64, output_component_value: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_f32_format(clip.info().format)?;
        let texture = usize::try_from(texture_radius).map_err(|_| Error::msg("texture_radius must be non-negative"))?;
        let structure = usize::try_from(structure_radius).map_err(|_| Error::msg("structure_radius must be positive"))?;
        let config = vs_residual::DecomposeConfig::new(texture, structure).map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, DecomposeProcessor { buffers: Buffers::new(), config, output: output_component(output_component_value)? }))))
    }
}

make_filter_function! {
    TemporalFunction, "Temporal"
    fn create_temporal<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, gain: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_f32_format(clip.info().format)?;
        let config = vs_residual::TemporalResidualConfig::new(non_negative_f32(gain, "gain")?).map_err(|error| Error::msg(error.to_string()))?;
        let filter = TemporalFilter::new(clip, 1, TemporalProcessor { buffers: Buffers::new(), config }).map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(filter)))
    }
}

make_filter_function! {
    SpectrumFunction, "Spectrum"
    fn create_spectrum<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, normalization_value: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_f32_format(clip.info().format)?;
        let config = vs_residual::SpectrumConfig::new(normalization(normalization_value)?);
        Ok(Some(Box::new(UnaryFilter::new(clip, SpectrumProcessor { buffers: Buffers::new(), config }))))
    }
}

make_filter_function! {
    CompareFunction, "Compare"
    fn create_compare<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, other: Node<'core>, metric_value: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_f32_format(clip.info().format)?;
        require_f32_format(other.info().format)?;
        if clip.info().framerate != other.info().framerate {
            return Err(Error::msg("residual clip frame rates differ"));
        }
        let filter = BinaryFilter::new(clip, other, CompareProcessor { buffers: Buffers::new(), config: vs_residual::CompareConfig::new(metric(metric_value)?) }).map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(filter)))
    }
}

#[cfg(test)]
mod tests {
    use super::{non_negative_f32, triplet_indices};

    #[test]
    fn temporal_endpoints_have_no_complete_triplet() {
        assert_eq!(triplet_indices(3, 0), None);
        assert_eq!(triplet_indices(3, 1), Some([0, 1, 2]));
        assert_eq!(triplet_indices(3, 2), None);
        assert_eq!(triplet_indices(1, 0), None);
    }

    #[test]
    fn gain_narrowing_rejects_negative_underflow() {
        assert!(non_negative_f32(-f64::MIN_POSITIVE, "gain").is_err());
        assert!(non_negative_f32(f64::MIN_POSITIVE, "gain").is_err());
    }
}

export_vapoursynth_plugin! {
    Metadata { identifier: PLUGIN_IDENTIFIER, namespace: "residual", name: "PlaneSight Residual", read_only: true },
    [DecomposeFunction::new(), TemporalFunction::new(), SpectrumFunction::new(), CompareFunction::new()]
}
