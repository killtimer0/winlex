use crate::{
    native::*,
    resources::{self, *},
};
use std::{
    ptr,
    rc::{Rc, Weak},
};
use windows_sys::Win32::Graphics::GdiPlus::{
    GdipCreateBitmapFromScan0, GdipCreateMetafileFromEmf, GdipDeleteGraphics, GdipDisposeImage,
    GdipDrawImageRectRectI, GdipGetImageGraphicsContext, GdipGraphicsClear, GdipSetSmoothingMode,
    GdiplusShutdown, GdiplusStartup, GdiplusStartupInput, GpGraphics, GpImage,
    SmoothingModeAntiAlias, Status, UnitPixel,
};
use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;

struct WindowIcon(HICON);
impl Drop for WindowIcon {
    fn drop(&mut self) {
        unsafe { DestroyIcon(self.0) };
    }
}

pub struct WindowIcons {
    small: WindowIcon,
    large: WindowIcon,
}
impl WindowIcons {
    pub fn load(dpi: u32) -> anyhow::Result<Self> {
        let load = |x, y| unsafe {
            let icon = LoadImageW(
                instance(),
                IDI_APP as usize as *const u16,
                IMAGE_ICON,
                GetSystemMetricsForDpi(x, dpi),
                GetSystemMetricsForDpi(y, dpi),
                0,
            );
            if icon.is_null() {
                Err(error("Load application icon"))
            } else {
                Ok(WindowIcon(icon))
            }
        };
        Ok(Self {
            small: load(SM_CXSMICON, SM_CYSMICON)?,
            large: load(SM_CXICON, SM_CYICON)?,
        })
    }
    pub fn apply(&self, hwnd: HWND) {
        send(hwnd, WM_SETICON, ICON_SMALL as usize, self.small.0 as isize);
        send(hwnd, WM_SETICON, ICON_BIG as usize, self.large.0 as isize);
    }
}

