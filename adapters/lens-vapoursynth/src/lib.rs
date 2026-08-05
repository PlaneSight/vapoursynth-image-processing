//! VapourSynth registration for Lens corrections.
//!
//! Focal lengths and rolling-shutter displacements use full-resolution
//! first-plane pixel units and are scaled for subsampled planes. Chromatic
//! correction is accepted only for three-plane RGB clips.

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
use vsip_vapoursynth::{FrameBuffers, UnaryFilter, UnaryFrameProcessor};
const PLUGIN_IDENTIFIER: &str = "com.planesight.vsip.lens";
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
fn area(f: &FrameRef<'_>, p: usize) -> Result<usize, Error> {
    Ok(FrameBuffers::plane_area(f, p)?)
}
fn read(b: &Buffers, f: &FrameRef<'_>, p: usize, o: &mut [f32]) -> Result<Extent, Error> {
    Ok(b.frames.read_f32(f, p, o)?)
}
fn write(b: &Buffers, i: &[f32], f: &mut FrameRefMut<'_>, p: usize) -> Result<(), Error> {
    b.frames.write_f32(i, f, p)?;
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

fn scale_x(value: f32, plane: Extent, full: Extent) -> f32 {
    (f64::from(value) * plane.width() as f64 / full.width() as f64) as f32
}

fn scale_y(value: f32, plane: Extent, full: Extent) -> f32 {
    (f64::from(value) * plane.height() as f64 / full.height() as f64) as f32
}

fn scale_shift(shift: vs_lens::ChannelShift, plane: Extent, full: Extent) -> vs_lens::ChannelShift {
    vs_lens::ChannelShift {
        x: scale_x(shift.x, plane, full),
        y: scale_y(shift.y, plane, full),
    }
}

fn unary(
    b: &Buffers,
    s: &FrameRef<'_>,
    d: &mut FrameRefMut<'_>,
    mut op: impl FnMut(Plane<'_, f32>, PlaneMut<'_, f32>, Extent, Extent) -> Result<(), Error>,
) -> Result<(), Error> {
    let full = Extent::new(s.width(0), s.height(0))?;
    for p in 0..s.format().plane_count() {
        let n = area(s, p)?;
        let mut i = b.frames.floats(n);
        let e = read(b, s, p, &mut i)?;
        let mut o = b.frames.floats(n);
        op(
            Plane::new(&i, e, e.width())?,
            PlaneMut::new(&mut o, e, e.width())?,
            e,
            full,
        )?;
        write(b, &o, d, p)?;
    }
    Ok(())
}
#[derive(Debug)]
struct Undistort {
    buffers: Buffers,
    focal_x: f32,
    focal_y: f32,
    k1: f32,
}
impl UnaryFrameProcessor for Undistort {
    fn process(&self, s: &FrameRef<'_>, d: &mut FrameRefMut<'_>) -> Result<(), Error> {
        unary(&self.buffers, s, d, |i, o, e, full| {
            let c = vs_lens::UndistortConfig {
                center_x: (e.width() - 1) as f32 * 0.5,
                center_y: (e.height() - 1) as f32 * 0.5,
                focal_x: scale_x(self.focal_x, e, full),
                focal_y: scale_y(self.focal_y, e, full),
                radial: [self.k1, 0., 0.],
                tangential_x: 0.,
                tangential_y: 0.,
                interpolation: vs_lens::Interpolation::Bilinear,
                border: vs_lens::Border::Clamp,
            };
            vs_lens::undistort(i, o, c).map_err(|x| Error::msg(x.to_string()))
        })
    }
}
#[derive(Debug)]
struct Vignette {
    buffers: Buffers,
    strength: f32,
    direction: vs_lens::VignetteDirection,
}
impl UnaryFrameProcessor for Vignette {
    fn process(&self, s: &FrameRef<'_>, d: &mut FrameRefMut<'_>) -> Result<(), Error> {
        unary(&self.buffers, s, d, |i, o, e, _| {
            let c = vs_lens::VignetteConfig {
                center_x: (e.width() - 1) as f32 * 0.5,
                center_y: (e.height() - 1) as f32 * 0.5,
                radius: e.width().max(e.height()) as f32 * 0.5,
                strength: self.strength,
                direction: self.direction,
            };
            vs_lens::vignette(i, o, c).map_err(|x| Error::msg(x.to_string()))
        })
    }
}
#[derive(Debug)]
struct Rolling {
    buffers: Buffers,
    top: vs_lens::ChannelShift,
    bottom: vs_lens::ChannelShift,
}
impl UnaryFrameProcessor for Rolling {
    fn process(&self, s: &FrameRef<'_>, d: &mut FrameRefMut<'_>) -> Result<(), Error> {
        unary(&self.buffers, s, d, |i, o, e, full| {
            vs_lens::rolling_shutter(
                i,
                o,
                vs_lens::RollingShutterConfig {
                    top: scale_shift(self.top, e, full),
                    bottom: scale_shift(self.bottom, e, full),
                    interpolation: vs_lens::Interpolation::Bilinear,
                    border: vs_lens::Border::Clamp,
                },
            )
            .map_err(|x| Error::msg(x.to_string()))
        })
    }
}
#[derive(Debug)]
struct Chromatic {
    buffers: Buffers,
    red: vs_lens::ChannelShift,
    blue: vs_lens::ChannelShift,
}
impl UnaryFrameProcessor for Chromatic {
    fn process(&self, s: &FrameRef<'_>, d: &mut FrameRefMut<'_>) -> Result<(), Error> {
        if s.format().color_family() != ColorFamily::RGB || s.format().plane_count() != 3 {
            return Err(Error::msg(
                "Chromatic requires an RGB clip with exactly three planar channels",
            ));
        }
        let n = area(s, 0)?;
        let mut r = self.buffers.frames.floats(n);
        let e = read(&self.buffers, s, 0, &mut r)?;
        let mut g = self.buffers.frames.floats(n);
        if read(&self.buffers, s, 1, &mut g)? != e {
            return Err(Error::msg("RGB plane extents differ"));
        }
        let mut b = self.buffers.frames.floats(n);
        if read(&self.buffers, s, 2, &mut b)? != e {
            return Err(Error::msg("RGB plane extents differ"));
        }
        let mut ro = self.buffers.frames.floats(n);
        let mut go = self.buffers.frames.floats(n);
        let mut bo = self.buffers.frames.floats(n);
        vs_lens::chromatic(
            Plane::new(&r, e, e.width())?,
            Plane::new(&g, e, e.width())?,
            Plane::new(&b, e, e.width())?,
            PlaneMut::new(&mut ro, e, e.width())?,
            PlaneMut::new(&mut go, e, e.width())?,
            PlaneMut::new(&mut bo, e, e.width())?,
            vs_lens::ChromaticConfig {
                red: self.red,
                blue: self.blue,
                interpolation: vs_lens::Interpolation::Bilinear,
                border: vs_lens::Border::Clamp,
            },
        )
        .map_err(|x| Error::msg(x.to_string()))?;
        write(&self.buffers, &ro, d, 0)?;
        write(&self.buffers, &go, d, 1)?;
        write(&self.buffers, &bo, d, 2)
    }
}
fn shift(x: f64, y: f64) -> Result<vs_lens::ChannelShift, Error> {
    let s = vs_lens::ChannelShift {
        x: x as f32,
        y: y as f32,
    };
    s.validate()
        .map_err(|error| Error::msg(error.to_string()))?;
    Ok(s)
}
fn vignette_direction(mode: i64) -> Result<vs_lens::VignetteDirection, Error> {
    match mode {
        0 => Ok(vs_lens::VignetteDirection::Apply),
        1 => Ok(vs_lens::VignetteDirection::Correct),
        _ => Err(Error::msg("direction must be 0 (apply) or 1 (correct)")),
    }
}
make_filter_function! {
    UndistortFunction, "Undistort"
    fn create_undistort<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, focal_x: f64, focal_y: f64, k1: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        let processor = Undistort { buffers: Buffers::new(), focal_x: focal_x as f32, focal_y: focal_y as f32, k1: k1 as f32 };
        vs_lens::UndistortConfig { center_x: 0.0, center_y: 0.0, focal_x: processor.focal_x, focal_y: processor.focal_y, radial: [processor.k1, 0.0, 0.0], tangential_x: 0.0, tangential_y: 0.0, interpolation: vs_lens::Interpolation::Bilinear, border: vs_lens::Border::Clamp }.validate().map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, processor))))
    }
}
make_filter_function! {
    ChromaticFunction, "Chromatic"
    fn create_chromatic<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, red_x: f64, red_y: f64, blue_x: f64, blue_y: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        if clip.info().format.color_family() != ColorFamily::RGB || clip.info().format.plane_count() != 3 { return Err(Error::msg("Chromatic requires an RGB clip with exactly three planar channels")); }
        let processor = Chromatic { buffers: Buffers::new(), red: shift(red_x, red_y)?, blue: shift(blue_x, blue_y)? };
        vs_lens::ChromaticConfig { red: processor.red, blue: processor.blue, interpolation: vs_lens::Interpolation::Bilinear, border: vs_lens::Border::Clamp }.validate().map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, processor))))
    }
}
make_filter_function! {
    VignetteFunction, "Vignette"
    fn create_vignette<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, strength: f64, direction: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        let processor = Vignette { buffers: Buffers::new(), strength: strength as f32, direction: vignette_direction(direction)? };
        vs_lens::VignetteConfig { center_x: 0.0, center_y: 0.0, radius: 1.0, strength: processor.strength, direction: processor.direction }.validate().map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, processor))))
    }
}
make_filter_function! {
    RollingShutterFunction, "RollingShutter"
    fn create_rolling_shutter<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, top_x: f64, top_y: f64, bottom_x: f64, bottom_y: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        let processor = Rolling { buffers: Buffers::new(), top: shift(top_x, top_y)?, bottom: shift(bottom_x, bottom_y)? };
        vs_lens::RollingShutterConfig { top: processor.top, bottom: processor.bottom, interpolation: vs_lens::Interpolation::Bilinear, border: vs_lens::Border::Clamp }.validate().map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, processor))))
    }
}
mod plugin_abi {
    #![allow(missing_docs, unsafe_code)]
    use super::*;
    export_vapoursynth_plugin! {Metadata{identifier:PLUGIN_IDENTIFIER,namespace:"lens",name:"PlaneSight Lens",read_only:true},[UndistortFunction::new(),ChromaticFunction::new(),VignetteFunction::new(),RollingShutterFunction::new()]}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsampled_planes_receive_scaled_geometry() {
        let full = Extent::new(8, 6).unwrap();
        let chroma = Extent::new(4, 3).unwrap();
        assert_eq!(scale_x(4.0, chroma, full), 2.0);
        assert_eq!(scale_y(2.0, chroma, full), 1.0);
        assert_eq!(
            scale_shift(vs_lens::ChannelShift { x: 2.0, y: -4.0 }, chroma, full),
            vs_lens::ChannelShift { x: 1.0, y: -2.0 }
        );
    }

    #[test]
    fn direction_and_shift_parsing_are_strict() {
        assert!(matches!(
            vignette_direction(0),
            Ok(vs_lens::VignetteDirection::Apply)
        ));
        assert!(vignette_direction(2).is_err());
        assert!(shift(f64::MAX, 0.0).is_err());
    }
}
