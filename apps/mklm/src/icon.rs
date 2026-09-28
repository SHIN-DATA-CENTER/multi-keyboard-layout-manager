//! The window and tray icon (from the M0 #8 prototype), drawn in code by [`crate::icon_image`] —
//! the same drawing as the executable's icon resource, which build.rs makes from it (design m3
//! F.7) — so that the GUI needs no image assets.

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

const SIZE: u32 = 32;

/// 32x32 RGBA keyboard icon used for the window and the tray.
pub fn app_icon() -> Image {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(SIZE, SIZE);
    for (px, [r, g, b, a]) in buffer
        .make_mut_slice()
        .iter_mut()
        .zip(crate::icon_image::pixels(SIZE))
    {
        *px = Rgba8Pixel::new(r, g, b, a);
    }
    Image::from_rgba8(buffer)
}
