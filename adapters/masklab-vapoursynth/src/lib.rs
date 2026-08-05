//! VapourSynth runtime registration for every MaskLab reference filter.

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
use vsip_core::{Extent, Plane, PlaneMut};
use vsip_vapoursynth::{
    BinaryFilter, BinaryFrameProcessor, FrameBuffers, ScratchPool, UnaryFilter,
    UnaryFrameProcessor, copy_plane_from, copy_plane_to,
};

const PLUGIN_IDENTIFIER: &str = "com.planesight.vsip.masklab";

#[derive(Debug)]
struct Buffers {
    frames: FrameBuffers,
    masks: ScratchPool<u8>,
}

impl Buffers {
    fn new() -> Self {
        Self {
            frames: FrameBuffers::new(16).expect("positive scratch retention"),
            masks: ScratchPool::new(16).expect("positive scratch retention"),
        }
    }
}

fn area(frame: &FrameRef<'_>, plane: usize) -> Result<usize, Error> {
    FrameBuffers::plane_area(frame, plane).map_err(Into::into)
}

fn require_supported_format(format: Format<'_>) -> Result<(), Error> {
    match (format.sample_type(), format.bytes_per_sample()) {
        (SampleType::Integer, 1 | 2) | (SampleType::Float, 4) => Ok(()),
        _ => Err(Error::msg(
            "only u8, u16, and single-precision f32 clips are supported",
        )),
    }
}

fn require_u8_format(format: Format<'_>) -> Result<(), Error> {
    if format.sample_type() == SampleType::Integer && format.bits_per_sample() == 8 {
        return Ok(());
    }
    Err(Error::msg("Reconstruct requires 8-bit integer clips"))
}

fn require_f32_format(format: Format<'_>, operation: &str) -> Result<(), Error> {
    if format.sample_type() == SampleType::Float && format.bytes_per_sample() == 4 {
        return Ok(());
    }
    Err(Error::msg(format!(
        "{operation} requires a single-precision f32 clip to preserve fractional output"
    )))
}

fn exact_integer_maximum(format: Format<'_>) -> Result<u32, Error> {
    match (format.sample_type(), format.bytes_per_sample()) {
        (SampleType::Integer, 1 | 2) => {
            let bits = format.bits_per_sample();
            if bits == 0 || bits > 16 {
                return Err(Error::msg("integer clip bit depth is invalid"));
            }
            Ok((1_u32 << bits) - 1)
        }
        (SampleType::Float, 4) => Ok(1_u32 << f32::MANTISSA_DIGITS),
        _ => Err(Error::msg(
            "integer-valued output requires u8, u16, or single-precision f32 clips",
        )),
    }
}

