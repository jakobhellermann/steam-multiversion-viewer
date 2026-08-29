// TODO(ai-review): review for style and correctness
//! `Texture2DReader` payload decode: `SurfaceFormat` + dimensions + mip
//! chain, base mip only, straight to PNG.

use anyhow::{Result, ensure};
use image::RgbaImage;

use super::XnbContentUnsupported;

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(self.pos + n <= self.data.len(), "texture payload truncated");
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

pub fn decode_to_png(payload: &[u8]) -> Result<Vec<u8>> {
    let mut c = Cursor { data: payload, pos: 0 };
    let format = c.i32()?;
    let width = c.u32()?;
    let height = c.u32()?;
    let mip_count = c.u32()?;
    ensure!(mip_count >= 1, "texture has no mip levels");
    let data_size = c.u32()? as usize;
    let data = c.take(data_size)?;

    let mut img = match format {
        0 => uncompressed_rgba(width, height, data)?,
        4 => block_compressed(width, height, data, texture2ddecoder::decode_bc1)?,
        5 => block_compressed(width, height, data, texture2ddecoder::decode_bc2)?,
        6 => block_compressed(width, height, data, texture2ddecoder::decode_bc3)?,
        other => {
            return Err(XnbContentUnsupported(format!("unsupported Texture2D format {other}")).into());
        }
    };
    unpremultiply_alpha(&mut img);

    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

fn uncompressed_rgba(width: u32, height: u32, data: &[u8]) -> Result<RgbaImage> {
    let (w, h) = (width as usize, height as usize);
    ensure!(data.len() >= w * h * 4, "texture data too short for {w}x{h} RGBA");
    RgbaImage::from_raw(width, height, data[..w * h * 4].to_vec())
        .ok_or_else(|| anyhow::anyhow!("texture dimensions don't fit the pixel buffer"))
}

type BlockDecoder = fn(&[u8], usize, usize, &mut [u32]) -> Result<(), &'static str>;

fn block_compressed(width: u32, height: u32, data: &[u8], decoder: BlockDecoder) -> Result<RgbaImage> {
    let (w, h) = (width as usize, height as usize);
    let mut buf = vec![0u32; w * h];
    decoder(data, w, h, &mut buf).map_err(|e| anyhow::anyhow!("block decode failed: {e}"))?;
    let mut img = RgbaImage::new(width, height);
    for y in 0..h {
        for x in 0..w {
            let [b, g, r, a] = buf[y * w + x].to_le_bytes();
            img.put_pixel(x as u32, y as u32, image::Rgba([r, g, b, a]));
        }
    }
    Ok(img)
}

/// XNA content stores color premultiplied by alpha; PNG expects straight alpha, so undo it.
fn unpremultiply_alpha(img: &mut RgbaImage) {
    for pixel in img.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        if a == 0 {
            pixel.0 = [0, 0, 0, 0];
            continue;
        }
        let unmul = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
        pixel.0 = [unmul(r), unmul(g), unmul(b), a];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_uncompressed_color() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0i32.to_le_bytes()); // format: Color
        payload.extend_from_slice(&1u32.to_le_bytes()); // width
        payload.extend_from_slice(&1u32.to_le_bytes()); // height
        payload.extend_from_slice(&1u32.to_le_bytes()); // mip count
        payload.extend_from_slice(&4u32.to_le_bytes()); // data size
        payload.extend_from_slice(&[255, 0, 0, 255]); // opaque red
        let png = decode_to_png(&payload).unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn rejects_unknown_format() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&99i32.to_le_bytes());
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        let err = decode_to_png(&payload).unwrap_err();
        assert!(err.downcast_ref::<XnbContentUnsupported>().is_some());
    }
}
