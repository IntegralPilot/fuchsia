// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::artifacts;
use crate::cancel::NamedFutureExt;
use crate::diagnostics::{self, LogCollectionOutcome};
use crate::outcome::{ConnectionError, RunTestSuiteError, UnexpectedEventError};
use crate::output::{
    ArtifactType, DirectoryArtifactType, DynDirectoryArtifact, DynReporter, EntityReporter,
};
use crate::stream_util::StreamUtil;
use anyhow::{Context as _, anyhow};
use flex_client::Peered;
use futures::AsyncReadExt;
use futures::future::{BoxFuture, FutureExt, TryFutureExt, join_all};
use futures::stream::{FuturesUnordered, StreamExt, TryStreamExt};
use log::{debug, warn};
use std::borrow::Borrow;
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;

#[cfg(feature = "fdomain")]
use test_diagnostics_fdomain as test_diagnostics;

use flex_fuchsia_io as fio;
use flex_fuchsia_test_manager as ftest_manager;
use fuchsia_async as fasync;
use test_diagnostics::zstd_compress::Decoder;

/// Given an |artifact| reported over fuchsia.test.manager, create the appropriate artifact in the
/// reporter. Returns a Future, which when polled to completion, drains the results from |artifact|
/// and saves them to the reporter.
///
/// This method is an async method returning a Future so that the lifetime of |reporter| is not
/// tied to the lifetime of the Future.
/// The returned Future resolves to LogCollectionOutcome when logs are processed.
pub(crate) async fn drain_artifact<'a, E, T>(
    reporter: &'a EntityReporter<E, T>,
    artifact: ftest_manager::Artifact,
    log_opts: diagnostics::LogCollectionOptions,
) -> Result<
    BoxFuture<'static, Result<Option<LogCollectionOutcome>, anyhow::Error>>,
    RunTestSuiteError,
>
where
    T: Borrow<DynReporter>,
{
    match artifact {
        ftest_manager::Artifact::Stdout(socket) => {
            let stdout = reporter.new_artifact(&ArtifactType::Stdout)?;
            Ok(copy_socket_artifact(socket, stdout).map_ok(|_| None).named("stdout").boxed())
        }
        ftest_manager::Artifact::Stderr(socket) => {
            let stderr = reporter.new_artifact(&ArtifactType::Stderr)?;
            Ok(copy_socket_artifact(socket, stderr).map_ok(|_| None).named("stderr").boxed())
        }
        ftest_manager::Artifact::Log(syslog) => {
            let syslog_artifact = reporter.new_artifact(&ArtifactType::Syslog)?;
            Ok(diagnostics::collect_logs(
                test_diagnostics::LogStream::from_syslog(syslog)
                    .map_err(|e| RunTestSuiteError::Connection(ConnectionError(e.into())))?,
                syslog_artifact,
                log_opts,
            )
            .map_ok(Some)
            .named("syslog")
            .boxed())
        }
        ftest_manager::Artifact::Custom(ftest_manager::CustomArtifact {
            directory_and_token,
            component_moniker,
            ..
        }) => {
            let ftest_manager::DirectoryAndToken { directory, token, .. } = directory_and_token
                .ok_or(UnexpectedEventError::MissingRequiredField {
                    containing_struct: "CustomArtifact",
                    field: "directory_and_token",
                })?;
            let directory_artifact = reporter
                .new_directory_artifact(&DirectoryArtifactType::Custom, component_moniker)?;
            Ok(async move {
                let directory = directory.into_proxy();
                let result =
                    artifacts::copy_custom_artifact_directory(directory, directory_artifact).await;
                // TODO(https://fxbug.dev/42165719): Remove this signal once Overnet
                // supports automatically signalling EVENTPAIR_CLOSED when the
                // handle is closed.
                let _ = token.signal_peer(fidl::Signals::empty(), fidl::Signals::USER_0);
                result
            }
            .map_ok(|()| None)
            .named("custom_artifacts")
            .boxed())
        }
        ftest_manager::Artifact::DebugData(iterator) => {
            let output_directory = reporter
                .new_directory_artifact(&DirectoryArtifactType::Debug, None /* moniker */)?;
            Ok(artifacts::copy_debug_data(iterator.into_proxy(), output_directory)
                .map(|()| Ok(None))
                .named("debug_data")
                .boxed())
        }
        ftest_manager::ArtifactUnknown!() => {
            warn!("Encountered an unknown artifact");
            Ok(futures::future::ready(Ok(None)).boxed())
        }
    }
}

