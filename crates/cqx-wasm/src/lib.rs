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

/// Builds the full dataset for the snapshot: the score, and the facts folded
/// into the shape an explorer renders.
///
/// The same function the exporter calls, so a commit analysed here and a commit
/// analysed in CI cannot disagree.
///
/// # Safety
/// Both pointers must reference that many bytes of valid UTF-8. `repo` names
/// the repository — `<org>/<repo>` — and `config` is a cqx.json, or empty.
#[no_mangle]
pub unsafe extern "C" fn cqx_dataset(
    repo: *const u8,
    repo_len: usize,
    config: *const u8,
    config_len: usize,
) -> *mut u8 {
    let repo = borrow(repo, repo_len);
    let config_text = borrow(config, config_len);
    let result = SNAPSHOT.with(|s| -> Result<String, String> {
        let vfs = s.borrow();
        let mut facts = Vec::new();
        cqx_rust::extract::run(&vfs, &mut facts).map_err(|e| e.to_string())?;
        let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8_lossy(&facts));
        let config = cqx_score::config::Config::from_text(if config_text.trim().is_empty() {
            None
        } else {
            Some(config_text.as_str())
        })?;
        let metrics = cqx_score::metrics::Metrics::compute(&stream, &config);
        let report = cqx_score::report_json(&config, &metrics);
        // A browser has one commit in hand and no git: the timeline it shows
        // comes from the index it already fetched, not from in here.
        let meta = cqx_view::Meta {
            repo: repo.as_str(),
            branch: "",
            remote: None,
            commits_url: None,
            // No clock here. The page times this call and fills it in, which
            // measures the same span the exporter does — and times its own
            // reading of the source, which is the other half.
            analysed_ms: None,
            fetched_ms: None,
        };
        Ok(cqx_view::dataset(&stream, report, serde_json::json!([]), &meta).to_string())
    });
    respond(match result {
        Ok(json) => json,
        Err(e) => serde_json::json!({ "error": e }).to_string(),
    })
}

// --- reading a workspace in pieces ------------------------------------------
//
// One reader is fast enough for most repositories and cannot manage the
// largest: makepad is a hundred and thirty three megabytes of Rust, and parsing
// it peaks near the four gigabytes wasm32 can address at all. Dividing the
// reading divides that, and the reading is all but a second of the work.
//
// What does not divide is what the reading is resolved against. A value's
// provenance routinely crosses a crate boundary, so a reader holding one slice
// would follow fewer of them and the scores would depend on how the work had
// been split. Hence three phases rather than one: everyone parses, everyone
// says what they found, everyone is told what everything else found, and only
// then does anyone write anything down.

thread_local! {
    /// The trees this reader parsed, held between saying what it found and
    /// being told what everyone else found.
    static PREPARED: RefCell<Option<cqx_rust::extract::Prepared>> = const { RefCell::new(None) };
    /// The coordinator's running union of what the readers found.
    static MERGING: RefCell<cqx_rust::prepass::Shared> =
        RefCell::new(cqx_rust::prepass::Shared::default());
    /// The coordinator's running collection of what the readers wrote.
    static FOLDING: RefCell<cqx_store::facts::Stream> =
        RefCell::new(cqx_store::facts::Stream::default());
}

/// What the workspace contains, read from the manifests alone.
///
/// Done once, by the coordinator, and handed to every reader. A manifest does
/// not list every target it has — cargo finds `src/bin/*.rs` by looking — so a
/// reader holding part of the sources would discover fewer targets, and a file
/// found under a different target is given a different module path. The
/// manifests are small; it is the sources that are not.
#[no_mangle]
pub extern "C" fn cqx_manifests() -> *mut u8 {
    let result = SNAPSHOT.with(|s| cqx_rust::manifest::read(&s.borrow()));
    respond(match result {
        Ok(metadata) => metadata.to_string(),
        Err(e) => serde_json::json!({ "error": e }).to_string(),
    })
}

/// Parses this reader's slice and reports what it found.
///
/// The trees stay here. What comes back is only the part that means something
/// beyond the file it came from — aliases are file-scoped and never travel.
///
/// # Safety
/// `metadata` must point to `metadata_len` bytes of valid UTF-8: what
/// `cqx_manifests` returned.
#[no_mangle]
pub unsafe extern "C" fn cqx_gather(metadata: *const u8, metadata_len: usize) -> *mut u8 {
    let text = borrow(metadata, metadata_len);
    let result = SNAPSHOT.with(|s| -> Result<String, String> {
        let metadata: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("metadata: {e}"))?;
        let prepared = cqx_rust::extract::prepare_with(&s.borrow(), metadata)
            .map_err(|e| e.to_string())?;
        let shared = prepared.gathered().shared();
        PREPARED.with(|p| *p.borrow_mut() = Some(prepared));
        serde_json::to_string(&shared).map_err(|e| e.to_string())
    });
    respond(match result {
        Ok(json) => json,
        Err(e) => serde_json::json!({ "error": e }).to_string(),
    })
}

