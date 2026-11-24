//! Screenshots of HDR screens (Settings > Capture, experimental, off by default).
//!
//! A screenshot is always taken with BitBlt first (`capture::grab`). With the setting on,
//! each monitor that has HDR on and lies in the screenshot is then read again as Windows
//! composes it, in scRGB (16-bit floats, via DXGI Desktop Duplication), tone mapped with
//! `crate::hdr` and written over that monitor's part of the screenshot. Monitors without HDR
//! keep the BitBlt picture; when anything fails, or the frame read is far darker than the
//! BitBlt picture (some drivers hand over an empty, black frame), the BitBlt picture stays and
//! the reason goes to `%LOCALAPPDATA%\Blink\hdr.log`.
//!
//! Cost: the check whether a monitor has HDR on is a call into user32 (already loaded), with
//! no allocations worth mentioning. Only when a monitor has HDR on does Blink load the
//! graphics driver (a D3D11 device) and read that monitor: keep it that way, so the setting
//! costs nothing on PCs without an HDR screen.

use super::capture::IRect;
use crate::hdr::{looks_blank, map_row, mean_bgra, mean_mapped, row_peak, Curve, Srgb, SAMPLE};
use std::io::Write;
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::*;
use windows_sys::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    QDC_ONLY_ACTIVE_PATHS,
};
use windows_sys::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};

/// How many frames to try when Desktop Duplication hands over an empty one.
const FRAME_ATTEMPTS: u32 = 3;

/// A monitor with HDR on.
struct HdrMonitor {
    rect: IRect,
    /// SDR white in scRGB units (1.0 = 80 nits), from Settings > Display > HDR.
    white: f32,
}

/// Appends a line to `%LOCALAPPDATA%\Blink\hdr.log` (kept below 1 MB). Only written when a
/// monitor has HDR on and the setting is on.
fn log(msg: &str) {
    let Some(dir) = dirs::data_local_dir().map(|d| d.join("Blink")) else { return };
    let path = dir.join("hdr.log");
    std::fs::create_dir_all(&dir).ok();
    if std::fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        std::fs::remove_file(&path).ok();
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let _ = writeln!(f, "{now} {msg}");
    }
}

unsafe extern "system" fn enum_monitor(m: HMONITOR, _dc: HDC, _r: *mut RECT, lp: LPARAM) -> BOOL {
    let out = &mut *(lp as *mut Vec<(IRect, [u16; 32])>);
    let mut mi: MONITORINFOEXW = std::mem::zeroed();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(m, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) != 0 {
        out.push((IRect::from_rect(mi.monitorInfo.rcMonitor), mi.szDevice));
    }
    1
}

fn header(kind: i32, size: usize, path: &DISPLAYCONFIG_PATH_INFO, source: bool) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    let (adapter, id) = if source {
        (path.sourceInfo.adapterId, path.sourceInfo.id)
    } else {
        (path.targetInfo.adapterId, path.targetInfo.id)
    };
    DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: kind, size: size as u32, adapterId: adapter, id }
}

