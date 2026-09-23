// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fuchsia.IO UTIL-ity library

use flex_fuchsia_io as fio;

pub mod directory;
pub mod file;
pub mod node;

// Types/constants re-exported from fidl_fuchsia_io for convenience.
// TODO(https://324111518): Remove the re-export of OpenFlags.
pub use fio::{Flags, OpenFlags, PERM_EXECUTABLE, PERM_READABLE, PERM_WRITABLE};

/// canonicalize_path will remove a leading `/` if it exists, since it's always unnecessary and in
/// some cases disallowed (https://fxbug.dev/42103076).
pub fn canonicalize_path(path: &str) -> &str {
    if path == "/" {
        return ".";
    }
    if path.starts_with('/') {
        return &path[1..];
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async as fasync;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;
    use vfs::file::vmo::read_only;
    use vfs::pseudo_directory;
    use vfs::remote::remote_dir;
    use zx_status;

    #[fasync::run_singlethreaded(test)]
    async fn open_and_read_file_test() {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        let data = "abc".repeat(10000);
        fs::write(tempdir.path().join("myfile"), &data).expect("failed writing file");

        let dir = crate::directory::open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE,
        )
        .expect("could not open tmp dir");
        let file = directory::open_file_async(&dir, "myfile", fio::PERM_READABLE)
            .expect("could not open file");
        let contents = file::read_to_string(&file).await.expect("could not read file");
        assert_eq!(&contents, &data, "File contents did not match");
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_and_write_file_test() {
        // Create temp dir for test.
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        let dir = crate::directory::open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .expect("could not open tmp dir");

        // Write contents.
        let file_name = Path::new("myfile");
        let data = "abc".repeat(10000);
        let file = directory::open_file_async(
            &dir,
            file_name.to_str().unwrap(),
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE,
        )
        .expect("could not open file");
        file::write(&file, &data).await.expect("could not write file");

        // Verify contents.
        let contents = std::fs::read_to_string(tempdir.path().join(file_name)).unwrap();
        assert_eq!(&contents, &data, "File contents did not match");
    }

    #[test]
    fn test_canonicalize_path() {
        assert_eq!(canonicalize_path("/"), ".");
        assert_eq!(canonicalize_path("/foo"), "foo");
        assert_eq!(canonicalize_path("/foo/bar/"), "foo/bar/");

        assert_eq!(canonicalize_path("."), ".");
        assert_eq!(canonicalize_path("./"), "./");
        assert_eq!(canonicalize_path("foo/bar/"), "foo/bar/");
    }

    #[fasync::run_singlethreaded(test)]
    async fn flags_test() {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        std::fs::write(tempdir.path().join("read_write"), "rw/read_write")
            .expect("failed to write file");
        let dir = crate::directory::open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .expect("could not open tmp dir");
        let example_dir = pseudo_directory! {
            "ro" => pseudo_directory! {
                "read_only" => read_only("ro/read_only"),
            },
            "rw" => remote_dir(dir)
        };
        let example_dir_proxy = vfs::directory::serve(
            example_dir,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        );

        for (file_name, flags, should_succeed) in vec![
            ("ro/read_only", fio::PERM_READABLE, true),
            ("ro/read_only", fio::PERM_READABLE | fio::PERM_WRITABLE, false),
            ("ro/read_only", fio::PERM_WRITABLE, false),
            ("rw/read_write", fio::PERM_READABLE, true),
            ("rw/read_write", fio::PERM_READABLE | fio::PERM_WRITABLE, true),
            ("rw/read_write", fio::PERM_WRITABLE, true),
        ] {
            let file_proxy =
                directory::open_file_async(&example_dir_proxy, file_name, flags).unwrap();
            match (should_succeed, file_proxy.query().await) {
                (true, Ok(_)) => (),
                (false, Err(_)) => continue,
                (true, Err(e)) => {
                    panic!("failed to open when expected success, couldn't describe: {:?}", e)
                }
                (false, Ok(d)) => {
                    panic!("successfully opened when expected failure, could describe: {:?}", d)
                }
            }
            if flags.intersects(fio::Flags::PERM_READ_BYTES) {
                assert_eq!(
                    file_name,
                    file::read_to_string(&file_proxy).await.expect("failed to read file")
                );
            }
            if flags.intersects(fio::Flags::PERM_WRITE_BYTES) {
                let _: u64 = file_proxy
                    .write(b"write_only")
                    .await
                    .expect("write failed")
                    .map_err(zx_status::Status::err_from_raw)
                    .expect("write error");
            }
            assert_eq!(file_proxy.close().await.unwrap(), Ok(()));
        }
    }
}
