//! VapourSynth registration for Register translation operations.
//!
//! Public translations use full-resolution first-plane pixel units and are
//! scaled for subsampled planes during resampling. `Estimate` preserves the
//! first input frame and writes the signed result to the `VSIPRegisterX` and
//! `VSIPRegisterY` floating-point frame properties. Temporal endpoint windows
//! are shortened, never duplicated. Estimation rejects candidates below the
//! caller's positive `minimum_overlap` finite-pair fraction.

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
    video_info::Property,
};
use vsip_core::{Extent, Plane, PlaneMut};
use vsip_vapoursynth::{
    BinaryFilter, BinaryFrameProcessor, FrameBuffers, TemporalFilter, TemporalFrameProcessor,
    UnaryFilter, UnaryFrameProcessor,
};

const PLUGIN_IDENTIFIER: &str = "com.planesight.vsip.register";

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
fn area(frame: &FrameRef<'_>, plane: usize) -> Result<usize, Error> {
    Ok(FrameBuffers::plane_area(frame, plane)?)
}
fn read(
    buffers: &Buffers,
    frame: &FrameRef<'_>,
    plane: usize,
    out: &mut [f32],
) -> Result<Extent, Error> {
    Ok(buffers.frames.read_f32(frame, plane, out)?)
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
fn validate_clip(clip: &Node<'_>) -> Result<(), Error> {
    let format = clip.info().format;
    match (format.sample_type(), format.bytes_per_sample()) {
        (SampleType::Integer, 1 | 2) | (SampleType::Float, 4) => Ok(()),
        _ => Err(Error::msg(
            "clip must use u8, u16, or single-precision f32 planar samples",
        )),
    }
}

fn full_extent(frame: &FrameRef<'_>) -> Result<Extent, Error> {
    Extent::new(frame.width(0), frame.height(0)).map_err(Error::from)
}

fn scale_translation(
    translation: vs_register::Translation,
    full: Extent,
    plane: Extent,
) -> Result<vs_register::Translation, Error> {
    let scaled = vs_register::Translation {
        x: (f64::from(translation.x) * plane.width() as f64 / full.width() as f64) as f32,
        y: (f64::from(translation.y) * plane.height() as f64 / full.height() as f64) as f32,
    };
    scaled
        .validate()
        .map_err(|error| Error::msg(error.to_string()))?;
    Ok(scaled)
}

fn unary(
    buffers: &Buffers,
    source: &FrameRef<'_>,
    destination: &mut FrameRefMut<'_>,
    mut op: impl FnMut(Plane<'_, f32>, PlaneMut<'_, f32>, Extent) -> Result<(), Error>,
) -> Result<(), Error> {
    let full = full_extent(source)?;
    for p in 0..source.format().plane_count() {
        let n = area(source, p)?;
        let mut input = buffers.frames.floats(n);
        let e = read(buffers, source, p, &mut input)?;
        let mut output = buffers.frames.floats(n);
        op(
            Plane::new(&input, e, e.width())?,
            PlaneMut::new(&mut output, e, e.width())?,
            full,
        )?;
        write(buffers, &output, destination, p)?;
    }
    Ok(())
}
fn binary(
    buffers: &Buffers,
    first: &FrameRef<'_>,
    second: &FrameRef<'_>,
    destination: &mut FrameRefMut<'_>,
    mut op: impl FnMut(
        Plane<'_, f32>,
        Plane<'_, f32>,
        PlaneMut<'_, f32>,
        &mut [f32],
        Extent,
    ) -> Result<(), Error>,
) -> Result<(), Error> {
    for p in 0..first.format().plane_count() {
        let n = area(first, p)?;
        let mut a = buffers.frames.floats(n);
        let e = read(buffers, first, p, &mut a)?;
        let mut b = buffers.frames.floats(n);
        if read(buffers, second, p, &mut b)? != e {
            return Err(Error::msg("input frame plane extents differ"));
        }
        let mut output = buffers.frames.floats(n);
        let mut scratch = buffers.frames.floats(2);
        op(
            Plane::new(&a, e, e.width())?,
            Plane::new(&b, e, e.width())?,
            PlaneMut::new(&mut output, e, e.width())?,
            &mut scratch,
            e,
        )?;
        write(buffers, &output, destination, p)?;
    }
    Ok(())
}

#[derive(Debug)]
struct WarpProcessor {
    buffers: Buffers,
    translation: vs_register::Translation,
    config: vs_register::WarpConfig,
}
impl UnaryFrameProcessor for WarpProcessor {
    fn process(&self, s: &FrameRef<'_>, d: &mut FrameRefMut<'_>) -> Result<(), Error> {
        unary(&self.buffers, s, d, |input, output, full| {
            let translation = scale_translation(self.translation, full, input.extent())?;
            vs_register::warp(input, output, translation, self.config)
                .map_err(|e| Error::msg(e.to_string()))
        })
    }
}
#[derive(Debug)]
struct EstimateProcessor {
    buffers: Buffers,
    config: vs_register::EstimateConfig,
}
impl BinaryFrameProcessor for EstimateProcessor {
    fn process(
        &self,
        a: &FrameRef<'_>,
        b: &FrameRef<'_>,
        d: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let length = area(a, 0)?;
        let mut reference = self.buffers.frames.floats(length);
        let extent = read(&self.buffers, a, 0, &mut reference)?;
        let mut moving = self.buffers.frames.floats(length);
        if read(&self.buffers, b, 0, &mut moving)? != extent {
            return Err(Error::msg("input frame plane extents differ"));
        }
        let translation = vs_register::estimate(
            Plane::new(&reference, extent, extent.width())?,
            Plane::new(&moving, extent, extent.width())?,
            self.config,
        )
        .map_err(|error| Error::msg(error.to_string()))?;
        let mut properties = d.props_mut();
        properties.set_float("VSIPRegisterX", f64::from(translation.x))?;
        properties.set_float("VSIPRegisterY", f64::from(translation.y))?;
        Ok(())
    }
}
#[derive(Debug)]
struct StackProcessor {
    buffers: Buffers,
    config: vs_register::StackConfig,
}
impl BinaryFrameProcessor for StackProcessor {
    fn process(
        &self,
        a: &FrameRef<'_>,
        b: &FrameRef<'_>,
        d: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        binary(
            &self.buffers,
            a,
            b,
            d,
            |first, second, output, scratch, _| {
                vs_register::stack(&[first, second], output, scratch, self.config)
                    .map_err(|e| Error::msg(e.to_string()))
            },
        )
    }
}
#[derive(Debug)]
struct StabilizeProcessor {
    buffers: Buffers,
    config: vs_register::StabilizeConfig,
    estimate: vs_register::EstimateConfig,
}
impl TemporalFrameProcessor for StabilizeProcessor {
    fn process(
        &self,
        frames: &[FrameRef<'_>],
        center: usize,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        let current = &frames[center];
        let n = area(current, 0)?;
        let mut reference = self.buffers.frames.floats(n);
        let e = read(&self.buffers, current, 0, &mut reference)?;
        let mut moving = self.buffers.frames.floats(n);
        let mut path = [vs_register::Translation { x: 0., y: 0. }; 3];
        let start = center.saturating_sub(1);
        let end = (center + 1).min(frames.len() - 1);
        let mut count = 0;
        for (index, frame) in frames.iter().enumerate().take(end + 1).skip(start) {
            path[count] = if index == center {
                vs_register::Translation { x: 0., y: 0. }
            } else {
                read(&self.buffers, frame, 0, &mut moving)?;
                vs_register::estimate(
                    Plane::new(&reference, e, e.width())?,
                    Plane::new(&moving, e, e.width())?,
                    self.estimate,
                )
                .map_err(|error| Error::msg(error.to_string()))?
            };
            count += 1;
        }
        let mut compensation = [vs_register::Translation { x: 0., y: 0. }; 3];
        let mut smooth = [vs_register::Translation { x: 0., y: 0. }; 3];
        vs_register::stabilize(
            &path[..count],
            &mut compensation[..count],
            &mut smooth[..count],
            self.config,
        )
        .map_err(|error| Error::msg(error.to_string()))?;
        let transform = compensation[center - start];
        let full = full_extent(current)?;
        for plane in 0..current.format().plane_count() {
            let len = area(current, plane)?;
            let mut input = self.buffers.frames.floats(len);
            let extent = read(&self.buffers, current, plane, &mut input)?;
            let mut out = self.buffers.frames.floats(len);
            let plane_transform = scale_translation(transform, full, extent)?;
            vs_register::warp(
                Plane::new(&input, extent, extent.width())?,
                PlaneMut::new(&mut out, extent, extent.width())?,
                plane_transform,
                vs_register::WarpConfig {
                    interpolation: vs_register::Interpolation::Bilinear,
                    border: vs_register::Border::Clamp,
                },
            )
            .map_err(|error| Error::msg(error.to_string()))?;
            write(&self.buffers, &out, destination, plane)?;
        }
        Ok(())
    }
}

fn translation(x: f64, y: f64) -> Result<vs_register::Translation, Error> {
    let t = vs_register::Translation {
        x: x as f32,
        y: y as f32,
    };
    t.validate().map_err(|e| Error::msg(e.to_string()))?;
    Ok(t)
}
fn estimate_config(
    max_shift: i64,
    minimum_overlap: f64,
) -> Result<vs_register::EstimateConfig, Error> {
    let c = vs_register::EstimateConfig {
        max_shift: i32::try_from(max_shift).map_err(|_| Error::msg("max_shift must fit i32"))?,
        sample_step: 1,
        minimum_overlap: minimum_overlap as f32,
    };
    c.validate().map_err(|e| Error::msg(e.to_string()))?;
    Ok(c)
}
fn validate_estimate_extent(
    clip: &Node<'_>,
    config: vs_register::EstimateConfig,
) -> Result<(), Error> {
    if let Property::Constant(resolution) = clip.info().resolution {
        let extent = Extent::new(resolution.width, resolution.height)?;
        config
            .validate_for_extent(extent)
            .map_err(|error| Error::msg(error.to_string()))?;
    }
    Ok(())
}
fn interpolation(mode: i64) -> Result<vs_register::Interpolation, Error> {
    match mode {
        0 => Ok(vs_register::Interpolation::Nearest),
        1 => Ok(vs_register::Interpolation::Bilinear),
        _ => Err(Error::msg(
            "interpolation must be 0 (nearest) or 1 (bilinear)",
        )),
    }
}
fn stack_method(mode: i64) -> Result<vs_register::StackMethod, Error> {
    match mode {
        0 => Ok(vs_register::StackMethod::Mean),
        1 => Ok(vs_register::StackMethod::Median),
        _ => Err(Error::msg("method must be 0 (mean) or 1 (median)")),
    }
}
make_filter_function! {
    EstimateFunction, "Estimate"
    fn create_estimate<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, moving: Node<'core>, max_shift: i64, minimum_overlap: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        validate_clip(&moving)?;
        let config = estimate_config(max_shift, minimum_overlap)?;
        validate_estimate_extent(&clip, config)?;
        let processor = EstimateProcessor { buffers: Buffers::new(), config };
        let filter = BinaryFilter::new(clip, moving, processor).map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(filter)))
    }
}
make_filter_function! {
    WarpFunction, "Warp"
    fn create_warp<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, x: f64, y: f64, interpolation_mode: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        let config = vs_register::WarpConfig { interpolation: interpolation(interpolation_mode)?, border: vs_register::Border::Clamp };
        config.validate().map_err(|error| Error::msg(error.to_string()))?;
        let processor = WarpProcessor { buffers: Buffers::new(), translation: translation(x, y)?, config };
        Ok(Some(Box::new(UnaryFilter::new(clip, processor))))
    }
}
make_filter_function! {
    StackFunction, "Stack"
    fn create_stack<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, other: Node<'core>, method: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        validate_clip(&other)?;
        let processor = StackProcessor { buffers: Buffers::new(), config: vs_register::StackConfig { method: stack_method(method)? } };
        let filter = BinaryFilter::new(clip, other, processor).map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(filter)))
    }
}
make_filter_function! {
    StabilizeFunction, "Stabilize"
    fn create_stabilize<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, strength: f64, max_shift: i64, minimum_overlap: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        let config = vs_register::StabilizeConfig { radius: 1, strength: strength as f32 };
        config.validate().map_err(|error| Error::msg(error.to_string()))?;
        let estimate = estimate_config(max_shift, minimum_overlap)?;
        validate_estimate_extent(&clip, estimate)?;
        let processor = StabilizeProcessor { buffers: Buffers::new(), config, estimate };
        let filter = TemporalFilter::new(clip, 1, processor).map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(filter)))
    }
}
mod plugin_abi {
    #![allow(missing_docs, unsafe_code)]
    use super::*;
    export_vapoursynth_plugin! {Metadata{identifier:PLUGIN_IDENTIFIER,namespace:"register",name:"PlaneSight Register",read_only:true},[EstimateFunction::new(),WarpFunction::new(),StackFunction::new(),StabilizeFunction::new()]}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsampled_planes_receive_scaled_translations() {
        let full = Extent::new(8, 6).unwrap();
        let chroma = Extent::new(4, 3).unwrap();
        let scaled =
            scale_translation(vs_register::Translation { x: 2.0, y: -4.0 }, full, chroma).unwrap();
        assert_eq!(scaled, vs_register::Translation { x: 1.0, y: -2.0 });
    }

    #[test]
    fn integer_modes_are_strict() {
        assert!(matches!(
            interpolation(0),
            Ok(vs_register::Interpolation::Nearest)
        ));
        assert!(interpolation(2).is_err());
        assert!(matches!(
            stack_method(1),
            Ok(vs_register::StackMethod::Median)
        ));
        assert!(stack_method(-1).is_err());
        assert!(estimate_config(1, 0.5).is_ok());
        assert!(estimate_config(1, 0.0).is_err());
        assert!(estimate_config(1, f64::MAX).is_err());
    }
}
