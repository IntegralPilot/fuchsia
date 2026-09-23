// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuse3::{Errno, Result};
use fxfs::errors::FxfsError;
use fxfs::log::info;

/// Pre-defined Fxfs Result with FxfsError.
pub type FxfsResult<T> = std::result::Result<T, anyhow::Error>;

/// Convert from FxfsError to libc error.
pub fn fxfs_error_to_fuse_error(err: &anyhow::Error) -> Errno {
    if let Some(root_cause) = err.root_cause().downcast_ref::<FxfsError>() {
        let err = match root_cause {
            FxfsError::NotFound => libc::ENOENT.into(),
            FxfsError::NotDir => libc::ENOTDIR.into(),
            FxfsError::Deleted => libc::ENOENT.into(),
            FxfsError::AlreadyExists => libc::EEXIST.into(),
            FxfsError::NotFile => libc::EISDIR.into(),
            FxfsError::NotEmpty => libc::ENOTEMPTY.into(),
            FxfsError::ReadOnlyFilesystem => libc::EROFS.into(),
            FxfsError::NoSpace => libc::ENOSPC.into(),
            FxfsError::InvalidArgs => libc::EINVAL.into(),
            FxfsError::TooBig => libc::EFBIG.into(),
            FxfsError::NotSupported => libc::ENOTSUP.into(),
            FxfsError::AccessDenied => libc::EACCES.into(),
            FxfsError::OutOfRange => libc::EINVAL.into(),
            FxfsError::BadPath => libc::ENAMETOOLONG.into(),

            // Below are Fxfs-specific errors without specific libc translation.
            FxfsError::Inconsistent => libc::EIO.into(),
            FxfsError::Internal => libc::EIO.into(),
            FxfsError::InvalidVersion => libc::ENOTSUP.into(),
            FxfsError::JournalFlushError => libc::EIO.into(),
            FxfsError::AlreadyBound => libc::EBUSY.into(),
            FxfsError::WrongType => libc::EOPNOTSUPP.into(),
            FxfsError::IntegrityError => libc::EIO.into(),
            FxfsError::Unavailable => libc::EBUSY.into(),
            FxfsError::NoKey => libc::ENOKEY.into(),
            FxfsError::InconsistentEncryptionPolicy => libc::EXDEV.into(),
        };
        info!("Converted from Fxfs error {:?} to libc error {:?}", root_cause, err);
        err
    } else {
        // No specific translation, so return a generic value.
        libc::EIO.into()
    }
}

pub trait FuseErrorParser<S> {
    /// Convert from Fxfs Result to Fuse Result.
    fn fxfs_result_to_fuse_result(self) -> Result<S>;
}

impl<S> FuseErrorParser<S> for std::result::Result<S, anyhow::Error> {
    fn fxfs_result_to_fuse_result(self) -> Result<S> {
        if let Ok(ret) = self {
            Ok(ret)
        } else {
            let err = &self.err().unwrap();
            Err(fxfs_error_to_fuse_error(err))
        }
    }
}
