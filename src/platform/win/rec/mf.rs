//! Media Foundation: H.264 in MP4 with the Sink Writer, and decoding with the Source Reader.
//!
//! Windows N editions (Europe, Korea) ship without Media Foundation unless the Media Feature
//! Pack is installed. Linking `mfplat.dll` and `mfreadwrite.dll` the normal way would stop
//! Blink.exe from starting there at all, screenshots included. So the few functions that
//! create objects are looked up at run time; everything after that goes through COM
//! interfaces, which need no linking. Without the DLLs, `Mf::load` returns `NoMediaFoundation`
//! and the recorder explains what to install.

use crate::rec::export::VideoSource;
use core::ffi::c_void;
use image::RgbaImage;
use std::path::Path;
use std::sync::OnceLock;
use windows::core::{Interface, GUID, HRESULT, PCWSTR, PROPVARIANT};
use windows::Win32::Media::MediaFoundation::*;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

/// Why Media Foundation cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MfError {
    /// The DLLs are missing: a Windows N edition without the Media Feature Pack.
    NoMediaFoundation,
    Failed(String),
}

impl std::fmt::Display for MfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MfError::NoMediaFoundation => f.write_str("Media Foundation is not installed"),
            MfError::Failed(e) => f.write_str(e),
        }
    }
}

fn failed(what: &str, e: windows::core::Error) -> MfError {
    MfError::Failed(format!("{what}: {e} ({:#010x})", e.code().0 as u32))
}

type FnStartup = unsafe extern "system" fn(u32, u32) -> HRESULT;
type FnCreate = unsafe extern "system" fn(*mut *mut c_void) -> HRESULT;
type FnCreateAttributes = unsafe extern "system" fn(*mut *mut c_void, u32) -> HRESULT;
type FnCreateMemoryBuffer = unsafe extern "system" fn(u32, *mut *mut c_void) -> HRESULT;
type FnCreateSinkWriter = unsafe extern "system" fn(PCWSTR, *mut c_void, *mut c_void, *mut *mut c_void) -> HRESULT;
type FnCreateSourceReader = unsafe extern "system" fn(PCWSTR, *mut c_void, *mut *mut c_void) -> HRESULT;

/// The Media Foundation functions Blink needs, from the DLLs.
pub struct Mf {
    create_media_type: FnCreate,
    create_sample: FnCreate,
    create_attributes: FnCreateAttributes,
    create_memory_buffer: FnCreateMemoryBuffer,
    create_sink_writer: FnCreateSinkWriter,
    create_source_reader: FnCreateSourceReader,
}

// Function pointers into system DLLs that stay loaded for the life of the process.
unsafe impl Send for Mf {}
unsafe impl Sync for Mf {}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn proc(module: *mut c_void, name: &str) -> Option<unsafe extern "system" fn() -> isize> {
    let name = std::ffi::CString::new(name).ok()?;
    GetProcAddress(module as _, name.as_ptr() as *const u8)
}

