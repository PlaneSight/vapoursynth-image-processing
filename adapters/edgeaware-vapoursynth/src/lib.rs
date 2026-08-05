//! VapourSynth registration for the EdgeAware reference filters.
//!
//! Spatial radii are measured in samples of each processed plane. Range and
//! regularization parameters use the clip's native numeric sample scale.

#[macro_use]
extern crate vapoursynth;

use vapoursynth::{
    anyhow::Error,
    core::CoreRef,
    format::SampleType,
    frame::{FrameRef, FrameRefMut},
    node::Node,
    plugins::{Filter, FilterArgument, Metadata},
    prelude::API,
};
use vsip_core::{Extent, Plane, PlaneMut};
use vsip_vapoursynth::{FrameBuffers, UnaryFilter, UnaryFrameProcessor};

const PLUGIN_IDENTIFIER: &str = "com.planesight.vsip.edgeaware";

#[derive(Debug)]
struct Buffers {
    frames: FrameBuffers,
}

impl Buffers {
    fn new() -> Self {
        Self {
            frames: FrameBuffers::new(16).expect("positive scratch retention"),
        }
    }
}

fn area(frame: &FrameRef<'_>, plane: usize) -> Result<usize, Error> {
    Ok(FrameBuffers::plane_area(frame, plane)?)
}

fn copy_to_f32(
    buffers: &Buffers,
    frame: &FrameRef<'_>,
    plane: usize,
    output: &mut [f32],
) -> Result<Extent, Error> {
    Ok(buffers.frames.read_f32(frame, plane, output)?)
}

fn copy_from_f32(
    buffers: &Buffers,
    input: &[f32],
    frame: &mut FrameRefMut<'_>,
    plane: usize,
) -> Result<(), Error> {
    buffers.frames.write_f32(input, frame, plane)?;
    Ok(())
}

fn validate_clip(clip: &Node<'_>) -> Result<(), Error> {
    let format = clip.info().format;
    match (format.sample_type(), format.bytes_per_sample()) {
        (SampleType::Integer, 1 | 2) | (SampleType::Float, 4) => Ok(()),
        _ => Err(Error::msg(
            "clip must use u8, u16, or single-precision f32 planar samples",
        )),
    }
}

fn process_three_scratch(
    buffers: &Buffers,
    source: &FrameRef<'_>,
    destination: &mut FrameRefMut<'_>,
    mut operation: impl FnMut(
        Plane<'_, f32>,
        PlaneMut<'_, f32>,
        PlaneMut<'_, f32>,
        PlaneMut<'_, f32>,
        PlaneMut<'_, f32>,
    ) -> Result<(), Error>,
) -> Result<(), Error> {
    for plane_index in 0..source.format().plane_count() {
        let length = area(source, plane_index)?;
        let mut input = buffers.frames.floats(length);
        let extent = copy_to_f32(buffers, source, plane_index, &mut input)?;
        let mut output = buffers.frames.floats(length);
        let mut first = buffers.frames.floats(length);
        let mut second = buffers.frames.floats(length);
        let mut third = buffers.frames.floats(length);
        operation(
            Plane::new(&input, extent, extent.width())?,
            PlaneMut::new(&mut output, extent, extent.width())?,
            PlaneMut::new(&mut first, extent, extent.width())?,
            PlaneMut::new(&mut second, extent, extent.width())?,
            PlaneMut::new(&mut third, extent, extent.width())?,
        )?;
        copy_from_f32(buffers, &output, destination, plane_index)?;
    }
    Ok(())
}

fn process_one_scratch(
    buffers: &Buffers,
    source: &FrameRef<'_>,
    destination: &mut FrameRefMut<'_>,
    mut operation: impl FnMut(Plane<'_, f32>, PlaneMut<'_, f32>, PlaneMut<'_, f32>) -> Result<(), Error>,
) -> Result<(), Error> {
    for plane_index in 0..source.format().plane_count() {
        let length = area(source, plane_index)?;
        let mut input = buffers.frames.floats(length);
        let extent = copy_to_f32(buffers, source, plane_index, &mut input)?;
        let mut output = buffers.frames.floats(length);
        let mut scratch = buffers.frames.floats(length);
        operation(
            Plane::new(&input, extent, extent.width())?,
            PlaneMut::new(&mut output, extent, extent.width())?,
            PlaneMut::new(&mut scratch, extent, extent.width())?,
        )?;
        copy_from_f32(buffers, &output, destination, plane_index)?;
    }
    Ok(())
}

#[derive(Debug)]
struct GuidedProcessor {
    buffers: Buffers,
    config: vs_edgeaware::GuidedConfig,
}

impl UnaryFrameProcessor for GuidedProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        process_three_scratch(
            &self.buffers,
            source,
            destination,
            |input, output, first, second, third| {
                vs_edgeaware::guided(input, output, first, second, third, self.config)
                    .map_err(|error| Error::msg(error.to_string()))
            },
        )
    }
}

#[derive(Debug)]
struct DomainTransformProcessor {
    buffers: Buffers,
    config: vs_edgeaware::DomainTransformConfig,
}

impl UnaryFrameProcessor for DomainTransformProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        process_one_scratch(
            &self.buffers,
            source,
            destination,
            |input, output, first| {
                vs_edgeaware::domain_transform(input, output, first, self.config)
                    .map_err(|error| Error::msg(error.to_string()))
            },
        )
    }
}

#[derive(Debug)]
struct RollingGuidanceProcessor {
    buffers: Buffers,
    config: vs_edgeaware::RollingGuidanceConfig,
}

impl UnaryFrameProcessor for RollingGuidanceProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        process_one_scratch(
            &self.buffers,
            source,
            destination,
            |input, output, first| {
                vs_edgeaware::rolling_guidance(input, output, first, self.config)
                    .map_err(|error| Error::msg(error.to_string()))
            },
        )
    }
}

