//! Minimal HTTPS GET with WinHTTP: uses Windows' own TLS, certificates and proxy settings,
//! so updates work behind company proxies without bundling a TLS library.
//!
//! Redirects are followed by hand: GitHub sends release downloads to a storage host with a
//! signed URL, and the Authorization header must not be sent there.

use super::wide;
use crate::i18n::{t, tf};
use windows_sys::Win32::Networking::WinHttp::{
    WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpCrackUrl, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetTimeouts, URL_COMPONENTS, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ADDREQ_FLAG_ADD,
    WINHTTP_FLAG_SECURE, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
    WINHTTP_QUERY_CONTENT_LENGTH, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_LOCATION, WINHTTP_QUERY_STATUS_CODE,
};

struct Handle(*mut core::ffi::c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }
}

struct Url {
    host: String,
    port: u16,
    path: String,
}

fn crack(url: &str) -> Result<Url, String> {
    let w: Vec<u16> = url.encode_utf16().collect();
    let mut host = vec![0u16; 256];
    let mut path = vec![0u16; 4096];
    let mut uc: URL_COMPONENTS = unsafe { std::mem::zeroed() };
    uc.dwStructSize = std::mem::size_of::<URL_COMPONENTS>() as u32;
    uc.lpszHostName = host.as_mut_ptr();
    uc.dwHostNameLength = host.len() as u32;
    uc.lpszUrlPath = path.as_mut_ptr();
    uc.dwUrlPathLength = path.len() as u32;
    // The query string comes separately; ask for it in the same buffer pass.
    let mut extra = vec![0u16; 8192];
    uc.lpszExtraInfo = extra.as_mut_ptr();
    uc.dwExtraInfoLength = extra.len() as u32;
    if unsafe { WinHttpCrackUrl(w.as_ptr(), w.len() as u32, 0, &mut uc) } == 0 {
        return Err(tf("Bad URL: {0}", &[&url]));
    }
    if uc.nScheme != 2 {
        return Err(t("Only HTTPS is allowed for updates.").into()); // INTERNET_SCHEME_HTTPS
    }
    let s = |b: &[u16], n: u32| String::from_utf16_lossy(&b[..n as usize]);
    Ok(Url {
        host: s(&host, uc.dwHostNameLength),
        port: uc.nPort,
        path: s(&path, uc.dwUrlPathLength) + &s(&extra, uc.dwExtraInfoLength),
    })
}

unsafe fn query_number(req: *mut core::ffi::c_void, what: u32) -> Option<u64> {
    let mut v: u32 = 0;
    let mut len = 4u32;
    let ok = WinHttpQueryHeaders(
        req,
        what | WINHTTP_QUERY_FLAG_NUMBER,
        std::ptr::null(),
        (&mut v as *mut u32).cast(),
        &mut len,
        std::ptr::null_mut(),
    );
    (ok != 0).then_some(v as u64)
}

unsafe fn query_string(req: *mut core::ffi::c_void, what: u32) -> Option<String> {
    let mut buf = vec![0u16; 8192];
    let mut len = (buf.len() * 2) as u32;
    let ok = WinHttpQueryHeaders(req, what, std::ptr::null(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut());
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize / 2]))
}

/// GET `url`. `body(chunk, content_length)` receives the response; return false to stop.
/// Returns the final HTTP status.
pub fn http_get(
    url: &str,
    headers: &[(&str, &str)],
    body: &mut dyn FnMut(&[u8], Option<u64>) -> bool,
) -> Result<u16, String> {
    let agent = wide(&format!("Blink-Updater/{}", env!("CARGO_PKG_VERSION")));
    let session = Handle(unsafe {
        WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0)
    });
    if session.0.is_null() {
        return Err(t("Could not start an internet connection (WinHTTP).").into());
    }
    unsafe {
        WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 60_000);
    }
    let mut url = url.to_string();
    let first_host = crack(&url)?.host;
    for _ in 0..6 {
        let u = crack(&url)?;
        let host = wide(&u.host);
        let conn = Handle(unsafe { WinHttpConnect(session.0, host.as_ptr(), u.port, 0) });
        if conn.0.is_null() {
            return Err(tf("Could not connect to {0}.", &[&u.host]));
        }
        let verb = wide("GET");
        let path = wide(&u.path);
        let req = Handle(unsafe {
            WinHttpOpenRequest(
                conn.0,
                verb.as_ptr(),
                path.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        });
        if req.0.is_null() {
            return Err(t("Could not create the request.").into());
        }
        unsafe {
            let never: u32 = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
            WinHttpSetOption(req.0, WINHTTP_OPTION_REDIRECT_POLICY, (&never as *const u32).cast(), 4);
        }
        for (k, v) in headers {
            // Credentials only go to the host they were meant for.
            if k.eq_ignore_ascii_case("authorization") && u.host != first_host {
                continue;
            }
            let line: Vec<u16> = format!("{k}: {v}").encode_utf16().collect();
            unsafe {
                WinHttpAddRequestHeaders(req.0, line.as_ptr(), line.len() as u32, WINHTTP_ADDREQ_FLAG_ADD);
            }
        }
        let sent = unsafe { WinHttpSendRequest(req.0, std::ptr::null(), 0, std::ptr::null(), 0, 0, 0) != 0 };
        if !sent || unsafe { WinHttpReceiveResponse(req.0, std::ptr::null_mut()) } == 0 {
            let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            return Err(tf("No connection to {1} (error {0}). Check your internet connection.", &[&code, &u.host]));
        }
        let status = unsafe { query_number(req.0, WINHTTP_QUERY_STATUS_CODE) }.unwrap_or(0) as u16;
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            url = unsafe { query_string(req.0, WINHTTP_QUERY_LOCATION) }.ok_or(t("Redirect without a target."))?;
            continue;
        }
        let total = unsafe { query_number(req.0, WINHTTP_QUERY_CONTENT_LENGTH) };
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut avail = 0u32;
            if unsafe { WinHttpQueryDataAvailable(req.0, &mut avail) } == 0 {
                return Err(t("The connection was interrupted.").into());
            }
            if avail == 0 {
                break;
            }
            let mut read = 0u32;
            let want = (avail as usize).min(buf.len()) as u32;
            if unsafe { WinHttpReadData(req.0, buf.as_mut_ptr().cast(), want, &mut read) } == 0 {
                return Err(t("The connection was interrupted.").into());
            }
            if read == 0 {
                break;
            }
            if !body(&buf[..read as usize], total) {
                break;
            }
        }
        return Ok(status);
    }
    Err(t("Too many redirects.").into())
}