impl Mf {
    /// Loads Media Foundation once per process and starts it.
    pub fn get() -> Result<&'static Mf, MfError> {
        static MF: OnceLock<Result<Mf, MfError>> = OnceLock::new();
        MF.get_or_init(|| unsafe { Mf::load() }).as_ref().map_err(|e| e.clone())
    }

    unsafe fn load() -> Result<Mf, MfError> {
        let plat = LoadLibraryW(wide("mfplat.dll").as_ptr());
        let rw = LoadLibraryW(wide("mfreadwrite.dll").as_ptr());
        if plat.is_null() || rw.is_null() {
            return Err(MfError::NoMediaFoundation);
        }
        let get = |m: *mut c_void, n: &str| proc(m, n).ok_or(MfError::NoMediaFoundation);
        let startup: FnStartup = std::mem::transmute(get(plat as _, "MFStartup")?);
        let mf = Mf {
            create_media_type: std::mem::transmute(get(plat as _, "MFCreateMediaType")?),
            create_sample: std::mem::transmute(get(plat as _, "MFCreateSample")?),
            create_attributes: std::mem::transmute(get(plat as _, "MFCreateAttributes")?),
            create_memory_buffer: std::mem::transmute(get(plat as _, "MFCreateMemoryBuffer")?),
            create_sink_writer: std::mem::transmute(get(rw as _, "MFCreateSinkWriterFromURL")?),
            create_source_reader: std::mem::transmute(get(rw as _, "MFCreateSourceReaderFromURL")?),
        };
        // COM on this thread (Media Foundation needs it); a thread that already has it is fine.
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        startup(MF_VERSION, MFSTARTUP_LITE).ok().map_err(|e| failed("MFStartup", e))?;
        Ok(mf)
    }

    unsafe fn media_type(&self) -> Result<IMFMediaType, MfError> {
        let mut p = std::ptr::null_mut();
        (self.create_media_type)(&mut p).ok().map_err(|e| failed("MFCreateMediaType", e))?;
        Ok(IMFMediaType::from_raw(p))
    }

    unsafe fn attributes(&self, n: u32) -> Result<IMFAttributes, MfError> {
        let mut p = std::ptr::null_mut();
        (self.create_attributes)(&mut p, n).ok().map_err(|e| failed("MFCreateAttributes", e))?;
        Ok(IMFAttributes::from_raw(p))
    }

    unsafe fn sample(&self) -> Result<IMFSample, MfError> {
        let mut p = std::ptr::null_mut();
        (self.create_sample)(&mut p).ok().map_err(|e| failed("MFCreateSample", e))?;
        Ok(IMFSample::from_raw(p))
    }

    unsafe fn memory_buffer(&self, len: u32) -> Result<IMFMediaBuffer, MfError> {
        let mut p = std::ptr::null_mut();
        (self.create_memory_buffer)(len, &mut p).ok().map_err(|e| failed("MFCreateMemoryBuffer", e))?;
        Ok(IMFMediaBuffer::from_raw(p))
    }
}

fn pack(hi: u32, lo: u32) -> u64 {
    (hi as u64) << 32 | lo as u64
}

/// 100-nanosecond units, Media Foundation's clock.
fn hns(ms: u32) -> i64 {
    ms as i64 * 10_000
}

// ---- encoding ------------------------------------------------------------------------------

/// Writes H.264 video in an MP4 file, frame by frame, at a constant frame rate.
pub struct Mp4Writer {
    writer: IMFSinkWriter,
    stream: u32,
    mf: &'static Mf,
    /// Size of the video (even numbers; the source may be one pixel larger).
    pub w: u32,
    pub h: u32,
    fps: u32,
    buf: Vec<u8>,
    last: Option<u32>,
    finished: bool,
}

// Used from one thread at a time.
unsafe impl Send for Mp4Writer {}

