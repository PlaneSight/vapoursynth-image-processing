//! Scalar reference filters for lens and sensor correction.

use core::fmt;
use vsip_core::{Extent, Plane, PlaneMut};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Errors returned by lens filters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LensError {
    /// Input or output planes have different visible extents.
    ExtentMismatch,
    /// A supplied lens model or sampling setting is invalid.
    InvalidConfiguration,
}
impl fmt::Display for LensError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ExtentMismatch => "all planes must have the same extent",
            Self::InvalidConfiguration => "lens configuration is invalid",
        })
    }
}
impl std::error::Error for LensError {}

/// Coordinate interpolation for geometric correction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Interpolation {
    /// Select the nearest source sample.
    Nearest,
    /// Blend four neighbouring source samples.
    Bilinear,
}
/// Out-of-frame source-coordinate behavior.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Border {
    /// Clamp source coordinates to the image edge.
    Clamp,
    /// Use this finite sample value beyond the image edge.
    Constant(f32),
}
impl Border {
    fn validate(self) -> Result<(), LensError> {
        match self {
            Self::Clamp => Ok(()),
            Self::Constant(x) if x.is_finite() => Ok(()),
            Self::Constant(_) => Err(LensError::InvalidConfiguration),
        }
    }
}
/// Brown-Conrady distortion parameters, expressed in pixel-centred normalized coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UndistortConfig {
    /// Principal-point horizontal coordinate in pixels.
    pub center_x: f32,
    /// Principal-point vertical coordinate in pixels.
    pub center_y: f32,
    /// Positive horizontal focal scale in pixels.
    pub focal_x: f32,
    /// Positive vertical focal scale in pixels.
    pub focal_y: f32,
    /// Three radial Brown-Conrady coefficients.
    pub radial: [f32; 3],
    /// Horizontal tangential distortion coefficient.
    pub tangential_x: f32,
    /// Vertical tangential distortion coefficient.
    pub tangential_y: f32,
    /// Resampling rule for the inverse map.
    pub interpolation: Interpolation,
    /// Out-of-bounds source-coordinate policy.
    pub border: Border,
}
impl UndistortConfig {
    /// Validates finite coefficients and positive focal scales.
    pub fn validate(self) -> Result<(), LensError> {
        if !self.center_x.is_finite()
            || !self.center_y.is_finite()
            || !self.focal_x.is_finite()
            || !self.focal_y.is_finite()
            || self.focal_x <= 0.
            || self.focal_y <= 0.
            || !self.radial.iter().all(|v| v.is_finite())
            || !self.tangential_x.is_finite()
            || !self.tangential_y.is_finite()
        {
            return Err(LensError::InvalidConfiguration);
        }
        self.border.validate()
    }
}
/// Per-channel displacement used by chromatic correction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelShift {
    /// Horizontal channel offset in pixels.
    pub x: f32,
    /// Vertical channel offset in pixels.
    pub y: f32,
}
impl ChannelShift {
    /// Rejects non-finite coordinate offsets.
    pub fn validate(self) -> Result<(), LensError> {
        if self.x.is_finite() && self.y.is_finite() {
            Ok(())
        } else {
            Err(LensError::InvalidConfiguration)
        }
    }
}
/// Configuration for [`chromatic`]. Green is kept as the reference channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromaticConfig {
    /// Offset for the red plane relative to green.
    pub red: ChannelShift,
    /// Offset for the blue plane relative to green.
    pub blue: ChannelShift,
    /// Resampling rule for shifted channels.
    pub interpolation: Interpolation,
    /// Out-of-bounds source-coordinate policy.
    pub border: Border,
}
impl ChromaticConfig {
    /// Validates channel shifts and border policy.
    pub fn validate(self) -> Result<(), LensError> {
        self.red.validate()?;
        self.blue.validate()?;
        self.border.validate()
    }
}
/// Whether vignetting is applied or removed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VignetteDirection {
    /// Apply the radial light falloff.
    Apply,
    /// Remove the radial light falloff.
    Correct,
}
/// Configuration for radial vignetting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VignetteConfig {
    /// Falloff centre horizontal coordinate in pixels.
    pub center_x: f32,
    /// Falloff centre vertical coordinate in pixels.
    pub center_y: f32,
    /// Positive reference radius in pixels.
    pub radius: f32,
    /// Non-negative quadratic falloff magnitude.
    pub strength: f32,
    /// Whether to apply or remove the falloff.
    pub direction: VignetteDirection,
}
impl VignetteConfig {
    /// Validates the finite radial falloff model.
    pub fn validate(self) -> Result<(), LensError> {
        if self.center_x.is_finite()
            && self.center_y.is_finite()
            && self.radius.is_finite()
            && self.radius > 0.
            && self.strength.is_finite()
            && self.strength >= 0.
        {
            Ok(())
        } else {
            Err(LensError::InvalidConfiguration)
        }
    }
}
/// Row-dependent translation from output pixel to source pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RollingShutterConfig {
    /// Source offset for the first row.
    pub top: ChannelShift,
    /// Source offset for the last row.
    pub bottom: ChannelShift,
    /// Resampling rule for row-dependent offsets.
    pub interpolation: Interpolation,
    /// Out-of-bounds source-coordinate policy.
    pub border: Border,
}
impl RollingShutterConfig {
    /// Validates offsets and the border policy.
    pub fn validate(self) -> Result<(), LensError> {
        self.top.validate()?;
        self.bottom.validate()?;
        self.border.validate()
    }
}

