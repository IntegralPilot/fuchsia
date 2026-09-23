// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Initializers for ICU data files.
//!
//! Use the library by instantiating a `Loader` and keeping a reference to it for as long as you
//! need access to timezone data.  You can do this in your program as many times as needed, and the
//! loader will make sure that the data is loaded before it is first used, and that it is unloaded
//! once no more loaders are live.
//!
//! It is also possible to clone a loader in case you need to pass it along to ensure that timezone
//! data is available.
//!
//! Example use:
//!
//! ```
//! fn basic() {
//!     let _loader = Loader::new().expect("loader is constructed with success");
//!     let _loader2 = Loader::new().expect("second initialization is a no-operation");
//!     let _loader3 = _loader2.clone();  // It is OK to clone a loader and keep it around.
//! }
//! ```

use std::path::PathBuf;

use anyhow::{Context, format_err};
use fuchsia_sync::Mutex;
use std::borrow::Cow;
use std::sync::{Arc, LazyLock, Weak};
use std::{env, fs, io};
use thiserror::Error;
use {rust_icu_common as icu, rust_icu_ucal as ucal, rust_icu_udata as udata};

// The storage for the loaded ICU data.  At most one may be loaded at any given time.
static REFCOUNT: LazyLock<Mutex<Weak<PathBuf>>> = LazyLock::new(|| Mutex::new(Weak::new()));

// The default location at which to find the ICU data.
// The icudtl.dat is deliberately omitted to conform to the loading
// rules described at:
// https://unicode-org.github.io/icu/userguide/icu_data/#how-data-loading-works
const ICU_DATA_PATH_DEFAULT: &str = "/pkg/data";

/// Minimum expected length of a time zone revision ID (e.g. "2019c").
const MIN_TZ_REVISION_ID_LENGTH: usize = 5;
/// Maximum expected length of a time zone revision ID.
const MAX_TZ_REVISION_ID_LENGTH: usize = 15;

/// Error type returned by `icu_udata`. The individual enum values encode
/// classes of possible errors returned.
#[derive(Error, Debug)]
pub enum Error {
    #[error("[icu_data]: {}", _0)]
    Fail(anyhow::Error),
    /// The operation failed due to an underlying Zircon error.
    #[error("[icu_data]: generic error: {}, details: {:?}", _0, _1)]
    Status(zx::Status, Option<Cow<'static, str>>),
    /// The operation failed due to an IO error.
    #[error("[icu_data]: IO error: {}", _0)]
    IO(io::Error),
    /// The operation failed due to an ICU library error.
    #[error("[icu_data]: ICU error: {}", _0)]
    ICU(icu::Error),
}
impl From<zx::Status> for Error {
    fn from(status: zx::Status) -> Self {
        Error::Status(status, None)
    }
}
impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::IO(err)
    }
}
impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Self {
        Error::Fail(err)
    }
}
impl From<icu::Error> for Error {
    fn from(err: icu::Error) -> Self {
        Error::ICU(err)
    }
}

/// Manages the lifecycle of the loaded ICU data.
///
/// `Loader` can be created using `Loader::new` and can be cloned.  For as long as any Loader
/// remains in scope, the ICU data will not be unloaded.
#[derive(Debug, Clone)]
pub struct Loader {
    // The reference here holds the ICU data in memory. It should be held live
    // until the end of the program.
    _refs: Arc<PathBuf>,
}
// Loader is OK to be sent to threads.
unsafe impl Sync for Loader {}

impl Loader {
    /// Initializes the ICU dynamic timezone data, based on the default resource directory.
    ///
    /// The caller should create a `Loader` very early on in the lifetime of the program, and keep
    /// instances of `Loader` alive until the ICU data is needed.  You can make as many `Loader`
    /// objects as you need.  The data will be unloaded only after the last of them leaves scope.
    pub fn new() -> Result<Self, Error> {
        Self::new_with_optional_tz_resources(None, None)
    }

    /// Initializes ICU data, loading time zone resources from the supplied `path`.
    ///
    /// See documentation for `new` for calling constraints.
    pub fn new_with_tz_resource_path(tzdata_dir_path: &str) -> Result<Self, Error> {
        Self::new_with_optional_tz_resources(Some(tzdata_dir_path), None)
    }

    /// Initializes ICU data, loading time zone resources from the supplied `path` and validating
    /// the time zone revision ID against the ID contained in the file at `revision_file_path`.
    ///
    /// See documentation for `new` for calling constraints.
    pub fn new_with_tz_resources_and_validation(
        tzdata_dir_path: &str,
        tz_revision_file_path: &str,
    ) -> Result<Self, Error> {
        Self::new_with_optional_tz_resources(Some(tzdata_dir_path), Some(tz_revision_file_path))
    }