impl Mp4Writer {
    /// A new MP4 at `path` for frames of `w` x `h` (made even by dropping the last column or
    /// row), at `fps`, `bitrate` bits per second and a key frame every `key_every` frames.
    pub fn create(path: &Path, w: u32, h: u32, fps: u32, bitrate: u32, key_every: u32) -> Result<Self, MfError> {
        let mf = Mf::get()?;
        let (w, h) = (w & !1, h & !1);
        if w < 2 || h < 2 {
            return Err(MfError::Failed(crate::i18n::t("The area is too small to record.").into()));
        }
        unsafe {
            let attrs = mf.attributes(2)?;
            // A hardware encoder when there is one; Media Foundation falls back to software.
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1).map_err(|e| failed("attributes", e))?;
            let url = wide(&path.display().to_string());
            let mut p = std::ptr::null_mut();
            (mf.create_sink_writer)(PCWSTR(url.as_ptr()), std::ptr::null_mut(), attrs.as_raw(), &mut p)
                .ok()
                .map_err(|e| failed("MFCreateSinkWriterFromURL", e))?;
            let writer = IMFSinkWriter::from_raw(p);

            let out = mf.media_type()?;
            let set = |r: windows::core::Result<()>| r.map_err(|e| failed("output type", e));
            set(out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video))?;
            set(out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264))?;
            set(out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate))?;
            set(out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32))?;
            set(out.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h)))?;
            set(out.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1)))?;
            set(out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)))?;
            set(out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32))?;
            // Not every encoder reads this; the codec API below is the other way to say it.
            let _ = out.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, key_every);
            let stream = writer.AddStream(&out).map_err(|e| failed("AddStream", e))?;

            let input = mf.media_type()?;
            let set = |r: windows::core::Result<()>| r.map_err(|e| failed("input type", e));
            set(input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video))?;
            set(input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32))?;
            set(input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32))?;
            set(input.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h)))?;
            set(input.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1)))?;
            set(input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)))?;
            // A positive stride: the rows are top-down. Without it RGB counts as bottom-up
            // and the video comes out upside down.
            set(input.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4))?;
            writer.SetInputMediaType(stream, &input, None).map_err(|e| failed("SetInputMediaType", e))?;

            // Key frames often enough to seek quickly in the clip window.
            let mut api: *mut c_void = std::ptr::null_mut();
            if writer.GetServiceForStream(stream, &GUID::zeroed(), &ICodecAPI::IID, &mut api).is_ok() && !api.is_null()
            {
                let api = ICodecAPI::from_raw(api);
                let _ = api.SetValue(&CODECAPI_AVEncMPVGOPSize, &windows::core::VARIANT::from(key_every));
            }
            writer.BeginWriting().map_err(|e| failed("BeginWriting", e))?;
            Ok(Mp4Writer { writer, stream, mf, w, h, fps: fps.max(1), buf: Vec::new(), last: None, finished: false })
        }
    }

    /// Writes one frame (top-down BGRA rows of `stride` bytes, at least `w` x `h`) shown from
    /// `t` ms until the next frame.
    pub fn write_bgra(&mut self, data: &[u8], stride: usize, t: u32) -> Result<(), MfError> {
        let row = self.w as usize * 4;
        self.buf.clear();
        for y in 0..self.h as usize {
            let start = y * stride;
            let Some(src) = data.get(start..start + row) else { return Err(MfError::Failed("frame too small".into())) };
            self.buf.extend_from_slice(src);
        }
        self.write_buf(t)
    }

    /// Writes one RGBA frame (at least `w` x `h`).
    pub fn write_rgba(&mut self, img: &RgbaImage, t: u32) -> Result<(), MfError> {
        if img.width() < self.w || img.height() < self.h {
            return Err(MfError::Failed("frame too small".into()));
        }
        self.buf.clear();
        let stride = img.width() as usize * 4;
        let raw = img.as_raw();
        for y in 0..self.h as usize {
            for p in raw[y * stride..y * stride + self.w as usize * 4].chunks_exact(4) {
                self.buf.extend_from_slice(&[p[2], p[1], p[0], 255]);
            }
        }
        self.write_buf(t)
    }

    fn write_buf(&mut self, t: u32) -> Result<(), MfError> {
        // Samples must move forward in time.
        if self.last.is_some_and(|l| t <= l) {
            return Ok(());
        }
        unsafe {
            let len = self.buf.len() as u32;
            let buffer = self.mf.memory_buffer(len)?;
            let mut p: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&mut p, None, None).map_err(|e| failed("Lock", e))?;
            std::ptr::copy_nonoverlapping(self.buf.as_ptr(), p, self.buf.len());
            buffer.Unlock().map_err(|e| failed("Unlock", e))?;
            buffer.SetCurrentLength(len).map_err(|e| failed("SetCurrentLength", e))?;
            let sample = self.mf.sample()?;
            sample.AddBuffer(&buffer).map_err(|e| failed("AddBuffer", e))?;
            sample.SetSampleTime(hns(t)).map_err(|e| failed("SetSampleTime", e))?;
            // One frame at the frame rate; a held frame is followed by a later time stamp.
            sample.SetSampleDuration(hns(1000 / self.fps)).map_err(|e| failed("SetSampleDuration", e))?;
            self.writer.WriteSample(self.stream, &sample).map_err(|e| failed("WriteSample", e))?;
        }
        self.last = Some(t);
        Ok(())
    }

    /// Closes the file. Without this the MP4 cannot be played.
    pub fn finish(&mut self) -> Result<(), MfError> {
        if !self.finished {
            self.finished = true;
            unsafe { self.writer.Finalize() }.map_err(|e| failed("Finalize", e))?;
        }
        Ok(())
    }
}

