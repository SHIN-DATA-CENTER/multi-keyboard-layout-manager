//! The window and tray icon, drawn in code so that the GUI needs no image assets (from the M0 #8
//! prototype). The executable's own icon resource comes with WP-U7 (design m3 F.7).

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

const SIZE: u32 = 32;

fn inside_rounded(x: u32, y: u32, (x0, y0, x1, y1): (u32, u32, u32, u32), r: u32) -> bool {
    if x < x0 || x > x1 || y < y0 || y > y1 {
        return false;
    }
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    let (dx, dy) = (x.abs_diff(cx), y.abs_diff(cy));
    dx * dx + dy * dy <= r * r
}

/// 32x32 RGBA keyboard icon used for the window and the tray.
pub fn app_icon() -> Image {
    let body = Rgba8Pixel::new(0x1F, 0x5F, 0xB8, 0xFF);
    let key = Rgba8Pixel::new(0xFF, 0xFF, 0xFF, 0xFF);
    let clear = Rgba8Pixel::new(0, 0, 0, 0);

    let mut keys = Vec::new();
    for (row, y) in [9u32, 14].into_iter().enumerate() {
        let offset = if row == 0 { 4 } else { 5 };
        for i in 0..6 {
            let x = offset + i * 4;
            keys.push((x, y, x + 2, y + 2));
        }
    }
    keys.push((8, 19, 23, 21));

    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(SIZE, SIZE);
    for (i, px) in buffer.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = (i as u32 % SIZE, i as u32 / SIZE);
        *px = if keys
            .iter()
            .any(|&(x0, y0, x1, y1)| (x0..=x1).contains(&x) && (y0..=y1).contains(&y))
        {
            key
        } else if inside_rounded(x, y, (1, 5, 30, 26), 4) {
            body
        } else {
            clear
        };
    }
    Image::from_rgba8(buffer)
}
