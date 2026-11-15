//! Copy borrowed GPU readback on the UI thread; encode PNG only on the inspector worker.

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::io::{self, Write};

const MAX_RGBA: usize = 64 * 1024 * 1024;
// Base64 fits comfortably inside the existing 4 MiB transport envelope.
const MAX_PNG: usize = 2 * 1024 * 1024;

pub(super) struct Pixels {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Pixels {
    pub(super) fn copy(
        width: u32,
        height: u32,
        stride: usize,
        data: &[u8],
    ) -> Result<Self, String> {
        let width_usize = usize::try_from(width).map_err(|_| "image width overflow")?;
        let height_usize = usize::try_from(height).map_err(|_| "image height overflow")?;
        let row = width_usize.checked_mul(4).ok_or("image row overflow")?;
        let total = row.checked_mul(height_usize).ok_or("image size overflow")?;
        if total == 0 || total > MAX_RGBA {
            return Err("capture must contain 1..64 MiB of RGBA pixels".into());
        }
        let source_len = stride
            .checked_mul(height_usize - 1)
            .and_then(|n| n.checked_add(row))
            .ok_or("capture stride overflow")?;
        if stride < row || data.len() < source_len {
            return Err("invalid capture stride or pixel buffer length".into());
        }
        let mut rgba = Vec::with_capacity(total);
        for y in 0..height_usize {
            rgba.extend_from_slice(&data[y * stride..y * stride + row]);
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    pub(super) fn encode(self) -> Result<Value, String> {
        let mut bytes = Limited(Vec::new());
        let mut encoder = png::Encoder::new(&mut bytes, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(&self.rgba)
            .map_err(|error| error.to_string())?;
        writer.finish().map_err(|error| error.to_string())?;
        Ok(json!({
            "format": "png", "encoding": "base64", "data": STANDARD.encode(&bytes.0),
            "width": self.width, "height": self.height, "bytes": bytes.0.len(),
        }))
    }
}

struct Limited(Vec<u8>);

impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_PNG.saturating_sub(self.0.len()) {
            return Err(io::Error::other(
                "PNG exceeds 2 MiB; use a smaller terminal viewport",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "pixels_tests.rs"]
mod tests;