/// Copy an artifact reported over a socket.
async fn copy_socket_artifact<W: Write>(
    socket: flex_client::Socket,
    mut artifact: W,
) -> Result<usize, anyhow::Error> {
    let mut async_socket = flex_client::socket_to_async(socket);
    let mut len = 0;
    loop {
        let done =
            test_diagnostics::SocketReadFut::new(&mut async_socket, |maybe_buf| match maybe_buf {
                Some(buf) => {
                    len += buf.len();
                    artifact.write_all(buf)?;
                    Ok(false)
                }
                None => Ok(true),
            })
            .await?;
        if done {
            artifact.flush()?;
            return Ok(len);
        }
    }
}

/// Copy and decompress (zstd) the artifact reported over a socket.
/// Returns (decompressed, compressed) sizes.
async fn copy_socket_artifact_and_decompress<W: Write>(
    socket: flex_client::Socket,
    mut artifact: W,
) -> Result<(usize, usize), anyhow::Error> {
    let mut async_socket = flex_client::socket_to_async(socket);
    let mut buf = vec![0u8; 1024 * 1024 * 2];

    let (mut decoder, mut receiver) = Decoder::new();
    let task: fasync::Task<Result<usize, anyhow::Error>> = fasync::Task::spawn(async move {
        let mut len = 0;
        loop {
            let l = async_socket.read(&mut buf).await?;
            match l {
                0 => {
                    decoder.finish().await?;
                    break;
                }
                _ => {
                    len += l;
                    decoder.decompress(&buf[..l]).await?;
                }
            }
        }
        Ok(len)
    });

    let mut decompressed_len = 0;
    while let Some(buf) = receiver.next().await {
        decompressed_len += buf.len();
        artifact.write_all(&buf)?;
    }
    artifact.flush()?;

    let compressed_len = task.await?;
    return Ok((decompressed_len, compressed_len));
}

/// Copy debug data reported over a debug data iterator to an output directory.
pub async fn copy_debug_data(
    iterator: ftest_manager::DebugDataIteratorProxy,
    output_directory: Box<DynDirectoryArtifact>,
) {
    let start = std::time::Instant::now();
    const PIPELINED_REQUESTS: usize = 4;
    let unprocessed_data_stream =
        futures::stream::repeat_with(move || iterator.get_next_compressed())
            .buffered(PIPELINED_REQUESTS);
    let terminated_event_stream =
        unprocessed_data_stream.take_until_stop_after(|result| match &result {
            Ok(events) => events.is_empty(),
            _ => true,
        });

    let data_futs = terminated_event_stream
        .map(|result| match result {
            Ok(vals) => vals,
            Err(e) => {
                warn!("Request failure: {:?}", e);
                vec![]
            }
        })
        .map(futures::stream::iter)
        .flatten()
        .map(|debug_data| {
            let output =
                debug_data.name.as_ref().ok_or_else(|| anyhow!("Missing profile name")).and_then(
                    |name| {
                        output_directory.new_file(&PathBuf::from(name)).map_err(anyhow::Error::from)
                    },
                );
            fasync::Task::spawn(async move {
                let _ = &debug_data;
                let mut output = output?;
                let socket =
                    debug_data.socket.ok_or_else(|| anyhow!("Missing profile socket handle"))?;
                debug!("Reading run profile \"{:?}\"", debug_data.name);
                let start = std::time::Instant::now();
                let (decompressed_len, compressed_len) =
                    copy_socket_artifact_and_decompress(socket, &mut output).await.map_err(
                        |e| {
                            warn!("Error copying artifact '{:?}': {:?}", debug_data.name, e);
                            e
                        },
                    )?;

                debug!(
                    "Copied file {:?}: {}({} - compressed) bytes in {:?}",
                    debug_data.name,
                    decompressed_len,
                    compressed_len,
                    start.elapsed()
                );
                Ok::<(), anyhow::Error>(())
            })
        })
        .collect::<Vec<_>>()
        .await;
    join_all(data_futs).await;
    debug!("All profiles downloaded in {:?}", start.elapsed());
}

