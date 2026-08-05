//! VapourSynth runtime registration for Segment reference filters.

#![allow(missing_docs)] // The upstream export macro generates the ABI entry point.

#[macro_use]
extern crate vapoursynth;

use vapoursynth::{
    anyhow::{Error, anyhow},
    core::CoreRef,
    format::{Format, SampleType},
    frame::{FrameRef, FrameRefMut},
    node::Node,
    plugins::{Filter, FilterArgument, FrameContext, Metadata},
    prelude::API,
    video_info::VideoInfo,
};
use vsip_core::{Plane, PlaneMut};
use vsip_vapoursynth::{FrameBuffers, UnaryFilter, UnaryFrameProcessor};

const PLUGIN_IDENTIFIER: &str = "com.planesight.vsip.segment";

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

fn require_supported_format(format: Format<'_>) -> Result<(), Error> {
    match (format.sample_type(), format.bytes_per_sample()) {
        (SampleType::Integer, 1 | 2) | (SampleType::Float, 4) => Ok(()),
        _ => Err(Error::msg(
            "only u8, u16, and single-precision f32 clips are supported",
        )),
    }
}

fn compatible_pair(first: VideoInfo<'_>, second: VideoInfo<'_>) -> Result<(), Error> {
    if first.resolution != second.resolution {
        return Err(Error::msg("input clip resolutions differ"));
    }
    if first.num_frames != second.num_frames {
        return Err(Error::msg("input clip frame counts differ"));
    }
    if first.framerate != second.framerate {
        return Err(Error::msg("input clip frame rates differ"));
    }
    if first.format.plane_count() != second.format.plane_count()
        || first.format.sub_sampling_w() != second.format.sub_sampling_w()
        || first.format.sub_sampling_h() != second.format.sub_sampling_h()
    {
        return Err(Error::msg("input clips have different plane layouts"));
    }
    Ok(())
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

trait PairFrameProcessor: Send + Sync {
    fn process(
        &self,
        first: &FrameRef<'_>,
        second: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error>;
}

#[derive(Debug)]
struct PairFilter<'core, P> {
    first: Node<'core>,
    second: Node<'core>,
    processor: P,
}

impl<'core, P> PairFilter<'core, P> {
    fn new(first: Node<'core>, second: Node<'core>, processor: P) -> Result<Self, Error> {
        compatible_pair(first.info(), second.info())?;
        Ok(Self {
            first,
            second,
            processor,
        })
    }
}

impl<'core, P> Filter<'core> for PairFilter<'core, P>
where
    P: PairFrameProcessor,
{
    fn video_info(&self, _api: API, _core: CoreRef<'core>) -> Vec<VideoInfo<'core>> {
        vec![self.first.info()]
    }

    fn get_frame_initial(
        &self,
        _api: API,
        _core: CoreRef<'core>,
        context: FrameContext<'_>,
        n: usize,
    ) -> Result<Option<FrameRef<'core>>, Error> {
        self.first.request_frame_filter(context, n);
        self.second.request_frame_filter(context, n);
        Ok(None)
    }

    fn get_frame(
        &self,
        _api: API,
        core: CoreRef<'core>,
        context: FrameContext<'_>,
        n: usize,
    ) -> Result<FrameRef<'core>, Error> {
        let first = self
            .first
            .get_frame_filter(context, n)
            .ok_or_else(|| anyhow!("first input frame {n} was not available"))?;
        let second = self
            .second
            .get_frame_filter(context, n)
            .ok_or_else(|| anyhow!("second input frame {n} was not available"))?;
        let mut destination = FrameRefMut::copy_of(core, &first);
        self.processor.process(&first, &second, &mut destination)?;
        Ok(destination.into())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LabelRepresentation {
    Integer { maximum: u32 },
    Float32,
}

fn label_representation(format: Format<'_>) -> Result<LabelRepresentation, Error> {
    match (format.sample_type(), format.bytes_per_sample()) {
        (SampleType::Integer, 1 | 2) => {
            let bits = format.bits_per_sample();
            if bits == 0 || bits > 16 {
                return Err(Error::msg("integer clip bit depth is invalid"));
            }
            Ok(LabelRepresentation::Integer {
                maximum: (1_u32 << bits) - 1,
            })
        }
        (SampleType::Float, 4) => Ok(LabelRepresentation::Float32),
        _ => Err(Error::msg(
            "label clips must use u8, u16, or single-precision f32 samples",
        )),
    }
}

fn labels_from_f32(input: &[f32], output: &mut [u32]) -> Result<(), Error> {
    if input.len() != output.len() {
        return Err(Error::msg("label conversion buffer lengths differ"));
    }
    for (destination, &value) in output.iter_mut().zip(input) {
        if !value.is_finite()
            || value < 0.0
            || f64::from(value) > f64::from(u32::MAX)
            || value.fract() != 0.0
        {
            return Err(Error::msg(
                "label planes must contain finite non-negative integer values within u32",
            ));
        }
        *destination = value as u32;
    }
    Ok(())
}

fn labels_to_f32(
    input: &[u32],
    output: &mut [f32],
    representation: LabelRepresentation,
) -> Result<(), Error> {
    if input.len() != output.len() {
        return Err(Error::msg("label rendering buffer lengths differ"));
    }
    for (destination, &value) in output.iter_mut().zip(input) {
        let rendered = value as f32;
        let is_exact = match representation {
            LabelRepresentation::Integer { maximum } => value <= maximum,
            LabelRepresentation::Float32 => f64::from(rendered) == f64::from(value),
        };
        if !is_exact {
            return Err(Error::msg(format!(
                "label or analysis value {value} is not exactly representable by the output format"
            )));
        }
        *destination = rendered;
    }
    Ok(())
}

fn connectivity(value: i64) -> Result<vs_segment::Connectivity, Error> {
    match value {
        4 => Ok(vs_segment::Connectivity::Four),
        8 => Ok(vs_segment::Connectivity::Eight),
        _ => Err(Error::msg("connectivity must be 4 or 8")),
    }
}

fn boundary(value: i64) -> Result<vs_segment::WatershedBoundary, Error> {
    match value {
        0 => Ok(vs_segment::WatershedBoundary::Line),
        1 => Ok(vs_segment::WatershedBoundary::LowestAdjacentLabel),
        _ => Err(Error::msg(
            "boundary must be 0 (line) or 1 (lowest-adjacent-label)",
        )),
    }
}

#[derive(Debug)]
struct WatershedProcessor {
    buffers: Buffers,
    config: vs_segment::WatershedConfig,
}

impl PairFrameProcessor for WatershedProcessor {
    fn process(
        &self,
        gradient: &FrameRef<'_>,
        markers: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..gradient.format().plane_count() {
            let length = area(gradient, plane)?;
            let mut gradient_values = self.buffers.frames.floats(length);
            let extent = self
                .buffers
                .frames
                .read_f32(gradient, plane, &mut gradient_values)?;
            let mut marker_samples = self.buffers.frames.floats(length);
            let marker_extent =
                self.buffers
                    .frames
                    .read_f32(markers, plane, &mut marker_samples)?;
            if extent != marker_extent {
                return Err(Error::msg("gradient and marker plane extents differ"));
            }
            let mut marker_values = self.buffers.frames.integers(length);
            labels_from_f32(&marker_samples, &mut marker_values)?;
            let mut output = self.buffers.frames.integers(length);
            let mut workspace = self.buffers.frames.integers(length);
            vs_segment::watershed(
                Plane::new(&gradient_values, extent, extent.width())?,
                Plane::new(&marker_values, extent, extent.width())?,
                PlaneMut::new(&mut output, extent, extent.width())?,
                PlaneMut::new(&mut workspace, extent, extent.width())?,
                self.config,
            )?;
            let mut rendered = self.buffers.frames.floats(length);
            labels_to_f32(
                &output,
                &mut rendered,
                label_representation(destination.format())?,
            )?;
            self.buffers
                .frames
                .write_f32(&rendered, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct SuperpixelsProcessor {
    buffers: Buffers,
    config: vs_segment::SuperpixelConfig,
}

impl UnaryFrameProcessor for SuperpixelsProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut source_values = self.buffers.frames.floats(length);
            let extent = self
                .buffers
                .frames
                .read_f32(source, plane, &mut source_values)?;
            let mut labels = self.buffers.frames.integers(length);
            vs_segment::superpixels(
                Plane::new(&source_values, extent, extent.width())?,
                PlaneMut::new(&mut labels, extent, extent.width())?,
                self.config,
            )?;
            let mut rendered = self.buffers.frames.floats(length);
            labels_to_f32(
                &labels,
                &mut rendered,
                label_representation(destination.format())?,
            )?;
            self.buffers
                .frames
                .write_f32(&rendered, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct RegionDegreeMapProcessor {
    buffers: Buffers,
    connectivity: vs_segment::Connectivity,
}

impl UnaryFrameProcessor for RegionDegreeMapProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut samples = self.buffers.frames.floats(length);
            let extent = self.buffers.frames.read_f32(source, plane, &mut samples)?;
            let mut labels = self.buffers.frames.integers(length);
            labels_from_f32(&samples, &mut labels)?;
            let graph = vs_segment::region_graph(
                Plane::new(&labels, extent, extent.width())?,
                self.connectivity,
            )?;
            let mut degrees = self.buffers.frames.integers(graph.nodes.len());
            for edge in &graph.edges {
                for label in [edge.left, edge.right] {
                    let index = graph
                        .nodes
                        .binary_search_by_key(&label, |node| node.label)
                        .map_err(|_| Error::msg("region graph edge referenced an absent node"))?;
                    degrees[index] = degrees[index]
                        .checked_add(1)
                        .ok_or_else(|| Error::msg("region degree overflowed u32"))?;
                }
            }
            let mut degree_map = self.buffers.frames.integers(length);
            for (destination, &label) in degree_map.iter_mut().zip(labels.iter()) {
                if label == 0 {
                    *destination = 0;
                    continue;
                }
                let index = graph
                    .nodes
                    .binary_search_by_key(&label, |node| node.label)
                    .map_err(|_| Error::msg("label plane referenced an absent graph node"))?;
                *destination = degrees[index];
            }
            let mut rendered = self.buffers.frames.floats(length);
            labels_to_f32(
                &degree_map,
                &mut rendered,
                label_representation(destination.format())?,
            )?;
            self.buffers
                .frames
                .write_f32(&rendered, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct MergeProcessor {
    buffers: Buffers,
    config: vs_segment::MergeConfig,
}

impl PairFrameProcessor for MergeProcessor {
    fn process(
        &self,
        labels: &FrameRef<'_>,
        signal: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..labels.format().plane_count() {
            let length = area(labels, plane)?;
            let mut label_samples = self.buffers.frames.floats(length);
            let extent = self
                .buffers
                .frames
                .read_f32(labels, plane, &mut label_samples)?;
            let mut label_values = self.buffers.frames.integers(length);
            labels_from_f32(&label_samples, &mut label_values)?;
            let mut signal_values = self.buffers.frames.floats(length);
            let signal_extent = self
                .buffers
                .frames
                .read_f32(signal, plane, &mut signal_values)?;
            if extent != signal_extent {
                return Err(Error::msg("label and signal plane extents differ"));
            }
            let mut output = self.buffers.frames.integers(length);
            vs_segment::merge(
                Plane::new(&label_values, extent, extent.width())?,
                Plane::new(&signal_values, extent, extent.width())?,
                PlaneMut::new(&mut output, extent, extent.width())?,
                self.config,
            )?;
            let mut rendered = self.buffers.frames.floats(length);
            labels_to_f32(
                &output,
                &mut rendered,
                label_representation(destination.format())?,
            )?;
            self.buffers
                .frames
                .write_f32(&rendered, destination, plane)?;
        }
        Ok(())
    }
}

make_filter_function! {
    WatershedFunction, "Watershed"
    fn create_watershed<'core>(_api: API, _core: CoreRef<'core>, gradient: Node<'core>, markers: Node<'core>, connectivity_value: i64, boundary_value: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(gradient.info().format)?;
        require_supported_format(markers.info().format)?;
        let config = vs_segment::WatershedConfig::new(connectivity(connectivity_value)?, boundary(boundary_value)?);
        let filter = PairFilter::new(gradient, markers, WatershedProcessor { buffers: Buffers::new(), config })?;
        Ok(Some(Box::new(filter)))
    }
}

make_filter_function! {
    SuperpixelsFunction, "Superpixels"
    fn create_superpixels<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, cell_width: i64, cell_height: i64, compactness: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(clip.info().format)?;
        let width = usize::try_from(cell_width).map_err(|_| Error::msg("cell_width must be positive"))?;
        let height = usize::try_from(cell_height).map_err(|_| Error::msg("cell_height must be positive"))?;
        let config = vs_segment::SuperpixelConfig::new(width, height, non_negative_f32(compactness, "compactness")?).map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, SuperpixelsProcessor { buffers: Buffers::new(), config }))))
    }
}

make_filter_function! {
    RegionDegreeMapFunction, "RegionDegreeMap"
    fn create_region_degree_map<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, connectivity_value: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(clip.info().format)?;
        Ok(Some(Box::new(UnaryFilter::new(clip, RegionDegreeMapProcessor { buffers: Buffers::new(), connectivity: connectivity(connectivity_value)? }))))
    }
}

make_filter_function! {
    MergeFunction, "Merge"
    fn create_merge<'core>(_api: API, _core: CoreRef<'core>, labels: Node<'core>, signal: Node<'core>, threshold: f64, connectivity_value: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(labels.info().format)?;
        require_supported_format(signal.info().format)?;
        let config = vs_segment::MergeConfig::new(vs_segment::MergeCriterion::BoundaryDifference, non_negative_f32(threshold, "threshold")?, connectivity(connectivity_value)?)
            .map_err(|error| Error::msg(error.to_string()))?;
        let filter = PairFilter::new(labels, signal, MergeProcessor { buffers: Buffers::new(), config })?;
        Ok(Some(Box::new(filter)))
    }
}