#[derive(Default)]
pub struct DocumentIcons(Vec<(Icon, i32, i32, Weak<WindowIcons>)>);
impl DocumentIcons {
    pub fn get(&mut self, icon: Icon, dpi: u32) -> anyhow::Result<Rc<WindowIcons>> {
        let small = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
        let large = unsafe { GetSystemMetricsForDpi(SM_CXICON, dpi) };
        self.0.retain(|(_, _, _, value)| value.strong_count() != 0);
        for (kind, s, l, value) in &self.0 {
            if (*kind, *s, *l) == (icon, small, large)
                && let Some(value) = value.upgrade()
            {
                return Ok(value);
            }
        }
        let render = |size| unsafe {
            let (id, color) = ICONS[icon as usize];
            let mut bitmap = Bitmap::render(resources::binary(id)?, size)?;
            bitmap.tint(color);
            let handle = ImageList_Create(size, size, ILC_COLOR32, 1, 0);
            anyhow::ensure!(handle != 0, "Create document icon list failed");
            let images = Images(handle);
            anyhow::ensure!(
                ImageList_Add(images.0, bitmap.handle, ptr::null_mut()) >= 0,
                "Cache document icon failed"
            );
            let handle = ImageList_GetIcon(images.0, 0, ILD_TRANSPARENT);
            anyhow::ensure!(!handle.is_null(), "Create document icon failed");
            Ok(WindowIcon(handle))
        };
        let value = Rc::new(WindowIcons {
            small: render(small)?,
            large: render(large)?,
        });
        self.0.push((icon, small, large, Rc::downgrade(&value)));
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(usize)]
pub enum Icon {
    Class,
    Interface,
    Struct,
    Enum,
    Method,
    Constant,
    Field,
    Property,
    Event,
    Reference,
    #[default]
    Misc,
    File,
}
impl Icon {
    pub fn for_kind(kind: &str) -> Self {
        match kind {
            "class" | "attribute" => Self::Class,
            "interface" => Self::Interface,
            "struct" => Self::Struct,
            "enum" => Self::Enum,
            "method" | "delegate" => Self::Method,
            "constant" => Self::Constant,
            "field" => Self::Field,
            "property" => Self::Property,
            "event" => Self::Event,
            "type_reference" => Self::Reference,
            _ => Self::Misc,
        }
    }
}

const ICONS: &[(u16, u32)] = &[
    (IDR_SYMBOL_CLASS, 0x993366),
    (IDR_SYMBOL_INTERFACE, 0x997333),
    (IDR_SYMBOL_STRUCT, 0x668000),
    (IDR_SYMBOL_ENUM, 0x007399),
    (IDR_SYMBOL_METHOD, 0x993366),
    (IDR_SYMBOL_CONSTANT, 0x996633),
    (IDR_SYMBOL_FIELD, 0x997333),
    (IDR_SYMBOL_PROPERTY, 0x666666),
    (IDR_SYMBOL_EVENT, 0x007399),
    (IDR_SYMBOL_REFERENCE, 0x666666),
    (IDR_SYMBOL_MISC, 0x666666),
    (IDR_FILE, 0x666666),
];

fn check(status: Status) -> anyhow::Result<()> {
    anyhow::ensure!(status == 0, "GDI+ icon operation failed: {status}");
    Ok(())
}

pub struct Runtime(usize);
impl Runtime {
    pub fn new() -> anyhow::Result<Self> {
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..unsafe { std::mem::zeroed() }
        };
        let mut token = 0;
        check(unsafe { GdiplusStartup(&mut token, &input, ptr::null_mut()) })?;
        Ok(Self(token))
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            GdiplusShutdown(self.0);
        }
    }
}
struct Image(*mut GpImage);
impl Drop for Image {
    fn drop(&mut self) {
        unsafe {
            GdipDisposeImage(self.0);
        }
    }
}
struct Graphics(*mut GpGraphics);
impl Drop for Graphics {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteGraphics(self.0);
        }
    }
}
struct Bitmap {
    handle: HBITMAP,
    pixels: *mut u32,
    count: usize,
}
impl Bitmap {
    fn render(bytes: &[u8], size: i32) -> anyhow::Result<Self> {
        unsafe {
            let emf = SetEnhMetaFileBits(bytes.len() as u32, bytes.as_ptr());
            anyhow::ensure!(!emf.is_null(), "Load result icon failed");
            let mut metafile = ptr::null_mut();
            let status = GdipCreateMetafileFromEmf(emf, 1, &mut metafile);
            if status != 0 {
                DeleteEnhMetaFile(emf);
                check(status)?;
            }
            let metafile = Image(metafile.cast());
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size,
                    biHeight: -size,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..std::mem::zeroed()
                },
                ..std::mem::zeroed()
            };
            let mut pixels = ptr::null_mut();
            let handle = CreateDIBSection(
                ptr::null_mut(),
                &info,
                DIB_RGB_COLORS,
                &mut pixels,
                ptr::null_mut(),
                0,
            );
            anyhow::ensure!(!handle.is_null(), "Create result icon bitmap failed");
            let bitmap = Self {
                handle,
                pixels: pixels.cast(),
                count: (size * size) as usize,
            };
            let mut image = ptr::null_mut();
            // PixelFormat32bppPARGB, available in GDI+ 1.0 on XP RTM.
            check(GdipCreateBitmapFromScan0(
                size,
                size,
                size * 4,
                0x000e200b,
                pixels.cast(),
                &mut image,
            ))?;
            let image = Image(image.cast());
            let mut graphics = ptr::null_mut();
            check(GdipGetImageGraphicsContext(image.0, &mut graphics))?;
            let graphics = Graphics(graphics);
            check(GdipGraphicsClear(graphics.0, 0))?;
            check(GdipSetSmoothingMode(graphics.0, SmoothingModeAntiAlias))?;
            // Metafile device bounds vary; source coordinates use the SVG's 16px viewBox.
            check(GdipDrawImageRectRectI(
                graphics.0,
                metafile.0,
                0,
                0,
                size,
                size,
                0,
                0,
                16,
                16,
                UnitPixel,
                ptr::null(),
                0,
                ptr::null_mut(),
            ))?;
            Ok(bitmap)
        }
    }
    fn tint(&mut self, color: u32) {
        for pixel in unsafe { std::slice::from_raw_parts_mut(self.pixels, self.count) } {
            let alpha = *pixel >> 24;
            let r = (color & 255) * alpha / 255;
            let g = ((color >> 8) & 255) * alpha / 255;
            let b = ((color >> 16) & 255) * alpha / 255;
            *pixel = alpha << 24 | r << 16 | g << 8 | b;
        }
    }
}
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.handle);
        }
    }
}