/// Copy a directory into a directory artifact.
async fn copy_custom_artifact_directory(
    directory: fio::DirectoryProxy,
    out_dir: Box<DynDirectoryArtifact>,
) -> Result<(), anyhow::Error> {
    let mut paths = vec![];
    let mut enumerate = fuchsia_fs::directory::readdir_recursive(&directory, None);
    while let Ok(Some(file)) = enumerate.try_next().await {
        if file.kind == fuchsia_fs::directory::DirentKind::File {
            paths.push(file.name);
        }
    }

    let futs = FuturesUnordered::new();
    paths.iter().for_each(|path| {
        let file =
            fuchsia_fs::directory::open_file_async(&directory, path, fuchsia_fs::PERM_READABLE);
        let output_file = out_dir.new_file(std::path::Path::new(path));
        futs.push(async move {
            let file = file.with_context(|| format!("with path {:?}", path))?;
            let mut output_file = output_file?;

            copy_file_to_writer(&file, &mut output_file).await.map(|_| ())
        });
    });

    futs.for_each(|result| {
        if let Err(e) = result {
            warn!("Custom artifact failure: {}", e);
        }
        async move {}
    })
    .await;

    Ok(())
}

async fn copy_file_to_writer<T: Write>(
    file: &fio::FileProxy,
    output: &mut T,
) -> Result<usize, anyhow::Error> {
    const READ_SIZE: u64 = fio::MAX_BUF;

    let mut vector = VecDeque::new();
    // Arbitrary number of reads to pipeline.
    const PIPELINED_READ_COUNT: u64 = 4;
    for _n in 0..PIPELINED_READ_COUNT {
        vector.push_back(file.read(READ_SIZE));
    }
    let mut len = 0;
    loop {
        let buf = vector.pop_front().unwrap().await?.map_err(zx_status::Status::err_from_raw)?;
        if buf.is_empty() {
            break;
        }
        len += buf.len();
        output.write_all(&buf)?;
        vector.push_back(file.read(READ_SIZE));
    }
    Ok(len)
}

#[cfg(test)]
mod socket_tests {
    use super::*;
    use futures::AsyncWriteExt;

    #[fuchsia::test]
    async fn copy_socket() {
        let cases = vec![vec![], b"0123456789abcde".to_vec(), vec![0u8; 4096]];

        for case in cases.iter() {
            let (client_socket, server_socket) = fidl::Socket::create_stream();
            let mut output = vec![];
            let write_fut = async move {
                let mut async_socket = fidl::AsyncSocket::from_socket(server_socket);
                async_socket.write_all(case.as_slice()).await.expect("write bytes");
            };

            let ((), res) =
                futures::future::join(write_fut, copy_socket_artifact(client_socket, &mut output))
                    .await;
            res.expect("copy contents");
            assert_eq!(output.as_slice(), case.as_slice());
        }
    }
}

// These tests use vfs, which is only available on Fuchsia.
#[cfg(target_os = "fuchsia")]
#[cfg(test)]
mod file_tests {
    use super::*;
    use crate::output::InMemoryDirectoryWriter;
    use flex_fuchsia_io as fio;
    use fuchsia_async as fasync;
    use futures::prelude::*;
    use maplit::hashmap;
    use std::collections::HashMap;
    use std::sync::Arc;
    use vfs::directory::helper::DirectlyMutable;
    use vfs::directory::immutable::Simple;
    use vfs::file::vmo::read_only;
    use vfs::pseudo_directory;

    async fn serve_content_over_socket(content: Vec<u8>, socket: zx::Socket) {
        let mut socket = fidl::AsyncSocket::from_socket(socket);
        socket.write_all(content.as_slice()).await.expect("Cannot serve content over socket");
    }

