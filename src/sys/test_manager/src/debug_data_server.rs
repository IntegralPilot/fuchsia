// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::run_events::{RunEvent, SuiteEvents};
use anyhow::Error;
use fidl::endpoints::create_request_stream;
use futures::channel::mpsc;
use futures::prelude::*;
use futures::stream::FusedStream;
use futures::{pin_mut, StreamExt, TryStreamExt};
use itertools::Either;
use log::warn;
use test_diagnostics::zstd_compress::{Encoder, Error as CompressionError};
use {fidl_fuchsia_io as fio, fidl_fuchsia_test_manager as ftest_manager, fuchsia_async as fasync};

const DEBUG_DATA_TIMEOUT_SECONDS: i64 = 15;
const EARLY_BOOT_DEBUG_DATA_PATH: &'static str = "/debugdata";

pub(crate) async fn send_kernel_debug_data(
    iterator: ftest_manager::DebugDataIteratorRequestStream,
) -> Result<(), Error> {
    log::info!("Serving kernel debug data");
    let directory = fuchsia_fs::directory::open_in_namespace(
        EARLY_BOOT_DEBUG_DATA_PATH,
        fuchsia_fs::PERM_READABLE,
    )?;

    serve_iterator(EARLY_BOOT_DEBUG_DATA_PATH, directory, iterator).await
}

const ITERATOR_BATCH_SIZE: usize = 10;

async fn filter_map_filename(
    entry_result: Result<
        fuchsia_fs::directory::DirEntry,
        fuchsia_fs::directory::RecursiveEnumerateError,
    >,
    dir_path: &str,
) -> Option<String> {
    match entry_result {
        Ok(fuchsia_fs::directory::DirEntry { name, kind }) => match kind {
            fuchsia_fs::directory::DirentKind::File => Some(name),
            _ => None,
        },
        Err(e) => {
            warn!("Error reading directory in {}: {:?}", dir_path, e);
            None
        }
    }
}

async fn serve_file_over_socket(
    file_name: String,
    file: fio::FileProxy,
    socket: zx::Socket,
    compress: bool,
) {
    let mut socket = fasync::Socket::from_socket(socket);

    // We keep a buffer of 20 MB (2MB buffer * channel size 10) while reading the file
    let num_bytes: u64 = 1024 * 1024 * 2;
    let (mut sender, mut recv) = match compress {
        true => {
            let (encoder, recv) = Encoder::new(0);
            (Either::Left(encoder), recv)
        }
        false => {
            let (sender, recv) = mpsc::channel(10);
            (Either::Right(sender), recv)
        }
    };
    let filename = file_name.clone();
    let _file_read_task = fasync::Task::spawn(async move {
        loop {
            let bytes = fuchsia_fs::file::read_num_bytes(&file, num_bytes).await.unwrap();
            let len = bytes.len();
            match sender {
                Either::Left(ref mut e) => {
                    if let Err(e) = e.compress(&bytes).await {
                        match e {
                            CompressionError::Send(_) => { /* means client is gone, ignore */ }
                            e => {
                                warn!("Error compressing file '{}':, {:?}", &filename, e);
                            }
                        }
                        break;
                    }
                }
                Either::Right(ref mut s) => {
                    if let Err(_) = s.send(bytes).await {
                        // no recv, don't read rest of the file.
                        break;
                    }
                }
            }

            if len != usize::try_from(num_bytes).unwrap() {
                if let Either::Left(e) = sender {
                    // This is EOF as fuchsia.io.Readable does not return short reads
                    if let Err(e) = e.finish().await {
                        match e {
                            CompressionError::Send(_) => { /* means client is gone, ignore */ }
                            e => {
                                warn!("Error compressing file '{}':, {:?}", &filename, e);
                            }
                        }
                    }
                }
                break;
            }
        }
    });

    while let Some(bytes) = recv.next().await {
        if let Err(e) = socket.write_all(bytes.as_slice()).await {
            warn!("cannot serve file '{}': {:?}", &file_name, e);
            return;
        }
    }
}

