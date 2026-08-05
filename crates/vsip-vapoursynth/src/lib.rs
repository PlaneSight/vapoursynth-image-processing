//! Safe, reusable adapters between VapourSynth frames and VSIP kernels.
//!
//! The upstream `vapoursynth` crate owns the raw ABI. This crate keeps frame
//! scheduling, format validation, tightly packed scratch storage, and plugin
//! algorithm dispatch out of individual pixel kernels.

use core::fmt;
use std::{
    ops::{Deref, DerefMut},
    sync::{Mutex, MutexGuard},
};

use vapoursynth::{
    anyhow::{Error, anyhow},
    component::Component,
    core::CoreRef,
    format::SampleType,
    frame::{FrameRef, FrameRefMut},
    node::Node,
    plugins::{Filter, FrameContext},
    prelude::API,
    video_info::VideoInfo,
};
use vsip_core::Extent;

/// Re-export of the pinned upstream API used by adapter crates.
pub use vapoursynth;

/// Failure while adapting a VapourSynth frame plane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameAdapterError {
    /// The requested plane does not exist.
    PlaneOutOfRange,
    /// The VapourSynth component type does not match `T`.
    ComponentMismatch,
    /// Plane geometry was not representable by the shared image model.
    InvalidGeometry,
    /// The caller-owned tight buffer has the wrong element count.
    BufferLengthMismatch,
    /// The frame uses a component type not supported by the f32 bridge.
    UnsupportedComponent,
    /// A floating-point result cannot be represented by an integer frame.
    NonFiniteIntegerOutput,
}

impl fmt::Display for FrameAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::PlaneOutOfRange => "frame plane is out of range",
            Self::ComponentMismatch => "frame component type does not match the requested type",
            Self::InvalidGeometry => "frame plane geometry is invalid or overflowed",
            Self::BufferLengthMismatch => "tight plane buffer length does not match its extent",
            Self::UnsupportedComponent => {
                "only u8, u16, and single-precision f32 frame planes are supported"
            }
            Self::NonFiniteIntegerOutput => {
                "non-finite output cannot be written to an integer frame"
            }
        })
    }
}

impl std::error::Error for FrameAdapterError {}

/// Copies one visible frame plane into caller-owned tightly packed storage.
///
/// This is the explicit copy boundary used when the upstream safe API exposes
/// a padded frame only as independent rows. `storage` must contain exactly one
/// element per visible pixel.
pub fn copy_plane_to<T>(
    frame: &FrameRef<'_>,
    plane: usize,
    storage: &mut [T],
) -> Result<Extent, FrameAdapterError>
where
    T: Component + Copy,
{
    validate_plane::<T>(frame, plane, storage.len())?;
    let extent = Extent::new(frame.width(plane), frame.height(plane))
        .map_err(|_| FrameAdapterError::InvalidGeometry)?;

    for (source, destination) in (0..extent.height())
        .map(|row| frame.plane_row::<T>(plane, row))
        .zip(storage.chunks_exact_mut(extent.width()))
    {
        destination.copy_from_slice(source);
    }
    Ok(extent)
}

/// Copies tightly packed caller-owned pixels into one visible frame plane.
pub fn copy_plane_from<T>(
    storage: &[T],
    frame: &mut FrameRefMut<'_>,
    plane: usize,
) -> Result<Extent, FrameAdapterError>
where
    T: Component + Copy,
{
    validate_plane::<T>(frame, plane, storage.len())?;
    let extent = Extent::new(frame.width(plane), frame.height(plane))
        .map_err(|_| FrameAdapterError::InvalidGeometry)?;

    for (source, row) in storage.chunks_exact(extent.width()).zip(0..extent.height()) {
        frame.plane_row_mut::<T>(plane, row).copy_from_slice(source);
    }
    Ok(extent)
}

fn validate_plane<T>(
    frame: &vapoursynth::frame::Frame<'_>,
    plane: usize,
    storage_len: usize,
) -> Result<(), FrameAdapterError>
where
    T: Component,
{
    if plane >= frame.format().plane_count() {
        return Err(FrameAdapterError::PlaneOutOfRange);
    }
    if !T::is_valid(frame.format()) {
        return Err(FrameAdapterError::ComponentMismatch);
    }
    let area = frame
        .width(plane)
        .checked_mul(frame.height(plane))
        .ok_or(FrameAdapterError::InvalidGeometry)?;
    if storage_len != area {
        return Err(FrameAdapterError::BufferLengthMismatch);
    }
    Ok(())
}

/// Bounded pool of reusable caller-owned scratch vectors.
///
/// A lease owns its vector while a frame is executing, so parallel callbacks
/// never alias scratch memory or hold the pool mutex during kernel work.
#[derive(Debug)]
pub struct ScratchPool<T> {
    retained_limit: usize,
    buffers: Mutex<Vec<Vec<T>>>,
}