    async fn serve_and_copy_debug_data(
        expected_files: &HashMap<PathBuf, Vec<u8>>,
        directory_writer: InMemoryDirectoryWriter,
    ) {
        let mut served_files = vec![];
        expected_files.iter().for_each(|(path, content)| {
            let mut compressor = zstd::bulk::Compressor::new(0).unwrap();
            let bytes = compressor.compress(&content).unwrap();
            let (client, server) = zx::Socket::create_stream();
            fasync::Task::spawn(serve_content_over_socket(bytes, server)).detach();
            served_files.push(ftest_manager::DebugData {
                name: Some(path.display().to_string()),
                socket: Some(client.into()),
                ..Default::default()
            });
        });

        let (iterator_proxy, mut iterator_stream) =
            fidl::endpoints::create_proxy_and_stream::<ftest_manager::DebugDataIteratorMarker>();
        let serve_fut = async move {
            let mut files_iter = served_files.into_iter();
            while let Ok(Some(request)) = iterator_stream.try_next().await {
                let responder = match request {
                    ftest_manager::DebugDataIteratorRequest::GetNext { .. } => {
                        panic!("Not Implemented");
                    }
                    ftest_manager::DebugDataIteratorRequest::GetNextCompressed { responder } => {
                        responder
                    }
                };
                let resp: Vec<_> = files_iter.by_ref().take(3).collect();
                let _ = responder.send(resp);
            }
        };
        futures::future::join(
            serve_fut,
            copy_debug_data(iterator_proxy, Box::new(directory_writer)),
        )
        .await;
    }

    fn test_cases() -> Vec<(&'static str, Arc<Simple>, HashMap<PathBuf, Vec<u8>>)> {
        vec![
            ("empty", pseudo_directory! {}, hashmap! {}),
            (
                "single file",
                pseudo_directory! {
                    "test_file.txt" => read_only("Hello, World!"),
                },
                hashmap! {
                    "test_file.txt".to_string().into() => b"Hello, World!".to_vec()
                },
            ),
            (
                "subdir",
                pseudo_directory! {
                    "sub" => pseudo_directory! {
                        "nested.txt" => read_only("Nested file!"),
                    }
                },
                hashmap! {
                    "sub/nested.txt".to_string().into() => b"Nested file!".to_vec()
                },
            ),
            (
                "empty file",
                pseudo_directory! {
                    "empty.txt" => read_only(""),
                },
                hashmap! {
                    "empty.txt".to_string().into() => b"".to_vec()
                },
            ),
            (
                "big file",
                pseudo_directory! {
                    "big.txt" => read_only(vec![b's'; (fio::MAX_BUF as usize)*2]),
                },
                hashmap! {
                    "big.txt".to_string().into() => vec![b's'; (fio::MAX_BUF as usize) *2 as usize]
                },
            ),
            (
                "100 files",
                {
                    let dir = pseudo_directory! {};
                    for i in 0..100 {
                        dir.add_entry(
                            format!("{:?}.txt", i),
                            read_only(format!("contents for {:?}", i)),
                        )
                        .expect("add file");
                    }
                    dir
                },
                (0..100)
                    .map(|i| {
                        (
                            format!("{:?}.txt", i).into(),
                            format!("contents for {:?}", i).into_bytes(),
                        )
                    })
                    .collect(),
            ),
        ]
    }

    #[fuchsia::test]
    async fn test_copy_dir() {
        for (name, fake_dir, expected_files) in test_cases() {
            let directory = vfs::directory::serve(
                fake_dir,
                vfs::execution_scope::ExecutionScope::new(),
                fio::PERM_READABLE | fio::PERM_WRITABLE,
            );
            let artifact = InMemoryDirectoryWriter::default();
            copy_custom_artifact_directory(directory, Box::new(artifact.clone()))
                .await
                .expect("reading custom directory");
            let actual_files: HashMap<_, _> = artifact
                .files
                .lock()
                .iter()
                .map(|(path, artifact)| (path.clone(), artifact.get_contents()))
                .collect();
            assert_eq!(expected_files, actual_files, "{}", name);
        }
    }

    #[fuchsia::test]
    async fn test_copy_debug_data() {
        for (name, _fake_dir, expected_files) in test_cases() {
            let artifact = InMemoryDirectoryWriter::default();
            serve_and_copy_debug_data(&expected_files, artifact.clone()).await;
            let actual_files: HashMap<_, _> = artifact
                .files
                .lock()
                .iter()
                .map(|(path, artifact)| (path.clone(), artifact.get_contents()))
                .collect();
            assert_eq!(expected_files, actual_files, "{}", name);
        }
    }
}
