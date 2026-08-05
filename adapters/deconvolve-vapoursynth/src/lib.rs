//! VapourSynth registration for Deconvolve scalar reference filters.

#[macro_use]
extern crate vapoursynth;

use std::{
    ops::{Deref, DerefMut},
    sync::Mutex,
};

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
use vsip_vapoursynth::{BinaryFilter, BinaryFrameProcessor, FrameBuffers};

const PLUGIN_IDENTIFIER: &str = "com.planesight.vsip.deconvolve";

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

#[derive(Debug)]
struct KernelScratchPool<T> {
    retained_limit: usize,
    buffers: Mutex<Vec<T>>,
}

impl<T> KernelScratchPool<T> {
    fn new(retained_limit: usize) -> Result<Self, Error> {
        if retained_limit == 0 {
            return Err(Error::msg("scratch retention must be positive"));
        }
        let mut buffers = Vec::new();
        buffers
            .try_reserve_exact(retained_limit)
            .map_err(|_| Error::msg("scratch-cache allocation failed"))?;
        Ok(Self {
            retained_limit,
            buffers: Mutex::new(buffers),
        })
    }

    fn acquire(
        &self,
        matches: impl Fn(&T) -> bool,
        create: impl FnOnce() -> Result<T, Error>,
    ) -> Result<KernelScratchLease<'_, T>, Error> {
        let cached = {
            let mut buffers = self
                .buffers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            buffers
                .iter()
                .position(matches)
                .map(|index| buffers.swap_remove(index))
        };
        Ok(KernelScratchLease {
            pool: self,
            scratch: Some(match cached {
                Some(scratch) => scratch,
                None => create()?,
            }),
        })
    }
}

#[derive(Debug)]
struct KernelScratchLease<'pool, T> {
    pool: &'pool KernelScratchPool<T>,
    scratch: Option<T>,
}

impl<T> Deref for KernelScratchLease<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.scratch.as_ref().expect("lease owns scratch")
    }
}

impl<T> DerefMut for KernelScratchLease<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.scratch.as_mut().expect("lease owns scratch")
    }
}

impl<T> Drop for KernelScratchLease<'_, T> {
    fn drop(&mut self) {
        let Some(scratch) = self.scratch.take() else {
            return;
        };
        let mut buffers = self
            .pool
            .buffers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if buffers.len() < self.pool.retained_limit {
            buffers.push(scratch);
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
            "Deconvolve adapters support only u8, u16, and 32-bit floating-point clips",
        )),
    }
}

fn binary(
    buffers: &Buffers,
    observed: &FrameRef<'_>,
    psf: &FrameRef<'_>,
    destination: &mut FrameRefMut<'_>,
    mut operation: impl FnMut(
        Plane<'_, f32>,
        Plane<'_, f32>,
        PlaneMut<'_, f32>,
        Extent,
    ) -> Result<(), Error>,
) -> Result<(), Error> {
    for plane in 0..observed.format().plane_count() {
        let length = FrameBuffers::plane_area(observed, plane)?;
        let mut input = buffers.frames.floats(length);
        let extent = buffers.frames.read_f32(observed, plane, &mut input)?;
        let mut point_spread = buffers.frames.floats(length);
        if buffers.frames.read_f32(psf, plane, &mut point_spread)? != extent {
            return Err(Error::msg("observed and PSF plane extents differ"));
        }
        let mut output = buffers.frames.floats(length);
        operation(
            Plane::new(&input, extent, extent.width())?,
            Plane::new(&point_spread, extent, extent.width())?,
            PlaneMut::new(&mut output, extent, extent.width())?,
            extent,
        )?;
        buffers.frames.write_f32(&output, destination, plane)?;
    }
    Ok(())
}

#[derive(Debug)]
struct WienerProcessor {
    buffers: Buffers,
    scratch: KernelScratchPool<vs_deconvolve::WienerScratch>,
    config: vs_deconvolve::WienerConfig,
}

impl BinaryFrameProcessor for WienerProcessor {
    fn process(
        &self,
        observed: &FrameRef<'_>,
        psf: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        binary(
            &self.buffers,
            observed,
            psf,
            destination,
            |input, psf, output, extent| {
                let mut scratch = self.scratch.acquire(
                    |scratch| scratch.extent() == extent,
                    || vs_deconvolve::WienerScratch::try_new(extent).map_err(kernel_error),
                )?;
                vs_deconvolve::wiener(input, psf, output, &mut scratch, self.config)
                    .map_err(kernel_error)
            },
        )
    }
}

#[derive(Debug)]
struct RichardsonLucyProcessor {
    buffers: Buffers,
    scratch: KernelScratchPool<vs_deconvolve::RichardsonLucyScratch>,
    config: vs_deconvolve::RichardsonLucyConfig,
}

