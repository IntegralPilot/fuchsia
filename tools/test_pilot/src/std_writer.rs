// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::TestRunError;
use std::path::PathBuf;
use std::{fs, io};

/// Writer for command invocation stdout and stderr. `StdWriter` writes to a file it creates
/// and to an additional writer. When dropped, it deletes the created file if nothing was
/// written.
pub struct StdWriter {
    path: PathBuf,

    file: Option<fs::File>,

    wrote: bool,

    also: Box<dyn io::Write + Send>,
}

impl StdWriter {
    /// Creates a new `StdWriter`, creating the file specified by `path`.
    pub fn new(path: PathBuf, also: Box<dyn io::Write + Send>) -> Result<Self, TestRunError> {
        fs::create_dir_all(path.parent().expect("stdout/err file path has parent"))
            .map_err(|e| TestRunError::FailedToCreateFile { path: path.clone(), source: e })?;
        let file = fs::File::create(&path)
            .map_err(|e| TestRunError::FailedToCreateFile { path: path.clone(), source: e })?;
        Ok(Self { path, file: Some(file), wrote: false, also })
    }

    /// Returns the path of the created file if a write has occurred, None if not.
    pub fn path_if_wrote(&self) -> Option<PathBuf> {
        if self.wrote {
            Some(self.path.clone())
        } else {
            None
        }
    }
}

impl io::Write for StdWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.wrote = true;
        if let Some(file) = &mut self.file {
            file.write(buf)?;
        }
        self.also.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = &mut self.file {
            file.flush()?;
        }
        self.also.flush()
    }
}

impl Drop for StdWriter {
    fn drop(&mut self) {
        self.file = None;
        if !self.wrote {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc::{sync_channel, TryRecvError};
    use tempfile::tempdir;

    struct FakeWriter {
        sender: std::sync::mpsc::SyncSender<FakeWriterMessage>,
    }

    #[derive(Debug, PartialEq)]
    enum FakeWriterMessage {
        Written(usize),
        Flushed,
    }

    impl io::Write for FakeWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.sender.send(FakeWriterMessage::Written(buf.len())).unwrap();
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.sender.send(FakeWriterMessage::Flushed).unwrap();
            Ok(())
        }
    }

    #[fuchsia::test]
    fn test_std_writer_no_write() {
        let temp_dir = tempdir().expect("to create temporary directory");
        let log_path = temp_dir.path().join("stderr");
        let (sender, receiver) = sync_channel(10);
        let fake_writer = Box::new(FakeWriter { sender });

        let under_test = StdWriter::new(log_path.clone(), fake_writer).expect("new should succeed");
        assert_eq!(true, log_path.exists());
        assert_eq!(true, under_test.path_if_wrote().is_none());
        drop(under_test);
        assert_eq!(false, log_path.exists());
        assert_eq!(Err(TryRecvError::Disconnected), receiver.try_recv());

        temp_dir.close().expect("temp directory successfully closed");
    }

    #[fuchsia::test]
    fn test_std_writer_write() {
        let temp_dir = tempdir().expect("to create temporary directory");
        let log_path = temp_dir.path().join("stderr");
        let (sender, receiver) = sync_channel(10);
        let fake_writer = Box::new(FakeWriter { sender });

        let mut under_test =
            StdWriter::new(log_path.clone(), fake_writer).expect("new should succeed");
        assert_eq!(true, log_path.exists());

        let _ = under_test.write(b"ostrich").expect("write succeeds");
        under_test.flush().expect("flush succeeds");
        assert_eq!(log_path, under_test.path_if_wrote().expect("path_if_wrote returns something"));

        drop(under_test);
        assert_eq!(true, log_path.exists());
        assert_eq!(
            FakeWriterMessage::Written(b"ostrich".len()),
            receiver.try_recv().expect("recv succeeds")
        );
        assert_eq!(FakeWriterMessage::Flushed, receiver.try_recv().expect("recv succeeds"));
        assert_eq!(Err(TryRecvError::Disconnected), receiver.try_recv());

        assert_eq!(b"ostrich".to_vec(), fs::read(log_path).expect("read from log succeeds"));

        temp_dir.close().expect("temp directory successfully closed");
    }
}