pub struct Images(HIMAGELIST);
impl Images {
    pub fn toolbar(
        size: i32,
        resources: impl ExactSizeIterator<Item = u16>,
    ) -> anyhow::Result<Self> {
        unsafe {
            let handle = ImageList_Create(size, size, ILC_COLOR32, resources.len() as i32, 0);
            anyhow::ensure!(handle != 0, "Create toolbar icon list failed");
            let images = Self(handle);
            for id in resources {
                let mut bitmap = Bitmap::render(resources::binary(id)?, size)?;
                bitmap.tint(GetSysColor(COLOR_BTNTEXT));
                anyhow::ensure!(
                    ImageList_Add(handle, bitmap.handle, ptr::null_mut()) >= 0,
                    "Cache toolbar icon failed"
                );
            }
            Ok(images)
        }
    }
    pub fn handle(&self) -> HIMAGELIST {
        self.0
    }
    pub fn new(size: i32) -> anyhow::Result<Self> {
        unsafe {
            let handle = ImageList_Create(size, size, ILC_COLOR32, ICONS.len() as i32 * 3, 0);
            anyhow::ensure!(handle != 0, "Create result icon list failed");
            let images = Self(handle);
            for &(id, color) in ICONS {
                let mut bitmap = Bitmap::render(resources::binary(id)?, size)?;
                for variant in 0..3 {
                    let color = match variant {
                        1 => GetSysColor(COLOR_WINDOWTEXT),
                        2 => GetSysColor(COLOR_HIGHLIGHTTEXT),
                        _ => color,
                    };
                    bitmap.tint(color);
                    anyhow::ensure!(
                        ImageList_Add(handle, bitmap.handle, ptr::null_mut()) >= 0,
                        "Cache result icon failed"
                    );
                }
            }
            Ok(images)
        }
    }
    pub fn draw(&self, icon: Icon, dc: HDC, x: i32, y: i32, system: bool, text_color: u32) {
        let variant = if system {
            if text_color == unsafe { GetSysColor(COLOR_HIGHLIGHTTEXT) } {
                2
            } else {
                1
            }
        } else {
            0
        };
        unsafe {
            ImageList_Draw(self.0, icon as i32 * 3 + variant, dc, x, y, ILD_TRANSPARENT);
        }
    }
}
impl Drop for Images {
    fn drop(&mut self) {
        unsafe {
            ImageList_Destroy(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn emf_plus_icons_render_with_alpha_at_each_supported_scale() {
        let _runtime = Runtime::new().unwrap();
        let assets: &[&[u8]] = &[
            include_bytes!("../res/codicons/symbol-class.emf"),
            include_bytes!("../res/codicons/symbol-interface.emf"),
            include_bytes!("../res/codicons/symbol-structure.emf"),
            include_bytes!("../res/codicons/symbol-enum.emf"),
            include_bytes!("../res/codicons/symbol-method.emf"),
            include_bytes!("../res/codicons/symbol-constant.emf"),
            include_bytes!("../res/codicons/symbol-field.emf"),
            include_bytes!("../res/codicons/symbol-property.emf"),
            include_bytes!("../res/codicons/symbol-event.emf"),
            include_bytes!("../res/codicons/references.emf"),
            include_bytes!("../res/codicons/symbol-misc.emf"),
            include_bytes!("../res/codicons/file.emf"),
            include_bytes!("../res/codicons/search.emf"),
            include_bytes!("../res/codicons/files.emf"),
        ];
        for &bytes in assets {
            assert!(bytes.windows(4).any(|b| b == b"EMF+"));
            for size in [16, 20, 24, 32, 48] {
                let mut bitmap = Bitmap::render(bytes, size).unwrap();
                bitmap.tint(0x00336699);
                let pixels = unsafe { std::slice::from_raw_parts(bitmap.pixels, bitmap.count) };
                assert!(
                    pixels.iter().any(|&p| p >> 24 > 0),
                    "empty icon at {size}px; max pixel {:08x}",
                    pixels.iter().max().unwrap()
                );
                assert!(pixels.contains(&0));
                let occupied: Vec<_> = pixels
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| **p >> 24 != 0)
                    .map(|(i, _)| i as i32 % size)
                    .collect();
                assert!(
                    (occupied.iter().min().unwrap() + occupied.iter().max().unwrap() + 1 - size)
                        .abs()
                        <= size / 4,
                    "icon is not centered at {size}px"
                );
                assert!(pixels.iter().any(|&p| p >> 24 != 0 && p >> 24 != 255));
                assert!(pixels.iter().all(|&p| (p & 255) <= p >> 24
                    && ((p >> 8) & 255) <= p >> 24
                    && ((p >> 16) & 255) <= p >> 24));
            }
        }
    }
}
