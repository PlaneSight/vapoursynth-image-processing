//! Deterministic, dependency-free test fixtures.

/// Produces a binary mask with one zero-valued seed.
#[must_use]
pub fn single_seed_mask(width: usize, height: usize, x: usize, y: usize) -> Vec<u8> {
    assert!(x < width && y < height, "seed must lie inside the mask");
    let mut mask = vec![1; width.checked_mul(height).expect("fixture size overflow")];
    mask[y * width + x] = 0;
    mask
}