/// Corrects Brown-Conrady radial and tangential lens distortion.
/// This inverse-map reference has `O(width * height)` work and no allocation.
/// Infinite mapped coordinates follow the configured border policy; an
/// indeterminate (`NaN`) mapped coordinate produces `NaN`.
pub fn undistort(
    input: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    config: UndistortConfig,
) -> Result<(), LensError> {
    config.validate()?;
    same(input.extent(), output.extent())?;
    let e = input.extent();
    for y in 0..e.height() {
        let row = output.row_mut(y).expect("validated output row");
        for (x, dst) in row.iter_mut().enumerate() {
            let nx = (x as f64 - f64::from(config.center_x)) / f64::from(config.focal_x);
            let ny = (y as f64 - f64::from(config.center_y)) / f64::from(config.focal_y);
            let r2 = nx * nx + ny * ny;
            let scale = 1.0
                + f64::from(config.radial[0]) * r2
                + f64::from(config.radial[1]) * r2 * r2
                + f64::from(config.radial[2]) * r2 * r2 * r2;
            let dx = nx * scale
                + 2.0 * f64::from(config.tangential_x) * nx * ny
                + f64::from(config.tangential_y) * (r2 + 2.0 * nx * nx);
            let dy = ny * scale
                + f64::from(config.tangential_x) * (r2 + 2.0 * ny * ny)
                + 2.0 * f64::from(config.tangential_y) * nx * ny;
            *dst = sample(
                input,
                dx.mul_add(f64::from(config.focal_x), f64::from(config.center_x)),
                dy.mul_add(f64::from(config.focal_y), f64::from(config.center_y)),
                config.interpolation,
                config.border,
            );
        }
    }
    Ok(())
}
/// Shifts red and blue planes relative to the green reference plane.
/// Work is linear in the three plane areas and needs no allocation.
pub fn chromatic(
    red: Plane<'_, f32>,
    green: Plane<'_, f32>,
    blue: Plane<'_, f32>,
    mut red_out: PlaneMut<'_, f32>,
    mut green_out: PlaneMut<'_, f32>,
    mut blue_out: PlaneMut<'_, f32>,
    config: ChromaticConfig,
) -> Result<(), LensError> {
    config.validate()?;
    let e = red.extent();
    for actual in [
        green.extent(),
        blue.extent(),
        red_out.extent(),
        green_out.extent(),
        blue_out.extent(),
    ] {
        same(e, actual)?;
    }
    for y in 0..e.height() {
        let rr = red_out.row_mut(y).unwrap();
        let gr = green_out.row_mut(y).unwrap();
        let br = blue_out.row_mut(y).unwrap();
        let input_green = green.row(y).unwrap();
        for x in 0..e.width() {
            rr[x] = sample(
                red,
                x as f64 + f64::from(config.red.x),
                y as f64 + f64::from(config.red.y),
                config.interpolation,
                config.border,
            );
            gr[x] = input_green[x];
            br[x] = sample(
                blue,
                x as f64 + f64::from(config.blue.x),
                y as f64 + f64::from(config.blue.y),
                config.interpolation,
                config.border,
            );
        }
    }
    Ok(())
}
/// Applies or removes a quadratic radial falloff. Non-finite samples propagate.
pub fn vignette(
    input: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    config: VignetteConfig,
) -> Result<(), LensError> {
    config.validate()?;
    same(input.extent(), output.extent())?;
    let e = input.extent();
    for y in 0..e.height() {
        let src = input.row(y).unwrap();
        let dst = output.row_mut(y).unwrap();
        for x in 0..e.width() {
            let value = src[x];
            let dx = x as f64 - f64::from(config.center_x);
            let dy = y as f64 - f64::from(config.center_y);
            let radius = f64::from(config.radius);
            let gain = 1.0 + f64::from(config.strength) * (dx * dx + dy * dy) / (radius * radius);
            dst[x] = match config.direction {
                VignetteDirection::Apply => f64::from(value) / gain,
                VignetteDirection::Correct => f64::from(value) * gain,
            } as f32;
        }
    }
    Ok(())
}
/// Corrects a linearly varying row-dependent translation.
/// Work is `O(width * height)` and requires no scratch or allocation.
pub fn rolling_shutter(
    input: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    config: RollingShutterConfig,
) -> Result<(), LensError> {
    config.validate()?;
    same(input.extent(), output.extent())?;
    let e = input.extent();
    let denominator = (e.height() - 1).max(1) as f64;
    for y in 0..e.height() {
        let t = y as f64 / denominator;
        let shift_x =
            f64::from(config.top.x) + (f64::from(config.bottom.x) - f64::from(config.top.x)) * t;
        let shift_y =
            f64::from(config.top.y) + (f64::from(config.bottom.y) - f64::from(config.top.y)) * t;
        let row = output.row_mut(y).unwrap();
        for (x, dst) in row.iter_mut().enumerate() {
            *dst = sample(
                input,
                x as f64 + shift_x,
                y as f64 + shift_y,
                config.interpolation,
                config.border,
            );
        }
    }
    Ok(())
}
fn same(a: Extent, b: Extent) -> Result<(), LensError> {
    if a == b {
        Ok(())
    } else {
        Err(LensError::ExtentMismatch)
    }
}
fn sample(
    input: Plane<'_, f32>,
    mut x: f64,
    mut y: f64,
    mode: Interpolation,
    border: Border,
) -> f32 {
    if x.is_nan() || y.is_nan() {
        return f32::NAN;
    }
    if x.is_infinite() || y.is_infinite() {
        match border {
            Border::Constant(value) => return value,
            Border::Clamp => {
                let extent = input.extent();
                if x.is_infinite() {
                    x = if x.is_sign_negative() {
                        0.0
                    } else {
                        (extent.width() - 1) as f64
                    };
                }
                if y.is_infinite() {
                    y = if y.is_sign_negative() {
                        0.0
                    } else {
                        (extent.height() - 1) as f64
                    };
                }
            }
        }
    }
    match mode {
        Interpolation::Nearest => one(input, x.round() as i64, y.round() as i64, border),
        Interpolation::Bilinear => {
            let x0 = x.floor();
            let y0 = y.floor();
            let tx = x - x0;
            let ty = y - y0;
            let a = one(input, x0 as i64, y0 as i64, border);
            if tx == 0.0 && ty == 0.0 {
                return a;
            }
            let b = one(input, x0 as i64 + 1, y0 as i64, border);
            if ty == 0.0 {
                return lerp(a, b, tx);
            }
            let c = one(input, x0 as i64, y0 as i64 + 1, border);
            if tx == 0.0 {
                return lerp(a, c, ty);
            }
            let d = one(input, x0 as i64 + 1, y0 as i64 + 1, border);
            if [a, b, c, d].iter().any(|v| !v.is_finite()) {
                f32::NAN
            } else {
                let top = f64::from(a) * (1.0 - tx) + f64::from(b) * tx;
                let bottom = f64::from(c) * (1.0 - tx) + f64::from(d) * tx;
                (top * (1.0 - ty) + bottom * ty) as f32
            }
        }
    }
}

