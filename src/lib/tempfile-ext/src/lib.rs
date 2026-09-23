// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::File;
use std::io::{self, ErrorKind, Read as _, Seek as _, SeekFrom};
use std::path::Path;
use tempfile::{NamedTempFile, PersistError};

const CHUNK_SIZE: usize = 8192;

pub trait NamedTempFileExt {
    /// Atomically overwrite the target path if the contents are different.
    ///
    /// This is conceptually similar to [tempfile::NamedTempFile::persist], but
    /// will only overwrite the target path if:
    ///
    /// * The target path does not exist.
    /// * The target path has different contents than `self`.
    ///
    /// It will not overwrite the target path if it already exists and has the
    /// same content.
    ///
    /// Either way, it will return an opened `File` for the target path.
    fn persist_if_changed<P: AsRef<Path>>(self, path: P) -> Result<File, PersistError>;
}

impl NamedTempFileExt for NamedTempFile {
    fn persist_if_changed<P: AsRef<Path>>(mut self, path: P) -> Result<File, PersistError> {
        let path = path.as_ref();

        // Open up the file and see if the contents are the same. If so, return
        // the old file, otherwise overwrite it with the new contents.
        if let Ok(mut old_file) = File::open(&path) {
            if let Err(err) = self.seek(SeekFrom::Start(0)) {
                return Err(PersistError { error: err, file: self });
            }

            match files_have_same_content(self.as_file_mut(), &mut old_file) {
                Ok(true) => {
                    // Since the two files have the same contents, we'll return
                    // the old one. We read from it, so reset back to the start.
                    if let Err(err) = old_file.seek(SeekFrom::Start(0)) {
                        return Err(PersistError { error: err, file: self });
                    }

                    return Ok(old_file);
                }
                Ok(false) => {}
                Err(err) => {
                    return Err(PersistError { error: err, file: self });
                }
            }
        }

        self.persist(path)
    }
}

/// Checks if the two files have the same contents. Returns `true` if so,
/// otherwise `false`.
fn files_have_same_content(lhs: &mut File, rhs: &mut File) -> io::Result<bool> {
    let lhs_metadata = lhs.metadata()?;
    let rhs_metadata = rhs.metadata()?;

    // The files cannot have the same content if they have different lengths.
    if lhs_metadata.len() != rhs_metadata.len() {
        return Ok(false);
    }

    files_after_checking_length_have_same_content(lhs, rhs)
}