impl<T> ScratchPool<T> {
    /// Creates a pool retaining at most `retained_limit` idle vectors.
    pub fn new(retained_limit: usize) -> Result<Self, ScratchPoolError> {
        if retained_limit == 0 {
            return Err(ScratchPoolError::ZeroRetention);
        }
        Ok(Self {
            retained_limit,
            buffers: Mutex::new(Vec::new()),
        })
    }

    fn buffers(&self) -> MutexGuard<'_, Vec<Vec<T>>> {
        self.buffers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<T> ScratchPool<T>
where
    T: Clone,
{
    /// Acquires a vector of exactly `length` elements, filling new elements
    /// with `value` and reusing retained capacity when available.
    pub fn acquire(&self, length: usize, value: T) -> ScratchLease<'_, T> {
        let mut buffer = self.buffers().pop().unwrap_or_default();
        buffer.resize(length, value);
        ScratchLease {
            pool: self,
            buffer: Some(buffer),
        }
    }
}

/// Invalid scratch-pool configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScratchPoolError {
    /// A zero retention limit would make pooling misleading.
    ZeroRetention,
}

impl fmt::Display for ScratchPoolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("scratch-pool retention limit must be positive")
    }
}

impl std::error::Error for ScratchPoolError {}

/// Exclusive scratch-vector lease returned by [`ScratchPool::acquire`].
#[derive(Debug)]
pub struct ScratchLease<'pool, T> {
    pool: &'pool ScratchPool<T>,
    buffer: Option<Vec<T>>,
}

impl<T> Deref for ScratchLease<'_, T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.buffer.as_deref().expect("lease always owns a buffer")
    }
}

impl<T> DerefMut for ScratchLease<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buffer
            .as_deref_mut()
            .expect("lease always owns a buffer")
    }
}

impl<T> Drop for ScratchLease<'_, T> {
    fn drop(&mut self) {
        let Some(mut buffer) = self.buffer.take() else {
            return;
        };
        buffer.clear();
        let mut buffers = self.pool.buffers();
        if buffers.len() < self.pool.retained_limit {
            buffers.push(buffer);
        }
    }
}

/// Shared scratch and numeric-conversion policy for runtime adapters.
///
/// Integer samples retain their native numeric scale when converted to f32;
/// adapter parameters therefore use the clip's native sample range. Integer
/// output is rounded and clamped to the format's declared bit depth. A
/// non-finite f32 result is rejected rather than silently converted to zero.
#[derive(Debug)]
pub struct FrameBuffers {
    floats: ScratchPool<f32>,
    bytes: ScratchPool<u8>,
    words: ScratchPool<u16>,
    integers: ScratchPool<u32>,
}

impl FrameBuffers {
    /// Creates buffers retaining at most `retained_per_type` idle vectors of
    /// each supported component type.
    pub fn new(retained_per_type: usize) -> Result<Self, ScratchPoolError> {
        Ok(Self {
            floats: ScratchPool::new(retained_per_type)?,
            bytes: ScratchPool::new(retained_per_type)?,
            words: ScratchPool::new(retained_per_type)?,
            integers: ScratchPool::new(retained_per_type)?,
        })
    }

    /// Returns the visible element count for one plane.
    pub fn plane_area(
        frame: &vapoursynth::frame::Frame<'_>,
        plane: usize,
    ) -> Result<usize, FrameAdapterError> {
        if plane >= frame.format().plane_count() {
            return Err(FrameAdapterError::PlaneOutOfRange);
        }
        frame
            .width(plane)
            .checked_mul(frame.height(plane))
            .ok_or(FrameAdapterError::InvalidGeometry)
    }

