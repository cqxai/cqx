//! The analysis, callable from a browser.
//!
//! No bindgen. The surface is four functions and a length-prefixed buffer,
//! which keeps the build to `cargo build --target wasm32-unknown-unknown` with
//! no extra toolchain — and a project that is awkward to build does not get
//! contributed to.
//!
//! The division of labour matters more than the calling convention: JavaScript
//! does the fetching, because that is where `fetch`, credentials and rate
//! limits live, and Rust does the analysis, which needs no I/O at all. Files go
//! in one at a time; nothing here ever reaches for a network or a disk.

use std::cell::RefCell;

use cqx_vfs::Vfs;

thread_local! {
    /// The snapshot being assembled. A thread local rather than a `static mut`:
    /// wasm is single threaded, and shared mutable state is a finding this very
    /// tool reports.
    static SNAPSHOT: RefCell<Vfs> = RefCell::new(Vfs::new(""));
}

/// Hands the caller a buffer to write into.
///
/// # Safety
/// The returned pointer is valid for `len` bytes and must be released with
/// [`cqx_free`] using the same length.
#[no_mangle]
pub extern "C" fn cqx_alloc(len: usize) -> *mut u8 {
    let mut buffer = Vec::<u8>::with_capacity(len);
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// Releases a buffer obtained from [`cqx_alloc`] or returned by a call here.
///
/// # Safety
/// `ptr` must have come from this module and `len` must be the length it was
/// created with.
#[no_mangle]
pub unsafe extern "C" fn cqx_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    drop(Vec::from_raw_parts(ptr, 0, len));
}

/// # Safety
/// `ptr` must point to `len` bytes of valid UTF-8.
unsafe fn borrow(ptr: *const u8, len: usize) -> String {
    if ptr.is_null() || len == 0 {
        return String::new();
    }
    String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned()
}

/// Returns a buffer whose first four bytes are the little-endian length of what
/// follows. One allocation, one pointer, no out-parameters.
fn respond(body: String) -> *mut u8 {
    let bytes = body.into_bytes();
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&bytes);
    let ptr = out.as_mut_ptr();
    std::mem::forget(out);
    ptr
}

/// Starts a new snapshot, discarding whatever was being assembled.
///
/// # Safety
/// `label` must point to `label_len` bytes of valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn cqx_reset(label: *const u8, label_len: usize) {
    let label = borrow(label, label_len);
    SNAPSHOT.with(|s| *s.borrow_mut() = Vfs::new(label));
}

/// Adds one file to the snapshot.
///
/// # Safety
/// Both pointers must reference that many bytes of valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn cqx_add_file(
    path: *const u8,
    path_len: usize,
    content: *const u8,
    content_len: usize,
) {
    let path = borrow(path, path_len);
    let content = borrow(content, content_len);
    SNAPSHOT.with(|s| s.borrow_mut().insert(path, content));
}

#[no_mangle]
pub extern "C" fn cqx_file_count() -> usize {
    SNAPSHOT.with(|s| s.borrow().len())
}

/// Extracts the snapshot and returns the fact stream as newline-delimited JSON.
#[no_mangle]
pub extern "C" fn cqx_facts() -> *mut u8 {
    let result = SNAPSHOT.with(|s| {
        let vfs = s.borrow();
        let mut out = Vec::new();
        cqx_rust::extract::run(&vfs, &mut out)
            .map(|_| String::from_utf8_lossy(&out).into_owned())
            .map_err(|e| e.to_string())
    });
    respond(match result {
        Ok(facts) => facts,
        // An error is data too: the caller gets one JSON object it can read
        // rather than a silent empty stream.
        Err(e) => serde_json::json!({ "t": "error", "message": e }).to_string(),
    })
}

/// Scores the snapshot. `config` is a cqx.json, or empty for the defaults.
///
/// # Safety
/// `config` must point to `config_len` bytes of valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn cqx_score(config: *const u8, config_len: usize) -> *mut u8 {
    let config_text = borrow(config, config_len);
    let result = SNAPSHOT.with(|s| -> Result<String, String> {
        let vfs = s.borrow();
        let mut facts = Vec::new();
        cqx_rust::extract::run(&vfs, &mut facts).map_err(|e| e.to_string())?;
        let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8_lossy(&facts));
        let config = cqx_score::config::Config::from_text(
            if config_text.trim().is_empty() {
                None
            } else {
                Some(config_text.as_str())
            },
        )?;
        let metrics = cqx_score::metrics::Metrics::compute(&stream, &config);
        Ok(cqx_score::report_json(&config, &metrics).to_string())
    });
    respond(match result {
        Ok(json) => json,
        Err(e) => serde_json::json!({ "error": e }).to_string(),
    })
}
