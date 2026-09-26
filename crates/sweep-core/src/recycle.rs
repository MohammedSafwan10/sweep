//! Explicit Recycle Bin inspection and emptying. Never called by `clean`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RecycleStats {
    pub items: u64,
    pub bytes: u64,
}

#[derive(Debug, Error)]
pub enum RecycleError {
    #[error("Recycle Bin operations are only supported on Windows")]
    Unsupported,
    #[error("{operation} failed with HRESULT 0x{hresult:08X}")]
    Shell {
        operation: &'static str,
        hresult: u32,
    },
    #[error("Recycle Bin returned invalid negative counts")]
    InvalidCounts,
}

#[cfg(windows)]
fn root_wide(drive: char) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let root = format!("{}:\\", drive.to_ascii_uppercase());
    std::ffi::OsStr::new(&root)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

#[cfg(windows)]
pub fn query(drive: char) -> Result<RecycleStats, RecycleError> {
    use windows_sys::Win32::UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO};

    let root = root_wide(drive);
    let mut info = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        ..SHQUERYRBINFO::default()
    };
    // SAFETY: `root` is NUL-terminated and lives for the call. `info` is a
    // writable structure whose cbSize matches its allocated size.
    let result = unsafe { SHQueryRecycleBinW(root.as_ptr(), &mut info) };
    if result < 0 {
        return Err(RecycleError::Shell {
            operation: "SHQueryRecycleBinW",
            hresult: result as u32,
        });
    }
    Ok(RecycleStats {
        items: u64::try_from(info.i64NumItems).map_err(|_| RecycleError::InvalidCounts)?,
        bytes: u64::try_from(info.i64Size).map_err(|_| RecycleError::InvalidCounts)?,
    })
}

#[cfg(windows)]
pub fn empty(drive: char) -> Result<(), RecycleError> {
    use windows_sys::Win32::UI::Shell::{
        SHEmptyRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND,
    };

    let root = root_wide(drive);
    let flags = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND;
    // SAFETY: `root` is NUL-terminated and lives for the call. The CLI
    // obtained explicit confirmation before invoking this function.
    let result = unsafe { SHEmptyRecycleBinW(std::ptr::null_mut(), root.as_ptr(), flags) };
    if result < 0 {
        return Err(RecycleError::Shell {
            operation: "SHEmptyRecycleBinW",
            hresult: result as u32,
        });
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn query(_drive: char) -> Result<RecycleStats, RecycleError> {
    Err(RecycleError::Unsupported)
}

#[cfg(not(windows))]
pub fn empty(_drive: char) -> Result<(), RecycleError> {
    Err(RecycleError::Unsupported)
}
