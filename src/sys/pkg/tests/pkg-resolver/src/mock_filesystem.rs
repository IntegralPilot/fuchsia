// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::RequestStream;
use fuchsia_sync::Mutex;
use futures::future::{BoxFuture, FutureExt};
use futures::stream::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use vfs::{ObjectRequest, ObjectRequestRef};
use zx::Status;
use {fidl_fuchsia_io as fio, fuchsia_async as fasync};

type OpenCounter = Arc<Mutex<HashMap<String, u64>>>;

fn handle_directory_request_stream(
    mut stream: fio::DirectoryRequestStream,
    open_counts: OpenCounter,
) -> BoxFuture<'static, ()> {
    async move {
        while let Some(req) = stream.next().await {
            handle_directory_request(req.unwrap(), Arc::clone(&open_counts)).await;
        }
    }
    .boxed()
}

async fn handle_directory_request(req: fio::DirectoryRequest, open_counts: OpenCounter) {
    match req {
        fio::DirectoryRequest::Open {
            path,
            flags,
            options,
            object,
            control_handle: _control_handle,
        } => {
            ObjectRequest::new(flags, &options, object).handle(|request| {
                if path == "." {
                    reopen_self(request, flags, Arc::clone(&open_counts))?;
                }
                *open_counts.lock().entry(path).or_insert(0) += 1;
                Ok(())
            });
        }
        request => panic!("Unhandled fuchsia.io/Directory request: {request:?}"),
    }
}

fn reopen_self(
    object_request: ObjectRequestRef<'_>,
    flags: fio::Flags,
    open_counts: OpenCounter,
) -> Result<(), Status> {
    let stream = fio::NodeRequestStream::from_channel(fasync::Channel::from_channel(
        object_request.take().into_channel(),
    ));
    send_directory_representation(flags, &stream)?;
    fasync::Task::spawn(handle_directory_request_stream(
        stream.cast_stream(),
        Arc::clone(&open_counts),
    ))
    .detach();
    Ok(())
}

pub fn send_directory_representation(
    flags: fio::Flags,
    stream: &fio::NodeRequestStream,
) -> Result<(), Status> {
    if flags.intersects(fio::Flags::FLAG_SEND_REPRESENTATION) {
        let control_handle = stream.control_handle();
        let representation = fio::Representation::Directory(fio::DirectoryInfo::default());
        control_handle.send_on_representation(representation).map_err(|_| Status::PEER_CLOSED)?;
    }

    Ok(())
}

pub fn spawn_directory_handler() -> (fio::DirectoryProxy, OpenCounter) {
    let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fio::DirectoryMarker>();
    let open_counts = Arc::new(Mutex::new(HashMap::<String, u64>::new()));
    fasync::Task::spawn(handle_directory_request_stream(stream, Arc::clone(&open_counts))).detach();
    (proxy, open_counts)
}
