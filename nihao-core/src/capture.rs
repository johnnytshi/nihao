use crate::config::CameraConfig;
use image::{ImageBuffer, RgbImage};
use std::fs;
use thiserror::Error;
use v4l::io::traits::CaptureStream;
use v4l::prelude::*;
use v4l::video::Capture as V4lCapture;
use v4l::{Device, FourCC};

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("Failed to open camera device: {0}")]
    DeviceOpen(String),
    #[error("Failed to capture frame: {0}")]
    Capture(String),
    #[error("Frame conversion failed: {0}")]
    Conversion(String),
    #[error("Timeout waiting for frame")]
    Timeout,
    #[error("V4L2 error: {0}")]
    V4L(#[from] std::io::Error),
    #[error("Bad frame: {0}")]
    BadFrame(String), // Separate error for bad frames that can be retried
}

pub struct Camera {
    device: Device,
    width: u32,
    height: u32,
    format: FourCC,
    config: CameraConfig, // Store config for quality checks
}

impl Camera {
    /// Create a new camera instance from configuration
    pub fn new(config: &CameraConfig) -> Result<Self, CaptureError> {
        let device_path = &config.device;

        // Open the device - extract device number from path (e.g., "/dev/video2" -> 2)
        let device_num = if device_path.starts_with("/dev/video") {
            device_path
                .trim_start_matches("/dev/video")
                .parse::<usize>()
                .unwrap_or(0)
        } else {
            device_path.parse::<usize>().unwrap_or(0)
        };

        let device = Device::new(device_num)
            .map_err(|e| CaptureError::DeviceOpen(format!("{}: {}", device_path, e)))?;

        // Get current format
        let fmt = device.format()
            .map_err(|e| CaptureError::DeviceOpen(format!("Failed to get format: {}", e)))?;

        // Try to set desired resolution
        let mut format = fmt.clone();
        format.width = config.width;
        format.height = config.height;

        // Prefer MJPEG if available, fallback to YUYV
        let preferred_formats = [
            FourCC::new(b"MJPG"),
            FourCC::new(b"YUYV"),
        ];

        let mut set_format = format;
        for &fourcc in &preferred_formats {
            set_format.fourcc = fourcc;
            if device.set_format(&set_format).is_ok() {
                break;
            }
        }

        // Get the actual format that was set
        let actual_format = device.format()
            .map_err(|e| CaptureError::DeviceOpen(format!("Failed to verify format: {}", e)))?;

        log::info!(
            "Camera initialized: {}x{} {}",
            actual_format.width,
            actual_format.height,
            actual_format.fourcc
        );

        Ok(Self {
            device,
            width: actual_format.width,
            height: actual_format.height,
            format: actual_format.fourcc,
            config: config.clone(),
        })
    }


    /// Capture a single frame from the camera with quality checks
    pub fn capture_frame(&mut self, check_quality: bool) -> Result<RgbImage, CaptureError> {
        let mut stream = MmapStream::with_buffers(&self.device, v4l::buffer::Type::VideoCapture, 4)
            .map_err(|e| CaptureError::Capture(format!("Failed to create stream: {}", e)))?;

        let (buf, _meta) = stream
            .next()
            .map_err(|e| CaptureError::Capture(format!("Failed to capture frame: {}", e)))?;

        let rgb = match self.format.str() {
            Ok("MJPG") => self.decode_mjpeg(buf)?,
            Ok("YUYV") => self.decode_yuyv(buf)?,
            _ => {
                return Err(CaptureError::Conversion(format!(
                    "Unsupported pixel format: {}",
                    self.format
                )))
            }
        };

        if check_quality {
            // Check frame darkness (filter bad IR emitter reads)
            let (darkness_pct, is_too_dark) =
                self.analyze_frame_darkness(&rgb, self.config.dark_threshold);

            if is_too_dark {
                return Err(CaptureError::BadFrame(format!(
                    "too dark: {:.1}% (threshold: {:.1}%)",
                    darkness_pct, self.config.dark_threshold
                )));
            }

            // Check for severe overexposure
            let (overexposed_pct, is_too_bright) = self.is_overexposed(&rgb);

            if is_too_bright {
                return Err(CaptureError::BadFrame(format!(
                    "overexposed: {:.1}% blown out (threshold: 15%)",
                    overexposed_pct
                )));
            }
        }

        Ok(rgb)
    }


