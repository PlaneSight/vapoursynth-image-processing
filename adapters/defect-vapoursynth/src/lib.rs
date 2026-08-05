//! VapourSynth runtime registration for Defect reference filters.

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
use vsip_vapoursynth::{
    FrameBuffers, ScratchPool, TemporalFilter, TemporalFrameProcessor, UnaryFilter,
    UnaryFrameProcessor,
};

const PLUGIN_IDENTIFIER: &str = "com.planesight.vsip.defect";

#[derive(Debug)]
struct Buffers {
    frames: FrameBuffers,
    masks: ScratchPool<u8>,
}

impl Buffers {
    fn new() -> Self {
        Self {
            frames: FrameBuffers::new(24).expect("positive scratch retention"),
            masks: ScratchPool::new(16).expect("positive scratch retention"),
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

fn require_supported_format(format: Format<'_>) -> Result<(), Error> {
    match (format.sample_type(), format.bytes_per_sample()) {
        (SampleType::Integer, 1 | 2) | (SampleType::Float, 4) => Ok(()),
        _ => Err(Error::msg(
            "only u8, u16, and single-precision f32 clips are supported",
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

fn unit_f32(value: f64, name: &str) -> Result<f32, Error> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(Error::msg(format!(
            "{name} must be finite and within zero through one"
        )));
    }
    let narrowed = value as f32;
    if value != 0.0 && narrowed == 0.0 {
        return Err(Error::msg(format!("{name} underflows f32")));
    }
    Ok(narrowed)
}

fn compatible_timing_and_resolution(
    first: VideoInfo<'_>,
    second: VideoInfo<'_>,
) -> Result<(), Error> {
    if first.resolution != second.resolution {
        return Err(Error::msg("input clip resolutions differ"));
    }
    if first.num_frames != second.num_frames {
        return Err(Error::msg("input clip frame counts differ"));
    }
    if first.framerate != second.framerate {
        return Err(Error::msg("input clip frame rates differ"));
    }
    Ok(())
}

fn compatible_plane_layout(first: Format<'_>, second: Format<'_>) -> Result<(), Error> {
    if first.plane_count() != second.plane_count()
        || first.sub_sampling_w() != second.sub_sampling_w()
        || first.sub_sampling_h() != second.sub_sampling_h()
    {
        return Err(Error::msg("input clips have different plane layouts"));
    }
    Ok(())
}

fn mask_from_samples(input: &[f32], output: &mut [u8]) -> Result<(), Error> {
    if input.len() != output.len() {
        return Err(Error::msg("mask conversion buffer lengths differ"));
    }
    if input.iter().any(|sample| !sample.is_finite()) {
        return Err(Error::msg("mask planes must contain finite samples"));
    }
    for (destination, &sample) in output.iter_mut().zip(input) {
        *destination = u8::from(sample != 0.0);
    }
    Ok(())
}

fn read_mask(
    buffers: &Buffers,
    frame: &FrameRef<'_>,
    plane: usize,
    output: &mut [u8],
) -> Result<vsip_core::Extent, Error> {
    let mut samples = buffers.frames.floats(output.len());
    let extent = buffers.frames.read_f32(frame, plane, &mut samples)?;
    mask_from_samples(&samples, output)?;
    Ok(extent)
}

trait TernaryFrameProcessor: Send + Sync {
    fn process(
        &self,
        first: &FrameRef<'_>,
        second: &FrameRef<'_>,
        third: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error>;
}

#[derive(Debug)]
struct TernaryFilter<'core, P> {
    first: Node<'core>,
    second: Node<'core>,
    third: Node<'core>,
    processor: P,
}

impl<'core, P> TernaryFilter<'core, P> {
    fn new(
        first: Node<'core>,
        second: Node<'core>,
        third: Node<'core>,
        processor: P,
    ) -> Result<Self, Error> {
        let first_info = first.info();
        let second_info = second.info();
        let third_info = third.info();
        if first_info.format != second_info.format {
            return Err(Error::msg("current and reference clip formats differ"));
        }
        compatible_timing_and_resolution(first_info, second_info)?;
        compatible_timing_and_resolution(first_info, third_info)?;
        compatible_plane_layout(first_info.format, third_info.format)?;
        Ok(Self {
            first,
            second,
            third,
            processor,
        })
    }
}

impl<'core, P> Filter<'core> for TernaryFilter<'core, P>
where
    P: TernaryFrameProcessor,
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
        self.third.request_frame_filter(context, n);
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
            .ok_or_else(|| anyhow!("current frame {n} was not available"))?;
        let second = self
            .second
            .get_frame_filter(context, n)
            .ok_or_else(|| anyhow!("reference frame {n} was not available"))?;
        let third = self
            .third
            .get_frame_filter(context, n)
            .ok_or_else(|| anyhow!("mask frame {n} was not available"))?;
        let mut destination = FrameRefMut::copy_of(core, &first);
        self.processor
            .process(&first, &second, &third, &mut destination)?;
        Ok(destination.into())
    }
}

trait MaskedTemporalFrameProcessor: Send + Sync {
    fn process(
        &self,
        frames: &[FrameRef<'_>],
        center: usize,
        mask: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error>;
}

#[derive(Debug)]
struct MaskedTemporalFilter<'core, P> {
    source: Node<'core>,
    mask: Node<'core>,
    processor: P,
}

impl<'core, P> MaskedTemporalFilter<'core, P> {
    fn new(source: Node<'core>, mask: Node<'core>, processor: P) -> Result<Self, Error> {
        compatible_timing_and_resolution(source.info(), mask.info())?;
        compatible_plane_layout(source.info().format, mask.info().format)?;
        Ok(Self {
            source,
            mask,
            processor,
        })
    }

    fn frame_range(&self, n: usize) -> std::ops::RangeInclusive<usize> {
        let last = self.source.info().num_frames - 1;
        n.saturating_sub(1)..=n.saturating_add(1).min(last)
    }
}

impl<'core, P> Filter<'core> for MaskedTemporalFilter<'core, P>
where
    P: MaskedTemporalFrameProcessor,
{
    fn video_info(&self, _api: API, _core: CoreRef<'core>) -> Vec<VideoInfo<'core>> {
        vec![self.source.info()]
    }

    fn get_frame_initial(
        &self,
        _api: API,
        _core: CoreRef<'core>,
        context: FrameContext<'_>,
        n: usize,
    ) -> Result<Option<FrameRef<'core>>, Error> {
        for frame_number in self.frame_range(n) {
            self.source.request_frame_filter(context, frame_number);
        }
        self.mask.request_frame_filter(context, n);
        Ok(None)
    }

    fn get_frame(
        &self,
        _api: API,
        core: CoreRef<'core>,
        context: FrameContext<'_>,
        n: usize,
    ) -> Result<FrameRef<'core>, Error> {
        let range = self.frame_range(n);
        let center = n - *range.start();
        let frames = range
            .map(|frame_number| {
                self.source
                    .get_frame_filter(context, frame_number)
                    .ok_or_else(|| anyhow!("source frame {frame_number} was not available"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mask = self
            .mask
            .get_frame_filter(context, n)
            .ok_or_else(|| anyhow!("mask frame {n} was not available"))?;
        let mut destination = FrameRefMut::copy_of(core, &frames[center]);
        self.processor
            .process(&frames, center, &mask, &mut destination)?;
        Ok(destination.into())
    }
}

fn peak(frame: &FrameRef<'_>) -> Result<f32, Error> {
    match (
        frame.format().sample_type(),
        frame.format().bytes_per_sample(),
    ) {
        (SampleType::Integer, 1 | 2) => {
            let bits = frame.format().bits_per_sample();
            if bits == 0 || bits > 16 {
                return Err(Error::msg("integer clip bit depth is invalid"));
            }
            Ok(((1_u32 << bits) - 1) as f32)
        }
        (SampleType::Float, 4) => Ok(1.0),
        _ => Err(Error::msg(
            "only u8, u16, and f32 frame planes are supported",
        )),
    }
}

fn triplet_indices(frame_count: usize, center: usize) -> Option<[usize; 3]> {
    let previous = center.checked_sub(1)?;
    let next = center.checked_add(1)?;
    (next < frame_count).then_some([previous, center, next])
}

fn require_temporal_triplet<'frames, 'core>(
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

#[derive(Debug)]
struct TemporalOutliersProcessor {
    buffers: Buffers,
    config: vs_defect::TemporalOutlierConfig,
}

impl TemporalFrameProcessor for TemporalOutliersProcessor {
    fn process(
        &self,
        frames: &[FrameRef<'_>],
        center: usize,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let current = frames
            .get(center)
            .ok_or_else(|| Error::msg("temporal window has no center frame"))?;
        let triplet = require_temporal_triplet(frames, center);
        for plane in 0..current.format().plane_count() {
            let length = area(current, plane)?;
            // A shortened endpoint window has no two-sided temporal baseline;
            // the detector's explicit endpoint result is an all-zero mask.
            let mut rendered = self.buffers.frames.floats(length);
            if let Some((previous, current, next)) = triplet {
                let mut before = self.buffers.frames.floats(length);
                let extent = self.buffers.frames.read_f32(previous, plane, &mut before)?;
                let mut sample = self.buffers.frames.floats(length);
                let current_extent = self.buffers.frames.read_f32(current, plane, &mut sample)?;
                let mut after = self.buffers.frames.floats(length);
                let next_extent = self.buffers.frames.read_f32(next, plane, &mut after)?;
                if extent != current_extent || extent != next_extent {
                    return Err(Error::msg("temporal frame plane extents differ"));
                }
                let mut detected = self.buffers.masks.acquire(length, 0);
                vs_defect::temporal_outliers(
                    Plane::new(&before, extent, extent.width())?,
                    Plane::new(&sample, extent, extent.width())?,
                    Plane::new(&after, extent, extent.width())?,
                    PlaneMut::new(&mut detected, extent, extent.width())?,
                    self.config,
                )?;
                let value = peak(current)?;
                for (destination, &mask) in rendered.iter_mut().zip(detected.iter()) {
                    *destination = if mask == 0 { 0.0 } else { value };
                }
            }
            write_f32(&self.buffers, &rendered, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ScratchDetectProcessor {
    buffers: Buffers,
    config: vs_defect::ScratchDetectConfig,
}

impl UnaryFrameProcessor for ScratchDetectProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut input = self.buffers.frames.floats(length);
            let extent = self.buffers.frames.read_f32(source, plane, &mut input)?;
            let report = vs_defect::scratch_detect(
                Plane::new(&input, extent, extent.width())?,
                self.config,
            )?;
            let mut rendered = self.buffers.frames.floats(length);
            let value = peak(source)?;
            for scratch in report.scratches {
                for y in scratch.start_y..scratch.start_y + scratch.length {
                    rendered[y * extent.width() + scratch.x] = value;
                }
            }
            write_f32(&self.buffers, &rendered, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct DropoutRepairProcessor {
    buffers: Buffers,
    config: vs_defect::DropoutRepairConfig,
}

impl TernaryFrameProcessor for DropoutRepairProcessor {
    fn process(
        &self,
        current: &FrameRef<'_>,
        reference: &FrameRef<'_>,
        dropout_mask: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..current.format().plane_count() {
            let length = area(current, plane)?;
            let mut source = self.buffers.frames.floats(length);
            let extent = self.buffers.frames.read_f32(current, plane, &mut source)?;
            let mut repair = self.buffers.frames.floats(length);
            let reference_extent = self
                .buffers
                .frames
                .read_f32(reference, plane, &mut repair)?;
            if extent != reference_extent {
                return Err(Error::msg("current and reference plane extents differ"));
            }
            let mut mask = self.buffers.masks.acquire(length, 0);
            let mask_extent = read_mask(&self.buffers, dropout_mask, plane, &mut mask)?;
            if extent != mask_extent {
                return Err(Error::msg("current and dropout-mask plane extents differ"));
            }
            let mut output = self.buffers.frames.floats(length);
            vs_defect::dropout_repair(
                Plane::new(&source, extent, extent.width())?,
                Plane::new(&repair, extent, extent.width())?,
                Plane::new(&mask, extent, extent.width())?,
                PlaneMut::new(&mut output, extent, extent.width())?,
                self.config,
            )?;
            write_f32(&self.buffers, &output, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct SpatialOutliersProcessor {
    buffers: Buffers,
    config: vs_defect::DeadPixelConfig,
}

impl UnaryFrameProcessor for SpatialOutliersProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane in 0..source.format().plane_count() {
            let length = area(source, plane)?;
            let mut input = self.buffers.frames.floats(length);
            let extent = self.buffers.frames.read_f32(source, plane, &mut input)?;
            let candidates =
                vs_defect::dead_pixels(Plane::new(&input, extent, extent.width())?, self.config)?;
            let mut rendered = self.buffers.frames.floats(length);
            let value = peak(source)?;
            for point in candidates {
                rendered[point.y * extent.width() + point.x] = value;
            }
            write_f32(&self.buffers, &rendered, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct InpaintTemporalProcessor {
    buffers: Buffers,
    config: vs_defect::TemporalInpaintConfig,
}

impl MaskedTemporalFrameProcessor for InpaintTemporalProcessor {
    fn process(
        &self,
        frames: &[FrameRef<'_>],
        center: usize,
        repair_mask: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let current = frames
            .get(center)
            .ok_or_else(|| Error::msg("temporal window has no center frame"))?;
        let triplet = require_temporal_triplet(frames, center);
        for plane in 0..current.format().plane_count() {
            let length = area(current, plane)?;
            let mut rendered = self.buffers.frames.floats(length);
            let mut current_values = self.buffers.frames.floats(length);
            let extent = self
                .buffers
                .frames
                .read_f32(current, plane, &mut current_values)?;
            if let Some((previous, _, next)) = triplet {
                let mut before = self.buffers.frames.floats(length);
                let before_extent = self.buffers.frames.read_f32(previous, plane, &mut before)?;
                let mut after = self.buffers.frames.floats(length);
                let after_extent = self.buffers.frames.read_f32(next, plane, &mut after)?;
                if before_extent != extent || after_extent != extent {
                    return Err(Error::msg("temporal frame plane extents differ"));
                }
                let mut mask = self.buffers.masks.acquire(length, 0);
                let mask_extent = read_mask(&self.buffers, repair_mask, plane, &mut mask)?;
                if mask_extent != extent {
                    return Err(Error::msg("current and repair-mask plane extents differ"));
                }
                vs_defect::inpaint_temporal(
                    Plane::new(&before, extent, extent.width())?,
                    Plane::new(&current_values, extent, extent.width())?,
                    Plane::new(&after, extent, extent.width())?,
                    Plane::new(&mask, extent, extent.width())?,
                    PlaneMut::new(&mut rendered, extent, extent.width())?,
                    self.config,
                )?;
            } else {
                rendered.copy_from_slice(&current_values);
            }
            write_f32(&self.buffers, &rendered, destination, plane)?;
        }
        Ok(())
    }
}

fn temporal_estimate(value: i64) -> Result<vs_defect::TemporalEstimate, Error> {
    match value {
        0 => Ok(vs_defect::TemporalEstimate::Average),
        1 => Ok(vs_defect::TemporalEstimate::NearestToCurrent),
        _ => Err(Error::msg(
            "estimate must be 0 (average) or 1 (nearest-to-current)",
        )),
    }
}

make_filter_function! {
    TemporalOutliersFunction, "TemporalOutliers"
    fn create_temporal_outliers<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, threshold: f64, neighbour_tolerance: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(clip.info().format)?;
        let config = vs_defect::TemporalOutlierConfig::new(
            non_negative_f32(threshold, "threshold")?,
            non_negative_f32(neighbour_tolerance, "neighbour_tolerance")?,
        )
            .map_err(|error| Error::msg(error.to_string()))?;
        let filter = TemporalFilter::new(clip, 1, TemporalOutliersProcessor { buffers: Buffers::new(), config })
            .map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(filter)))
    }
}

make_filter_function! {
    ScratchDetectFunction, "ScratchDetect"
    fn create_scratch_detect<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, contrast_threshold: f64, minimum_length: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(clip.info().format)?;
        let length = usize::try_from(minimum_length).map_err(|_| Error::msg("minimum_length must be positive"))?;
        let config = vs_defect::ScratchDetectConfig::new(non_negative_f32(contrast_threshold, "contrast_threshold")?, length)
            .map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, ScratchDetectProcessor { buffers: Buffers::new(), config }))))
    }
}

make_filter_function! {
    DropoutRepairFunction, "DropoutRepair"
    fn create_dropout_repair<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, reference: Node<'core>, mask: Node<'core>, repair_weight: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(clip.info().format)?;
        require_supported_format(reference.info().format)?;
        require_supported_format(mask.info().format)?;
        let config = vs_defect::DropoutRepairConfig::new(unit_f32(repair_weight, "repair_weight")?)
            .map_err(|error| Error::msg(error.to_string()))?;
        let filter = TernaryFilter::new(clip, reference, mask, DropoutRepairProcessor { buffers: Buffers::new(), config })?;
        Ok(Some(Box::new(filter)))
    }
}

make_filter_function! {
    SpatialOutliersFunction, "SpatialOutliers"
    fn create_spatial_outliers<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, deviation_threshold: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(clip.info().format)?;
        let config = vs_defect::DeadPixelConfig::new(non_negative_f32(deviation_threshold, "deviation_threshold")?)
            .map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, SpatialOutliersProcessor { buffers: Buffers::new(), config }))))
    }
}

make_filter_function! {
    InpaintTemporalFunction, "InpaintTemporal"
    fn create_inpaint_temporal<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, mask: Node<'core>, estimate_value: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported_format(clip.info().format)?;
        require_supported_format(mask.info().format)?;
        let config = vs_defect::TemporalInpaintConfig::new(temporal_estimate(estimate_value)?);
        let filter = MaskedTemporalFilter::new(clip, mask, InpaintTemporalProcessor { buffers: Buffers::new(), config })?;
        Ok(Some(Box::new(filter)))
    }
}

export_vapoursynth_plugin! {
    Metadata {
        identifier: PLUGIN_IDENTIFIER,
        namespace: "defect",
        name: "PlaneSight Defect",
        read_only: true,
    },
    [
        TemporalOutliersFunction::new(),
        ScratchDetectFunction::new(),
        DropoutRepairFunction::new(),
        SpatialOutliersFunction::new(),
        InpaintTemporalFunction::new(),
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        DropoutRepairFunction, InpaintTemporalFunction, SpatialOutliersFunction, mask_from_samples,
        non_negative_f32, triplet_indices, unit_f32,
    };
    use vapoursynth::plugins::FilterFunction;

    #[test]
    fn temporal_endpoints_have_no_complete_triplet() {
        assert_eq!(triplet_indices(3, 0), None);
        assert_eq!(triplet_indices(3, 1), Some([0, 1, 2]));
        assert_eq!(triplet_indices(3, 2), None);
    }

    #[test]
    fn numeric_narrowing_rejects_values_outside_the_declared_domain() {
        assert!(non_negative_f32(-f64::MIN_POSITIVE, "threshold").is_err());
        assert!(non_negative_f32(f64::MIN_POSITIVE, "threshold").is_err());
        assert!(unit_f32(f64::MIN_POSITIVE, "weight").is_err());
        assert!(unit_f32(1.000_001, "weight").is_err());
    }

    #[test]
    fn explicit_mask_conversion_rejects_non_finite_samples() {
        let mut output = [0; 3];
        mask_from_samples(&[0.0, -2.0, 4.0], &mut output).expect("finite mask");
        assert_eq!(output, [0, 1, 1]);
        assert!(mask_from_samples(&[f32::NAN], &mut output[..1]).is_err());
    }

    #[test]
    fn repaired_filter_contracts_match_the_catalogue_and_require_masks() {
        assert_eq!(
            SpatialOutliersFunction::new().name(),
            vs_defect::PLUGIN.filters[3].name
        );
        assert!(DropoutRepairFunction::new().args().contains("mask:vnode;"));
        assert!(
            InpaintTemporalFunction::new()
                .args()
                .contains("mask:vnode;")
        );
    }
}
