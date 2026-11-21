//! Screen frames for a recording: DXGI Desktop Duplication, with GDI as the fallback.
//!
//! Desktop Duplication (Windows 8+) hands over the composed desktop of one monitor from the
//! GPU, without the pointer and without a yellow border, and only when something changed. It
//! fails on some laptops with two GPUs when Blink runs on the other GPU than the one that
//! drives the monitor (DXGI_ERROR_UNSUPPORTED): the device is therefore made on the adapter
//! that owns the output. It is also lost on a resolution change, a UAC prompt or a full-screen
//! switch (DXGI_ERROR_ACCESS_LOST), and is then set up again. Rotated monitors and HDR formats
//! other than 8-bit BGRA go to GDI, which copies the screen with BitBlt at a lower frame rate.
//!
//! Both copy only the recorded area into tightly packed top-down BGRA rows.

use super::super::capture::IRect;
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::*;
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};

/// What a grab brought.
#[derive(Debug, PartialEq, Eq)]
pub enum Grab {
    /// A new picture is in the buffer.
    Frame,
    /// Nothing changed: the buffer still holds the last picture.
    Same,
}

#[derive(Debug)]
pub enum GrabError {
    /// Set up again and go on (resolution change, UAC prompt, ...).
    Lost,
    /// Give up on this method.
    Failed(String),
}

pub trait Grabber {
    /// Copies the area into `out` (w * h * 4 bytes, BGRA, top-down), waiting at most
    /// `timeout_ms` for a change.
    fn grab(&mut self, timeout_ms: u32, out: &mut Vec<u8>) -> Result<Grab, GrabError>;
    fn name(&self) -> &'static str;
}

// ---- Desktop Duplication -------------------------------------------------------------------

pub struct Duplication {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    /// The area relative to the top-left of the monitor.
    area: IRect,
    /// The first frame always counts as new, even when the system says nothing changed.
    first: bool,
}

// Used from the capture thread only.
unsafe impl Send for Duplication {}

fn err(what: &str, e: windows::core::Error) -> String {
    format!("{what}: {e} ({:#010x})", e.code().0 as u32)
}

impl Duplication {
    /// Duplication of the monitor at `monitor` (desktop coordinates), for `area` (desktop
    /// coordinates, inside the monitor).
    pub fn new(monitor: IRect, area: IRect) -> Result<Self, String> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| err("CreateDXGIFactory1", e))?;
            let mut found = None;
            let mut i = 0;
            'adapters: while let Ok(adapter) = factory.EnumAdapters1(i) {
                i += 1;
                let mut j = 0;
                while let Ok(output) = adapter.EnumOutputs(j) {
                    j += 1;
                    let Ok(desc) = output.GetDesc() else { continue };
                    let r = desc.DesktopCoordinates;
                    let rect = IRect::from_ltrb(r.left, r.top, r.right, r.bottom);
                    if rect == monitor {
                        if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY
                            && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED
                        {
                            return Err("the monitor is rotated".into());
                        }
                        found = Some((adapter, output));
                        break 'adapters;
                    }
                }
            }
            let (adapter, output) = found.ok_or("no DXGI output for this monitor")?;
            let mut device = None;
            let mut context = None;
            // The device on the adapter that drives the monitor (two-GPU laptops).
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|e| err("D3D11CreateDevice", e))?;
            let device = device.ok_or("no D3D11 device")?;
            let context = context.ok_or("no D3D11 context")?;
            // Ask for 8-bit BGRA where Windows can convert to it (HDR screens, 1703+).
            let dup = match output.cast::<IDXGIOutput5>() {
                Ok(o5) => o5.DuplicateOutput1(&device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM]),
                Err(_) => output.cast::<IDXGIOutput1>().map_err(|e| err("IDXGIOutput1", e))?.DuplicateOutput(&device),
            }
            .map_err(|e| err("DuplicateOutput", e))?;
            let local = IRect { x: area.x - monitor.x, y: area.y - monitor.y, ..area };
            let desc = D3D11_TEXTURE2D_DESC {
                Width: local.w as u32,
                Height: local.h as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            device.CreateTexture2D(&desc, None, Some(&mut staging)).map_err(|e| err("CreateTexture2D", e))?;
            let staging = staging.ok_or("no staging texture")?;
            Ok(Duplication { device, context, dup, staging, area: local, first: true })
        }
    }
}

