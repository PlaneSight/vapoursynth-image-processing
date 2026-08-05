//! VapourSynth registration for ToneLab filters.
//!
//! Intensity parameters use the clip's native numeric sample scale. CLAHE tile
//! counts describe the full-resolution first plane and are scaled to preserve
//! approximately the same physical tile size on subsampled planes.

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
use vsip_vapoursynth::{
    BinaryFilter, BinaryFrameProcessor, FrameBuffers, UnaryFilter, UnaryFrameProcessor,
};
const PLUGIN_IDENTIFIER: &str = "com.planesight.vsip.tonelab";
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

fn unary(
    b: &Buffers,
    s: &FrameRef<'_>,
    d: &mut FrameRefMut<'_>,
    mut op: impl FnMut(Plane<'_, f32>, PlaneMut<'_, f32>, &mut [f32], Extent) -> Result<(), Error>,
) -> Result<(), Error> {
    for p in 0..s.format().plane_count() {
        let n = area(s, p)?;
        let mut i = b.frames.floats(n);
        let e = read(b, s, p, &mut i)?;
        let mut o = b.frames.floats(n);
        let mut scratch = b.frames.floats(n);
        op(
            Plane::new(&i, e, e.width())?,
            PlaneMut::new(&mut o, e, e.width())?,
            &mut scratch,
            e,
        )?;
        write(b, &o, d, p)?;
    }
    Ok(())
}
fn binary(
    b: &Buffers,
    a: &FrameRef<'_>,
    c: &FrameRef<'_>,
    d: &mut FrameRefMut<'_>,
    mut op: impl FnMut(
        Plane<'_, f32>,
        Plane<'_, f32>,
        PlaneMut<'_, f32>,
        &mut [f32],
        Extent,
    ) -> Result<(), Error>,
) -> Result<(), Error> {
    for p in 0..a.format().plane_count() {
        let n = area(a, p)?;
        let mut first = b.frames.floats(n);
        let e = read(b, a, p, &mut first)?;
        let mut second = b.frames.floats(n);
        if read(b, c, p, &mut second)? != e {
            return Err(Error::msg("input plane extents differ"));
        }
        let mut output = b.frames.floats(n);
        let mut scratch = b.frames.floats(2);
        op(
            Plane::new(&first, e, e.width())?,
            Plane::new(&second, e, e.width())?,
            PlaneMut::new(&mut output, e, e.width())?,
            &mut scratch,
            e,
        )?;
        write(b, &output, d, p)?;
    }
    Ok(())
}
#[derive(Debug)]
struct Clahe {
    buffers: Buffers,
    config: vs_tonelab::ClaheConfig,
}
impl UnaryFrameProcessor for Clahe {
    fn process(&self, s: &FrameRef<'_>, d: &mut FrameRefMut<'_>) -> Result<(), Error> {
        let full = Extent::new(s.width(0), s.height(0))?;
        for p in 0..s.format().plane_count() {
            let n = area(s, p)?;
            let mut input = self.buffers.frames.floats(n);
            let e = read(&self.buffers, s, p, &mut input)?;
            let mut out = self.buffers.frames.floats(n);
            let config = vs_tonelab::ClaheConfig {
                tiles_x: scaled_count(self.config.tiles_x, e.width(), full.width()),
                tiles_y: scaled_count(self.config.tiles_y, e.height(), full.height()),
                ..self.config
            };
            let required = config.validate(e).map_err(|x| Error::msg(x.to_string()))?;
            let mut histogram = self.buffers.frames.integers(required);
            vs_tonelab::clahe(
                Plane::new(&input, e, e.width())?,
                PlaneMut::new(&mut out, e, e.width())?,
                &mut histogram,
                config,
            )
            .map_err(|x| Error::msg(x.to_string()))?;
            write(&self.buffers, &out, d, p)?;
        }
        Ok(())
    }
}
#[derive(Debug)]
struct Local {
    buffers: Buffers,
    config: vs_tonelab::LocalLaplacianConfig,
}
impl UnaryFrameProcessor for Local {
    fn process(&self, s: &FrameRef<'_>, d: &mut FrameRefMut<'_>) -> Result<(), Error> {
        unary(&self.buffers, s, d, |i, o, scratch, e| {
            vs_tonelab::local_laplacian(i, o, PlaneMut::new(scratch, e, e.width())?, self.config)
                .map_err(|x| Error::msg(x.to_string()))
        })
    }
}
#[derive(Debug)]
struct Normalize {
    buffers: Buffers,
    config: vs_tonelab::NormalizeIlluminationConfig,
}
impl UnaryFrameProcessor for Normalize {
    fn process(&self, s: &FrameRef<'_>, d: &mut FrameRefMut<'_>) -> Result<(), Error> {
        unary(&self.buffers, s, d, |i, o, scratch, e| {
            vs_tonelab::normalize_illumination(
                i,
                o,
                PlaneMut::new(scratch, e, e.width())?,
                self.config,
            )
            .map_err(|x| Error::msg(x.to_string()))
        })
    }
}
#[derive(Debug)]
struct Fusion {
    buffers: Buffers,
    config: vs_tonelab::ExposureFusionConfig,
}
impl BinaryFrameProcessor for Fusion {
    fn process(
        &self,
        a: &FrameRef<'_>,
        b: &FrameRef<'_>,
        d: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        binary(&self.buffers, a, b, d, |first, second, out, scratch, _| {
            vs_tonelab::exposure_fusion(&[first, second], out, scratch, self.config)
                .map_err(|x| Error::msg(x.to_string()))
        })
    }
}
fn range(minimum: f64, maximum: f64) -> Result<vs_tonelab::SampleRange, Error> {
    let r = vs_tonelab::SampleRange {
        minimum: minimum as f32,
        maximum: maximum as f32,
    };
    r.validate().map_err(|x| Error::msg(x.to_string()))?;
    Ok(r)
}
fn scaled_count(count: usize, plane_length: usize, full_length: usize) -> usize {
    let scaled = (count as u128 * plane_length as u128).div_ceil(full_length as u128);
    scaled.clamp(1, plane_length as u128) as usize
}
make_filter_function! {
    ClaheFunction, "Clahe"
    #[allow(clippy::too_many_arguments)]
    fn create_clahe<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, tiles_x: i64, tiles_y: i64, bins: i64, clip_limit: i64, minimum: f64, maximum: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        let config = vs_tonelab::ClaheConfig {
            tiles_x: usize::try_from(tiles_x).map_err(|_| Error::msg("tiles_x must be positive"))?,
            tiles_y: usize::try_from(tiles_y).map_err(|_| Error::msg("tiles_y must be positive"))?,
            bins: usize::try_from(bins).map_err(|_| Error::msg("bins must be positive"))?,
            clip_limit: u32::try_from(clip_limit).map_err(|_| Error::msg("clip_limit must fit u32"))?,
            range: range(minimum, maximum)?,
        };
        config.scratch_len().map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, Clahe { buffers: Buffers::new(), config }))))
    }
}
make_filter_function! {
    LocalLaplacianFunction, "LocalLaplacian"
    fn create_local<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, radius: i64, detail: f64, edge_threshold: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        let config = vs_tonelab::LocalLaplacianConfig { radius: usize::try_from(radius).map_err(|_| Error::msg("radius must be positive"))?, detail: detail as f32, edge_threshold: edge_threshold as f32 };
        config.validate().map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, Local { buffers: Buffers::new(), config }))))
    }
}
make_filter_function! {
    ExposureFusionFunction, "ExposureFusion"
    fn create_fusion<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, other: Node<'core>, midpoint: f64, sigma: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        validate_clip(&other)?;
        let config = vs_tonelab::ExposureFusionConfig { midpoint: midpoint as f32, sigma: sigma as f32 };
        config.validate().map_err(|error| Error::msg(error.to_string()))?;
        let filter = BinaryFilter::new(clip, other, Fusion { buffers: Buffers::new(), config }).map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(filter)))
    }
}
make_filter_function! {
    NormalizeIlluminationFunction, "NormalizeIllumination"
    fn create_normalize<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, radius: i64, target: f64, floor: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
        validate_clip(&clip)?;
        let config = vs_tonelab::NormalizeIlluminationConfig { radius: usize::try_from(radius).map_err(|_| Error::msg("radius must be positive"))?, target: target as f32, floor: floor as f32 };
        config.validate().map_err(|error| Error::msg(error.to_string()))?;
        Ok(Some(Box::new(UnaryFilter::new(clip, Normalize { buffers: Buffers::new(), config }))))
    }
}
mod plugin_abi {
    #![allow(missing_docs, unsafe_code)]
    use super::*;
    export_vapoursynth_plugin! {Metadata{identifier:PLUGIN_IDENTIFIER,namespace:"tonelab",name:"PlaneSight ToneLab",read_only:true},[ClaheFunction::new(),LocalLaplacianFunction::new(),ExposureFusionFunction::new(),NormalizeIlluminationFunction::new()]}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_counts_scale_to_subsampled_plane_geometry() {
        assert_eq!(scaled_count(8, 1920, 1920), 8);
        assert_eq!(scaled_count(8, 960, 1920), 4);
        assert_eq!(scaled_count(1, 1, 1920), 1);
    }

    #[test]
    fn sample_ranges_reject_lossy_non_finite_conversion() {
        assert!(range(0.0, 1.0).is_ok());
        assert!(range(0.0, f64::MAX).is_err());
    }
}