/// Writes down everything this reader holds, resolved against what every
/// reader found.
///
/// # Safety
/// `shared` must point to `shared_len` bytes of valid UTF-8: what
/// `cqx_merge_done` returned.
#[no_mangle]
pub unsafe extern "C" fn cqx_emit(shared: *const u8, shared_len: usize) -> *mut u8 {
    let text = borrow(shared, shared_len);
    let result = SNAPSHOT.with(|s| -> Result<String, String> {
        let shared: cqx_rust::prepass::Shared =
            serde_json::from_str(&text).map_err(|e| format!("shared facts: {e}"))?;
        PREPARED.with(|p| {
            let held = p.borrow();
            let prepared = held.as_ref().ok_or("nothing has been gathered here yet")?;
            // Its own aliases, everyone's names.
            let mut facts = prepared.gathered();
            facts.adopt(shared);
            let mut out = Vec::new();
            prepared
                .emit(&s.borrow(), &facts, &mut out)
                .map_err(|e| e.to_string())?;
            Ok(String::from_utf8_lossy(&out).into_owned())
        })
    });
    respond(match result {
        Ok(facts) => facts,
        Err(e) => serde_json::json!({ "t": "error", "message": e }).to_string(),
    })
}

/// Starts a fresh union.
#[no_mangle]
pub extern "C" fn cqx_merge_reset() {
    MERGING.with(|m| *m.borrow_mut() = cqx_rust::prepass::Shared::default());
}

/// Adds one reader's report to the union.
///
/// Call order is the reading order, because later wins — exactly as a later
/// file wins within one pass.
///
/// # Safety
/// `shared` must point to `shared_len` bytes of valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn cqx_merge_add(shared: *const u8, shared_len: usize) -> *mut u8 {
    let text = borrow(shared, shared_len);
    let result: Result<(), String> = serde_json::from_str(&text)
        .map_err(|e| format!("shared facts: {e}"))
        .map(|one: cqx_rust::prepass::Shared| MERGING.with(|m| m.borrow_mut().merge(one)));
    respond(match result {
        Ok(()) => "{}".to_string(),
        Err(e) => serde_json::json!({ "error": e }).to_string(),
    })
}

/// Follows the union to its conclusion and hands it back.
#[no_mangle]
pub extern "C" fn cqx_merge_done() -> *mut u8 {
    let json = MERGING.with(|m| {
        let mut shared = m.borrow_mut();
        shared.resolve();
        serde_json::to_string(&*shared).unwrap_or_else(|_| "{}".to_string())
    });
    respond(json)
}

/// Starts a fresh collection of facts.
#[no_mangle]
pub extern "C" fn cqx_fold_reset() {
    FOLDING.with(|f| *f.borrow_mut() = cqx_store::facts::Stream::default());
}

/// Adds one reader's facts to the collection.
///
/// Parsed as it arrives rather than concatenated, because a large repository's
/// facts do not want to exist twice: makepad's are ninety-six megabytes.
///
/// # Safety
/// `facts` must point to `facts_len` bytes of valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn cqx_fold_add(facts: *const u8, facts_len: usize) -> *mut u8 {
    let text = borrow(facts, facts_len);
    let part = cqx_store::facts::Stream::from_ndjson(&text);
    FOLDING.with(|f| {
        let mut held = f.borrow_mut();
        held.nodes.extend(part.nodes);
        held.edges.extend(part.edges);
        if held.schema.is_empty() {
            held.schema = part.schema;
            held.root = part.root;
        }
    });
    respond("{}".to_string())
}

/// Turns everything the readers wrote into one dataset.
///
/// # Safety
/// Both pointers must reference that many bytes of valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn cqx_fold_done(
    repo: *const u8,
    repo_len: usize,
    config: *const u8,
    config_len: usize,
) -> *mut u8 {
    let repo = borrow(repo, repo_len);
    let config_text = borrow(config, config_len);
    let result = FOLDING.with(|f| -> Result<String, String> {
        let mut stream = f.borrow_mut();
        // Readers all describe the packages, because they all read every
        // manifest. The same node twice says nothing new, and the same
        // containment edge twice multiplies every path through it.
        stream.dedupe();
        let config = cqx_score::config::Config::from_text(if config_text.trim().is_empty() {
            None
        } else {
            Some(config_text.as_str())
        })?;
        let metrics = cqx_score::metrics::Metrics::compute(&stream, &config);
        let report = cqx_score::report_json(&config, &metrics);
        let meta = cqx_view::Meta {
            repo: repo.as_str(),
            branch: "",
            remote: None,
            commits_url: None,
            analysed_ms: None,
            fetched_ms: None,
        };
        Ok(cqx_view::dataset(&stream, report, serde_json::json!([]), &meta).to_string())
    });
    respond(match result {
        Ok(json) => json,
        Err(e) => serde_json::json!({ "error": e }).to_string(),
    })
}
