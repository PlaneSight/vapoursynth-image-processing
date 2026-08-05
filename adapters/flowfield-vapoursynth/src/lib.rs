//! VapourSynth registration for FlowField scalar reference filters.
//!
//! Motion fields use an RGB f32 clip with horizontal displacement in red,
//! vertical displacement in green, and magnitude in blue. Keeping the signed
//! components in f32 avoids lossy or biased integer encodings.

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
    BinaryFilter, BinaryFrameProcessor, FrameBuffers, ScratchPool, TemporalFilter,
    TemporalFrameProcessor, UnaryFilter, UnaryFrameProcessor,
};

const PLUGIN_IDENTIFIER: &str = "com.planesight.vsip.flowfield";
const RGB_WEIGHTS: [f32; 3] = [0.2126, 0.7152, 0.0722];

#[derive(Debug)]
struct Buffers {
    frames: FrameBuffers,
    floats: ScratchPool<f32>,
    vectors: ScratchPool<vs_flowfield::MotionVector>,
    colors: ScratchPool<vs_flowfield::Rgb>,
}

impl Buffers {
    fn new() -> Self {
        Self {
            frames: FrameBuffers::new(16).expect("positive retention"),
            floats: ScratchPool::new(16).expect("positive retention"),
            vectors: ScratchPool::new(8).expect("positive retention"),
            colors: ScratchPool::new(4).expect("positive retention"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct EstimateParameters {
    search_radius: usize,
    patch_radius: usize,
}

impl EstimateParameters {
    fn config(self) -> Result<vs_flowfield::EstimateConfig, Error> {
        vs_flowfield::EstimateConfig::new(
            self.search_radius,
            self.patch_radius,
            vs_flowfield::Direction::Forward,
        )
        .map_err(kernel_error)
    }
}

fn kernel_error(error: impl core::fmt::Display) -> Error {
    Error::msg(error.to_string())
}

fn require_rgb_f32(clip: &Node<'_>) -> Result<(), Error> {
    let format = clip.info().format;
    if format.color_family() == ColorFamily::RGB
        && format.plane_count() == 3
        && format.sample_type() == SampleType::Float
        && format.bytes_per_sample() == 4
    {
        Ok(())
    } else {
        Err(Error::msg(
            "FlowField filters require planar RGB 32-bit floating-point clips",
        ))
    }
}

fn area(frame: &FrameRef<'_>) -> Result<usize, Error> {
    Ok(FrameBuffers::plane_area(frame, 0)?)
}

fn read_plane(
    buffers: &Buffers,
    frame: &FrameRef<'_>,
    plane: usize,
    output: &mut [f32],
) -> Result<Extent, Error> {
    Ok(buffers.frames.read_f32(frame, plane, output)?)
}

fn write_plane(
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
        let extent = read_plane(buffers, frame, plane, &mut component)?;
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

fn read_field(
    buffers: &Buffers,
    frame: &FrameRef<'_>,
    output: &mut [vs_flowfield::MotionVector],
) -> Result<Extent, Error> {
    let mut horizontal = buffers.floats.acquire(output.len(), 0.0);
    let extent = read_plane(buffers, frame, 0, &mut horizontal)?;
    let mut vertical = buffers.floats.acquire(output.len(), 0.0);
    if read_plane(buffers, frame, 1, &mut vertical)? != extent {
        return Err(Error::msg("motion-field component extents differ"));
    }
    for ((vector, &x), &y) in output
        .iter_mut()
        .zip(horizontal.iter())
        .zip(vertical.iter())
    {
        *vector = vs_flowfield::MotionVector { x, y };
    }
    Ok(extent)
}

fn write_field(
    buffers: &Buffers,
    field: &[vs_flowfield::MotionVector],
    destination: &mut FrameRefMut<'_>,
) -> Result<(), Error> {
    let mut component = buffers.floats.acquire(field.len(), 0.0);
    for (output, vector) in component.iter_mut().zip(field) {
        *output = vector.x;
    }
    write_plane(buffers, &component, destination, 0)?;
    for (output, vector) in component.iter_mut().zip(field) {
        *output = vector.y;
    }
    write_plane(buffers, &component, destination, 1)?;
    for (output, vector) in component.iter_mut().zip(field) {
        *output = f64::from(vector.x)
            .hypot(f64::from(vector.y))
            .min(f64::from(f32::MAX)) as f32;
    }
    write_plane(buffers, &component, destination, 2)
}

fn write_scalar(
    buffers: &Buffers,
    values: &[f32],
    destination: &mut FrameRefMut<'_>,
) -> Result<(), Error> {
    for plane in 0..3 {
        write_plane(buffers, values, destination, plane)?;
    }
    Ok(())
}

fn write_colors(
    buffers: &Buffers,
    colors: &[vs_flowfield::Rgb],
    destination: &mut FrameRefMut<'_>,
) -> Result<(), Error> {
    let mut component = buffers.floats.acquire(colors.len(), 0.0);
    for (output, color) in component.iter_mut().zip(colors) {
        *output = color.red;
    }
    write_plane(buffers, &component, destination, 0)?;
    for (output, color) in component.iter_mut().zip(colors) {
        *output = color.green;
    }
    write_plane(buffers, &component, destination, 1)?;
    for (output, color) in component.iter_mut().zip(colors) {
        *output = color.blue;
    }
    write_plane(buffers, &component, destination, 2)
}

#[derive(Debug)]
struct EstimateProcessor {
    buffers: Buffers,
    parameters: EstimateParameters,
}

impl TemporalFrameProcessor for EstimateProcessor {
    fn process(
        &self,
        frames: &[FrameRef<'_>],
        center: usize,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let source = &frames[center];
        let compared = frames.get(center + 1).unwrap_or(source);
        let length = area(source)?;
        let mut first = self.buffers.floats.acquire(length, 0.0);
        let extent = read_luma(&self.buffers, source, &mut first)?;
        let mut second = self.buffers.floats.acquire(length, 0.0);
        if read_luma(&self.buffers, compared, &mut second)? != extent {
            return Err(Error::msg("temporal frame extents differ"));
        }
        let mut field = self
            .buffers
            .vectors
            .acquire(length, vs_flowfield::MotionVector::default());
        vs_flowfield::estimate(
            Plane::new(&first, extent, extent.width())?,
            Plane::new(&second, extent, extent.width())?,
            PlaneMut::new(&mut field, extent, extent.width())?,
            self.parameters.config()?,
        )
        .map_err(kernel_error)?;
        write_field(&self.buffers, &field, destination)
    }
}

#[derive(Debug)]
struct WarpProcessor {
    buffers: Buffers,
    config: vs_flowfield::WarpConfig,
}

impl BinaryFrameProcessor for WarpProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        field_frame: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let length = area(source)?;
        let mut field = self
            .buffers
            .vectors
            .acquire(length, vs_flowfield::MotionVector::default());
        let extent = read_field(&self.buffers, field_frame, &mut field)?;
        for plane in 0..3 {
            let mut input = self.buffers.floats.acquire(length, 0.0);
            if read_plane(&self.buffers, source, plane, &mut input)? != extent {
                return Err(Error::msg("source and motion-field extents differ"));
            }
            let mut output = self.buffers.floats.acquire(length, 0.0);
            vs_flowfield::warp(
                Plane::new(&input, extent, extent.width())?,
                Plane::new(&field, extent, extent.width())?,
                PlaneMut::new(&mut output, extent, extent.width())?,
                self.config,
            )
            .map_err(kernel_error)?;
            write_plane(&self.buffers, &output, destination, plane)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ConfidenceProcessor {
    buffers: Buffers,
}

impl BinaryFrameProcessor for ConfidenceProcessor {
    fn process(
        &self,
        first: &FrameRef<'_>,
        second: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let length = area(first)?;
        let mut forward = self
            .buffers
            .vectors
            .acquire(length, vs_flowfield::MotionVector::default());
        let extent = read_field(&self.buffers, first, &mut forward)?;
        let mut backward = self
            .buffers
            .vectors
            .acquire(length, vs_flowfield::MotionVector::default());
        if read_field(&self.buffers, second, &mut backward)? != extent {
            return Err(Error::msg("motion-field extents differ"));
        }
        let mut output = self.buffers.floats.acquire(length, 0.0);
        vs_flowfield::confidence(
            Plane::new(&forward, extent, extent.width())?,
            Plane::new(&backward, extent, extent.width())?,
            PlaneMut::new(&mut output, extent, extent.width())?,
        )
        .map_err(kernel_error)?;
        write_scalar(&self.buffers, &output, destination)
    }
}

#[derive(Debug)]
struct ComposeProcessor {
    buffers: Buffers,
}

impl BinaryFrameProcessor for ComposeProcessor {
    fn process(
        &self,
        first: &FrameRef<'_>,
        second: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let length = area(first)?;
        let mut first_field = self
            .buffers
            .vectors
            .acquire(length, vs_flowfield::MotionVector::default());
        let extent = read_field(&self.buffers, first, &mut first_field)?;
        let mut second_field = self
            .buffers
            .vectors
            .acquire(length, vs_flowfield::MotionVector::default());
        if read_field(&self.buffers, second, &mut second_field)? != extent {
            return Err(Error::msg("motion-field extents differ"));
        }
        let mut composed = self
            .buffers
            .vectors
            .acquire(length, vs_flowfield::MotionVector::default());
        vs_flowfield::compose(
            Plane::new(&first_field, extent, extent.width())?,
            Plane::new(&second_field, extent, extent.width())?,
            PlaneMut::new(&mut composed, extent, extent.width())?,
        )
        .map_err(kernel_error)?;
        write_field(&self.buffers, &composed, destination)
    }
}

#[derive(Debug)]
struct VisualizeProcessor {
    buffers: Buffers,
    config: vs_flowfield::VisualizationConfig,
}

impl UnaryFrameProcessor for VisualizeProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let length = area(source)?;
        let mut field = self
            .buffers
            .vectors
            .acquire(length, vs_flowfield::MotionVector::default());
        let extent = read_field(&self.buffers, source, &mut field)?;
        let mut colors = self
            .buffers
            .colors
            .acquire(length, vs_flowfield::Rgb::default());
        vs_flowfield::visualize(
            Plane::new(&field, extent, extent.width())?,
            PlaneMut::new(&mut colors, extent, extent.width())?,
            self.config,
        )
        .map_err(kernel_error)?;
        write_colors(&self.buffers, &colors, destination)
    }
}

fn estimate_parameters(search_radius: i64, patch_radius: i64) -> Result<EstimateParameters, Error> {
    let parameters = EstimateParameters {
        search_radius: usize::try_from(search_radius)
            .map_err(|_| Error::msg("search_radius must be positive"))?,
        patch_radius: usize::try_from(patch_radius)
            .map_err(|_| Error::msg("patch_radius must be non-negative"))?,
    };
    parameters.config()?;
    Ok(parameters)
}

fn temporal_clip(clip: &Node<'_>) -> Result<(), Error> {
    require_rgb_f32(clip)?;
    if clip.info().num_frames == 0 {
        Err(Error::msg("filter requires a non-empty clip"))
    } else {
        Ok(())
    }
}

fn interpolation(value: i64) -> Result<vs_flowfield::Interpolation, Error> {
    match value {
        0 => Ok(vs_flowfield::Interpolation::Nearest),
        1 => Ok(vs_flowfield::Interpolation::Bilinear),
        _ => Err(Error::msg("bilinear must be 0 or 1")),
    }
}

make_filter_function! {
    EstimateFunction, "Estimate"
    fn create_estimate<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, search_radius: i64, patch_radius: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        temporal_clip(&clip)?;
        Ok(Some(Box::new(TemporalFilter::new(clip, 1, EstimateProcessor { buffers: Buffers::new(), parameters: estimate_parameters(search_radius, patch_radius)? }).map_err(kernel_error)?)))
    }
}

make_filter_function! {
    WarpFunction, "Warp"
    fn create_warp<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, field: Node<'core>, bilinear: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_rgb_f32(&clip)?;
        require_rgb_f32(&field)?;
        let config = vs_flowfield::WarpConfig::new(vs_flowfield::BorderMode::Clamp, interpolation(bilinear)?).map_err(kernel_error)?;
        Ok(Some(Box::new(BinaryFilter::new(clip, field, WarpProcessor { buffers: Buffers::new(), config }).map_err(kernel_error)?)))
    }
}

make_filter_function! {
    ConfidenceFunction, "Confidence"
    fn create_confidence<'core>(_api: API, _core: CoreRef<'core>, forward: Node<'core>, backward: Node<'core>) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_rgb_f32(&forward)?;
        require_rgb_f32(&backward)?;
        Ok(Some(Box::new(BinaryFilter::new(forward, backward, ConfidenceProcessor { buffers: Buffers::new() }).map_err(kernel_error)?)))
    }
}

make_filter_function! {
    ComposeFunction, "Compose"
    fn create_compose<'core>(_api: API, _core: CoreRef<'core>, first: Node<'core>, second: Node<'core>) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_rgb_f32(&first)?;
        require_rgb_f32(&second)?;
        Ok(Some(Box::new(BinaryFilter::new(first, second, ComposeProcessor { buffers: Buffers::new() }).map_err(kernel_error)?)))
    }
}

make_filter_function! {
    VisualizeFunction, "Visualize"
    fn create_visualize<'core>(_api: API, _core: CoreRef<'core>, field: Node<'core>, maximum_magnitude: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        require_rgb_f32(&field)?;
        let config = vs_flowfield::VisualizationConfig::new(maximum_magnitude as f32).map_err(kernel_error)?;
        Ok(Some(Box::new(UnaryFilter::new(field, VisualizeProcessor { buffers: Buffers::new(), config }))))
    }
}

mod plugin_abi {
    #![allow(missing_docs)]

    use super::*;

    export_vapoursynth_plugin! {
        Metadata { identifier: PLUGIN_IDENTIFIER, namespace: "flowfield", name: "PlaneSight FlowField", read_only: true },
        [EstimateFunction::new(), WarpFunction::new(), ConfidenceFunction::new(), ComposeFunction::new(), VisualizeFunction::new()]
    }
}
