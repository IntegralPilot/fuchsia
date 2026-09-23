// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module provides a way to copy files preferentially using hardlinks, and
//! will fall back to a slow copy if that fails.

use anyhow::{Context, Result};
use std::path::Path;
use walkdir::WalkDir;
use xattr;

/// Copy a file from `src` to `dst`, using hardlinks if possible, and silently
/// falling back to a "slow" copy (using `std::fs::copy()`) if the hardlink
/// fails to be created.
pub fn fast_copy(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();

    // Defer to a function that doesn't need to be monomorphized.
    fast_copy_impl(src, dst)
}

/// Copy a directory from `src` to `dst`, recursively.
pub fn copy_dir(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();
    let walker = WalkDir::new(src);
    for entry in walker.into_iter() {
        let entry = entry?;
        let to_path = dst.join(entry.path().strip_prefix(src)?);
        if entry.metadata()?.is_dir() {
            if to_path.exists() {
                continue;
            } else {
                std::fs::create_dir_all(&to_path)
                    .with_context(|| format!("creating {}", to_path.display()))?;
            }
        } else {
            fast_copy(entry.path(), &to_path).with_context(|| {
                format!("copying {} to {}", entry.path().display(), to_path.display())
            })?;
        }
    }
    Ok(())
}

/// An internal helper method that doesn't use generics
fn fast_copy_impl(src: &Path, dst: &Path) -> Result<()> {
    // Get the actual src file, if it exists, as 'hard_link' and 'copy' won't
    // follow symlinks, but copy the symlink itself (which isn't our goal)
    let real_path = src
        .canonicalize()
        .with_context(|| format!("getting the real path for: {}", src.display()))?;

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating parent directories for {}: ", dst.display()))?;
    }

    // Perform a hardlink if possible
    std::fs::hard_link(&real_path, dst).or_else(|_| {
        // falling back to a slow copy if it can't be copied.
        std::fs::copy(&real_path, dst)
            .map(|_| ())
            .with_context(|| format!("copying {} to {}", src.display(), dst.display()))?;
        // Also copy the extended attributes (such as hashes used by build systems)
        _copy_xattrs(&real_path, dst);
        Ok(())
    })
}

fn _copy_xattrs(src: &Path, dst: &Path) {
    if let Ok(xattr_names) = xattr::list(&src) {
        for xattr_name in xattr_names {
            if let Ok(Some(value)) = xattr::get(&src, &xattr_name) {
                let _ = xattr::set(dst, &xattr_name, &value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_simple_copy() {
        let temp_dir = TempDir::new().unwrap();
        let src = temp_dir.path().join("src.txt");
        let dst = temp_dir.path().join("dst.txt");

        std::fs::write(&src, "some text").unwrap();

        fast_copy(src, &dst).unwrap();

        let dst_contents = std::fs::read(dst).unwrap();
        let dst_string = String::from_utf8(dst_contents).unwrap();

        assert_eq!(dst_string, "some text");
    }

    #[test]
    fn test_missing_source_fails() {
        let temp_dir = TempDir::new().unwrap();
        let src = temp_dir.path().join("src.txt");
        let dst = temp_dir.path().join("dst.txt");

        let result = fast_copy(src, &dst);
        assert!(result.is_err());
    }

    #[test]
    fn test_creates_parents_of_dest() {
        let temp_dir = TempDir::new().unwrap();
        let src = temp_dir.path().join("src.txt");
        let dst = temp_dir.path().join("some/nonexistent/parent/dst.txt");

        std::fs::write(&src, "some text").unwrap();

        fast_copy(src, &dst).unwrap();

        let dst_contents = std::fs::read(dst).unwrap();
        let dst_string = String::from_utf8(dst_contents).unwrap();

        assert_eq!(dst_string, "some text");
    }
}

#[cfg(test)]
#[cfg(target_family = "unix")]
mod tests_unix {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use tempfile::TempDir;

    #[test]
    fn test_copy_is_hardlink() {
        let temp_dir = TempDir::new().unwrap();
        let src = temp_dir.path().join("src.txt");
        let dst = temp_dir.path().join("dst.txt");

        std::fs::write(&src, "some text").unwrap();

        fast_copy(&src, &dst).unwrap();

        let dst_contents = std::fs::read(&dst).unwrap();
        let dst_string = String::from_utf8(dst_contents).unwrap();

        assert_eq!(dst_string, "some text");

        let src_metadata = std::fs::metadata(&src).unwrap();
        let dst_metadata = std::fs::metadata(&dst).unwrap();

        assert_eq!(
            src_metadata.ino(),
            dst_metadata.ino(),
            "src and dst files have different inodes"
        )
    }

    #[test]
    fn test_copy_through_symlink_is_hardlink() {
        let temp_dir = TempDir::new().unwrap();
        let src = temp_dir.path().join("src.txt");
        let symlink = temp_dir.path().join("symlink.txt");
        let dst = temp_dir.path().join("dst.txt");

        std::fs::write(&src, "some text").unwrap();

        std::os::unix::fs::symlink(&src, &symlink).unwrap();

        fast_copy(&symlink, &dst).unwrap();

        let dst_contents = std::fs::read(&dst).unwrap();
        let dst_string = String::from_utf8(dst_contents).unwrap();

        assert_eq!(dst_string, "some text");

        // Validate that it copied the source file, not the symlink.  This is after
        // the contents have been validated to ensure that the file actually exists,
        // as this will return false if the file doesn't exist.
        assert!(!dst.is_symlink());

        // Validate that the dst file is a hardlink to the src file, as they should
        // have the same inode if they are hardlinks to the same file.
        let src_metadata = std::fs::metadata(&src).unwrap();
        let dst_metadata = std::fs::metadata(&dst).unwrap();
        assert_eq!(
            src_metadata.ino(),
            dst_metadata.ino(),
            "src and dst files have different inodes"
        )
    }

    #[test]
    fn test_copy_through_broken_symlink_fails() {
        let temp_dir = TempDir::new().unwrap();
        let src = temp_dir.path().join("src.txt");
        let symlink = temp_dir.path().join("symlink.txt");
        let dst = temp_dir.path().join("dst.txt");

        std::os::unix::fs::symlink(&src, &symlink).unwrap();

        let result = fast_copy(&symlink, &dst);
        assert!(result.is_err());
    }

    #[test]
    fn test_copy_xattrs() {
        let temp_dir = TempDir::new().unwrap();
        let src = temp_dir.path().join("src.txt");
        let dst = temp_dir.path().join("dst.txt");

        std::fs::write(&src, "some text").unwrap();
        std::fs::write(&dst, "some text").unwrap();

        let name = std::ffi::OsString::from("user.fuchsia.test");
        let value = "some_value".as_bytes().to_vec();
        xattr::set(&src, &name, &value).unwrap();

        _copy_xattrs(&src, &dst);

        assert_eq!(Some(value), xattr::get(&dst, &name).unwrap());
    }
}