/// The monitors (inside `area`) that have HDR on. Cheap: no graphics driver, nothing kept.
/// Advanced colour on an SDR screen (Auto Color Management, "wide colour enforced") does not
/// count: that is not HDR.
fn hdr_monitors(area: IRect) -> Vec<HdrMonitor> {
    let mut found = Vec::new();
    unsafe {
        let mut monitors: Vec<(IRect, [u16; 32])> = Vec::new();
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(enum_monitor),
            &mut monitors as *mut _ as LPARAM,
        );
        monitors.retain(|(r, _)| !r.intersect(&area).is_empty());
        if monitors.is_empty() {
            return found;
        }
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != 0 {
            return found;
        }
        let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = vec![std::mem::zeroed(); np as usize];
        let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = vec![std::mem::zeroed(); nm as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            std::ptr::null_mut(),
        ) != 0
        {
            return found;
        }
        paths.truncate(np as usize);
        for path in &paths {
            let mut name: DISPLAYCONFIG_SOURCE_DEVICE_NAME = std::mem::zeroed();
            name.header = header(DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, std::mem::size_of_val(&name), path, true);
            if DisplayConfigGetDeviceInfo(&mut name.header) != 0 {
                continue;
            }
            let Some((rect, _)) = monitors.iter().find(|(_, dev)| {
                let n = |s: &[u16]| s.iter().take_while(|&&c| c != 0).copied().collect::<Vec<u16>>();
                n(dev) == n(&name.viewGdiDeviceName)
            }) else {
                continue;
            };
            let mut color: DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO = std::mem::zeroed();
            color.header =
                header(DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, std::mem::size_of_val(&color), path, false);
            if DisplayConfigGetDeviceInfo(&mut color.header) != 0 {
                continue;
            }
            // Bit 1: advanced colour enabled; bit 2: wide colour enforced (ACM on SDR).
            let bits = color.Anonymous.value;
            if bits & 0b010 == 0 || bits & 0b100 != 0 {
                continue;
            }
            let mut white: DISPLAYCONFIG_SDR_WHITE_LEVEL = std::mem::zeroed();
            white.header =
                header(DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, std::mem::size_of_val(&white), path, false);
            // 1000 = 80 nits. Without an answer: Windows' default of 200 nits.
            let level = if DisplayConfigGetDeviceInfo(&mut white.header) == 0 && white.SDRWhiteLevel >= 1000 {
                white.SDRWhiteLevel as f32 / 1000.0
            } else {
                2.5
            };
            found.push(HdrMonitor { rect: *rect, white: level });
        }
    }
    found
}

fn err(what: &str, e: windows::core::Error) -> String {
    format!("{what}: {e} ({:#010x})", e.code().0 as u32)
}

