use crate::detect::FacialLandmarks;
use image::{Rgb, RgbImage};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AlignmentError {
    #[error("Failed to compute affine transform: {0}")]
    Transform(String),
    #[error("Failed to warp image: {0}")]
    Warp(String),
}

/// Output size for aligned face
pub const ALIGNED_SIZE: u32 = 112;

/// Canonical landmark positions for 112x112 face alignment
/// These are standard positions used for ArcFace models
pub const CANONICAL_LANDMARKS: [(f32, f32); 5] = [
    (38.2946, 51.6963), // left eye
    (73.5318, 51.5014), // right eye
    (56.0252, 71.7366), // nose
    (41.5493, 92.3655), // left mouth
    (70.7299, 92.2041), // right mouth
];

pub struct FaceAligner;

impl FaceAligner {
    /// Align a face to canonical position for embedding
    pub fn align(
        image: &RgbImage,
        landmarks: &FacialLandmarks,
    ) -> Result<RgbImage, AlignmentError> {
        // Extract source landmarks as array
        let src_landmarks = [
            landmarks.left_eye,
            landmarks.right_eye,
            landmarks.nose,
            landmarks.left_mouth,
            landmarks.right_mouth,
        ];

        // Compute similarity transform (scale, rotation, translation)
        let transform = Self::estimate_similarity_transform(&src_landmarks, &CANONICAL_LANDMARKS)
            .ok_or_else(|| AlignmentError::Transform("Failed to compute transform".to_string()))?;

        // Apply transform to create aligned face
        let aligned = Self::warp_affine(image, &transform, ALIGNED_SIZE, ALIGNED_SIZE)?;

        Ok(aligned)
    }

    /// Estimate similarity transform from source to destination landmarks
    /// Returns [a, b, tx, ty] where transform is:
    /// x' = a*x - b*y + tx
    /// y' = b*x + a*y + ty
    fn estimate_similarity_transform(
        src: &[(f32, f32); 5],
        dst: &[(f32, f32); 5],
    ) -> Option<[f32; 4]> {
        // Use least squares to solve for similarity transform
        // We use the first 3 points (eyes and nose) for a stable estimate

        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut sum_u = 0.0;
        let mut sum_v = 0.0;
        let mut sum_xx_yy = 0.0;
        let mut sum_ux_vy = 0.0;
        let mut sum_vx_uy = 0.0;

        let n = 5.0;

        for i in 0..5 {
            let (x, y) = src[i];
            let (u, v) = dst[i];

            sum_x += x;
            sum_y += y;
            sum_u += u;
            sum_v += v;
            sum_xx_yy += x * x + y * y;
            sum_ux_vy += u * x + v * y;
            sum_vx_uy += v * x - u * y;
        }

        let denom = n * sum_xx_yy - sum_x * sum_x - sum_y * sum_y;
        if denom.abs() < 1e-6 {
            return None;
        }

        let a = (n * sum_ux_vy - sum_u * sum_x - sum_v * sum_y) / denom;
        let b = (n * sum_vx_uy + sum_u * sum_y - sum_v * sum_x) / denom;
        let tx = (sum_u - a * sum_x + b * sum_y) / n;
        let ty = (sum_v - b * sum_x - a * sum_y) / n;

        Some([a, b, tx, ty])
    }

    /// Apply affine warp to image
    fn warp_affine(
        image: &RgbImage,
        transform: &[f32; 4],
        out_width: u32,
        out_height: u32,
    ) -> Result<RgbImage, AlignmentError> {
        let [a, b, tx, ty] = *transform;

        // Compute inverse transform for backward mapping
        let det = a * a + b * b;
        if det.abs() < 1e-6 {
            return Err(AlignmentError::Warp(
                "Singular transform matrix".to_string(),
            ));
        }

        let a_inv = a / det;
        let b_inv = -b / det;

        let mut output = RgbImage::new(out_width, out_height);

        for y_out in 0..out_height {
            for x_out in 0..out_width {
                let x_out_f = x_out as f32;
                let y_out_f = y_out as f32;

                // Apply inverse transform to find source coordinate
                let x_in = a_inv * (x_out_f - tx) - b_inv * (y_out_f - ty);
                let y_in = b_inv * (x_out_f - tx) + a_inv * (y_out_f - ty);

                // Bilinear interpolation
                let x_floor = x_in.floor();
                let y_floor = y_in.floor();
                let x_frac = x_in - x_floor;
                let y_frac = y_in - y_floor;

                let x0 = x_floor as i32;
                let y0 = y_floor as i32;
                let x1 = x0 + 1;
                let y1 = y0 + 1;

                // Check bounds
                if x0 < 0
                    || y0 < 0
                    || x1 >= image.width() as i32
                    || y1 >= image.height() as i32
                {
                    // Out of bounds - use black
                    output.put_pixel(x_out, y_out, Rgb([0, 0, 0]));
                    continue;
                }

                // Get four neighboring pixels
                let p00 = image.get_pixel(x0 as u32, y0 as u32);
                let p10 = image.get_pixel(x1 as u32, y0 as u32);
                let p01 = image.get_pixel(x0 as u32, y1 as u32);
                let p11 = image.get_pixel(x1 as u32, y1 as u32);

                // Interpolate each channel
                let mut pixel = [0u8; 3];
                for c in 0..3 {
                    let v00 = p00[c] as f32;
                    let v10 = p10[c] as f32;
                    let v01 = p01[c] as f32;
                    let v11 = p11[c] as f32;

                    let v0 = v00 * (1.0 - x_frac) + v10 * x_frac;
                    let v1 = v01 * (1.0 - x_frac) + v11 * x_frac;
                    let v = v0 * (1.0 - y_frac) + v1 * y_frac;

                    pixel[c] = v.round().clamp(0.0, 255.0) as u8;
                }

                output.put_pixel(x_out, y_out, Rgb(pixel));
            }
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_transform() {
        let src = CANONICAL_LANDMARKS;
        let dst = CANONICAL_LANDMARKS;

        let transform = FaceAligner::estimate_similarity_transform(&src, &dst).unwrap();

        // Should be identity: a≈1, b≈0, tx≈0, ty≈0
        assert!((transform[0] - 1.0).abs() < 0.1);
        assert!(transform[1].abs() < 0.1);
        assert!(transform[2].abs() < 0.1);
        assert!(transform[3].abs() < 0.1);
    }

    #[test]
    fn test_translation_transform() {
        let mut src = CANONICAL_LANDMARKS;
        // Shift all points by (10, 20)
        for point in &mut src {
            point.0 += 10.0;
            point.1 += 20.0;
        }
        let dst = CANONICAL_LANDMARKS;

        let transform = FaceAligner::estimate_similarity_transform(&src, &dst).unwrap();

        // Should have translation components
        assert!((transform[0] - 1.0).abs() < 0.1);
        assert!(transform[1].abs() < 0.1);
        assert!((transform[2] + 10.0).abs() < 1.0);
        assert!((transform[3] + 20.0).abs() < 1.0);
    }
}

