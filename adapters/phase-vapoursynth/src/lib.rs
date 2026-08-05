//! VapourSynth registration for Phase scalar reference filters.
//!
//! Correlation outputs use RGB f32 planes for horizontal translation,
//! vertical translation, and magnitude. Event-energy outputs use those planes
//! for mean energy, peak energy, and active fraction respectively.

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
use vsip_vapoursynth::{FrameBuffers, TemporalFilter, TemporalFrameProcessor};

const PLUGIN_IDENTIFIER: &str = "com.planesight.vsip.phase";
const RGB_WEIGHTS: [f32; 3] = [0.2126, 0.7152, 0.0722];

#[derive(Debug)]
struct Buffers {
    frames: FrameBuffers,
}

impl Buffers {
    fn new() -> Self {
        Self {
            frames: FrameBuffers::new(16).expect("positive retention"),
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
            "Phase Magnify supports only u8, u16, and 32-bit floating-point clips",
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
            "Phase analysis filters require planar RGB 32-bit floating-point clips",
        ))
    }
}

fn read_luma(buffers: &Buffers, frame: &FrameRef<'_>, output: &mut [f32]) -> Result<Extent, Error> {
    output.fill(0.0);
    let mut expected_extent = None;
    for (plane, weight) in RGB_WEIGHTS.into_iter().enumerate() {
        let mut component = buffers.frames.floats(output.len());
        let extent = buffers.frames.read_f32(frame, plane, &mut component)?;
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

fn write_constant(
    buffers: &Buffers,
    destination: &mut FrameRefMut<'_>,
    plane: usize,
    length: usize,
    value: f32,
) -> Result<(), Error> {
    let mut output = buffers.frames.floats(length);
    output.fill(value);
    buffers.frames.write_f32(&output, destination, plane)?;
    Ok(())
}

fn write_translation(
    buffers: &Buffers,
    destination: &mut FrameRefMut<'_>,
    length: usize,
    translation: vs_phase::Translation,
) -> Result<(), Error> {
    write_constant(buffers, destination, 0, length, translation.x)?;
    write_constant(buffers, destination, 1, length, translation.y)?;
    write_constant(
        buffers,
        destination,
        2,
        length,
        translation.x.hypot(translation.y),
    )
}

fn fill_tile(
    output: &mut [f32],
    extent: Extent,
    tile: vs_phase::TileMotion,
    value: f32,
) -> Result<(), Error> {
    let end_y = tile
        .y
        .checked_add(tile.height)
        .ok_or_else(|| Error::msg("tile vertical extent overflowed"))?;
    for row in tile.y..end_y {
        let start = row
            .checked_mul(extent.width())
            .and_then(|offset| offset.checked_add(tile.x))
            .ok_or_else(|| Error::msg("tile output offset overflowed"))?;
        let end = start
            .checked_add(tile.width)
            .ok_or_else(|| Error::msg("tile output offset overflowed"))?;
        output
            .get_mut(start..end)
            .ok_or_else(|| Error::msg("tile exceeds the output extent"))?
            .fill(value);
    }
    Ok(())
}

fn forward_pair<'a>(
    frames: &'a [FrameRef<'a>],
    center: usize,
) -> Result<(&'a FrameRef<'a>, &'a FrameRef<'a>), Error> {
    let current = frames
        .get(center)
        .ok_or_else(|| Error::msg("temporal center frame is unavailable"))?;
    Ok((current, frames.get(center + 1).unwrap_or(current)))
}

#[derive(Debug)]
struct CorrelateProcessor {
    buffers: Buffers,
    config: vs_phase::CorrelationConfig,
}