/// Check the contents of two files have the same contents, after we have
/// checked they have the same length. Returns `true` if they have the same
/// contents, `false` if not.
///
/// This will return `false` if the files changed length while checking.
fn files_after_checking_length_have_same_content(
    lhs: &mut File,
    rhs: &mut File,
) -> io::Result<bool> {
    let mut lhs_bytes = [0; CHUNK_SIZE];
    let mut rhs_bytes = [0; CHUNK_SIZE];

    loop {
        // Read the next chunk.
        let lhs_len = lhs.read(&mut lhs_bytes)?;
        let lhs_bytes = &lhs_bytes[0..lhs_len];

        if lhs_bytes.is_empty() {
            // We hit the end of `lhs`, so we'll need to check if `rhs` is also
            // at the end of the file. We can't use `read_exact` to check for
            // this, since it's not guaranteed to read from the file if we pass
            // it a zero-sized slice.
            if rhs.read(&mut rhs_bytes)? == 0 {
                return Ok(true);
            } else {
                return Ok(false);
            }
        }

        // Otherwise, read the same length from the other file.
        let rhs_bytes = &mut rhs_bytes[0..lhs_len];
        match rhs.read_exact(rhs_bytes) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::UnexpectedEof => {
                // The file was truncated while we were reading from it, so
                // return that the two files are different.
                return Ok(false);
            }
            Err(err) => {
                return Err(err);
            }
        }

        // Return false if the two chunks are not the same.
        if lhs_bytes != rhs_bytes {
            return Ok(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::os::unix::fs::MetadataExt as _;
    use tempfile::TempDir;

    #[test]
    fn test_persist_if_changed() {
        for size in [1, 20, CHUNK_SIZE - 1, CHUNK_SIZE, CHUNK_SIZE + 1, CHUNK_SIZE * 10] {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("file");

            let mut body = (0..u8::MAX).cycle().take(size).collect::<Vec<_>>();

            // We should persist the file if it doesn't already exist.
            let mut tmp1 = NamedTempFile::new_in(dir.path()).unwrap();
            tmp1.write_all(&body).unwrap();
            tmp1.persist_if_changed(&path).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), body);

            let metadata1 = std::fs::metadata(&path).unwrap();

            // We should not persist if the contents didn't change.
            let mut tmp2 = NamedTempFile::new_in(dir.path()).unwrap();
            tmp2.write_all(&body).unwrap();
            tmp2.persist_if_changed(&path).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), body);

            // inode shouldn't change if the content doesn't change.
            let metadata2 = std::fs::metadata(&path).unwrap();
            assert_eq!(metadata1.dev(), metadata2.dev());
            assert_eq!(metadata1.ino(), metadata2.ino());

            // We should persist if the contents changed (even though the length
            // did not).
            *body.last_mut().unwrap() = 1;

            let mut tmp3 = NamedTempFile::new_in(dir.path()).unwrap();
            tmp3.write_all(&body).unwrap();
            tmp3.persist_if_changed(&path).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), body);

            let metadata3 = std::fs::metadata(&path).unwrap();
            assert_eq!(metadata1.dev(), metadata3.dev());
            assert_ne!(metadata1.ino(), metadata3.ino());

            // We should persist if the contents changed length.
            body.push(2);

            let mut tmp4 = NamedTempFile::new_in(dir.path()).unwrap();
            tmp4.write_all(&body).unwrap();
            tmp4.persist_if_changed(&path).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), body);

            let metadata4 = std::fs::metadata(&path).unwrap();
            assert_eq!(metadata3.dev(), metadata4.dev());
            assert_ne!(metadata3.ino(), metadata4.ino());
        }
    }

    #[test]
    fn test_files_have_same_content_same_contents() {
        let dir = TempDir::new().unwrap();
        let path1 = dir.path().join("file1");
        let path2 = dir.path().join("file2");

        std::fs::write(&path1, b"hello world").unwrap();
        let mut file1 = File::open(&path1).unwrap();

        std::fs::write(&path2, b"hello world").unwrap();
        let mut file2 = File::open(&path2).unwrap();

        assert!(files_have_same_content(&mut file1, &mut file2).unwrap());
    }

    #[test]
    fn test_files_have_same_content_different_contents() {
        let dir = TempDir::new().unwrap();
        let path1 = dir.path().join("file1");
        let path2 = dir.path().join("file2");

        std::fs::write(&path1, b"hello world").unwrap();
        let mut file1 = File::open(&path1).unwrap();

        std::fs::write(&path2, b"jello world").unwrap();
        let mut file2 = File::open(&path2).unwrap();

        assert!(!files_have_same_content(&mut file1, &mut file2).unwrap());
    }

    #[test]
    fn test_files_have_same_content_truncated() {
        let dir = TempDir::new().unwrap();
        let path1 = dir.path().join("file1");
        let path2 = dir.path().join("file2");

        std::fs::write(&path1, b"hello world").unwrap();
        let mut file1 = File::open(&path1).unwrap();

        std::fs::write(&path2, b"jello").unwrap();
        let mut file2 = File::open(&path2).unwrap();

        assert!(!files_after_checking_length_have_same_content(&mut file1, &mut file2).unwrap());
    }

    #[test]
    fn test_files_have_same_content_too_long() {
        let dir = TempDir::new().unwrap();
        let path1 = dir.path().join("file1");
        let path2 = dir.path().join("file2");

        std::fs::write(&path1, b"hello world").unwrap();
        let mut file1 = File::open(&path1).unwrap();

        std::fs::write(&path2, b"hello world jello").unwrap();
        let mut file2 = File::open(&path2).unwrap();

        assert!(!files_after_checking_length_have_same_content(&mut file1, &mut file2).unwrap());
    }
}