impl BinaryFrameProcessor for RichardsonLucyProcessor {
    fn process(
        &self,
        observed: &FrameRef<'_>,
        psf: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        binary(
            &self.buffers,
            observed,
            psf,
            destination,
            |input, psf, output, extent| {
                let mut scratch = self.scratch.acquire(
                    |scratch| scratch.extent() == extent,
                    || vs_deconvolve::RichardsonLucyScratch::try_new(extent).map_err(kernel_error),
                )?;
                vs_deconvolve::richardson_lucy(input, psf, output, &mut scratch, self.config)
                    .map_err(kernel_error)
            },
        )
    }
}

#[derive(Debug)]
struct RegularizedProcessor {
    buffers: Buffers,
    scratch: KernelScratchPool<vs_deconvolve::RegularizedScratch>,
    config: vs_deconvolve::RegularizedConfig,
}

impl BinaryFrameProcessor for RegularizedProcessor {
    fn process(
        &self,
        observed: &FrameRef<'_>,
        psf: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        binary(
            &self.buffers,
            observed,
            psf,
            destination,
            |input, psf, output, extent| {
                let mut scratch = self.scratch.acquire(
                    |scratch| scratch.extent() == extent,
                    || vs_deconvolve::RegularizedScratch::try_new(extent).map_err(kernel_error),
                )?;
                vs_deconvolve::regularized(input, psf, output, &mut scratch, self.config)
                    .map_err(kernel_error)
            },
        )
    }
}

#[derive(Debug)]
struct EstimatePsfProcessor {
    buffers: Buffers,
    config: vs_deconvolve::PsfEstimateConfig,
}

impl BinaryFrameProcessor for EstimatePsfProcessor {
    fn process(
        &self,
        observed: &FrameRef<'_>,
        reference: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error> {
        binary(
            &self.buffers,
            observed,
            reference,
            destination,
            |input, reference, output, _| {
                vs_deconvolve::estimate_psf(input, reference, output, self.config)
                    .map_err(kernel_error)
            },
        )
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

fn non_negative(value: i64, name: &str) -> Result<usize, Error> {
    usize::try_from(value).map_err(|_| Error::msg(format!("{name} must be non-negative")))
}

make_filter_function! { WienerFunction, "Wiener" fn create_wiener<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, psf: Node<'core>, noise_to_signal: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> { require_supported(&clip)?; require_supported(&psf)?; let config = vs_deconvolve::WienerConfig::new(noise_to_signal as f32).map_err(kernel_error)?; Ok(Some(Box::new(BinaryFilter::new(clip, psf, WienerProcessor { buffers: Buffers::new(), scratch: KernelScratchPool::new(8)?, config }).map_err(kernel_error)?))) } }
make_filter_function! { RichardsonLucyFunction, "RichardsonLucy" fn create_richardson_lucy<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, psf: Node<'core>, iterations: i64, epsilon: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> { require_supported(&clip)?; require_supported(&psf)?; let config = vs_deconvolve::RichardsonLucyConfig::new(positive(iterations, "iterations")?, epsilon as f32).map_err(kernel_error)?; Ok(Some(Box::new(BinaryFilter::new(clip, psf, RichardsonLucyProcessor { buffers: Buffers::new(), scratch: KernelScratchPool::new(8)?, config }).map_err(kernel_error)?))) } }
make_filter_function! { RegularizedFunction, "Regularized" fn create_regularized<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, psf: Node<'core>, iterations: i64, data_step: f64, regularization: f64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> { require_supported(&clip)?; require_supported(&psf)?; let config = vs_deconvolve::RegularizedConfig::new(positive(iterations, "iterations")?, data_step as f32, regularization as f32, 1.0e-3).map_err(kernel_error)?; Ok(Some(Box::new(BinaryFilter::new(clip, psf, RegularizedProcessor { buffers: Buffers::new(), scratch: KernelScratchPool::new(8)?, config }).map_err(kernel_error)?))) } }
make_filter_function! { EstimatePsfFunction, "EstimatePsf" fn create_estimate_psf<'core>(_api: API, _core: CoreRef<'core>, clip: Node<'core>, reference: Node<'core>, max_shift: i64) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> { require_supported(&clip)?; require_supported(&reference)?; let config = vs_deconvolve::PsfEstimateConfig::new(non_negative(max_shift, "max_shift")?).map_err(kernel_error)?; Ok(Some(Box::new(BinaryFilter::new(clip, reference, EstimatePsfProcessor { buffers: Buffers::new(), config }).map_err(kernel_error)?))) } }

mod plugin_abi {
    #![allow(missing_docs)]

    use super::*;

    export_vapoursynth_plugin! { Metadata { identifier: PLUGIN_IDENTIFIER, namespace: "deconvolve", name: "PlaneSight Deconvolve", read_only: true }, [WienerFunction::new(), RichardsonLucyFunction::new(), RegularizedFunction::new(), EstimatePsfFunction::new()] }
}
