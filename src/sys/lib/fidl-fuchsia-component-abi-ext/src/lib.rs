// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl_fuchsia_component_resolution as fresolution;
use fidl_fuchsia_io as fio;
use fuchsia_fs::file::ReadError;
use fuchsia_fs::node::OpenError;
use thiserror::Error;
use version_history::AbiRevision;
use zx_status::Status;

#[derive(Error, Debug)]
pub enum AbiRevisionFileError {
    #[error("Failed to decode ABI revision value")]
    Decode,
    #[error("Failed to open ABI revision file: {0}")]
    Open(#[from] OpenError),
    #[error("Failed to read ABI revision file: {0}")]
    Read(#[from] ReadError),
}

impl From<AbiRevisionFileError> for fresolution::ResolverError {
    fn from(err: AbiRevisionFileError) -> fresolution::ResolverError {
        match err {
            AbiRevisionFileError::Open(_) => fresolution::ResolverError::AbiRevisionNotFound,
            AbiRevisionFileError::Read(_) | AbiRevisionFileError::Decode => {
                fresolution::ResolverError::InvalidAbiRevision
            }
        }
    }
}

/// Attempt to read an ABI revision value from the given file path, but do not fail if the file is absent.
pub async fn read_abi_revision_optional(
    dir: &fio::DirectoryProxy,
    path: &str,
) -> Result<Option<AbiRevision>, AbiRevisionFileError> {
    match read_abi_revision(dir, path).await {
        Ok(abi) => Ok(Some(abi)),
        Err(AbiRevisionFileError::Open(OpenError::OpenError(Status::NOT_FOUND))) => Ok(None),
        Err(e) => Err(e),
    }
}

// TODO(https://fxbug.dev/42063073): return fuchsia.version.AbiRevision & use decode_persistent().
/// Read an ABI revision value from the given file path.
async fn read_abi_revision(
    dir: &fio::DirectoryProxy,
    path: &str,
) -> Result<AbiRevision, AbiRevisionFileError> {
    let file = fuchsia_fs::directory::open_file(&dir, path, fio::PERM_READABLE).await?;
    let bytes: [u8; 8] = fuchsia_fs::file::read(&file)
        .await?
        .try_into()
        .map_err(|_| AbiRevisionFileError::Decode)?;
    Ok(AbiRevision::from_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_fs::directory::open_in_namespace;
    use vfs::file::vmo::read_only;
    use vfs::pseudo_directory;

    fn init_fuchsia_abi_dir(filename: &'static str, content: &'static [u8]) -> fio::DirectoryProxy {
        let dir = pseudo_directory! {
        "meta" => pseudo_directory! {
              "fuchsia.abi" => pseudo_directory! {
                filename => read_only(content),
              }
          }
        };
        vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new())
    }

    const ABI_REV_MAX: &'static [u8] = &u64::MAX.to_le_bytes();
    const ABI_REV_ZERO: &'static [u8] = &0u64.to_le_bytes();

    #[fuchsia::test]
    async fn test_read_abi_revision_impl() -> Result<(), AbiRevisionFileError> {
        // Test input that cannot be decoded into a u64 fails
        let dir = init_fuchsia_abi_dir("abi-revision", b"Invalid ABI revision string");
        let res = read_abi_revision_optional(&dir, AbiRevision::PATH).await;
        assert!(matches!(res.unwrap_err(), AbiRevisionFileError::Decode));

        let dir = init_fuchsia_abi_dir("abi-revision", b"");
        let res = read_abi_revision_optional(&dir, AbiRevision::PATH).await;
        assert!(matches!(res.unwrap_err(), AbiRevisionFileError::Decode));

        // Test u64 inputs can be read
        let dir = init_fuchsia_abi_dir("abi-revision", ABI_REV_MAX);
        let res = read_abi_revision_optional(&dir, AbiRevision::PATH).await.unwrap();
        assert_eq!(res, Some(u64::MAX.into()));

        let dir = init_fuchsia_abi_dir("abi-revision", ABI_REV_ZERO);
        let res = read_abi_revision_optional(&dir, AbiRevision::PATH).await.unwrap();
        assert_eq!(res, Some(0u64.into()));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_read_abi_revision_optional_allows_absent_file() -> Result<(), AbiRevisionFileError>
    {
        // Test abi-revision file not found produces Ok(None)
        let dir = init_fuchsia_abi_dir("abi-revision-staging", ABI_REV_MAX);
        let res = read_abi_revision_optional(&dir, AbiRevision::PATH).await.unwrap();
        assert_eq!(res, None);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_read_abi_revision_fails_absent_file() -> Result<(), AbiRevisionFileError> {
        let dir = init_fuchsia_abi_dir("a-different-file", ABI_REV_MAX);
        let err = read_abi_revision(&dir, AbiRevision::PATH).await.unwrap_err();
        assert!(matches!(err, AbiRevisionFileError::Open(OpenError::OpenError(Status::NOT_FOUND))));
        Ok(())
    }

    // Read this test package's ABI revision.
    #[fuchsia::test]
    async fn read_test_pkg_abi_revision() -> Result<(), AbiRevisionFileError> {
        let dir_proxy = open_in_namespace("/pkg", fio::PERM_READABLE).unwrap();
        let abi_revision = read_abi_revision(&dir_proxy, AbiRevision::PATH)
            .await
            .expect("test package doesn't contain an ABI revision");
        version_history_data::HISTORY
            .check_abi_revision_for_runtime(abi_revision)
            .expect("test package ABI revision should be valid");
        Ok(())
    }
}