pub(crate) async fn serve_directory(
    dir_path: &str,
    mut event_sender: mpsc::Sender<RunEvent>,
) -> Result<(), Error> {
    let directory = fuchsia_fs::directory::open_in_namespace(dir_path, fuchsia_fs::PERM_READABLE)?;
    {
        let file_stream = fuchsia_fs::directory::readdir_recursive(
            &directory,
            Some(fasync::MonotonicDuration::from_seconds(DEBUG_DATA_TIMEOUT_SECONDS)),
        )
        .filter_map(|entry| filter_map_filename(entry, dir_path));
        pin_mut!(file_stream);
        if file_stream.next().await.is_none() {
            // No files to serve.
            return Ok(());
        }

        drop(file_stream);
    }

    let (client, iterator) = create_request_stream::<ftest_manager::DebugDataIteratorMarker>();
    let _ = event_sender.send(RunEvent::debug_data(client).into()).await;
    event_sender.disconnect(); // No need to hold this open while we serve the iterator.

    serve_iterator(dir_path, directory, iterator).await
}

pub(crate) async fn serve_directory_for_suite(
    dir_path: &str,
    mut event_sender: mpsc::Sender<Result<SuiteEvents, ftest_manager::LaunchError>>,
) -> Result<(), Error> {
    let directory = fuchsia_fs::directory::open_in_namespace(dir_path, fuchsia_fs::PERM_READABLE)?;
    {
        let file_stream = fuchsia_fs::directory::readdir_recursive(
            &directory,
            Some(fasync::MonotonicDuration::from_seconds(DEBUG_DATA_TIMEOUT_SECONDS)),
        )
        .filter_map(|entry| filter_map_filename(entry, dir_path));
        pin_mut!(file_stream);
        if file_stream.next().await.is_none() {
            // No files to serve.
            return Ok(());
        }

        drop(file_stream);
    }

    let (client, iterator) = create_request_stream::<ftest_manager::DebugDataIteratorMarker>();
    let _ = event_sender.send(Ok(SuiteEvents::debug_data(client).into())).await;
    event_sender.disconnect(); // No need to hold this open while we serve the iterator.

    serve_iterator(dir_path, directory, iterator).await
}

