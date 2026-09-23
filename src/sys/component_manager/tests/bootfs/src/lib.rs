// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_io as fio;
use fuchsia_fs::{directory, file};
use futures::StreamExt;

// Since this is a system test, we're actually going to verify real system critical files. That
// means that these tests take a dependency on these files existing in the system, which may
// not forever be true. If any of the files listed here are removed, it's fine to update the set
// of checked files.
const SAMPLE_UTF8_READONLY_FILE: &str = "/boot/config/build_info/minimum_utc_stamp";
const KERNEL_VDSO_DIRECTORY: &str = "/boot/kernel/vdso";
const BOOTFS_READONLY_FILES: &[&str] = &["/boot/config/component_manager"];
const BOOTFS_DATA_DIRECTORY: &str = "/boot/data";
const BOOTFS_EXECUTABLE_LIB_FILES: &[&str] = &["ld.so.1"];
const BOOTFS_EXECUTABLE_NON_LIB_FILES: &[&str] = &["/boot/bin/component_manager"];

#[fuchsia::test]
async fn basic_filenode_test() -> Result<(), Error> {
    let file = file::open_in_namespace(SAMPLE_UTF8_READONLY_FILE, fio::PERM_READABLE)
        .context("failed to open as a readable file")?;

    // We only support the attributes the rust VFS VmoFile type does.
    let query = fio::NodeAttributesQuery::CONTENT_SIZE
        | fio::NodeAttributesQuery::ID
        | fio::NodeAttributesQuery::STORAGE_SIZE;
    let (_, immutable_attributes) = file.get_attributes(query).await.unwrap().unwrap();

    assert_ne!(immutable_attributes.id.unwrap(), fio::INO_UNKNOWN);
    assert!(immutable_attributes.content_size.unwrap() > 0);
    assert!(immutable_attributes.storage_size.unwrap() > 0);

    // Check for data corruption. This file should contain a single utf-8 string which can
    // be converted into a non-zero unsigned integer.
    let file_contents =
        file::read_to_string(&file).await.context("failed to read utf-8 file to string")?;
    let parsed_time = file_contents
        .trim()
        .parse::<u64>()
        .context("failed to utf-8 string as a number (and it should be a number!)")?;
    assert_ne!(parsed_time, 0);

    file::close(file).await?;

    Ok(())
}

#[fuchsia::test]
async fn check_kernel_vmos() -> Result<(), Error> {
    let directory = directory::open_in_namespace(
        KERNEL_VDSO_DIRECTORY,
        fio::PERM_READABLE | fio::PERM_EXECUTABLE,
    )
    .context("failed to open kernel vdso directory")?;
    let vdsos = fuchsia_fs::directory::readdir(&directory)
        .await
        .context("failed to read kernel vdso directory")?;

    // We should have added at least the default VDSO.
    assert_ne!(vdsos.len(), 0);
    directory::close(directory).await?;

    // All VDSOs should have execution rights.
    for vdso in vdsos {
        let name = format!("{}/{}", KERNEL_VDSO_DIRECTORY, vdso.name);
        let file = file::open_in_namespace(&name, fio::PERM_READABLE | fio::PERM_EXECUTABLE)
            .context("failed to open file")?;
        let data = file::read_num_bytes(&file, 1).await.context(format!(
            "failed to read a single byte from a vdso opened as read-execute: {}",
            name
        ))?;
        assert_ne!(data.len(), 0);
        file::close(file).await?;
    }

    Ok(())
}

#[fuchsia::test]
async fn check_executable_files() -> Result<(), Error> {
    // Sanitizers nest lib files within '/boot/lib/asan' or '/boot/lib/asan-ubsan' etc., so
    // we need to just search recursively for these files instead.
    let directory =
        directory::open_in_namespace("/boot/lib", fio::PERM_READABLE | fio::PERM_EXECUTABLE)
            .context("failed to open /boot/lib directory")?;
    let lib_paths = fuchsia_fs::directory::readdir_recursive(&directory, None)
        .filter_map(|result| async {
            assert!(result.is_ok());
            let entry = result.unwrap();
            for file in BOOTFS_EXECUTABLE_LIB_FILES {
                if entry.name.ends_with(file) {
                    return Some(format!("/boot/lib/{}", entry.name));
                }
            }

            None
        })
        .collect::<Vec<String>>()
        .await;
    directory::close(directory).await?;

    // Should have found all of the library files.
    assert_eq!(lib_paths.len(), BOOTFS_EXECUTABLE_LIB_FILES.len());
    let paths = [
        lib_paths,
        BOOTFS_EXECUTABLE_NON_LIB_FILES.iter().map(|val| val.to_string()).collect::<Vec<_>>(),
    ]
    .concat();

    for path in paths {
        let file = file::open_in_namespace(&path, fio::PERM_READABLE | fio::PERM_EXECUTABLE)
            .context("failed to open file")?;
        let data = file::read_num_bytes(&file, 1).await.context(format!(
            "failed to read a single byte from a file opened as read-execute: {}",
            path
        ))?;
        assert_ne!(data.len(), 0);
        file::close(file).await?;
    }

    Ok(())
}

#[fuchsia::test]
async fn check_readonly_files() -> Result<(), Error> {
    // There is a large variation in what different products have in the data directory, so
    // just search it during the test time and find some files. Every file in the data directory
    // should be readonly.
    let directory = directory::open_in_namespace(
        BOOTFS_DATA_DIRECTORY,
        fio::PERM_READABLE | fio::PERM_EXECUTABLE,
    )
    .context("failed to open data directory")?;
    let data_paths = fuchsia_fs::directory::readdir_recursive(&directory, None)
        .filter_map(|result| async {
            assert!(result.is_ok());
            Some(format!("{}/{}", BOOTFS_DATA_DIRECTORY, result.unwrap().name))
        })
        .collect::<Vec<String>>()
        .await;
    directory::close(directory).await?;

    let paths =
        [data_paths, BOOTFS_READONLY_FILES.iter().map(|val| val.to_string()).collect::<Vec<_>>()]
            .concat();

    for path in paths {
        // A readonly file should not be usable when opened as executable.
        let mut file = file::open_in_namespace(&path, fio::PERM_READABLE | fio::PERM_EXECUTABLE)
            .context("failed to open file")?;
        let result = file::read_num_bytes(&file, 1).await;
        assert!(result.is_err());
        // Don't close the file proxy -- the access error above has already closed the channel.

        // Reopen as readonly, and confirm that it can be read from.
        file = file::open_in_namespace(&path, fio::PERM_READABLE).context("failed to open file")?;
        let data = file::read_num_bytes(&file, 1).await.context(format!(
            "failed to read a single byte from a file opened as readonly: {}",
            path
        ))?;
        assert_ne!(data.len(), 0);
        file::close(file).await?;
    }

    Ok(())
}