fn lerp(a: f32, b: f32, amount: f64) -> f32 {
    if !a.is_finite() || !b.is_finite() {
        return f32::NAN;
    }
    (f64::from(a) * (1.0 - amount) + f64::from(b) * amount) as f32
}
fn one(input: Plane<'_, f32>, x: i64, y: i64, border: Border) -> f32 {
    let e = input.extent();
    if x >= 0 && y >= 0 && x < e.width() as i64 && y < e.height() as i64 {
        return input.row(y as usize).unwrap()[x as usize];
    }
    match border {
        Border::Clamp => input
            .row(y.clamp(0, e.height() as i64 - 1) as usize)
            .unwrap()[x.clamp(0, e.width() as i64 - 1) as usize],
        Border::Constant(value) => value,
    }
}

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-lens",
    namespace: "lens",
    summary: "Lens, chromatic aberration and rolling-shutter correction",
    filters: &[
        Filter {
            name: "Undistort",
            summary: "Correct radial and tangential distortion",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "Chromatic",
            summary: "Correct wavelength-dependent displacement",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "Vignette",
            summary: "Correct or apply radial light falloff",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "RollingShutter",
            summary: "Correct row-dependent camera motion",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use vsip_core::{Extent, Plane, PlaneMut};
    #[test]
    fn corrections_handle_odd_padded_planes() {
        let e = Extent::new(3, 3).unwrap();
        let src = [1., 2., 3., 99., 4., 5., 6., 99., 7., 8., 9.];
        let mut out = [-1.; 11];
        undistort(
            Plane::new(&src, e, 4).unwrap(),
            PlaneMut::new(&mut out, e, 4).unwrap(),
            UndistortConfig {
                center_x: 1.,
                center_y: 1.,
                focal_x: 1.,
                focal_y: 1.,
                radial: [0.; 3],
                tangential_x: 0.,
                tangential_y: 0.,
                interpolation: Interpolation::Nearest,
                border: Border::Clamp,
            },
        )
        .unwrap();
        assert_eq!(&out[..3], &[1., 2., 3.]);
        assert_eq!(out[3], -1.);
        vignette(
            Plane::new(&src, e, 4).unwrap(),
            PlaneMut::new(&mut out, e, 4).unwrap(),
            VignetteConfig {
                center_x: 1.,
                center_y: 1.,
                radius: 1.,
                strength: 1.,
                direction: VignetteDirection::Correct,
            },
        )
        .unwrap();
        assert_eq!(out[5], 5.);
    }
    #[test]
    fn chromatic_and_rolling_validate_geometry_and_nan() {
        let e = Extent::new(1, 1).unwrap();
        let a = [f32::NAN];
        let b = [1.];
        let mut o = [0.];
        assert_eq!(
            rolling_shutter(
                Plane::new(&a, e, 1).unwrap(),
                PlaneMut::new(&mut o, e, 1).unwrap(),
                RollingShutterConfig {
                    top: ChannelShift { x: f32::NAN, y: 0. },
                    bottom: ChannelShift { x: 0., y: 0. },
                    interpolation: Interpolation::Nearest,
                    border: Border::Clamp
                }
            ),
            Err(LensError::InvalidConfiguration)
        );
        chromatic(
            Plane::new(&b, e, 1).unwrap(),
            Plane::new(&b, e, 1).unwrap(),
            Plane::new(&a, e, 1).unwrap(),
            PlaneMut::new(&mut o, e, 1).unwrap(),
            PlaneMut::new(&mut [0.], e, 1).unwrap(),
            PlaneMut::new(&mut [0.], e, 1).unwrap(),
            ChromaticConfig {
                red: ChannelShift { x: 0., y: 0. },
                blue: ChannelShift { x: 0., y: 0. },
                interpolation: Interpolation::Nearest,
                border: Border::Clamp,
            },
        )
        .unwrap();
        assert!(o[0].is_finite());
    }

    #[test]
    fn bilinear_identity_ignores_zero_weight_nan_neighbours() {
        let e = Extent::new(2, 1).unwrap();
        let input = [7.0, f32::NAN];
        let mut output = [0.0; 2];
        rolling_shutter(
            Plane::new(&input, e, 2).unwrap(),
            PlaneMut::new(&mut output, e, 2).unwrap(),
            RollingShutterConfig {
                top: ChannelShift { x: 0.0, y: 0.0 },
                bottom: ChannelShift { x: 0.0, y: 0.0 },
                interpolation: Interpolation::Bilinear,
                border: Border::Clamp,
            },
        )
        .unwrap();
        assert_eq!(output[0], 7.0);
        assert!(output[1].is_nan());
    }

    #[test]
    fn identity_undistortion_handles_tiny_focal_scales() {
        let e = Extent::new(2, 1).unwrap();
        let input = [1.0, 2.0];
        let mut output = [0.0; 2];
        undistort(
            Plane::new(&input, e, 2).unwrap(),
            PlaneMut::new(&mut output, e, 2).unwrap(),
            UndistortConfig {
                center_x: 0.0,
                center_y: 0.0,
                focal_x: f32::MIN_POSITIVE,
                focal_y: f32::MIN_POSITIVE,
                radial: [0.0; 3],
                tangential_x: 0.0,
                tangential_y: 0.0,
                interpolation: Interpolation::Nearest,
                border: Border::Clamp,
            },
        )
        .unwrap();
        assert_eq!(output, input);
    }
}
