#![cfg(windows)]

//! Windows-only icon extraction.
//!
//! Unlike Linux `.desktop` files, `.lnk` shortcuts and `.exe` files do not point
//! at an icon file on disk: the icon is either embedded in the binary or resolved
//! by the shell. `SHGetFileInfoW` hands back the shell's `HICON` for a path, which
//! is then read into an RGBA buffer and wrapped in a Slint `Image`.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW, HBITMAP, SelectObject,
};
use windows_sys::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

/// Extract the shell icon for `path` (a `.lnk`, `.exe`, or any file the shell
/// knows how to render) as a Slint image.
pub fn extract_icon_from_path(path: &Path) -> Option<Image> {
    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut shfi: SHFILEINFOW = std::mem::zeroed();
        // SHGFI_LARGEICON is 32x32, plenty for a launcher row. The shell resolves
        // the shortcut target for us, so a .lnk yields the target's icon.
        let res = SHGetFileInfoW(
            wide_path.as_ptr(),
            0,
            &mut shfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );

        if res == 0 || shfi.hIcon.is_null() {
            return None;
        }

        let img = hicon_to_slint_image(shfi.hIcon);
        // The HICON returned by SHGetFileInfoW is owned by us.
        DestroyIcon(shfi.hIcon);
        img
    }
}

/// Copy an `HICON`'s colour bitmap into a straight-alpha RGBA pixel buffer.
fn hicon_to_slint_image(hicon: HICON) -> Option<Image> {
    let (hbm_color, hbm_mask, width, height) = unsafe {
        let mut icon_info: ICONINFO = std::mem::zeroed();
        if GetIconInfo(hicon, &mut icon_info) == 0 {
            return None;
        }

        let hbm_color = icon_info.hbmColor;
        let hbm_mask = icon_info.hbmMask;

        // A monochrome icon has no colour bitmap; nothing worth converting.
        if hbm_color.is_null() {
            delete_bitmaps(hbm_color, hbm_mask);
            return None;
        }

        let mut bm: BITMAP = std::mem::zeroed();
        if GetObjectW(
            hbm_color as _,
            std::mem::size_of::<BITMAP>() as i32,
            &mut bm as *mut _ as _,
        ) == 0
        {
            delete_bitmaps(hbm_color, hbm_mask);
            return None;
        }

        if bm.bmWidth <= 0 || bm.bmHeight <= 0 {
            delete_bitmaps(hbm_color, hbm_mask);
            return None;
        }

        (hbm_color, hbm_mask, bm.bmWidth as u32, bm.bmHeight as u32)
    };

    let buffer = unsafe {
        let hdc = CreateCompatibleDC(std::ptr::null_mut());
        if hdc.is_null() {
            delete_bitmaps(hbm_color, hbm_mask);
            return None;
        }
        let old_obj = SelectObject(hdc, hbm_color as _);

        // Negative height requests a top-down DIB, matching Slint's row order.
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width as i32;
        bmi.bmiHeader.biHeight = -(height as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let mut buffer: Vec<u8> = vec![0; (width * height * 4) as usize];
        let rows = GetDIBits(
            hdc,
            hbm_color,
            0,
            height,
            buffer.as_mut_ptr() as _,
            &mut bmi,
            DIB_RGB_COLORS,
        );

        SelectObject(hdc, old_obj);
        DeleteDC(hdc);
        delete_bitmaps(hbm_color, hbm_mask);

        if rows == 0 {
            return None;
        }
        buffer
    };

    // Icon bitmaps come back premultiplied and in BGRA order. Slint wants
    // straight RGBA, so undo the premultiplication while swizzling. Icons whose
    // alpha channel is entirely zero are treated as opaque.
    let has_alpha = buffer.chunks_exact(4).any(|c| c[3] != 0);

    let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let pixels = pixel_buffer.make_mut_slice();
    for (src, dst) in buffer.chunks_exact(4).zip(pixels.iter_mut()) {
        let alpha = if has_alpha { src[3] } else { 0xFF };
        dst.r = unpremultiply(src[2], alpha);
        dst.g = unpremultiply(src[1], alpha);
        dst.b = unpremultiply(src[0], alpha);
        dst.a = alpha;
    }

    Some(Image::from_rgba8(pixel_buffer))
}

/// Icon bitmaps store premultiplied colour; divide it back out (or, for a fully
/// transparent icon where the colour is meaningless, leave it as-is).
fn unpremultiply(value: u8, alpha: u8) -> u8 {
    if alpha == 0 || alpha == 0xFF {
        return value;
    }
    ((value as u16 * 255) / alpha as u16).min(255) as u8
}

/// Both bitmaps come from `GetIconInfo` and are ours to release.
fn delete_bitmaps(hbm_color: HBITMAP, hbm_mask: HBITMAP) {
    unsafe {
        if !hbm_color.is_null() {
            DeleteObject(hbm_color as _);
        }
        if !hbm_mask.is_null() {
            DeleteObject(hbm_mask as _);
        }
    }
}