    /// Analyze frame darkness to filter out bad IR emitter reads
    /// Returns (darkness_percentage, is_bad_frame)
    /// Based on Howdy's approach: compare.py:254-274
    fn analyze_frame_darkness(&self, image: &RgbImage, dark_threshold: f32) -> (f32, bool) {
        // Convert to grayscale and build 8-bin histogram
        const BINS: usize = 8;
        const BIN_SIZE: f32 = 256.0 / BINS as f32;

        let mut histogram = [0u32; BINS];
        let mut total_pixels = 0u32;
        let mut black_pixels = 0u32;

        for pixel in image.pixels() {
            let gray = ((pixel[0] as u16 + pixel[1] as u16 + pixel[2] as u16) / 3) as u8;

            // Count 100% black pixels (bad camera read)
            if gray == 0 {
                black_pixels += 1;
            }

            // Add to histogram bin
            let bin = ((gray as f32 / BIN_SIZE).floor() as usize).min(BINS - 1);
            histogram[bin] += 1;
            total_pixels += 1;
        }

        // Check for 100% black frame (bad camera read)
        if black_pixels == total_pixels {
            log::warn!("Frame is 100% black - bad camera read, skipping");
            return (100.0, true);
        }

        // Calculate darkness from first bin (darkest pixels)
        let darkness_pct = (histogram[0] as f32 / total_pixels as f32) * 100.0;
        let is_too_dark = darkness_pct > dark_threshold;

        if is_too_dark {
            log::warn!(
                "Frame too dark: {:.1}% (threshold: {:.1}%) - IR emitter flash issue, skipping",
                darkness_pct,
                dark_threshold
            );
        } else {
            log::debug!("Frame darkness: {:.1}%", darkness_pct);
        }

        (darkness_pct, is_too_dark)
    }

    /// Detect if image is overexposed (too many bright pixels)
    /// Returns (overexposure_percentage, is_too_bright)
    fn is_overexposed(&self, image: &RgbImage) -> (f32, bool) {
        let total_pixels = image.pixels().len();
        let bright_threshold = 240u8; // Pixels above this are "blown out"

        let bright_pixels = image
            .pixels()
            .filter(|p| {
                let avg = (p[0] as u16 + p[1] as u16 + p[2] as u16) / 3;
                avg as u8 > bright_threshold
            })
            .count();

        let overexposed_pct = (bright_pixels as f32 / total_pixels as f32) * 100.0;

        // Relaxed threshold for faster authentication (3-second target)
        // 15% allows more frames to pass on first try
        let is_too_bright = overexposed_pct > 15.0;

        if is_too_bright {
            log::warn!(
                "Image is severely overexposed: {:.1}% of pixels are blown out (threshold: 15%)",
                overexposed_pct
            );
        } else if overexposed_pct > 8.0 {
            log::debug!(
                "Image has some overexposure: {:.1}% of pixels are blown out",
                overexposed_pct
            );
        }

        (overexposed_pct, is_too_bright)
    }


    /// Decode MJPEG frame to RGB
    fn decode_mjpeg(&self, data: &[u8]) -> Result<RgbImage, CaptureError> {
        let img = image::load_from_memory_with_format(data, image::ImageFormat::Jpeg)
            .map_err(|e| CaptureError::Conversion(format!("MJPEG decode failed: {}", e)))?;

        Ok(img.to_rgb8())
    }