#[derive(Debug)]
struct GlobalSmoothProcessor {
    buffers: Buffers,
    config: vs_edgeaware::GlobalSmoothConfig,
}

impl UnaryFrameProcessor for GlobalSmoothProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        process_one_scratch(
            &self.buffers,
            source,
            destination,
            |input, output, first| {
                vs_edgeaware::global_smooth(input, output, first, self.config)
                    .map_err(|error| Error::msg(error.to_string()))
            },
        )
    }
}

#[derive(Debug)]
struct JointGuidedProcessor {
    buffers: Buffers,
    config: vs_edgeaware::GuidedConfig,
}

impl vsip_vapoursynth::BinaryFrameProcessor for JointGuidedProcessor {
    fn process(
        &self,
        source: &FrameRef<'_>,
        guide: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        for plane_index in 0..source.format().plane_count() {
            let length = area(source, plane_index)?;
            let mut input = self.buffers.frames.floats(length);
            let extent = copy_to_f32(&self.buffers, source, plane_index, &mut input)?;
            let mut guide_values = self.buffers.frames.floats(length);
            let guide_extent = copy_to_f32(&self.buffers, guide, plane_index, &mut guide_values)?;
            if extent != guide_extent {
                return Err(Error::msg("source and guide frame plane extents differ"));
            }
            let mut output = self.buffers.frames.floats(length);
            let mut first = self.buffers.frames.floats(length);
            let mut second = self.buffers.frames.floats(length);
            let mut third = self.buffers.frames.floats(length);
            vs_edgeaware::guided_with_guide(
                Plane::new(&input, extent, extent.width())?,
                Plane::new(&guide_values, extent, extent.width())?,
                PlaneMut::new(&mut output, extent, extent.width())?,
                PlaneMut::new(&mut first, extent, extent.width())?,
                PlaneMut::new(&mut second, extent, extent.width())?,
                PlaneMut::new(&mut third, extent, extent.width())?,
                self.config,
            )
            .map_err(|error| Error::msg(error.to_string()))?;
            copy_from_f32(&self.buffers, &output, destination, plane_index)?;
        }
        Ok(())
    }
}

fn guided_config(radius: i64, epsilon: f64) -> Result<vs_edgeaware::GuidedConfig, Error> {
    let radius = usize::try_from(radius).map_err(|_| Error::msg("radius must be positive"))?;
    let config = vs_edgeaware::GuidedConfig {
        radius,
        epsilon: epsilon as f32,
    };
    config
        .validate()
        .map_err(|error| Error::msg(error.to_string()))?;
    Ok(config)
}

make_filter_function! {
    GuidedFunction, "Guided"
    fn create_guided<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, radius: i64, epsilon: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        Ok(Some(Box::new(UnaryFilter::new(clip, GuidedProcessor { buffers: Buffers::new(), config: guided_config(radius, epsilon)? }))))
    }
}

make_filter_function! {
    JointGuidedFunction, "JointGuided"
    fn create_joint_guided<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, guide: Node<'core>, radius: i64, epsilon: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        validate_clip(&guide)?;
        Ok(Some(Box::new(vsip_vapoursynth::BinaryFilter::new(clip, guide, JointGuidedProcessor { buffers: Buffers::new(), config: guided_config(radius, epsilon)? }).map_err(|error| Error::msg(error.to_string()))?)))
    }
}

make_filter_function! {
    DomainTransformFunction, "DomainTransform"
    fn create_domain_transform<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, sigma_spatial: f64, sigma_range: f64, iterations: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        let config = vs_edgeaware::DomainTransformConfig { sigma_spatial: sigma_spatial as f32, sigma_range: sigma_range as f32, iterations: u8::try_from(iterations).map_err(|_| Error::msg("iterations must fit u8"))? };
        config.validate().map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, DomainTransformProcessor { buffers: Buffers::new(), config }))))
    }
}

make_filter_function! {
    RollingGuidanceFunction, "RollingGuidance"
    fn create_rolling_guidance<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, radius: i64, range_sigma: f64, iterations: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        let config = vs_edgeaware::RollingGuidanceConfig { radius: usize::try_from(radius).map_err(|_| Error::msg("radius must be positive"))?, range_sigma: range_sigma as f32, iterations: u8::try_from(iterations).map_err(|_| Error::msg("iterations must fit u8"))? };
        config.validate().map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, RollingGuidanceProcessor { buffers: Buffers::new(), config }))))
    }
}

make_filter_function! {
    GlobalSmoothFunction, "GlobalSmooth"
    fn create_global_smooth<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, iterations: i64, edge_sigma: f64, step: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        let config = vs_edgeaware::GlobalSmoothConfig { iterations: u8::try_from(iterations).map_err(|_| Error::msg("iterations must fit u8"))?, edge_sigma: edge_sigma as f32, step: step as f32 };
        config.validate().map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, GlobalSmoothProcessor { buffers: Buffers::new(), config }))))
    }
}

mod plugin_abi {
    #![allow(missing_docs, unsafe_code)]
    use super::*;
    export_vapoursynth_plugin! {
        Metadata { identifier: PLUGIN_IDENTIFIER, namespace: "edgeaware", name: "PlaneSight EdgeAware", read_only: true },
        [GuidedFunction::new(), JointGuidedFunction::new(), DomainTransformFunction::new(), RollingGuidanceFunction::new(), GlobalSmoothFunction::new()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guided_arguments_are_validated_after_f32_conversion() {
        assert!(guided_config(1, 0.01).is_ok());
        assert!(guided_config(0, 0.01).is_err());
        assert!(guided_config(1, f64::MAX).is_err());
    }
}
