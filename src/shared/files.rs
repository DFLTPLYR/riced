//! File identity for decoded asset reuse, including atomic replacements.
use std::{path::Path, time::SystemTime};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FileStamp {
    length: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64, u64, u64),
}

pub(crate) fn stamp(path: &Path) -> Option<FileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Some(FileStamp {
        length: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        changed: (
            metadata.ctime(),
            metadata.ctime_nsec(),
            metadata.ino(),
            metadata.dev(),
        ),
    })
}
