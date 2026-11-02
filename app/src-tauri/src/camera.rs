//! Frame helpers: decode OBS screenshots, downscale, JPEG previews, and the
//! on-screen paper guide box.

use coach_core::video::{Frame, Rect};
use image::{ImageBuffer, Rgb};

/// Where the user holds the white paper during the "hold paper" step, as
/// fractions of the frame (x0, y0, x1, y1): beside the face so the face stays
/// visible. The UI draws the same box on the preview.
pub const PAPER_GUIDE: (f64, f64, f64, f64) = (0.66, 0.35, 0.94, 0.75);

/// Frames are analysed at this width (keeps memory and CPU low).
pub const ANALYSIS_WIDTH: usize = 320;

pub fn paper_rect(frame: &Frame) -> Rect {
    let (x0, y0, x1, y1) = PAPER_GUIDE;
    let w = frame.width as f64;
    let h = frame.height as f64;
    // Use the inner 80 % of the box so fingers/edges don't count.
    let (ix0, iy0) = (x0 + (x1 - x0) * 0.1, y0 + (y1 - y0) * 0.1);
    let (ix1, iy1) = (x1 - (x1 - x0) * 0.1, y1 - (y1 - y0) * 0.1);
    Rect::new((ix0 * w) as usize, (iy0 * h) as usize, ((ix1 - ix0) * w) as usize, ((iy1 - iy0) * h) as usize)
}

pub fn decode_image(bytes: &[u8], timestamp_s: f64) -> Result<Frame, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("Couldn't read the camera image ({e})."))?.to_rgb8();
    Ok(Frame { width: img.width() as usize, height: img.height() as usize, rgb: img.into_raw(), timestamp_s })
}

/// Box-filter downscale to `width` (keeps aspect). No-op if already smaller.
pub fn downscale(f: &Frame, width: usize) -> Frame {
    if f.width <= width || f.width == 0 {
        return f.clone();
    }
    let height = (f.height * width / f.width).max(1);
    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        let y0 = y * f.height / height;
        let y1 = ((y + 1) * f.height / height).max(y0 + 1);
        for x in 0..width {
            let x0 = x * f.width / width;
            let x1 = ((x + 1) * f.width / width).max(x0 + 1);
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = (yy * f.width + xx) * 3;
                    acc[0] += f.rgb[i] as u32;
                    acc[1] += f.rgb[i + 1] as u32;
                    acc[2] += f.rgb[i + 2] as u32;
                    n += 1;
                }
            }
            rgb.extend(acc.iter().map(|v| (v / n.max(1)) as u8));
        }
    }
    Frame { width, height, rgb, timestamp_s: f.timestamp_s }
}

/// Scale a rectangle found on `from` onto a frame of size `to`.
pub fn scale_rect(r: Rect, from: (usize, usize), to: (usize, usize)) -> Rect {
    let sx = to.0 as f64 / from.0.max(1) as f64;
    let sy = to.1 as f64 / from.1.max(1) as f64;
    Rect::new((r.x as f64 * sx) as usize, (r.y as f64 * sy) as usize, (r.w as f64 * sx) as usize, (r.h as f64 * sy) as usize)
}

pub fn encode_jpeg(f: &Frame, quality: u8) -> Result<Vec<u8>, image::ImageError> {
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_raw(f.width as u32, f.height as u32, f.rgb.clone()).ok_or_else(|| image::ImageError::Parameter(image::error::ParameterError::from_kind(image::error::ParameterErrorKind::DimensionMismatch)))?;
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    img.write_with_encoder(enc)?;
    Ok(out)
}

pub fn jpeg_data_url(f: &Frame) -> Option<String> {
    let jpeg = encode_jpeg(f, 70).ok()?;
    Some(format!("data:image/jpeg;base64,{}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, jpeg)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_roundtrip_and_downscale() {
        let f = Frame::solid(640, 480, [200, 100, 50], 1.0);
        let small = downscale(&f, 320);
        assert_eq!((small.width, small.height), (320, 240));
        assert_eq!(small.px(10, 10), [200, 100, 50]);
        let back = decode_image(&encode_jpeg(&small, 90).unwrap(), 2.0).unwrap();
        assert_eq!((back.width, back.height), (320, 240));
        let p = back.px(100, 100);
        assert!((p[0] as i32 - 200).abs() < 6);
        assert!(jpeg_data_url(&small).unwrap().starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn paper_box_is_inside_guide() {
        let f = Frame::solid(320, 240, [0, 0, 0], 0.0);
        let r = paper_rect(&f);
        assert!(r.x > 211 && r.right() < 300 && r.y > 84 && r.bottom() < 180, "{r:?}");
        assert_eq!(scale_rect(Rect::new(10, 10, 20, 20), (100, 100), (200, 50)), Rect::new(20, 5, 40, 10));
    }
}
