//! MKLM's icon, drawn in code (from the M0 #8 prototype): a blue keyboard with two rows of white
//! keys and a space bar. The window and the tray use the 32-pixel image (`icon::app_icon`); the
//! executable's icon resource (design m3 F.7) is the `.ico` [`ico_file`] makes, which build.rs
//! writes at build time and res/mklm.rc embeds. M5 replaces the drawing with the final artwork.
//!
//! No dependencies, so that build.rs can include this file as it is (`#[path]`).

/// The sizes of the `.ico`: the small icons at 100 % to 250 % and the large ones up to 64 (the
/// placeholder has no 256-pixel image; Windows scales the 64 one up).
pub const ICO_SIZES: [u32; 7] = [16, 20, 24, 32, 40, 48, 64];

/// The drawing's grid: shapes are given on a 32 × 32 square.
const GRID: f32 = 32.0;

/// Samples per pixel and axis (anti-aliasing).
const SAMPLES: u32 = 4;

const BODY: [u8; 3] = [0x1F, 0x5F, 0xB8];
const KEY: [u8; 3] = [0xFF, 0xFF, 0xFF];

/// Rectangles `[x0, x1) × [y0, y1)` on the grid: two rows of six keys and the space bar.
fn keys() -> Vec<(f32, f32, f32, f32)> {
    let mut keys = Vec::new();
    for (offset, y) in [(4.0, 9.0), (5.0, 14.0)] {
        for i in 0..6u8 {
            let x = offset + f32::from(i) * 4.0;
            keys.push((x, y, x + 3.0, y + 3.0));
        }
    }
    keys.push((8.0, 19.0, 24.0, 22.0));
    keys
}

/// Inside the keyboard's body: `[1, 31) × [5, 27)` with corners of radius 4.
fn in_body(x: f32, y: f32) -> bool {
    let (x0, y0, x1, y1, r) = (1.0, 5.0, 31.0, 27.0, 4.0);
    if !(x0..x1).contains(&x) || !(y0..y1).contains(&y) {
        return false;
    }
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// The icon at `size` × `size` pixels: RGBA, straight alpha, rows from the top.
pub fn pixels(size: u32) -> Vec<[u8; 4]> {
    let keys = keys();
    let scale = GRID / size as f32;
    let total = (SAMPLES * SAMPLES) as f32;
    let mut out = Vec::with_capacity((size * size) as usize);
    for py in 0..size {
        for px in 0..size {
            // Premultiplied sums over the samples.
            let mut sum = [0.0f32; 4];
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) * scale;
                    let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) * scale;
                    let color = if keys
                        .iter()
                        .any(|&(x0, y0, x1, y1)| (x0..x1).contains(&x) && (y0..y1).contains(&y))
                    {
                        Some(KEY)
                    } else if in_body(x, y) {
                        Some(BODY)
                    } else {
                        None
                    };
                    if let Some(color) = color {
                        for (channel, value) in sum.iter_mut().zip(color) {
                            *channel += f32::from(value);
                        }
                        sum[3] += 255.0;
                    }
                }
            }
            let alpha = sum[3] / total;
            let pixel = if alpha <= 0.0 {
                [0, 0, 0, 0]
            } else {
                let straight =
                    |value: f32| (value / total * 255.0 / alpha).round().min(255.0) as u8;
                [
                    straight(sum[0]),
                    straight(sum[1]),
                    straight(sum[2]),
                    alpha.round().min(255.0) as u8,
                ]
            };
            out.push(pixel);
        }
    }
    out
}

/// One `.ico` image: a 32-bit DIB (`BITMAPINFOHEADER`, BGRA rows from the bottom, then the 1-bit
/// AND mask, rows padded to 4 bytes; transparent pixels set in the mask).
fn dib(size: u32) -> Vec<u8> {
    let pixels = pixels(size);
    let mask_row = size.div_ceil(32) * 4;
    let image_size = size * size * 4 + mask_row * size;
    let mut out = Vec::with_capacity(40 + image_size as usize);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    // The height covers the colour bitmap and the mask.
    out.extend_from_slice(&(size * 2).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&image_size.to_le_bytes());
    out.extend_from_slice(&[0; 16]); // resolution and palette: unused
    for row in (0..size).rev() {
        for column in 0..size {
            let [r, g, b, a] = pixels[(row * size + column) as usize];
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    for row in (0..size).rev() {
        let mut bits = vec![0u8; mask_row as usize];
        for column in 0..size {
            if pixels[(row * size + column) as usize][3] == 0 {
                bits[(column / 8) as usize] |= 0x80 >> (column % 8);
            }
        }
        out.extend_from_slice(&bits);
    }
    out
}

/// The `.ico` file with every size of [`ICO_SIZES`].
pub fn ico_file() -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = ICO_SIZES.iter().map(|&size| (size, dib(size))).collect();
    let count = images.len() as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // an icon
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = 6 + 16 * u32::from(count);
    for (size, image) in &images {
        // A width or height of 256 is written as 0; the sizes here stay below.
        let side = u8::try_from(*size).unwrap_or(0);
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        let length = image.len() as u32;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += length;
    }
    for (_, image) in images {
        out.extend_from_slice(&image);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([bytes[at], bytes[at + 1]])
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    }

    #[test]
    fn the_32_pixel_image_keeps_the_prototype_drawing() {
        let image = pixels(32);
        assert_eq!(image.len(), 32 * 32);
        let at = |x: usize, y: usize| image[y * 32 + x];
        // A key, the body between keys, the space bar, the transparent corners.
        assert_eq!(at(5, 10), [0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(at(7, 10), [0x1F, 0x5F, 0xB8, 0xFF]);
        assert_eq!(at(15, 20), [0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(at(0, 0), [0, 0, 0, 0]);
        assert_eq!(at(31, 31), [0, 0, 0, 0]);
        // The rounded corner is smoothed, not cut.
        let corner = at(2, 6)[3];
        assert!(corner > 0 && corner < 0xFF, "{corner}");
    }

    #[test]
    fn the_ico_file_is_well_formed() {
        let ico = ico_file();
        assert_eq!((u16_at(&ico, 0), u16_at(&ico, 2)), (0, 1));
        assert_eq!(usize::from(u16_at(&ico, 4)), ICO_SIZES.len());
        let mut expected_offset = 6 + 16 * ICO_SIZES.len();
        for (index, &size) in ICO_SIZES.iter().enumerate() {
            let entry = 6 + 16 * index;
            assert_eq!(u32::from(ico[entry]), size);
            assert_eq!(u32::from(ico[entry + 1]), size);
            assert_eq!((u16_at(&ico, entry + 4), u16_at(&ico, entry + 6)), (1, 32));
            let length = u32_at(&ico, entry + 8) as usize;
            let offset = u32_at(&ico, entry + 12) as usize;
            assert_eq!(offset, expected_offset);
            expected_offset += length;
            // BITMAPINFOHEADER: width, double height, 32 bits, BI_RGB.
            assert_eq!(u32_at(&ico, offset), 40);
            assert_eq!(u32_at(&ico, offset + 4), size);
            assert_eq!(u32_at(&ico, offset + 8), size * 2);
            assert_eq!(u16_at(&ico, offset + 14), 32);
            assert_eq!(u32_at(&ico, offset + 16), 0);
            let mask_row = size.div_ceil(32) * 4;
            assert_eq!(length, (40 + size * size * 4 + mask_row * size) as usize);
        }
        assert_eq!(expected_offset, ico.len());
    }
}
