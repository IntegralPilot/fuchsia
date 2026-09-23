// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuse3::FileType;
use fuse3::raw::prelude::FileAttr;
use fxfs::object_store::Timestamp;
use std::time::SystemTime;

// Fxfs does not support file mode, so all files have mode 755.
const DEFAULT_FILE_MODE: u32 = 0o755;

/// Create a FUSE-style attribute given the object type and properties.
pub fn create_attr(
    id: u64,
    object_type: FileType,
    size: u64,
    creation_time: Timestamp,
    modification_time: Timestamp,
    nlink: u32,
) -> FileAttr {
    FileAttr {
        ino: id,
        size,
        blocks: 0,
        atime: SystemTime::UNIX_EPOCH.into(),
        mtime: to_fuse_time(modification_time),
        ctime: to_fuse_time(creation_time),
        kind: object_type,
        perm: fuse3::perm_from_mode_and_kind(object_type, DEFAULT_FILE_MODE),
        nlink,
        uid: 0,
        gid: 0,
        rdev: 0,
        blksize: 0,
    }
}

/// Create a FUSE-style directory attribute with specified size and timestamps.
pub fn create_dir_attr(
    id: u64,
    size: u64,
    creation_time: Timestamp,
    modification_time: Timestamp,
) -> FileAttr {
    create_attr(id, FileType::Directory, size, creation_time, modification_time, 1)
}

/// Create a FUSE-style file attribute with specified size, timestamps and number of hard links.
pub fn create_file_attr(
    id: u64,
    size: u64,
    creation_time: Timestamp,
    modification_time: Timestamp,
    nlink: u32,
) -> FileAttr {
    create_attr(id, FileType::RegularFile, size, creation_time, modification_time, nlink)
}

/// Create a FUSE-style symlink attribute with specified timestamps.
pub fn create_symlink_attr(
    id: u64,
    creation_time: Timestamp,
    modification_time: Timestamp,
) -> FileAttr {
    create_attr(id, FileType::Symlink, 0, creation_time, modification_time, 1)
}

/// Convert from Fxfs timestamp to FUSE timestamp.
pub fn to_fuse_time(time: Timestamp) -> fuse3::Timestamp {
    fuse3::Timestamp::new(time.as_secs() as i64, time.subsec_nanos())
}

/// Convert from FUSE timestamp to Fxfs timestamp.
pub fn to_fxfs_time(time: fuse3::Timestamp) -> Timestamp {
    match time.sec.try_into() {
        Ok(secs) => Timestamp::from_secs_and_nanos(secs, time.nsec),
        Err(_) => {
            // Fxfs doesn't support negative timestamps.
            Timestamp::zero()
        }
    }
}