    // Ensures that all calls to create a `Loader` go through the same code path.
    fn new_with_optional_tz_resources(
        tzdata_dir_path: Option<&str>,
        tz_revision_file_path: Option<&str>,
    ) -> Result<Self, Error> {
        // The lock contention should not be an issue.  Only a few calls (single digits) to this
        // function are expected.  So we take a write lock immmediately.
        let mut l = REFCOUNT.lock();
        match l.upgrade() {
            Some(_refs) => Ok(Loader { _refs }),
            None => {
                // Load up the TZ files directory.
                if let Some(p) = tzdata_dir_path {
                    let for_path = fs::File::open(p)
                        .map_err(|e| Error::Fail(format_err!("io error: {}", e)))
                        .with_context(|| format!("error while opening: {:?}", tzdata_dir_path))?;
                    let meta = for_path
                        .metadata()
                        .with_context(|| format!("while getting metadata for: {:?}", p))?;
                    if !meta.is_dir() {
                        return Err(Error::Fail(format_err!("not a directory: {}", p)));
                    }
                    // This is the default API used to configure the ICU library, so we are
                    // just using it here.  Even though it is not a preferred way to configure
                    // Fuchsia programs.
                    // Further, we want to load the same ICU data for all programs that need this
                    // file.
                    unsafe { env::set_var("ICU_TIMEZONE_FILES_DIR", p) };
                }

                // Read ICU data file from the filesystem. The ICU library should
                // take care to load only the needed parts, since the entire common
                // data file is fairly large.
                let path = PathBuf::from(ICU_DATA_PATH_DEFAULT);
                udata::set_data_directory(&path);
                let _refs = Arc::new(path);
                Self::validate_revision(tz_revision_file_path)?;
                (*l) = Arc::downgrade(&_refs);
                Ok(Loader { _refs })
            }
        }
    }

    fn validate_revision(tz_revision_file_path: Option<&str>) -> Result<(), Error> {
        match tz_revision_file_path {
            None => Ok(()),
            Some(tz_revision_file_path) => {
                let expected_revision_id = std::fs::read_to_string(tz_revision_file_path)
                    .with_context(|| {
                        format!("could not read file: {:?}", tz_revision_file_path)
                    })?;
                if !(MIN_TZ_REVISION_ID_LENGTH..=MAX_TZ_REVISION_ID_LENGTH)
                    .contains(&expected_revision_id.len())
                {
                    return Err(Error::Status(
                        zx::Status::IO_DATA_INTEGRITY,
                        Some(
                            format!(
                                "invalid revision ID in {}: {}",
                                tz_revision_file_path, expected_revision_id
                            )
                            .into(),
                        ),
                    ));
                }

                let actual_revision_id = ucal::get_tz_data_version().with_context(|| {
                    format!("while getting data version from: {:?}", tz_revision_file_path)
                })?;
                if expected_revision_id != actual_revision_id {
                    return Err(Error::Status(
                        zx::Status::IO_DATA_INTEGRITY,
                        Some(
                            format!(
                                "expected revision ID {} but got {}",
                                expected_revision_id, actual_revision_id
                            )
                            .into(),
                        ),
                    ));
                }

                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use rust_icu_uenum as uenum;

    // [START loader_example]
    #[test]
    fn initialization() {
        let _loader = Loader::new().expect("loader is constructed with success");
        let _loader2 = Loader::new().expect("loader is just fine with a second initialization");
        let tz: String = uenum::open_time_zones().unwrap().take(1).map(|e| e.unwrap()).collect();
        assert_eq!(tz, "ACT");
        // The library will be cleaned up after the last of the loaders goes out of scope.
    }

    #[test]
    fn you_can_also_clone_loaders() {
        let _loader = Loader::new().expect("loader is constructed with success");
        let _loader2 = Loader::new().expect("loader is just fine with a second initialization");
        let _loader3 = _loader2.clone();
        let tz: String = uenum::open_time_zones().unwrap().take(1).map(|e| e.unwrap()).collect();
        assert_eq!(tz, "ACT");
    }

    #[test]
    fn two_initializations_in_a_row() {
        {
            let _loader = Loader::new().expect("loader is constructed with success");
            let tz: String =
                uenum::open_time_zones().unwrap().take(1).map(|e| e.unwrap()).collect();
            assert_eq!(tz, "ACT");
        }
        {
            let _loader2 = Loader::new().expect("loader is just fine with a second initialization");
            let tz: String =
                uenum::open_time_zones().unwrap().take(1).map(|e| e.unwrap()).collect();
            assert_eq!(tz, "ACT");
        }
    }
    // [END loader_example]

    #[test]
    fn test_tz_res_loading_without_validation() -> Result<(), Error> {
        let _loader = Loader::new().expect("loader is constructed with success");
        let tz: String = uenum::open_time_zones()?.take(1).map(|e| e.unwrap()).collect();
        assert_eq!(tz, "ACT");
        Ok(())
    }

    #[test]
    fn test_tz_res_loading_with_validation_valid() -> Result<(), Error> {
        let _loader = Loader::new_with_tz_resources_and_validation(
            "/pkg/data/tzdata/icu/44/le",
            "/pkg/data/tzdata/revision.txt",
        )
        .expect("loader is constructed successfully");
        let tz: String = uenum::open_time_zones()?.take(1).map(|e| e.unwrap()).collect();
        assert_eq!(tz, "ACT");
        Ok(())
    }

    #[test]
    fn test_tz_res_loading_with_validation_invalid() -> Result<(), Error> {
        let result = Loader::new_with_tz_resources_and_validation(
            "/pkg/data/tzdata/icu/44/le",
            "/pkg/data/test_inconsistent_revision.txt",
        );
        let err = result.unwrap_err();
        assert_matches!(err, Error::Status(zx::Status::IO_DATA_INTEGRITY, Some(_)));
        Ok(())
    }
}