export_vapoursynth_plugin! {
    Metadata { identifier: PLUGIN_IDENTIFIER, namespace: "segment", name: "PlaneSight Segment", read_only: true },
    [WatershedFunction::new(), SuperpixelsFunction::new(), RegionDegreeMapFunction::new(), MergeFunction::new()]
}

#[cfg(test)]
mod tests {
    use super::{
        LabelRepresentation, RegionDegreeMapFunction, labels_from_f32, labels_to_f32,
        non_negative_f32,
    };
    use vapoursynth::plugins::FilterFunction;

    #[test]
    fn label_input_rejects_the_rounded_u32_upper_bound() {
        let mut output = [0];
        assert!(labels_from_f32(&[u32::MAX as f32], &mut output).is_err());
        labels_from_f32(&[16_777_216.0], &mut output).expect("exact f32 integer");
        assert_eq!(output, [16_777_216]);
    }

    #[test]
    fn label_rendering_rejects_clipping_and_rounding() {
        let mut output = [0.0];
        assert!(
            labels_to_f32(
                &[256],
                &mut output,
                LabelRepresentation::Integer { maximum: 255 },
            )
            .is_err()
        );
        assert!(labels_to_f32(&[16_777_217], &mut output, LabelRepresentation::Float32).is_err());
        labels_to_f32(&[16_777_216], &mut output, LabelRepresentation::Float32)
            .expect("exact label");
    }

    #[test]
    fn config_narrowing_rejects_negative_underflow() {
        assert!(non_negative_f32(-f64::MIN_POSITIVE, "threshold").is_err());
        assert!(non_negative_f32(f64::MIN_POSITIVE, "threshold").is_err());
    }

    #[test]
    fn region_degree_registration_matches_the_catalogue() {
        assert_eq!(
            RegionDegreeMapFunction::new().name(),
            vs_segment::PLUGIN.filters[2].name
        );
    }
}