    /// Decode YUYV frame to RGB
    fn decode_yuyv(&self, data: &[u8]) -> Result<RgbImage, CaptureError> {
        let width = self.width as usize;
        let height = self.height as usize;

        if data.len() < width * height * 2 {
            return Err(CaptureError::Conversion(
                "YUYV buffer too small".to_string(),
            ));
        }

        let mut rgb_data = vec![0u8; width * height * 3];

        // Convert YUYV to RGB
        // YUYV format: Y0 U Y1 V (2 pixels in 4 bytes)
        for y in 0..height {
            for x in 0..(width / 2) {
                let yuyv_offset = (y * width * 2) + (x * 4);
                let rgb_offset = (y * width * 3) + (x * 2 * 3);

                let y0 = data[yuyv_offset] as i32;
                let u = data[yuyv_offset + 1] as i32 - 128;
                let y1 = data[yuyv_offset + 2] as i32;
                let v = data[yuyv_offset + 3] as i32 - 128;

                // Convert YUV to RGB for pixel 0
                let r0 = (y0 + ((1436 * v) >> 10)).clamp(0, 255) as u8;
                let g0 = (y0 - ((354 * u + 732 * v) >> 10)).clamp(0, 255) as u8;
                let b0 = (y0 + ((1814 * u) >> 10)).clamp(0, 255) as u8;

                rgb_data[rgb_offset] = r0;
                rgb_data[rgb_offset + 1] = g0;
                rgb_data[rgb_offset + 2] = b0;

                // Convert YUV to RGB for pixel 1
                let r1 = (y1 + ((1436 * v) >> 10)).clamp(0, 255) as u8;
                let g1 = (y1 - ((354 * u + 732 * v) >> 10)).clamp(0, 255) as u8;
                let b1 = (y1 + ((1814 * u) >> 10)).clamp(0, 255) as u8;

                rgb_data[rgb_offset + 3] = r1;
                rgb_data[rgb_offset + 4] = g1;
                rgb_data[rgb_offset + 5] = b1;
            }
        }

        ImageBuffer::from_raw(width as u32, height as u32, rgb_data)
            .ok_or_else(|| CaptureError::Conversion("Failed to create RGB image".to_string()))
    }

    /// Enumerate available camera devices
    pub fn list_devices() -> Result<Vec<String>, CaptureError> {
        let mut devices = Vec::new();

        // Scan /dev/video* devices
        for entry in fs::read_dir("/dev")
            .map_err(|e| CaptureError::DeviceOpen(format!("Failed to read /dev: {}", e)))?
        {
            let entry = entry.map_err(|e| CaptureError::DeviceOpen(e.to_string()))?;
            let path = entry.path();

            if let Some(name) = path.file_name() {
                if let Some(name_str) = name.to_str() {
                    if name_str.starts_with("video") {
                        if let Some(path_str) = path.to_str() {
                            devices.push(path_str.to_string());
                        }
                    }
                }
            }
        }

        devices.sort();
        Ok(devices)
    }

    /// Check if a device supports IR input
    pub fn is_ir_camera(device_path: &str) -> Result<bool, CaptureError> {
        // Parse device number from path
        let device_num = if device_path.starts_with("/dev/video") {
            device_path
                .trim_start_matches("/dev/video")
                .parse::<usize>()
                .unwrap_or(0)
        } else {
            device_path.parse::<usize>().unwrap_or(0)
        };

        // Try to open the device
        let device = Device::new(device_num)
            .map_err(|e| CaptureError::DeviceOpen(format!("{}: {}", device_path, e)))?;

        // Get device capabilities
        let caps = device.query_caps()
            .map_err(|e| CaptureError::DeviceOpen(format!("Failed to query caps: {}", e)))?;

        // Check device name for IR indicators
        let name_lower = caps.card.to_lowercase();
        let is_ir = name_lower.contains("ir") ||
                    name_lower.contains("infrared") ||
                    name_lower.contains("depth");

        Ok(is_ir)
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_devices() {
        // This test requires a system with V4L2 devices
        match Camera::list_devices() {
            Ok(devices) => {
                println!("Found {} camera devices", devices.len());
                for device in devices {
                    println!("  {}", device);
                }
            }
            Err(e) => println!("Could not list devices: {}", e),
        }
    }

    #[test]
    #[ignore] // Requires actual camera hardware
    fn test_camera_capture() {
        let config = CameraConfig {
            device: "/dev/video0".to_string(),
            width: 640,
            height: 480,
            dark_threshold: 80.0,
            detection_scale: 0.5,
        };

        let mut camera = Camera::new(&config).expect("Failed to open camera");
        let frame = camera
            .capture_frame(false)
            .expect("Failed to capture frame");

        assert_eq!(frame.width(), config.width);
        assert_eq!(frame.height(), config.height);
    }
}