/// Serves the |DebugDataIterator| protocol by serving all the files contained under
/// |dir_path|.
///
/// The contents under |dir_path| are assumed to not change while the iterator is served.
pub(crate) async fn serve_iterator(
    dir_path: &str,
    directory: fio::DirectoryProxy,
    mut iterator: ftest_manager::DebugDataIteratorRequestStream,
) -> Result<(), Error> {
    let file_stream = fuchsia_fs::directory::readdir_recursive(
        &directory,
        Some(fasync::MonotonicDuration::from_seconds(DEBUG_DATA_TIMEOUT_SECONDS)),
    )
    .filter_map(|entry| filter_map_filename(entry, dir_path));
    pin_mut!(file_stream);
    let mut file_stream = file_stream.fuse();

    let mut file_tasks = vec![];
    while let Some(request) = iterator.try_next().await? {
        let mut compress = false;
        let responder = match request {
            ftest_manager::DebugDataIteratorRequest::GetNext { responder } => {
                Either::Left(responder)
            }
            ftest_manager::DebugDataIteratorRequest::GetNextCompressed { responder } => {
                compress = true;
                Either::Right(responder)
            }
        };
        let next_files = match file_stream.is_terminated() {
            true => vec![],
            false => file_stream.by_ref().take(ITERATOR_BATCH_SIZE).collect().await,
        };
        let debug_data = next_files
            .into_iter()
            .map(|file_name| {
                let file = fuchsia_fs::directory::open_file_async(
                    &directory,
                    &file_name,
                    fio::PERM_READABLE,
                )?;
                let (client, server) = zx::Socket::create_stream();
                let t = fasync::Task::spawn(serve_file_over_socket(
                    file_name.clone(),
                    file,
                    server,
                    compress,
                ));
                file_tasks.push(t);
                Ok(ftest_manager::DebugData {
                    socket: Some(client.into()),
                    name: file_name.into(),
                    ..Default::default()
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let _ = match responder {
            Either::Left(responder) => responder.send(debug_data),
            Either::Right(responder) => responder.send(debug_data),
        };
    }

    // make sure all tasks complete
    future::join_all(file_tasks).await;
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::run_events::{RunEventPayload, SuiteEventPayload};
    use ftest_manager::{DebugData, DebugDataIteratorProxy};
    use fuchsia_async as fasync;
    use std::collections::HashSet;
    use tempfile::tempdir;
    use test_case::test_case;
    use test_manager_test_lib::collect_string_from_socket_helper;

    async fn serve_iterator_from_tmp(
        dir: &tempfile::TempDir,
    ) -> (Option<ftest_manager::DebugDataIteratorProxy>, fasync::Task<Result<(), Error>>) {
        let (send, mut recv) = mpsc::channel(0);
        let dir_path = dir.path().to_str().unwrap().to_string();
        let task = fasync::Task::local(async move { serve_directory(&dir_path, send).await });
        let proxy = recv.next().await.map(|event| {
            let RunEventPayload::DebugData(client) = event.into_payload();
            client.into_proxy()
        });
        (proxy, task)
    }

    #[fuchsia::test]
    async fn serve_iterator_empty_dir_returns_no_client() {
        let dir = tempdir().unwrap();
        let (client, task) = serve_iterator_from_tmp(&dir).await;
        assert!(client.is_none());
        task.await.expect("iterator server should not fail");
    }

    async fn get_next_debug_data(
        proxy: &DebugDataIteratorProxy,
        compressed: bool,
    ) -> Vec<DebugData> {
        match compressed {
            true => proxy.get_next_compressed().await.expect("get next compressed"),
            false => proxy.get_next().await.expect("get next"),
        }
    }

    #[test_case(true; "compressed debug_data")]
    #[test_case(false; "uncompressed debug_data")]
    #[fuchsia::test]
    async fn serve_iterator_single_response(compressed: bool) {
        let dir = tempdir().unwrap();
        fuchsia_fs::file::write_in_namespace(&dir.path().join("file").to_string_lossy(), "test")
            .await
            .expect("write to file");

        let (client, task) = serve_iterator_from_tmp(&dir).await;

        let proxy = client.expect("client to be returned");

        let mut values = get_next_debug_data(&proxy, compressed).await;
        assert_eq!(1usize, values.len());
        let ftest_manager::DebugData { name, socket, .. } = values.pop().unwrap();
        assert_eq!(Some("file".to_string()), name);
        let contents = collect_string_from_socket_helper(socket.unwrap(), compressed)
            .await
            .expect("read socket");
        assert_eq!("test", contents);

        let values = get_next_debug_data(&proxy, compressed).await;
        assert_eq!(values, vec![]);

        // Calling again is okay and should also return empty vector.
        let values = get_next_debug_data(&proxy, compressed).await;
        assert_eq!(values, vec![]);

        drop(proxy);
        task.await.expect("iterator server should not fail");
    }

    #[test_case(true; "compressed debug_data")]
    #[test_case(false; "uncompressed debug_data")]
    #[fuchsia::test]
    async fn serve_iterator_multiple_responses(compressed: bool) {
        let num_files_served = ITERATOR_BATCH_SIZE * 2;

        let dir = tempdir().unwrap();
        for idx in 0..num_files_served {
            fuchsia_fs::file::write_in_namespace(
                &dir.path().join(format!("file-{:?}", idx)).to_string_lossy(),
                &format!("test-{:?}", idx),
            )
            .await
            .expect("write to file");
        }

        let (client, task) = serve_iterator_from_tmp(&dir).await;

        let proxy = client.expect("client to be returned");

        let mut all_files = vec![];
        loop {
            let mut next = get_next_debug_data(&proxy, compressed).await;
            if next.is_empty() {
                break;
            }
            all_files.append(&mut next);
        }

        let file_contents: HashSet<_> = futures::stream::iter(all_files)
            .then(|ftest_manager::DebugData { name, socket, .. }| async move {
                let contents = collect_string_from_socket_helper(socket.unwrap(), compressed)
                    .await
                    .expect("read socket");
                (name.unwrap(), contents)
            })
            .collect()
            .await;

        let expected_files: HashSet<_> = (0..num_files_served)
            .map(|idx| (format!("file-{:?}", idx), format!("test-{:?}", idx)))
            .collect();

        assert_eq!(file_contents, expected_files);
        drop(proxy);
        task.await.expect("iterator server should not fail");
    }

    async fn serve_iterator_for_suite_from_tmp(
        dir: &tempfile::TempDir,
    ) -> (Option<ftest_manager::DebugDataIteratorProxy>, fasync::Task<Result<(), Error>>) {
        let (send, mut recv) = mpsc::channel(0);
        let dir_path = dir.path().to_str().unwrap().to_string();
        let task =
            fasync::Task::local(async move { serve_directory_for_suite(&dir_path, send).await });
        let proxy = recv.next().await.map(|event| {
            if let SuiteEventPayload::DebugData(client) = event.unwrap().into_payload() {
                Some(client.into_proxy())
            } else {
                None // Event is not a DebugData
            }
            .unwrap()
        });
        (proxy, task)
    }

    #[fuchsia::test]
    async fn serve_iterator_for_suite_empty_dir_returns_no_client() {
        let dir = tempdir().unwrap();
        let (client, task) = serve_iterator_for_suite_from_tmp(&dir).await;
        assert!(client.is_none());
        task.await.expect("iterator server should not fail");
    }

    #[test_case(true; "compressed debug_data")]
    #[test_case(false; "uncompressed debug_data")]
    #[fuchsia::test]
    async fn serve_iterator_for_suite_single_response(compressed: bool) {
        let dir = tempdir().unwrap();
        fuchsia_fs::file::write_in_namespace(&dir.path().join("file").to_string_lossy(), "test")
            .await
            .expect("write to file");

        let (client, task) = serve_iterator_for_suite_from_tmp(&dir).await;

        let proxy = client.expect("client to be returned");

        let mut values = get_next_debug_data(&proxy, compressed).await;
        assert_eq!(1usize, values.len());
        let ftest_manager::DebugData { name, socket, .. } = values.pop().unwrap();
        assert_eq!(Some("file".to_string()), name);
        let contents = collect_string_from_socket_helper(socket.unwrap(), compressed)
            .await
            .expect("read socket");
        assert_eq!("test", contents);

        let values = get_next_debug_data(&proxy, compressed).await;
        assert_eq!(values, vec![]);

        // Calling again is okay and should also return empty vector.
        let values = get_next_debug_data(&proxy, compressed).await;
        assert_eq!(values, vec![]);

        drop(proxy);
        task.await.expect("iterator server should not fail");
    }

    #[test_case(true; "compressed debug_data")]
    #[test_case(false; "uncompressed debug_data")]
    #[fuchsia::test]
    async fn serve_iterator_for_suite_multiple_responses(compressed: bool) {
        let num_files_served = ITERATOR_BATCH_SIZE * 2;

        let dir = tempdir().unwrap();
        for idx in 0..num_files_served {
            fuchsia_fs::file::write_in_namespace(
                &dir.path().join(format!("file-{:?}", idx)).to_string_lossy(),
                &format!("test-{:?}", idx),
            )
            .await
            .expect("write to file");
        }

        let (client, task) = serve_iterator_from_tmp(&dir).await;

        let proxy = client.expect("client to be returned");

        let mut all_files = vec![];
        loop {
            let mut next = get_next_debug_data(&proxy, compressed).await;
            if next.is_empty() {
                break;
            }
            all_files.append(&mut next);
        }

        let file_contents: HashSet<_> = futures::stream::iter(all_files)
            .then(|ftest_manager::DebugData { name, socket, .. }| async move {
                let contents = collect_string_from_socket_helper(socket.unwrap(), compressed)
                    .await
                    .expect("read socket");
                (name.unwrap(), contents)
            })
            .collect()
            .await;

        let expected_files: HashSet<_> = (0..num_files_served)
            .map(|idx| (format!("file-{:?}", idx), format!("test-{:?}", idx)))
            .collect();

        assert_eq!(file_contents, expected_files);
        drop(proxy);
        task.await.expect("iterator server should not fail");
    }
}