/// Reads `area` (desktop coordinates, inside the monitor) of an HDR monitor as scRGB and
/// writes it tone mapped into `dst`, the BGRA pixels of a screenshot of `dst_rect`.
unsafe fn tone_map_monitor(mon: &HdrMonitor, area: IRect, dst_rect: IRect, dst: &mut [u8]) -> Result<(), String> {
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
            if IRect::from_ltrb(r.left, r.top, r.right, r.bottom) == mon.rect {
                if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED {
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
        D3D11_CREATE_DEVICE_FLAG(0),
        None,
        D3D11_SDK_VERSION,
        Some(&mut device),
        None,
        Some(&mut context),
    )
    .map_err(|e| err("D3D11CreateDevice", e))?;
    let device: ID3D11Device = device.ok_or("no D3D11 device")?;
    let context: ID3D11DeviceContext = context.ok_or("no D3D11 context")?;
    let output5 = output.cast::<IDXGIOutput5>().map_err(|e| err("IDXGIOutput5 (Windows 10 1703+)", e))?;
    let dup = output5
        .DuplicateOutput1(&device, 0, &[DXGI_FORMAT_R16G16B16A16_FLOAT])
        .map_err(|e| err("DuplicateOutput1", e))?;

    let local = IRect { x: area.x - mon.rect.x, y: area.y - mon.rect.y, ..area };
    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: local.w as u32,
        Height: local.h as u32,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging = None;
    device.CreateTexture2D(&staging_desc, None, Some(&mut staging)).map_err(|e| err("CreateTexture2D", e))?;
    let staging = staging.ok_or("no staging texture")?;
    let bx = D3D11_BOX {
        left: local.x as u32,
        top: local.y as u32,
        front: 0,
        right: (local.x + local.w) as u32,
        bottom: (local.y + local.h) as u32,
        back: 1,
    };
    // What BitBlt saw there, to check the frame against before it is written over it.
    let stride = dst_rect.w as usize * 4;
    let x0 = (area.x - dst_rect.x) as usize * 4;
    let y0 = (area.y - dst_rect.y) as usize;
    let row_end = x0 + local.w as usize * 4;
    let (mut sum, mut n) = (0u64, 0u64);
    for r in dst.chunks(stride).skip(y0).take(local.h as usize).step_by(SAMPLE) {
        let (s, c) = mean_bgra(&r[x0..row_end]);
        sum += s;
        n += c;
    }
    let sdr_mean = sum as f32 / n.max(1) as f32;

    // The first frame of a new duplication should hold the whole desktop, but some drivers
    // hand over an empty (black) one first: then the next frame is tried. On a screen where
    // nothing moves no next frame comes, and BitBlt's picture stays.
    let mut why = String::new();
    for attempt in 1..=FRAME_ATTEMPTS {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        let wait = if attempt == 1 { 500 } else { 250 };
        if let Err(e) = dup.AcquireNextFrame(wait, &mut info, &mut resource) {
            if e.code() == DXGI_ERROR_WAIT_TIMEOUT && attempt > 1 {
                return Err(format!("{why}; no further frame within {wait} ms"));
            }
            return Err(err("AcquireNextFrame", e));
        }
        let copied = (|| -> Result<(), String> {
            let tex: ID3D11Texture2D =
                resource.as_ref().ok_or("no desktop texture")?.cast().map_err(|e| err("ID3D11Texture2D", e))?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut desc);
            if desc.Format != DXGI_FORMAT_R16G16B16A16_FLOAT {
                return Err(format!("desktop format {:?} is not scRGB", desc.Format));
            }
            context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &tex, 0, Some(&bx));
            Ok(())
        })();
        // The copy is on the GPU: the frame can go back at once.
        let _ = dup.ReleaseFrame();
        copied?;

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(|e| err("Map", e))?;
        // Straight from the mapped texture into the screenshot: no extra copy of the screen.
        let pitch = mapped.RowPitch as usize;
        let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * local.h as usize);
        let row_bytes = local.w as usize * 8;
        let rows: Vec<&[u8]> = src.chunks(pitch).map(|r| &r[..row_bytes]).collect();
        let peak = rows.iter().map(|r| row_peak(r, mon.white)).fold(0.0f32, f32::max);
        let curve = Curve::new(peak);
        let srgb = Srgb::default();
        let (mut sum, mut n) = (0u64, 0u64);
        for r in rows.iter().step_by(SAMPLE) {
            let (s, c) = mean_mapped(r, mon.white, &curve, &srgb);
            sum += s;
            n += c;
        }
        let hdr_mean = sum as f32 / n.max(1) as f32;
        let frame = format!(
            "frame {attempt}: present time {}, {} frames, peak {peak:.2}x SDR white, average {hdr_mean:.0} \
             (normal screenshot {sdr_mean:.0})",
            info.LastPresentTime, info.AccumulatedFrames
        );
        if looks_blank(hdr_mean, sdr_mean) {
            context.Unmap(&staging, 0);
            log(&format!("monitor {:?}: {frame}: looks empty, not used", mon.rect));
            why = format!("frame {attempt} looked empty");
            continue;
        }
        let mut out: Vec<&mut [u8]> =
            dst.chunks_mut(stride).skip(y0).take(local.h as usize).map(|r| &mut r[x0..row_end]).collect();
        // On all cores: about 8 million pixels at 4K.
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 16);
        let per = rows.len().div_ceil(threads).max(1);
        std::thread::scope(|s| {
            for (src, dst) in rows.chunks(per).zip(out.chunks_mut(per)) {
                let (curve, srgb, white) = (&curve, &srgb, mon.white);
                s.spawn(move || {
                    for (r, o) in src.iter().zip(dst.iter_mut()) {
                        map_row(r, white, curve, srgb, o);
                    }
                });
            }
        });
        context.Unmap(&staging, 0);
        log(&format!(
            "monitor {:?}: SDR white {:.0} nits, {frame}{}",
            mon.rect,
            mon.white * 80.0,
            if curve.is_identity() { " (no highlights, unchanged)" } else { ", highlights compressed" }
        ));
        return Ok(());
    }
    Err(why)
}

/// Replaces the HDR monitors' part of a BitBlt screenshot (`rect`, BGRA pixels `bgra`) with a
/// tone-mapped copy. Does nothing when no monitor in it has HDR on.
pub fn tone_map_into(rect: IRect, bgra: &mut [u8]) {
    for mon in hdr_monitors(rect) {
        let area = mon.rect.intersect(&rect);
        if area.is_empty() {
            continue;
        }
        if let Err(e) = unsafe { tone_map_monitor(&mon, area, rect, bgra) } {
            log(&format!("monitor {:?}: kept the normal screenshot: {e}", mon.rect));
        }
    }
}