impl Drop for Mp4Writer {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

/// `Mp4Writer` as the end of an export.
pub struct Mp4Sink {
    pub writer: Mp4Writer,
    pub path: std::path::PathBuf,
}

impl crate::rec::export::FrameSink for Mp4Sink {
    fn push(&mut self, frame: &RgbaImage, t: u32) -> Result<(), String> {
        self.writer.write_rgba(frame, t).map_err(|e| e.to_string())
    }
    fn finish(&mut self, _end: u32) -> Result<u64, String> {
        self.writer.finish().map_err(|e| e.to_string())?;
        std::fs::metadata(&self.path).map(|m| m.len()).map_err(|e| e.to_string())
    }
}

// ---- decoding ------------------------------------------------------------------------------

/// Reads the frames of a video file as RGBA.
pub struct Mp4Reader {
    reader: IMFSourceReader,
    /// The size the video should have, when known (the decoder may pad it).
    expected: Option<(u32, u32)>,
    w: u32,
    h: u32,
    /// Size of the decoded frames (may be padded, e.g. 1088 rows for 1080).
    frame_h: u32,
    /// Bytes from one row to the next; negative for bottom-up rows.
    stride: i32,
    duration: u32,
}

unsafe impl Send for Mp4Reader {}

const FIRST_VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const TYPE_CHANGED: u32 = 0x20; // MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED

impl Mp4Reader {
    /// Opens `path`. `size`: the size the video should have, when known.
    pub fn open(path: &Path, size: Option<(u32, u32)>) -> Result<Self, MfError> {
        let mf = Mf::get()?;
        unsafe {
            let attrs = mf.attributes(1)?;
            // Lets the reader convert the decoder's YUV to RGB32.
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(|e| failed("attributes", e))?;
            let url = wide(&path.display().to_string());
            let mut p = std::ptr::null_mut();
            (mf.create_source_reader)(PCWSTR(url.as_ptr()), attrs.as_raw(), &mut p)
                .ok()
                .map_err(|e| failed("MFCreateSourceReaderFromURL", e))?;
            let reader = IMFSourceReader::from_raw(p);
            let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
            reader.SetStreamSelection(FIRST_VIDEO, true).map_err(|e| failed("SetStreamSelection", e))?;
            let t = mf.media_type()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| failed("type", e))?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(|e| failed("type", e))?;
            reader.SetCurrentMediaType(FIRST_VIDEO, None, &t).map_err(|e| failed("SetCurrentMediaType", e))?;
            let duration = reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .ok()
                .and_then(|v| u64::try_from(&v).ok())
                .map_or(0, |d| (d / 10_000) as u32);
            let mut r = Mp4Reader { reader, expected: size, w: 0, h: 0, frame_h: 0, stride: 0, duration };
            r.read_type()?;
            Ok(r)
        }
    }

    /// Reads the size and row layout of the decoded frames (again after the decoder changed
    /// them).
    unsafe fn read_type(&mut self) -> Result<(), MfError> {
        let cur = self.reader.GetCurrentMediaType(FIRST_VIDEO).map_err(|e| failed("GetCurrentMediaType", e))?;
        let fs = cur.GetUINT64(&MF_MT_FRAME_SIZE).map_err(|e| failed("frame size", e))?;
        let (frame_w, frame_h) = ((fs >> 32) as u32, fs as u32);
        // Without the attribute, RGB rows are bottom-up (the bitmap convention).
        self.stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(-(frame_w as i32 * 4));
        // The visible part: the display aperture, else the size we expect, else all.
        let mut area = MFVideoArea::default();
        let blob = std::slice::from_raw_parts_mut(
            &mut area as *mut MFVideoArea as *mut u8,
            std::mem::size_of::<MFVideoArea>(),
        );
        let (mut w, mut h) = (frame_w, frame_h);
        if cur.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, blob, None).is_ok() && area.Area.cx > 0 && area.Area.cy > 0 {
            w = (area.Area.cx as u32).min(frame_w);
            h = (area.Area.cy as u32).min(frame_h);
        }
        if let Some((ew, eh)) = self.expected {
            w = w.min(ew);
            h = h.min(eh);
        }
        (self.w, self.h, self.frame_h) = (w, h, frame_h);
        Ok(())
    }
}

