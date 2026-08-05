//! Distance-transform reference kernels.

use core::fmt;
use vsip_core::{Extent, Plane, PlaneMut};

/// Distance-transform failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DistanceError {
    /// Input and output extents differ.
    ExtentMismatch,
    /// The largest possible distance is not representable by u32.
    DistanceOverflow,
}

impl fmt::Display for DistanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExtentMismatch => "input and output extents differ",
            Self::DistanceOverflow => "distance cannot be represented by u32",
        })
    }
}

impl std::error::Error for DistanceError {}

/// Computes Manhattan distance to the nearest zero-valued mask pixel.
///
/// The output is caller-owned and may be reused between frames. The algorithm
/// performs two linear passes and allocates no memory. When the mask contains
/// no zero pixel, all output values are set to width + height.
pub fn l1_to_zero(
    mask: Plane<'_, u8>,
    mut output: PlaneMut<'_, u32>,
) -> Result<(), DistanceError> {
    let extent = mask.extent();
    if output.extent() != extent {
        return Err(DistanceError::ExtentMismatch);
    }
    let infinity = maximum_distance(extent)?;

    for (source, destination) in mask.rows().zip(output.rows_mut()) {
        for (&pixel, distance) in source.iter().zip(destination) {
            *distance = if pixel == 0 { 0 } else { infinity };
        }
    }

    forward_pass(&mut output);
    backward_pass(&mut output);
    Ok(())
}

fn maximum_distance(extent: Extent) -> Result<u32, DistanceError> {
    extent
        .width()
        .checked_add(extent.height())
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(DistanceError::DistanceOverflow)
}

fn forward_pass(output: &mut PlaneMut<'_, u32>) {
    let extent = output.extent();
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            let from_left = if x == 0 {
                u32::MAX
            } else {
                output.row_mut(y).expect("validated row")[x - 1]
            };
            let from_above = if y == 0 {
                u32::MAX
            } else {
                output.row_mut(y - 1).expect("validated row")[x]
            };
            let candidate = from_left.min(from_above).saturating_add(1);
            let pixel = &mut output.row_mut(y).expect("validated row")[x];
            *pixel = (*pixel).min(candidate);
        }
    }
}

fn backward_pass(output: &mut PlaneMut<'_, u32>) {
    let extent = output.extent();
    for y in (0..extent.height()).rev() {
        for x in (0..extent.width()).rev() {
            let from_right = if x + 1 == extent.width() {
                u32::MAX
            } else {
                output.row_mut(y).expect("validated row")[x + 1]
            };
            let from_below = if y + 1 == extent.height() {
                u32::MAX
            } else {
                output.row_mut(y + 1).expect("validated row")[x]
            };
            let candidate = from_right.min(from_below).saturating_add(1);
            let pixel = &mut output.row_mut(y).expect("validated row")[x];
            *pixel = (*pixel).min(candidate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::l1_to_zero;
    use vsip_core::{Extent, Plane, PlaneMut};

    #[test]
    fn computes_distance_around_centre() {
        let extent = Extent::new(3, 3).expect("valid extent");
        let mask = [1, 1, 1, 1, 0, 1, 1, 1, 1];
        let mut output = [0_u32; 9];
        l1_to_zero(
            Plane::new(&mask, extent, 3).expect("valid input"),
            PlaneMut::new(&mut output, extent, 3).expect("valid output"),
        )
        .expect("distance transform succeeds");
        assert_eq!(output, [2, 1, 2, 1, 0, 1, 2, 1, 2]);
    }

    #[test]
    fn respects_padding() {
        let extent = Extent::new(2, 2).expect("valid extent");
        let mask = [0, 1, 99, 1, 1];
        let mut output = [0_u32; 5];
        l1_to_zero(
            Plane::new(&mask, extent, 3).expect("valid input"),
            PlaneMut::new(&mut output, extent, 3).expect("valid output"),
        )
        .expect("distance transform succeeds");
        assert_eq!(output, [0, 1, 0, 1, 2]);
    }

    #[test]
    fn represents_missing_background_explicitly() {
        let extent = Extent::new(2, 2).expect("valid extent");
        let mask = [1, 1, 1, 1];
        let mut output = [0_u32; 4];
        l1_to_zero(
            Plane::new(&mask, extent, 2).expect("valid input"),
            PlaneMut::new(&mut output, extent, 2).expect("valid output"),
        )
        .expect("distance transform succeeds");
        assert_eq!(output, [4; 4]);
    }
}