    /// Acquires reusable single-precision scratch.
    pub fn floats(&self, length: usize) -> ScratchLease<'_, f32> {
        self.floats.acquire(length, 0.0)
    }

    /// Acquires reusable u32 scratch.
    pub fn integers(&self, length: usize) -> ScratchLease<'_, u32> {
        self.integers.acquire(length, 0)
    }

    /// Copies a supported frame plane to tightly packed f32 storage.
    pub fn read_f32(
        &self,
        frame: &FrameRef<'_>,
        plane: usize,
        output: &mut [f32],
    ) -> Result<Extent, FrameAdapterError> {
        match (
            frame.format().sample_type(),
            frame.format().bytes_per_sample(),
        ) {
            (SampleType::Integer, 1) => {
                let mut storage = self.bytes.acquire(output.len(), 0);
                let extent = copy_plane_to(frame, plane, &mut storage)?;
                output
                    .iter_mut()
                    .zip(storage.iter())
                    .for_each(|(destination, &source)| *destination = f32::from(source));
                Ok(extent)
            }
            (SampleType::Integer, 2) => {
                let mut storage = self.words.acquire(output.len(), 0);
                let extent = copy_plane_to(frame, plane, &mut storage)?;
                output
                    .iter_mut()
                    .zip(storage.iter())
                    .for_each(|(destination, &source)| *destination = f32::from(source));
                Ok(extent)
            }
            (SampleType::Float, 4) => copy_plane_to(frame, plane, output),
            _ => Err(FrameAdapterError::UnsupportedComponent),
        }
    }

    /// Copies tightly packed f32 storage into a supported frame plane.
    pub fn write_f32(
        &self,
        input: &[f32],
        frame: &mut FrameRefMut<'_>,
        plane: usize,
    ) -> Result<Extent, FrameAdapterError> {
        match (
            frame.format().sample_type(),
            frame.format().bytes_per_sample(),
        ) {
            (SampleType::Integer, 1) => {
                let maximum = ((1_u32 << frame.format().bits_per_sample()) - 1) as f32;
                let mut storage = self.bytes.acquire(input.len(), 0);
                convert_integer_output(input, &mut storage, maximum, |value| value as u8)?;
                copy_plane_from(&storage, frame, plane)
            }
            (SampleType::Integer, 2) => {
                let maximum = ((1_u32 << frame.format().bits_per_sample()) - 1) as f32;
                let mut storage = self.words.acquire(input.len(), 0);
                convert_integer_output(input, &mut storage, maximum, |value| value as u16)?;
                copy_plane_from(&storage, frame, plane)
            }
            (SampleType::Float, 4) => copy_plane_from(input, frame, plane),
            _ => Err(FrameAdapterError::UnsupportedComponent),
        }
    }
}

fn convert_integer_output<T>(
    input: &[f32],
    output: &mut [T],
    maximum: f32,
    convert: impl Fn(f32) -> T,
) -> Result<(), FrameAdapterError> {
    if input.iter().any(|value| !value.is_finite()) {
        return Err(FrameAdapterError::NonFiniteIntegerOutput);
    }
    for (destination, &source) in output.iter_mut().zip(input) {
        *destination = convert(source.clamp(0.0, maximum).round());
    }
    Ok(())
}

/// Processing contract for a same-format, single-input frame operation.
pub trait UnaryFrameProcessor: Send + Sync {
    /// Processes `source` into the copy-on-write `destination` frame.
    fn process(
        &self,
        source: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error>;
}

/// VapourSynth scheduling adapter for a [`UnaryFrameProcessor`].
#[derive(Debug)]
pub struct UnaryFilter<'core, P> {
    source: Node<'core>,
    processor: P,
}

impl<'core, P> UnaryFilter<'core, P> {
    /// Creates a filter that preserves the source clip's video information.
    pub const fn new(source: Node<'core>, processor: P) -> Self {
        Self { source, processor }
    }
}

impl<'core, P> Filter<'core> for UnaryFilter<'core, P>
where
    P: UnaryFrameProcessor,
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
        self.source.request_frame_filter(context, n);
        Ok(None)
    }

    fn get_frame(
        &self,
        _api: API,
        core: CoreRef<'core>,
        context: FrameContext<'_>,
        n: usize,
    ) -> Result<FrameRef<'core>, Error> {
        let source = self
            .source
            .get_frame_filter(context, n)
            .ok_or_else(|| anyhow!("source frame {n} was not available"))?;
        let mut destination = FrameRefMut::copy_of(core, &source);
        self.processor.process(&source, &mut destination)?;
        Ok(destination.into())
    }
}

/// Processing contract for a same-format, two-input frame operation.
pub trait BinaryFrameProcessor: Send + Sync {
    /// Processes corresponding frames from two compatible clips.
    fn process(
        &self,
        first: &FrameRef<'_>,
        second: &FrameRef<'_>,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error>;
}

/// Incompatibility between clips passed to a same-format adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipCompatibilityError {
    /// The clips use different pixel formats.
    FormatMismatch,
    /// The clips have different constant or variable resolution contracts.
    ResolutionMismatch,
    /// The clips expose different frame counts.
    FrameCountMismatch,
    /// The clips use different constant or variable frame-rate contracts.
    FramerateMismatch,
}

impl fmt::Display for ClipCompatibilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::FormatMismatch => "input clip formats differ",
            Self::ResolutionMismatch => "input clip resolutions differ",
            Self::FrameCountMismatch => "input clip frame counts differ",
            Self::FramerateMismatch => "input clip frame rates differ",
        })
    }
}

impl std::error::Error for ClipCompatibilityError {}

