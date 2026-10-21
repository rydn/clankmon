//! Return freed heap pages to the OS after the initial scan.
//!
//! Parsing hundreds of megabytes of transcripts churns small allocations; the
//! live data afterwards is a fraction of the peak, but the system allocator
//! keeps the freed pages. Measured on macOS: 26 MB live, 73 MB fragmented.

#[cfg(target_os = "macos")]
unsafe extern "C" {
    // <malloc/malloc.h>: a null zone means every zone; goal 0 means "as much as possible".
    fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
}

/// Ask the allocator to release unused memory. Returns bytes released when known.
pub fn release() -> usize {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: documented libmalloc API; a null zone is explicitly allowed.
        unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) }
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: glibc malloc_trim has no preconditions.
        unsafe { libc::malloc_trim(0) };
        0
    }
    #[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
    {
        0
    }
}
