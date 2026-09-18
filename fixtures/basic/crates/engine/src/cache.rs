/// Planted: an unsafe block in a crate that has no business having one.
pub fn fast_len(bytes: &[u8]) -> usize {
    unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()).len() }
}

pub static mut HITS: u64 = 0;