/// VapourSynth scheduling adapter for a [`BinaryFrameProcessor`].
#[derive(Debug)]
pub struct BinaryFilter<'core, P> {
    first: Node<'core>,
    second: Node<'core>,
    processor: P,
}

impl<'core, P> BinaryFilter<'core, P> {
    /// Creates a filter after validating the two clips' public video contract.
    pub fn new(
        first: Node<'core>,
        second: Node<'core>,
        processor: P,
    ) -> Result<Self, ClipCompatibilityError> {
        validate_clip_compatibility(first.info(), second.info())?;
        Ok(Self {
            first,
            second,
            processor,
        })
    }
}

fn validate_clip_compatibility(
    first: VideoInfo<'_>,
    second: VideoInfo<'_>,
) -> Result<(), ClipCompatibilityError> {
    if first.format != second.format {
        return Err(ClipCompatibilityError::FormatMismatch);
    }
    if first.resolution != second.resolution {
        return Err(ClipCompatibilityError::ResolutionMismatch);
    }
    if first.num_frames != second.num_frames {
        return Err(ClipCompatibilityError::FrameCountMismatch);
    }
    if first.framerate != second.framerate {
        return Err(ClipCompatibilityError::FramerateMismatch);
    }
    Ok(())
}

impl<'core, P> Filter<'core> for BinaryFilter<'core, P>
where
    P: BinaryFrameProcessor,
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

/// Processing contract for a bounded temporal window from one clip.
pub trait TemporalFrameProcessor: Send + Sync {
    /// Processes an ascending window whose `center` entry is the requested
    /// output frame. Boundary windows are shortened rather than duplicated.
    fn process(
        &self,
        frames: &[FrameRef<'_>],
        center: usize,
        destination: &mut FrameRefMut<'_>,
    ) -> Result<(), Error>;
}

/// Invalid temporal adapter configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemporalConfigError {
    /// A zero radius is a unary operation, not a temporal window.
    ZeroRadius,
}

impl fmt::Display for TemporalConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("temporal radius must be positive")
    }
}

impl std::error::Error for TemporalConfigError {}

/// VapourSynth scheduling adapter for a [`TemporalFrameProcessor`].
#[derive(Debug)]
pub struct TemporalFilter<'core, P> {
    source: Node<'core>,
    radius: usize,
    processor: P,
}

impl<'core, P> TemporalFilter<'core, P> {
    /// Creates a temporal filter with a strictly positive frame radius.
    pub fn new(
        source: Node<'core>,
        radius: usize,
        processor: P,
    ) -> Result<Self, TemporalConfigError> {
        if radius == 0 {
            return Err(TemporalConfigError::ZeroRadius);
        }
        Ok(Self {
            source,
            radius,
            processor,
        })
    }

    fn frame_range(&self, n: usize) -> std::ops::RangeInclusive<usize> {
        let last = self.source.info().num_frames.saturating_sub(1);
        n.saturating_sub(self.radius)..=n.saturating_add(self.radius).min(last)
    }
}

impl<'core, P> Filter<'core> for TemporalFilter<'core, P>
where
    P: TemporalFrameProcessor,
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
        let mut destination = FrameRefMut::copy_of(core, &frames[center]);
        self.processor.process(&frames, center, &mut destination)?;
        Ok(destination.into())
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameAdapterError, ScratchPool, ScratchPoolError, convert_integer_output};

    #[test]
    fn scratch_pool_reuses_capacity_without_aliasing() {
        let pool = ScratchPool::new(1).expect("positive retention");
        let capacity = {
            let mut first = pool.acquire(32, 0_u8);
            first[0] = 7;
            first.len()
        };
        let second = pool.acquire(8, 9_u8);
        assert_eq!(second.len(), 8);
        assert!(capacity >= second.len());
        assert!(second.iter().all(|&value| value == 9));
    }

    #[test]
    fn scratch_pool_rejects_zero_retention() {
        assert!(matches!(
            ScratchPool::<u8>::new(0),
            Err(ScratchPoolError::ZeroRetention)
        ));
    }

    #[test]
    fn integer_conversion_clamps_and_rounds() {
        let mut output = [0_u8; 4];
        convert_integer_output(&[-2.0, 4.4, 4.6, 300.0], &mut output, 255.0, |value| {
            value as u8
        })
        .expect("finite conversion");
        assert_eq!(output, [0, 4, 5, 255]);
    }

    #[test]
    fn integer_conversion_rejects_non_finite_values() {
        let mut output = [0_u8; 1];
        assert_eq!(
            convert_integer_output(&[f32::NAN], &mut output, 255.0, |value| value as u8),
            Err(FrameAdapterError::NonFiniteIntegerOutput)
        );
    }
}
