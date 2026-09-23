// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Contains tests that require next VDSO.

mod logger;

use anyhow::{Context, Error};
use fake_log_sink::FakeLogSink;
use fidl::endpoints::{DiscoverableProtocolMarker, RequestStream, create_endpoints};
use fidl_fuchsia_logger::{LogSinkMarker, LogSinkRequestStream};
use fuchsia_component::server::{ServiceFs, ServiceObjLocal};
use futures::{StreamExt, try_join};
use logger::{LogWriter, OutputLevel, SyslogWriter};
use namespace::Namespace;
use std::sync::Arc;
use {fidl_fuchsia_component_runner as fcrunner, fidl_fuchsia_io as fio};

pub enum MockServiceRequest {
    LogSink(LogSinkRequestStream),
}

pub type MockServiceFs<'a> = ServiceFs<ServiceObjLocal<'a, MockServiceRequest>>;

#[fuchsia::test]
async fn syslog_writer_decodes_valid_utf8_message() -> Result<(), Error> {
    let (dir, ns_entries) = create_fs_with_mock_logsink()?;

    let ((), actual) = try_join!(
        write_to_syslog_or_panic(ns_entries, b"Hello World!"),
        read_message_from_syslog(dir)
    )?;

    assert_eq!(actual, Some("Hello World!".to_owned()));
    Ok(())
}

#[fuchsia::test]
async fn syslog_writer_decodes_non_utf8_message() -> Result<(), Error> {
    let (dir, ns_entries) = create_fs_with_mock_logsink()?;

    let ((), actual) = try_join!(
        write_to_syslog_or_panic(ns_entries, b"Hello \xF0\x90\x80World!"),
        read_message_from_syslog(dir)
    )?;

    assert_eq!(actual, Some("Hello �World!".to_owned()));
    Ok(())
}

async fn write_to_syslog_or_panic(
    ns_entries: Vec<fcrunner::ComponentNamespaceEntry>,
    message: &[u8],
) -> Result<(), Error> {
    let ns = Namespace::try_from(ns_entries).context("Failed to create Namespace")?;
    let logger = logger::create_namespace_logger(&ns)
        .context("Failed to create log Publisher (1)")?
        .await
        .context("Failed to create log Publisher (2)")?;
    let mut writer = SyslogWriter::new(logger, OutputLevel::Info);
    writer.write(message).await;

    Ok(())
}

async fn read_message_from_syslog(
    mut dir: MockServiceFs<'static>,
) -> Result<Option<String>, Error> {
    let mut fake_sink = FakeLogSink::new();

    let MockServiceRequest::LogSink(r) = dir.next().await.unwrap();

    fake_sink
        .serve(Arc::try_unwrap(r.into_inner().0).unwrap().into_channel().into_zx_channel().into());

    Ok(Some(fake_sink.read_message().await))
}

/// Create a new local fs and install a mock LogSink service into.
/// Returns the created directory and corresponding namespace entries.
fn create_fs_with_mock_logsink()
-> Result<(MockServiceFs<'static>, Vec<fcrunner::ComponentNamespaceEntry>), Error> {
    let (dir_client, dir_server) = create_endpoints::<fio::DirectoryMarker>();

    let mut dir = ServiceFs::new_local();
    dir.add_fidl_service_at(LogSinkMarker::PROTOCOL_NAME, MockServiceRequest::LogSink);
    dir.serve_connection(dir_server).context("Failed to add serving channel.")?;

    let namespace = vec![fcrunner::ComponentNamespaceEntry {
        path: Some("/svc".to_string()),
        directory: Some(dir_client),
        ..Default::default()
    }];

    Ok((dir, namespace))
}