impl TemporalFrameProcessor for CorrelateProcessor {
    fn process(
        &self,
        frames: &[FrameRef<'_>],
        center: usize,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let (current, compared) = forward_pair(frames, center)?;
        let length = FrameBuffers::plane_area(current, 0)?;
        let mut reference = self.buffers.frames.floats(length);
        let extent = read_luma(&self.buffers, current, &mut reference)?;
        let mut moving = self.buffers.frames.floats(length);
        if read_luma(&self.buffers, compared, &mut moving)? != extent {
            return Err(Error::msg("temporal frame extents differ"));
        }
        let translation = vs_phase::correlate(
            Plane::new(&reference, extent, extent.width())?,
            Plane::new(&moving, extent, extent.width())?,
            self.config,
        )
        .map_err(kernel_error)?;
        write_translation(&self.buffers, destination, length, translation)
    }
}

#[derive(Debug)]
struct LocalMotionProcessor {
    buffers: Buffers,
    config: vs_phase::LocalMotionConfig,
}

impl TemporalFrameProcessor for LocalMotionProcessor {
    fn process(
        &self,
        frames: &[FrameRef<'_>],
        center: usize,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let (current, compared) = forward_pair(frames, center)?;
        let length = FrameBuffers::plane_area(current, 0)?;
        let mut reference = self.buffers.frames.floats(length);
        let extent = read_luma(&self.buffers, current, &mut reference)?;
        let mut moving = self.buffers.frames.floats(length);
        if read_luma(&self.buffers, compared, &mut moving)? != extent {
            return Err(Error::msg("temporal frame extents differ"));
        }
        let mut horizontal = self.buffers.frames.floats(length);
        let mut vertical = self.buffers.frames.floats(length);
        let mut magnitude = self.buffers.frames.floats(length);
        let mut fill_error = None;
        vs_phase::visit_local_motion(
            Plane::new(&reference, extent, extent.width())?,
            Plane::new(&moving, extent, extent.width())?,
            self.config,
            |tile| {
                if fill_error.is_some() {
                    return;
                }
                let result = fill_tile(&mut horizontal, extent, tile, tile.translation.x)
                    .and_then(|()| fill_tile(&mut vertical, extent, tile, tile.translation.y))
                    .and_then(|()| {
                        fill_tile(
                            &mut magnitude,
                            extent,
                            tile,
                            tile.translation.x.hypot(tile.translation.y),
                        )
                    });
                if let Err(error) = result {
                    fill_error = Some(error);
                }
            },
        )
        .map_err(kernel_error)?;
        if let Some(error) = fill_error {
            return Err(error);
        }
        self.buffers.frames.write_f32(&horizontal, destination, 0)?;
        self.buffers.frames.write_f32(&vertical, destination, 1)?;
        self.buffers.frames.write_f32(&magnitude, destination, 2)?;
        Ok(())
    }
}

#[derive(Debug)]
struct MagnifyProcessor {
    buffers: Buffers,
    config: vs_phase::MagnifyConfig,
}

impl TemporalFrameProcessor for MagnifyProcessor {
    fn process(
        &self,
        frames: &[FrameRef<'_>],
        center: usize,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let current = &frames[center];
        let previous = center
            .checked_sub(1)
            .and_then(|index| frames.get(index))
            .unwrap_or(current);
        let next = frames.get(center + 1).unwrap_or(current);
        for plane in 0..current.format().plane_count() {
            let length = FrameBuffers::plane_area(current, plane)?;
            let mut before = self.buffers.frames.floats(length);
            let extent = self.buffers.frames.read_f32(previous, plane, &mut before)?;
            let mut middle = self.buffers.frames.floats(length);
            if self.buffers.frames.read_f32(current, plane, &mut middle)? != extent {
                return Err(Error::msg("temporal frame plane extents differ"));
            }
            let mut after = self.buffers.frames.floats(length);
            if self.buffers.frames.read_f32(next, plane, &mut after)? != extent {
                return Err(Error::msg("temporal frame plane extents differ"));
            }
            let mut output = self.buffers.frames.floats(length);
            vs_phase::magnify(
                Plane::new(&before, extent, extent.width())?,
                Plane::new(&middle, extent, extent.width())?,
                Plane::new(&after, extent, extent.width())?,
                PlaneMut::new(&mut output, extent, extent.width())?,
                self.config,
            )
            .map_err(kernel_error)?;
            self.buffers.frames.write_f32(&output, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct EventEnergyProcessor {
    buffers: Buffers,
    config: vs_phase::EventEnergyConfig,
}

impl TemporalFrameProcessor for EventEnergyProcessor {
    fn process(
        &self,
        frames: &[FrameRef<'_>],
        center: usize,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let current = &frames[center];
        let previous = center
            .checked_sub(1)
            .and_then(|index| frames.get(index))
            .unwrap_or(current);
        let next = frames.get(center + 1).unwrap_or(current);
        let length = FrameBuffers::plane_area(current, 0)?;
        let mut before = self.buffers.frames.floats(length);
        let extent = read_luma(&self.buffers, previous, &mut before)?;
        let mut middle = self.buffers.frames.floats(length);
        if read_luma(&self.buffers, current, &mut middle)? != extent {
            return Err(Error::msg("temporal frame extents differ"));
        }
        let mut after = self.buffers.frames.floats(length);
        if read_luma(&self.buffers, next, &mut after)? != extent {
            return Err(Error::msg("temporal frame extents differ"));
        }
        let energy = vs_phase::event_energy(
            Plane::new(&before, extent, extent.width())?,
            Plane::new(&middle, extent, extent.width())?,
            Plane::new(&after, extent, extent.width())?,
            self.config,
        )
        .map_err(kernel_error)?;
        write_constant(&self.buffers, destination, 0, length, energy.mean())?;
        write_constant(&self.buffers, destination, 1, length, energy.peak())?;
        write_constant(
            &self.buffers,
            destination,
            2,
            length,
            energy.active_fraction(),
        )
    }
}

fn temporal_clip(clip: &Node<'_>) -> Result<(), Error> {
    if clip.info().num_frames == 0 {
        Err(Error::msg("filter requires a non-empty clip"))
    } else {
        Ok(())
    }
}

fn positive(value: i64, name: &str) -> Result<usize, Error> {
    let value =
        usize::try_from(value).map_err(|_| Error::msg(format!("{name} must be positive")))?;
    if value == 0 {
        Err(Error::msg(format!("{name} must be positive")))
    } else {
        Ok(value)
    }
}

make_filter_function! {
    CorrelateFunction, "Correlate"
    fn create_correlate<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, max_displacement: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_analysis_format(&clip)?;
        temporal_clip(&clip)?;
        let config = vs_phase::CorrelationConfig::new(positive(max_displacement, "max_displacement")?).map_err(kernel_error)?;
        Ok(Some(Box::new(TemporalFilter::new(clip, 1, CorrelateProcessor { buffers: Buffers::new(), config }).map_err(kernel_error)?)))
    }
}

make_filter_function! {
    LocalMotionFunction, "LocalMotion"
    fn create_local_motion<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, tile_width: i64, tile_height: i64, max_displacement: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_analysis_format(&clip)?;
        temporal_clip(&clip)?;
        let config = vs_phase::LocalMotionConfig::new(positive(tile_width, "tile_width")?, positive(tile_height, "tile_height")?, positive(max_displacement, "max_displacement")?).map_err(kernel_error)?;
        Ok(Some(Box::new(TemporalFilter::new(clip, 1, LocalMotionProcessor { buffers: Buffers::new(), config }).map_err(kernel_error)?)))
    }
}

make_filter_function! {
    MagnifyFunction, "Magnify"
    fn create_magnify<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, gain: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_supported(&clip)?;
        temporal_clip(&clip)?;
        let config = vs_phase::MagnifyConfig::new(gain as f32).map_err(kernel_error)?;
        Ok(Some(Box::new(TemporalFilter::new(clip, 1, MagnifyProcessor { buffers: Buffers::new(), config }).map_err(kernel_error)?)))
    }
}

make_filter_function! {
    EventEnergyFunction, "EventEnergy"
    fn create_event_energy<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, energy_threshold: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_analysis_format(&clip)?;
        temporal_clip(&clip)?;
        let config = vs_phase::EventEnergyConfig::new(energy_threshold as f32).map_err(kernel_error)?;
        Ok(Some(Box::new(TemporalFilter::new(clip, 1, EventEnergyProcessor { buffers: Buffers::new(), config }).map_err(kernel_error)?)))
    }
}

mod plugin_abi {
    #![allow(missing_docs)]

    use super::*;

    export_vapoursynth_plugin! {
        Metadata { identifier: PLUGIN_IDENTIFIER, namespace: "phase", name: "PlaneSight Phase", read_only: true },
        [CorrelateFunction::new(), LocalMotionFunction::new(), MagnifyFunction::new(), EventEnergyFunction::new()]
    }
}