impl VideoSource for Mp4Reader {
    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    fn duration(&self) -> u32 {
        self.duration
    }

    fn seek(&mut self, ms: u32) -> Result<(), String> {
        let pos = PROPVARIANT::from(hns(ms));
        unsafe { self.reader.SetCurrentPosition(&GUID::zeroed(), &pos) }.map_err(|e| failed("seek", e).to_string())
    }

    fn next(&mut self) -> Result<Option<(u32, RgbaImage)>, String> {
        loop {
            let mut flags = 0u32;
            let mut time = 0i64;
            let mut sample: Option<IMFSample> = None;
            unsafe {
                self.reader
                    .ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), Some(&mut time), Some(&mut sample))
                    .map_err(|e| failed("ReadSample", e).to_string())?;
                if flags & TYPE_CHANGED != 0 {
                    self.read_type().map_err(|e| e.to_string())?;
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Ok(None);
            }
            let Some(sample) = sample else { continue };
            let t = (time.max(0) / 10_000) as u32;
            return unsafe { self.frame(&sample) }.map(|img| Some((t, img)));
        }
    }
}

impl Mp4Reader {
    unsafe fn frame(&self, sample: &IMFSample) -> Result<RgbaImage, String> {
        // One buffer is the rule for video; a 2D buffer knows its real pitch and which row
        // comes first.
        let single = sample.GetBufferCount().map(|n| n == 1).unwrap_or(false);
        let buffer = if single { sample.GetBufferByIndex(0) } else { sample.ConvertToContiguousBuffer() }
            .map_err(|e| failed("buffer", e).to_string())?;
        if let Ok(b2) = buffer.cast::<IMF2DBuffer>() {
            let mut p: *mut u8 = std::ptr::null_mut();
            let mut pitch = 0i32;
            if b2.Lock2D(&mut p, &mut pitch).is_ok() {
                let img = self.copy(p, pitch, None);
                let _ = b2.Unlock2D();
                return img;
            }
        }
        let mut p: *mut u8 = std::ptr::null_mut();
        let mut len = 0u32;
        buffer.Lock(&mut p, None, Some(&mut len)).map_err(|e| failed("Lock", e).to_string())?;
        let rows = self.frame_h.max(self.h) as usize;
        let need = self.stride.unsigned_abs() as usize * rows;
        let img = if (len as usize) < need {
            Err("decoded frame is smaller than expected".to_string())
        } else if self.stride < 0 {
            // Bottom-up: the top row is the last in memory.
            let start = p.add((rows - 1) * self.stride.unsigned_abs() as usize);
            self.copy(start, self.stride, Some(len as usize))
        } else {
            self.copy(p, self.stride, Some(len as usize))
        };
        let _ = buffer.Unlock();
        img
    }

    /// Copies the visible part of a BGRA(X) frame whose top row starts at `row0`, `pitch`
    /// bytes from row to row, into RGBA.
    unsafe fn copy(&self, row0: *const u8, pitch: i32, _len: Option<usize>) -> Result<RgbaImage, String> {
        if pitch.unsigned_abs() < self.w * 4 {
            return Err("decoded frame has an unexpected size".into());
        }
        let mut out = Vec::with_capacity((self.w * self.h * 4) as usize);
        for y in 0..self.h as isize {
            let row = std::slice::from_raw_parts(row0.offset(y * pitch as isize), self.w as usize * 4);
            for px in row.chunks_exact(4) {
                out.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        }
        RgbaImage::from_raw(self.w, self.h, out).ok_or_else(|| "frame".to_string())
    }
}