fn mask_peak(frame: &FrameRef<'_>) -> Result<f32, Error> {
    match (
        frame.format().sample_type(),
        frame.format().bytes_per_sample(),
    ) {
        (SampleType::Integer, 1 | 2) => {
            Ok(((1_u32 << frame.format().bits_per_sample()) - 1) as f32)
        }
        (SampleType::Float, 4) => Ok(1.0),
        _ => Err(Error::msg(
            "only u8, u16, and f32 frame planes are supported",
        )),
    }
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

fn copy_to_mask(
    buffers: &Buffers,
    frame: &FrameRef<'_>,
    plane: usize,
    output: &mut [u8],
) -> Result<Extent, Error> {
    let mut values = buffers.frames.floats(output.len());
    let extent = buffers.frames.read_f32(frame, plane, &mut values)?;
    if values.iter().any(|value| !value.is_finite()) {
        return Err(Error::msg("binary mask planes must contain finite samples"));
    }
    for (destination, &value) in output.iter_mut().zip(values.iter()) {
        *destination = if value == 0.0 { 0 } else { u8::MAX };
    }
    Ok(extent)
}

fn render_exact_integers(
    values: &[u32],
    output: &mut [f32],
    format: Format<'_>,
) -> Result<(), Error> {
    let maximum = exact_integer_maximum(format)?;
    let largest = values.iter().copied().max().unwrap_or(0);
    if largest > maximum {
        return Err(Error::msg(format!(
            "integer-valued output label or distance {largest} exceeds the exact format limit {maximum}"
        )));
    }
    for (destination, &value) in output.iter_mut().zip(values) {
        *destination = value as f32;
    }
    Ok(())
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

fn copy_from_u8(frame: &mut FrameRefMut<'_>, plane: usize, input: &[u8]) -> Result<(), Error> {
    if frame.format().sample_type() != SampleType::Integer || frame.format().bytes_per_sample() != 1
    {
        return Err(Error::msg("this operation requires u8 frame planes"));
    }
    copy_plane_from(input, frame, plane)
        .map(|_| ())
        .map_err(Into::into)
}

fn connectivity(value: i64) -> Result<vs_masklab::Connectivity, Error> {
    match value {
        4 => Ok(vs_masklab::Connectivity::Four),
        8 => Ok(vs_masklab::Connectivity::Eight),
        _ => Err(Error::msg("connectivity must be 4 or 8")),
    }
}

#[derive(Debug)]
struct DistanceL1Processor {
    buffers: Buffers,
}

impl UnaryFrameProcessor for DistanceL1Processor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut mask = self.buffers.masks.acquire(length, 0);
            let extent = copy_to_mask(&self.buffers, source, plane, &mut mask)?;
            let mut distances = self.buffers.frames.integers(length);
            vs_masklab::l1_to_zero(
                Plane::new(&mask, extent, extent.width())?,
                PlaneMut::new(&mut distances, extent, extent.width())?,
            )?;
            let mut rendered = self.buffers.frames.floats(length);
            render_exact_integers(&distances, &mut rendered, source.format())?;
            write_f32(&self.buffers, &rendered, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct EuclideanProcessor {
    buffers: Buffers,
}

impl UnaryFrameProcessor for EuclideanProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        require_f32_format(source.format(), "DistanceEuclidean")?;
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut mask = self.buffers.masks.acquire(length, 0);
            let extent = copy_to_mask(&self.buffers, source, plane, &mut mask)?;
            let mut rendered = self.buffers.frames.floats(length);
            vs_masklab::euclidean_to_zero(
                Plane::new(&mask, extent, extent.width())?,
                PlaneMut::new(&mut rendered, extent, extent.width())?,
            )?;
            write_f32(&self.buffers, &rendered, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ComponentsProcessor {
    buffers: Buffers,
    connectivity: vs_masklab::Connectivity,
}

impl UnaryFrameProcessor for ComponentsProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut mask = self.buffers.masks.acquire(length, 0);
            let extent = copy_to_mask(&self.buffers, source, plane, &mut mask)?;
            let analysis = vs_masklab::components(
                Plane::new(&mask, extent, extent.width())?,
                self.connectivity,
            )?;
            let mut rendered = self.buffers.frames.floats(length);
            render_exact_integers(&analysis.labels, &mut rendered, source.format())?;
            write_f32(&self.buffers, &rendered, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ReconstructProcessor {
    buffers: Buffers,
    connectivity: vs_masklab::Connectivity,
}

impl BinaryFrameProcessor for ReconstructProcessor {
    fn process(
        &self,
        marker: &FrameRef<'_>,
        mask: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..marker.format().plane_count() {
            let length = area(marker, plane)?;
            let mut marker_values = self.buffers.masks.acquire(length, 0);
            let extent = copy_plane_to(marker, plane, &mut marker_values)?;
            let mut mask_values = self.buffers.masks.acquire(length, 0);
            let mask_extent = copy_plane_to(mask, plane, &mut mask_values)?;
            if extent != mask_extent {
                return Err(Error::msg("marker and mask plane extents differ"));
            }
            let mut output = self.buffers.masks.acquire(length, 0);
            vs_masklab::reconstruct(
                Plane::new(&marker_values, extent, extent.width())?,
                Plane::new(&mask_values, extent, extent.width())?,
                PlaneMut::new(&mut output, extent, extent.width())?,
                self.connectivity,
            )?;
            copy_from_u8(destination, plane, &output)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ThinProcessor {
    buffers: Buffers,
    config: vs_masklab::ThinConfig,
}

impl UnaryFrameProcessor for ThinProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut mask = self.buffers.masks.acquire(length, 0);
            let extent = copy_to_mask(&self.buffers, source, plane, &mut mask)?;
            let mut output = self.buffers.masks.acquire(length, 0);
            let mut scratch = self.buffers.masks.acquire(length, 0);
            vs_masklab::thin(
                Plane::new(&mask, extent, extent.width())?,
                PlaneMut::new(&mut output, extent, extent.width())?,
                PlaneMut::new(&mut scratch, extent, extent.width())?,
                self.config,
            )?;
            let mut rendered = self.buffers.frames.floats(length);
            let peak = mask_peak(source)?;
            for (destination, &value) in rendered.iter_mut().zip(output.iter()) {
                *destination = if value == 0 { 0.0 } else { peak };
            }
            write_f32(&self.buffers, &rendered, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct FeatherProcessor {
    buffers: Buffers,
    config: vs_masklab::FeatherConfig,
}

impl UnaryFrameProcessor for FeatherProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut mask = self.buffers.masks.acquire(length, 0);
            let extent = copy_to_mask(&self.buffers, source, plane, &mut mask)?;
            let mut output = self.buffers.frames.floats(length);
            vs_masklab::feather(
                Plane::new(&mask, extent, extent.width())?,
                PlaneMut::new(&mut output, extent, extent.width())?,
                self.config,
            )?;
            let peak = mask_peak(source)?;
            for value in output.iter_mut() {
                *value *= peak;
            }
            write_f32(&self.buffers, &output, destination, plane)?;
        }
        Ok(())
    }
}

make_filter_function! {
    DistanceL1Function, "DistanceL1"
    fn create_distance_l1<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(clip.info().format)?;
        Ok(Some(Box::new(UnaryFilter::new(clip, DistanceL1Processor { buffers: Buffers::new() }))))
    }
}

make_filter_function! {
    DistanceEuclideanFunction, "DistanceEuclidean"
    fn create_distance_euclidean<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_f32_format(clip.info().format, "DistanceEuclidean")?;
        Ok(Some(Box::new(UnaryFilter::new(clip, EuclideanProcessor { buffers: Buffers::new() }))))
    }
}

make_filter_function! {
    ComponentLabelsFunction, "ComponentLabels"
    fn create_component_labels<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, connectivity_value: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(clip.info().format)?;
        Ok(Some(Box::new(UnaryFilter::new(clip, ComponentsProcessor { buffers: Buffers::new(), connectivity: connectivity(connectivity_value)? }))))
    }
}

make_filter_function! {
    ReconstructFunction, "Reconstruct"
    fn create_reconstruct<'core>(_api: API, _core: CoreRef<'core>, marker: Node<'core>, mask: Node<'core>, connectivity_value: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_u8_format(marker.info().format)?;
        require_u8_format(mask.info().format)?;
        if marker.info().framerate != mask.info().framerate {
            return Err(Error::msg("marker and mask clip frame rates differ"));
        }
        let filter = BinaryFilter::new(marker, mask, ReconstructProcessor { buffers: Buffers::new(), connectivity: connectivity(connectivity_value)? })
            .map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(filter)))
    }
}

