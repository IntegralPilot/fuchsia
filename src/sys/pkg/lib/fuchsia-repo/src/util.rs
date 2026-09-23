// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result};
use bytes::{Bytes, BytesMut};
use camino::{Utf8Path, Utf8PathBuf};
use futures::{Stream, TryStreamExt as _, stream};
use std::cmp::min;
use std::fs::{copy, create_dir_all};
use std::io;
use std::path::Path;
use std::task::Poll;
use walkdir::WalkDir;

/// Read files in chunks of this size off the local storage.
// Note: this is internally public to allow repository tests to check they work across chunks.
pub(crate) const CHUNK_SIZE: usize = 8_192;

/// Helper to read to the end of a [Bytes] stream.
pub(crate) async fn read_stream_to_end<S>(mut stream: S, buf: &mut Vec<u8>) -> io::Result<()>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
{
    while let Some(chunk) = stream.try_next().await? {
        buf.extend_from_slice(&chunk);
    }
    Ok(())
}

#[cfg(unix)]
fn path_nlink(path: &Utf8Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).ok().map(|metadata| metadata.nlink())
}

#[cfg(not(unix))]
fn path_nlink(_path: &Utf8Path) -> Option<usize> {
    None
}

/// Read a file up to `len` bytes in batches of [CHUNK_SIZE], and return a stream of [Bytes].
///
/// This will return an error if the file changed size during streaming.
pub(super) fn file_stream(
    expected_len: u64,
    mut file: impl io::Read,
    path: Option<Utf8PathBuf>,
) -> impl Stream<Item = io::Result<Bytes>> {
    let mut buf = BytesMut::new();
    let mut remaining_len = expected_len;

    stream::poll_fn(move |_cx| {
        if remaining_len == 0 {
            return Poll::Ready(None);
        }

        buf.resize(min(CHUNK_SIZE, remaining_len as usize), 0);

        // Read a chunk from the file.
        // FIXME(https://fxbug.dev/42079310): We should figure out why we were occasionally getting
        // zero-sized reads from async IO, even though we knew there were more bytes available in
        // the file. Once that bug is fixed, we should switch back to async IO to avoid stalling
        // the executor.
        let n = match file.read(&mut buf) {
            Ok(n) => n as u64,
            Err(err) => {
                return Poll::Ready(Some(Err(err)));
            }
        };

        // If we read zero bytes, then the file changed size while we were streaming it.
        if n == 0 {
            let msg = if let Some(path) = &path {
                if let Some(nlink) = path_nlink(path) {
                    format!(
                        "file {} truncated: only read {} out of {} bytes: nlink: {}",
                        path,
                        expected_len - remaining_len,
                        expected_len,
                        nlink,
                    )
                } else {
                    format!(
                        "file {} truncated: only read {} out of {} bytes",
                        path,
                        expected_len - remaining_len,
                        expected_len,
                    )
                }
            } else {
                format!(
                    "file truncated: only read {} out of {} bytes",
                    expected_len - remaining_len,
                    expected_len,
                )
            };
            // Clear out the remaining_len so we'll return None next time we're polled.
            remaining_len = 0;
            return Poll::Ready(Some(Err(io::Error::other(msg))));
        }

        // Return the chunk read from the file. The file may have changed size during streaming, so
        // it's possible we could have read more than expected. If so, truncate the result to the
        // limited size.
        let mut chunk = buf.split_to(n as usize).freeze();
        if n > remaining_len {
            chunk = chunk.split_to(remaining_len as usize);
            remaining_len = 0;
        } else {
            remaining_len -= n;
        }

        Poll::Ready(Some(Ok(chunk)))
    })
}

pub fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    let walker = WalkDir::new(from);
    for entry in walker.into_iter() {
        let entry = entry?;
        let to_path = to.join(entry.path().strip_prefix(from)?);
        if entry.metadata()?.is_dir() {
            if to_path.exists() {
                continue;
            } else {
                create_dir_all(&to_path).with_context(|| format!("creating {to_path:?}"))?;
            }
        } else {
            copy(entry.path(), &to_path)
                .with_context(|| format!("copying {:?} to {:?}", entry.path(), to_path))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[fuchsia::test]
    async fn test_file_stream() {
        for size in [0, CHUNK_SIZE - 1, CHUNK_SIZE, CHUNK_SIZE + 1, CHUNK_SIZE * 2 + 1] {
            let expected = (0..u8::MAX).cycle().take(size).collect::<Vec<_>>();
            let stream = file_stream(size as u64, &*expected, None);

            let mut actual = vec![];
            read_stream_to_end(stream, &mut actual).await.unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[fuchsia::test]
    async fn test_file_stream_chunks() {
        let size = CHUNK_SIZE * 3 + 10;

        let expected = (0..u8::MAX).cycle().take(size).collect::<Vec<_>>();
        let mut stream = file_stream(size as u64, &*expected, None);

        let mut expected_chunks = expected.chunks(CHUNK_SIZE).map(Bytes::copy_from_slice);

        assert_eq!(stream.try_next().await.unwrap(), expected_chunks.next());
        assert_eq!(stream.try_next().await.unwrap(), expected_chunks.next());
        assert_eq!(stream.try_next().await.unwrap(), expected_chunks.next());
        assert_eq!(stream.try_next().await.unwrap(), expected_chunks.next());
        assert_eq!(stream.try_next().await.unwrap(), None);
        assert_eq!(expected_chunks.next(), None);
    }

    #[fuchsia::test]
    async fn test_file_stream_file_truncated() {
        let len = CHUNK_SIZE * 2;
        let long_len = CHUNK_SIZE * 3;

        let truncated_buf = vec![0; len];
        let stream = file_stream(long_len as u64, truncated_buf.as_slice(), None);

        let mut actual = vec![];
        assert_eq!(
            read_stream_to_end(stream, &mut actual).await.unwrap_err().to_string(),
            format!("file truncated: only read {len} out of {long_len} bytes")
        );
    }

    #[fuchsia::test]
    async fn test_file_stream_file_extended() {
        let len = CHUNK_SIZE * 3;
        let short_len = CHUNK_SIZE * 2;

        let buf = (0..u8::MAX).cycle().take(len).collect::<Vec<_>>();
        let stream = file_stream(short_len as u64, buf.as_slice(), None);

        let mut actual = vec![];
        read_stream_to_end(stream, &mut actual).await.unwrap();
        assert_eq!(actual, &buf[..short_len]);
    }

    proptest! {
        #[test]
        fn test_file_stream_proptest(len in 0usize..CHUNK_SIZE * 100) {
            let mut executor = fuchsia_async::TestExecutor::new();
            let () = executor.run_singlethreaded(async move {
                let expected = (0..u8::MAX).cycle().take(len).collect::<Vec<_>>();
                let stream = file_stream(expected.len() as u64, expected.as_slice(), None);

                let mut actual = vec![];
                read_stream_to_end(stream, &mut actual).await.unwrap();

                assert_eq!(expected, actual);
            });
        }
    }
}
