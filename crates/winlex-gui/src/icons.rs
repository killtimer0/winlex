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

owned_handle! { pub struct WindowIcon(pub HICON); DestroyIcon, null = ptr::null_mut() }

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
pub struct DocumentIcons(Vec<(Icon, i32, i32, u32, Weak<WindowIcons>)>);
impl DocumentIcons {
    pub fn get(&mut self, icon: Icon, dpi: u32) -> anyhow::Result<Rc<WindowIcons>> {
        let small = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
        let large = unsafe { GetSystemMetricsForDpi(SM_CXICON, dpi) };
        let (id, color) = ICONS[icon as usize];
        let color = symbol_color(color);
        self.0
            .retain(|(_, _, _, _, value)| value.strong_count() != 0);
        for (kind, s, l, tint, value) in &self.0 {
            if (*kind, *s, *l, *tint) == (icon, small, large, color)
                && let Some(value) = value.upgrade()
            {
                return Ok(value);
            }
        }
        let render = |size| unsafe {
            let images = Images::build(size, std::iter::once((id, [color])), "document")?;
            let handle = ImageList_GetIcon(images.0, 0, ILD_TRANSPARENT);
            anyhow::ensure!(!handle.is_null(), "Create document icon failed");
            Ok(WindowIcon(handle))
        };
        let value = Rc::new(WindowIcons {
            small: render(small)?,
            large: render(large)?,
        });
        self.0
            .push((icon, small, large, color, Rc::downgrade(&value)));
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
fn kind_entry(kind: &str) -> Option<(Icon, u16)> {
    KINDS
        .iter()
        .find(|(value, _, _)| *value == kind)
        .map(|&(_, icon, label)| (icon, label))
}
impl Icon {
    pub fn for_kind(kind: &str) -> Self {
        kind_entry(kind).map_or(Self::Misc, |(icon, _)| icon)
    }
}

pub fn kind_label(kind: &str) -> u16 {
    kind_entry(kind).map_or(IDS_KIND_UNKNOWN, |(_, label)| label)
}

/// Single presentation vocabulary for metadata entity kinds.
const KINDS: &[(&str, Icon, u16)] = &[
    ("class", Icon::Class, IDS_KIND_CLASS),
    ("interface", Icon::Interface, IDS_KIND_INTERFACE),
    ("struct", Icon::Struct, IDS_KIND_STRUCT),
    ("enum", Icon::Enum, IDS_KIND_ENUM),
    ("delegate", Icon::Method, IDS_KIND_DELEGATE),
    ("attribute", Icon::Class, IDS_KIND_ATTRIBUTE),
    ("module", Icon::Misc, IDS_KIND_MODULE),
    ("method", Icon::Method, IDS_KIND_METHOD),
    ("constant", Icon::Constant, IDS_KIND_CONSTANT),
    ("field", Icon::Field, IDS_KIND_FIELD),
    ("property", Icon::Property, IDS_KIND_PROPERTY),
    ("event", Icon::Event, IDS_KIND_EVENT),
    ("type_reference", Icon::Reference, IDS_KIND_REFERENCE),
];

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

fn symbol_color(color: u32) -> u32 {
    if high_contrast() {
        crate::theme::color(COLOR_WINDOWTEXT)
    } else if crate::theme::is_dark() {
        lighten(color)
    } else {
        color
    }
}

fn lighten(color: u32) -> u32 {
    let channel = |shift: u32| (((color >> shift) & 255) + 255) / 2;
    channel(0) | channel(8) << 8 | channel(16) << 16
}

fn check(status: Status) -> anyhow::Result<()> {
    anyhow::ensure!(status == 0, "GDI+ icon operation failed: {status}");
    Ok(())
}

owned_handle! { pub struct Runtime(usize); GdiplusShutdown }
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
owned_handle! { struct Image(*mut GpImage); GdipDisposeImage }
owned_handle! { struct Graphics(*mut GpGraphics); GdipDeleteGraphics }
struct Bitmap {
    handle: GdiObject,
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
                handle: GdiObject(handle),
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

owned_handle! { pub struct Images(HIMAGELIST); ImageList_Destroy }
impl Images {
    pub fn toolbar(
        size: i32,
        resources: impl ExactSizeIterator<Item = u16>,
    ) -> anyhow::Result<Self> {
        let color = crate::theme::color(COLOR_BTNTEXT);
        Self::build(size, resources.map(|id| (id, [color])), "toolbar")
    }
    pub fn handle(&self) -> HIMAGELIST {
        self.0
    }
    pub fn new(size: i32) -> anyhow::Result<Self> {
        let (normal, selected) = (
            crate::theme::color(COLOR_WINDOWTEXT),
            crate::theme::color(COLOR_HIGHLIGHTTEXT),
        );
        Self::build(
            size,
            ICONS
                .iter()
                .map(|&(id, color)| (id, [symbol_color(color), normal, selected])),
            "result",
        )
    }
    fn build<const N: usize>(
        size: i32,
        icons: impl ExactSizeIterator<Item = (u16, [u32; N])>,
        purpose: &str,
    ) -> anyhow::Result<Self> {
        unsafe {
            let handle = ImageList_Create(size, size, ILC_COLOR32, (icons.len() * N) as i32, 0);
            anyhow::ensure!(handle != 0, "Create {purpose} icon list failed");
            let images = Self(handle);
            for (id, colors) in icons {
                let mut bitmap = Bitmap::render(resources::binary(id)?, size)?;
                for color in colors {
                    bitmap.tint(color);
                    anyhow::ensure!(
                        ImageList_Add(handle, bitmap.handle.0, ptr::null_mut()) >= 0,
                        "Cache {purpose} icon failed"
                    );
                }
            }
            Ok(images)
        }
    }
    pub fn draw(&self, icon: Icon, dc: HDC, x: i32, y: i32, system: bool, text_color: u32) {
        let variant = if system {
            if text_color == crate::theme::color(COLOR_HIGHLIGHTTEXT) {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recoloring_reuses_alpha_without_carrying_previous_palette_colors() {
        let mut pixels = [0, 0x80332211, 0xff664422];
        let mut bitmap = Bitmap {
            handle: GdiObject(ptr::null_mut()),
            pixels: pixels.as_mut_ptr(),
            count: pixels.len(),
        };
        for (color, expected) in [
            (0x993366, [0, 0x8033194c, 0xff663399]),
            (0xffffff, [0, 0x80808080, 0xffffffff]),
            (0, [0, 0x80000000, 0xff000000]),
            (0x993366, [0, 0x8033194c, 0xff663399]),
        ] {
            bitmap.tint(color);
            assert_eq!(pixels, expected, "color {color:06x}");
        }
    }

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