make_filter_function! {
    ThinFunction, "Thin"
    fn create_thin<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, max_iterations: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(clip.info().format)?;
        let iterations = usize::try_from(max_iterations).map_err(|_| Error::msg("max_iterations must be positive"))?;
        let config = vs_masklab::ThinConfig::new(iterations).map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, ThinProcessor { buffers: Buffers::new(), config }))))
    }
}

make_filter_function! {
    FeatherFunction, "Feather"
    fn create_feather<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, inner_radius: f64, outer_radius: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(clip.info().format)?;
        let config = vs_masklab::FeatherConfig::new(
            non_negative_f32(inner_radius, "inner_radius")?,
            non_negative_f32(outer_radius, "outer_radius")?,
        )
            .map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, FeatherProcessor { buffers: Buffers::new(), config }))))
    }
}

#[cfg(test)]
mod tests {
    use super::{ComponentLabelsFunction, non_negative_f32};
    use vapoursynth::plugins::FilterFunction;

    #[test]
    fn config_narrowing_rejects_negative_underflow() {
        assert!(non_negative_f32(-f64::MIN_POSITIVE, "radius").is_err());
        assert!(non_negative_f32(f64::MIN_POSITIVE, "radius").is_err());
    }

    #[test]
    fn component_label_registration_matches_the_catalogue() {
        assert_eq!(
            ComponentLabelsFunction::new().name(),
            vs_masklab::PLUGIN.filters[2].name
        );
    }
}

export_vapoursynth_plugin! {
    Metadata {
        identifier: PLUGIN_IDENTIFIER,
        namespace: "masklab",
        name: "PlaneSight MaskLab",
        read_only: true,
    },
    [
        DistanceL1Function::new(),
        DistanceEuclideanFunction::new(),
        ComponentLabelsFunction::new(),
        ReconstructFunction::new(),
        ThinFunction::new(),
        FeatherFunction::new(),
    ]
}