impl Grabber for Duplication {
    fn name(&self) -> &'static str {
        "DXGI Desktop Duplication"
    }

    fn grab(&mut self, timeout_ms: u32, out: &mut Vec<u8>) -> Result<Grab, GrabError> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            if let Err(e) = self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut resource) {
                return match e.code() {
                    c if c == DXGI_ERROR_WAIT_TIMEOUT => Ok(Grab::Same),
                    c if c == DXGI_ERROR_ACCESS_LOST || c == DXGI_ERROR_INVALID_CALL => Err(GrabError::Lost),
                    _ => Err(GrabError::Failed(err("AcquireNextFrame", e))),
                };
            }
            // Only the pointer moved: the picture is the same.
            let changed = info.LastPresentTime != 0 || self.first;
            let result = if changed {
                match resource.as_ref().map(|r| r.cast::<ID3D11Texture2D>()) {
                    Some(Ok(tex)) => self.copy(&tex, out),
                    _ => Err(GrabError::Failed("no desktop texture".into())),
                }
            } else {
                Ok(Grab::Same)
            };
            let _ = self.dup.ReleaseFrame();
            if result.is_ok() {
                self.first = false;
            }
            result
        }
    }
}

impl Duplication {
    unsafe fn copy(&self, tex: &ID3D11Texture2D, out: &mut Vec<u8>) -> Result<Grab, GrabError> {
        // An HDR desktop that Windows did not convert to 8-bit BGRA: let GDI do it.
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(GrabError::Failed(format!("desktop format {:?} is not 8-bit BGRA", desc.Format)));
        }
        let a = self.area;
        let bx = D3D11_BOX {
            left: a.x as u32,
            top: a.y as u32,
            front: 0,
            right: (a.x + a.w) as u32,
            bottom: (a.y + a.h) as u32,
            back: 1,
        };
        self.context.CopySubresourceRegion(&self.staging, 0, 0, 0, 0, tex, 0, Some(&bx));
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        self.context
            .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| GrabError::Failed(err("Map", e)))?;
        let row = a.w as usize * 4;
        out.resize(row * a.h as usize, 0);
        for y in 0..a.h as usize {
            let src = std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), row);
            out[y * row..(y + 1) * row].copy_from_slice(src);
        }
        self.context.Unmap(&self.staging, 0);
        let _ = &self.device;
        Ok(Grab::Frame)
    }
}

// ---- GDI -------------------------------------------------------------------------------------

/// BitBlt of the area from the screen into a DIB that is kept between frames. Without
/// CAPTUREBLT: with composition always on it adds nothing, and it makes the pointer flicker.
pub struct Gdi {
    area: IRect,
    screen: HDC,
    mem: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
}

unsafe impl Send for Gdi {}

impl Gdi {
    pub fn new(area: IRect) -> Result<Self, String> {
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            if screen.is_null() {
                return Err("GetDC failed".into());
            }
            let mem = CreateCompatibleDC(screen);
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: area.w,
                biHeight: -area.h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                DeleteDC(mem);
                ReleaseDC(std::ptr::null_mut(), screen);
                return Err("CreateDIBSection failed".into());
            }
            let old = SelectObject(mem, bmp);
            Ok(Gdi { area, screen, mem, bmp, old, bits: bits as *mut u8 })
        }
    }
}

impl Grabber for Gdi {
    fn name(&self) -> &'static str {
        "GDI BitBlt"
    }

    fn grab(&mut self, _timeout_ms: u32, out: &mut Vec<u8>) -> Result<Grab, GrabError> {
        let a = self.area;
        unsafe {
            if BitBlt(self.mem, 0, 0, a.w, a.h, self.screen, a.x, a.y, SRCCOPY) == 0 {
                // The secure desktop (UAC) cannot be copied: keep the last picture.
                return Ok(Grab::Same);
            }
            let len = a.w as usize * a.h as usize * 4;
            out.resize(len, 0);
            out.copy_from_slice(std::slice::from_raw_parts(self.bits, len));
        }
        Ok(Grab::Frame)
    }
}

impl Drop for Gdi {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.mem, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.mem);
            ReleaseDC(std::ptr::null_mut(), self.screen);
        }
    }
}
