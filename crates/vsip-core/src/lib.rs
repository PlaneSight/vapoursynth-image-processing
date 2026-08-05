//! Validated, runtime-independent image domain types.

use core::fmt;

/// Positive two-dimensional image extent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Extent {
    width: usize,
    height: usize,
}

impl Extent {
    /// Creates a non-empty image extent.
    pub const fn new(width: usize, height: usize) -> Result<Self, GeometryError> {
        if width == 0 || height == 0 {
            return Err(GeometryError::EmptyExtent);
        }
        Ok(Self { width, height })
    }

    /// Returns the width in pixels.
    pub const fn width(self) -> usize {
        self.width
    }

    /// Returns the height in pixels.
    pub const fn height(self) -> usize {
        self.height
    }

    /// Returns the tightly packed element count.
    pub const fn area(self) -> Option<usize> {
        self.width.checked_mul(self.height)
    }
}

/// Geometry validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeometryError {
    /// Width or height was zero.
    EmptyExtent,
    /// A row stride was shorter than the visible width.
    ShortStride,
    /// The backing slice did not cover all visible rows.
    ShortBuffer,
    /// Extent arithmetic overflowed.
    Overflow,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyExtent => "image extent must be non-empty",
            Self::ShortStride => "row stride is shorter than the visible width",
            Self::ShortBuffer => "buffer does not cover the visible plane",
            Self::Overflow => "image geometry overflowed",
        })
    }
}

impl std::error::Error for GeometryError {}

/// Immutable plane view with explicit row stride.
#[derive(Clone, Copy, Debug)]
pub struct Plane<'a, T> {
    data: &'a [T],
    extent: Extent,
    stride: usize,
}

impl<'a, T> Plane<'a, T> {
    /// Validates and constructs a plane view.
    pub fn new(data: &'a [T], extent: Extent, stride: usize) -> Result<Self, GeometryError> {
        validate_buffer(data.len(), extent, stride)?;
        Ok(Self { data, extent, stride })
    }

    /// Returns the visible extent.
    pub const fn extent(self) -> Extent {
        self.extent
    }

    /// Returns visible rows without exposing row padding.
    pub fn rows(self) -> impl ExactSizeIterator<Item = &'a [T]> {
        self.data
            .chunks(self.stride)
            .take(self.extent.height)
            .map(move |row| &row[..self.extent.width])
    }

    /// Returns one visible row.
    pub fn row(self, y: usize) -> Option<&'a [T]> {
        if y >= self.extent.height {
            return None;
        }
        let start = y.checked_mul(self.stride)?;
        Some(&self.data[start..start + self.extent.width])
    }
}

/// Mutable plane view with explicit row stride.
#[derive(Debug)]
pub struct PlaneMut<'a, T> {
    data: &'a mut [T],
    extent: Extent,
    stride: usize,
}

impl<'a, T> PlaneMut<'a, T> {
    /// Validates and constructs a mutable plane view.
    pub fn new(
        data: &'a mut [T],
        extent: Extent,
        stride: usize,
    ) -> Result<Self, GeometryError> {
        validate_buffer(data.len(), extent, stride)?;
        Ok(Self { data, extent, stride })
    }

    /// Returns the visible extent.
    pub const fn extent(&self) -> Extent {
        self.extent
    }

    /// Returns visible mutable rows without exposing row padding.
    pub fn rows_mut(&mut self) -> impl ExactSizeIterator<Item = &mut [T]> {
        let width = self.extent.width;
        self.data
            .chunks_mut(self.stride)
            .take(self.extent.height)
            .map(move |row| &mut row[..width])
    }

    /// Returns one visible mutable row.
    pub fn row_mut(&mut self, y: usize) -> Option<&mut [T]> {
        if y >= self.extent.height {
            return None;
        }
        let start = y.checked_mul(self.stride)?;
        Some(&mut self.data[start..start + self.extent.width])
    }
}

fn validate_buffer(length: usize, extent: Extent, stride: usize) -> Result<(), GeometryError> {
    if stride < extent.width {
        return Err(GeometryError::ShortStride);
    }
    let preceding_rows = extent
        .height
        .checked_sub(1)
        .and_then(|rows| rows.checked_mul(stride))
        .ok_or(GeometryError::Overflow)?;
    let required = preceding_rows
        .checked_add(extent.width)
        .ok_or(GeometryError::Overflow)?;
    if length < required {
        return Err(GeometryError::ShortBuffer);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Extent, GeometryError, Plane};

    #[test]
    fn plane_hides_padding() {
        let extent = Extent::new(2, 2).expect("valid extent");
        let data = [1, 2, 99, 3, 4];
        let rows = Plane::new(&data, extent, 3)
            .expect("valid plane")
            .rows()
            .collect::<Vec<_>>();
        assert_eq!(rows, vec![&[1, 2][..], &[3, 4][..]]);
    }

    #[test]
    fn plane_rejects_short_last_row() {
        let extent = Extent::new(2, 2).expect("valid extent");
        assert!(matches!(
            Plane::new(&[0_u8; 4], extent, 3),
            Err(GeometryError::ShortBuffer)
        ));
    }
}
