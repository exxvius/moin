//! Toggling a file's OS "hidden" attribute, used to keep in-progress `.part`
//! files out of sight while a download runs. Only Windows exposes a per-file
//! hidden flag we can flip in place (Unix "hidden" is the dotfile convention,
//! which would mean renaming), so it's a no-op elsewhere.

/// Show or hide `path` via the OS hidden attribute. Best-effort: a missing file
/// or an unsupported platform is silently ignored.
#[cfg(windows)]
pub fn set_hidden(path: &str, hidden: bool) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
    const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;

    // Linked from kernel32, which std already brings in on Windows.
    extern "system" {
        fn GetFileAttributesW(name: *const u16) -> u32;
        fn SetFileAttributesW(name: *const u16, attrs: u32) -> i32;
    }

    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives both calls;
    // the attribute functions only read through the pointer for their duration.
    unsafe {
        let current = GetFileAttributesW(wide.as_ptr());
        if current == INVALID_FILE_ATTRIBUTES {
            return; // file gone or inaccessible
        }
        let mut next = if hidden {
            current | FILE_ATTRIBUTE_HIDDEN
        } else {
            current & !FILE_ATTRIBUTE_HIDDEN
        };
        if next == 0 {
            next = FILE_ATTRIBUTE_NORMAL; // an empty attribute set isn't valid
        }
        if next != current {
            SetFileAttributesW(wide.as_ptr(), next);
        }
    }
}

#[cfg(not(windows))]
pub fn set_hidden(_path: &str, _hidden: bool) {}
