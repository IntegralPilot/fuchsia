// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::identity::ComponentIdentity;
use crate::logs::error::LogsError;
use crate::logs::repository::LogsRepository;
use crate::logs::shared_buffer::FilterCursor;
use diagnostics_log_encoding::encode::{Encoder, EncoderOpts, ResizableBuffer};
use diagnostics_log_encoding::{Argument, Header, LOG_CONTROL_BIT, MONIKER, Record, URL};
use fidl::endpoints::{DiscoverableProtocolMarker, RequestStream};
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_diagnostics::StreamMode;
use fidl_fuchsia_diagnostics_types::Severity;
use fuchsia_async as fasync;
use futures::{AsyncWriteExt, StreamExt};
use log::warn;
use std::collections::HashMap;
use std::future::poll_fn;
use std::io::Cursor;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Poll, ready};
use zerocopy::{FromBytes, IntoBytes};

#[derive(thiserror::Error, Debug)]
pub enum StreamError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct LogStreamServer {
    /// The repository holding the logs.
    logs_repo: Arc<LogsRepository>,

    /// Scope in which we spawn all of the server tasks.
    scope: fasync::Scope,
}

impl LogStreamServer {
    pub fn new(logs_repo: Arc<LogsRepository>, scope: fasync::Scope) -> Self {
        Self { logs_repo, scope }
    }

    /// Spawn a task to handle requests from components reading the shared log.
    pub fn spawn(&self, stream: fdiagnostics::LogStreamRequestStream) {
        let logs_repo = Arc::clone(&self.logs_repo);
        let scope = self.scope.to_handle();
        self.scope.spawn(async move {
            if let Err(e) = Self::handle_requests(logs_repo, stream, scope).await {
                warn!("error handling Log requests: {}", e);
            }
        });
    }

    /// Handle requests to `fuchsia.diagnostics.LogStream`. All request types read the
    /// whole backlog from memory, `DumpLogs(Safe)` stops listening after that.
    async fn handle_requests(
        logs_repo: Arc<LogsRepository>,
        mut stream: fdiagnostics::LogStreamRequestStream,
        scope: fasync::ScopeHandle,
    ) -> Result<(), LogsError> {
        while let Some(request) = stream.next().await {
            let request = request.map_err(|source| LogsError::HandlingRequests {
                protocol: fdiagnostics::LogStreamMarker::PROTOCOL_NAME,
                source,
            })?;

            match request {
                fdiagnostics::LogStreamRequest::Connect { socket, opts, .. } => {
                    let logs = logs_repo.logs_cursor_raw(
                        opts.mode.unwrap_or(StreamMode::SnapshotThenSubscribe),
                        Vec::new(),
                    );
                    let opts = ExtendRecordOpts::from(opts);
                    if opts.subscribe_to_manifest {
                        if opts.moniker || opts.component_url || opts.rolled_out {
                            stream.control_handle().shutdown_with_epitaph(zx::Status::INVALID_ARGS);
                            return Ok(());
                        }

                        scope.spawn(async move {
                            let _ = Self::stream_logs_with_manifest(
                                fasync::Socket::from_socket(socket),
                                logs,
                            )
                            .await;
                        });
                    } else {
                        scope.spawn(Self::stream_logs(
                            fasync::Socket::from_socket(socket),
                            logs,
                            opts,
                        ));
                    }
                }
                fdiagnostics::LogStreamRequest::_UnknownMethod {
                    ordinal,
                    method_type,
                    control_handle,
                    ..
                } => {
                    warn!(ordinal, method_type:?; "Unknown request. Closing connection");
                    control_handle.shutdown_with_epitaph(zx::Status::UNAVAILABLE);
                }
            }
        }
        Ok(())
    }

    async fn stream_logs(mut socket: fasync::Socket, logs: FilterCursor, opts: ExtendRecordOpts) {
        let mut logs = pin!(logs);
        let mut buffer = Vec::new();
        while let Some((identity, dropped)) = poll_fn(|cx| {
            loop {
                match ready!(logs.as_mut().poll_next(cx)) {
                    Some(message) => {
                        let (identity_ref, header, data) = match message.parse() {
                            Ok(res) => res,
                            Err(_) => {
                                // If we fail to parse the message, just ignore it and move on
                                // to the next message.
                                continue;
                            }
                        };
                        let identity = Arc::clone(identity_ref);
                        buffer.clear();
                        buffer.reserve(data.len());
                        buffer.extend_from_slice(header.as_bytes());
                        buffer.extend_from_slice(&data[8..]);
                        return Poll::Ready(Some((identity, message.dropped)));
                    }
                    None => return Poll::Ready(None),
                }
            }
        })
        .await
        {
            extend_fxt_record(&identity, dropped, &opts, &mut buffer);
            if socket.write_all(&buffer).await.is_err() {
                // Assume an error means the peer closed for now.
                break;
            }
        }
    }

    pub async fn stream_logs_with_manifest(
        mut socket: fasync::Socket,
        logs: FilterCursor,
    ) -> Result<(), StreamError> {
        let mut logs = pin!(logs);
        let mut sent_tags = HashMap::new();
        let mut buffer = Vec::new();
        while let Some((identity, tag)) = poll_fn(|cx| {
            loop {
                match ready!(logs.as_mut().poll_next(cx)) {
                    Some(message) => {
                        let (identity_ref, header, data) = match message.parse() {
                            Ok(res) => res,
                            Err(_) => continue,
                        };
                        let identity = Arc::clone(identity_ref);
                        buffer.clear();
                        buffer.reserve(data.len());
                        buffer.extend_from_slice(header.as_bytes());
                        buffer.extend_from_slice(&data[8..]);
                        return Poll::Ready(Some((identity, header.tag() as u64)));
                    }
                    None => return Poll::Ready(None),
                }
            }
        })
        .await
        {
            let send = match sent_tags.entry(tag) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(Arc::clone(&identity));
                    true
                }
                std::collections::hash_map::Entry::Occupied(mut e) => {
                    if !Arc::ptr_eq(e.get(), &identity) && **e.get() != *identity {
                        e.insert(Arc::clone(&identity));
                        true
                    } else {
                        false
                    }
                }
            };

            if send {
                Self::send_component_change(&mut socket, tag, &identity).await?;
            }
            socket.write_all(&buffer).await?;
        }
        Ok(())
    }

    async fn send_component_change(
        socket: &mut fasync::Socket,
        id: u64,
        identity: &ComponentIdentity,
    ) -> Result<(), std::io::Error> {
        let mut encoder =
            Encoder::new(Cursor::new(ResizableBuffer::from(Vec::new())), EncoderOpts::default());
        let record = Record {
            timestamp: zx::BootInstant::from_nanos(0),
            severity: Severity::Info.into_primitive(),
            arguments: vec![
                Argument::other(MONIKER, identity.moniker.to_string()),
                Argument::other(URL, identity.url.as_str()),
            ],
        };
        encoder.write_record(record).map_err(std::io::Error::other)?;

        let mut buffer = encoder.take().into_inner().into_inner();
        if buffer.len() >= 8 {
            let mut header = Header::read_from_bytes(&buffer[0..8]).unwrap();
            header.set_tag((id as u32) | LOG_CONTROL_BIT);
            buffer[0..8].copy_from_slice(header.as_bytes());
        }
        socket.write_all(&buffer).await?;
        Ok(())
    }
}

#[derive(Default)]
pub struct ExtendRecordOpts {
    pub moniker: bool,
    pub component_url: bool,
    pub rolled_out: bool,
    pub subscribe_to_manifest: bool,
}

impl ExtendRecordOpts {
    fn should_extend(&self) -> bool {
        let Self { moniker, component_url, rolled_out, subscribe_to_manifest: _ } = self;
        *moniker || *component_url || *rolled_out
    }
}

impl From<fdiagnostics::LogStreamOptions> for ExtendRecordOpts {
    fn from(opts: fdiagnostics::LogStreamOptions) -> Self {
        let fdiagnostics::LogStreamOptions {
            include_moniker,
            include_component_url,
            include_rolled_out,
            mode: _,
            __source_breaking: _,
            subscribe_to_manifest,
        } = opts;
        Self {
            moniker: include_moniker.unwrap_or(false),
            component_url: include_component_url.unwrap_or(false),
            rolled_out: include_rolled_out.unwrap_or(false),
            subscribe_to_manifest: subscribe_to_manifest.unwrap_or(false),
        }
    }
}

/// Returns zero padding for `len`.
fn padding(len: usize) -> &'static [u8] {
    &[0; 8][(len + 7) % 8 + 1..]
}

pub fn extend_fxt_record(
    identity: &ComponentIdentity,
    rolled_out: u64,
    opts: &ExtendRecordOpts,
    buffer: &mut Vec<u8>,
) {
    if !opts.should_extend() {
        return;
    }

    let moniker = if opts.moniker { identity.moniker.as_ref() } else { "" };
    let component_url = if opts.component_url { identity.url.as_ref() } else { "" };
    let rolled_out_value = if opts.rolled_out { rolled_out } else { 0 };

    let moniker_len = moniker.len() as u32;
    let component_url_len = component_url.len() as u32;

    buffer.extend_from_slice(&moniker_len.to_le_bytes());
    buffer.extend_from_slice(&component_url_len.to_le_bytes());
    buffer.extend_from_slice(&rolled_out_value.to_le_bytes());

    buffer.extend_from_slice(moniker.as_bytes());
    buffer.extend_from_slice(padding(moniker.len()));

    buffer.extend_from_slice(component_url.as_bytes());
    buffer.extend_from_slice(padding(component_url.len()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::testing::make_message;
    use diagnostics_log_encoding::Value;
    use diagnostics_log_encoding::parse::parse_record;
    use futures::AsyncReadExt;
    use moniker::ExtendedMoniker;
    use test_case::test_case;
    use zx;

    #[fuchsia::test]
    async fn log_stream_with_manifest() {
        let repo = LogsRepository::for_test(fasync::Scope::new());
        let identity = Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./foo").unwrap(),
            "fuchsia-pkg://foo",
        ));
        let container = repo.get_log_container(Arc::clone(&identity));
        let container_tag = container.buffer().iob_tag() as u32;

        let scope = fasync::Scope::new();
        let server = Arc::new(LogStreamServer::new(Arc::clone(&repo), scope));
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fdiagnostics::LogStreamMarker>();
        server.spawn(stream);

        let (client_socket, server_socket) = zx::Socket::create_stream();
        let mut client_socket = fasync::Socket::from_socket(client_socket);

        let opts = fdiagnostics::LogStreamOptions {
            subscribe_to_manifest: Some(true),
            mode: Some(StreamMode::SnapshotThenSubscribe),
            ..Default::default()
        };
        proxy.connect(server_socket, &opts).expect("connect");

        // Wait for connection to be established/handled?
        // We can just ingest. The cursor should pick it up.
        container.ingest_message(make_message("a", None, zx::BootInstant::from_nanos(1)));

        let mut buf = vec![0u8; 4096];
        let mut offset = 0;

        // 1. Read Manifest
        let (manifest_record, manifest_len) = loop {
            if offset > 0
                && let Ok((record, rest)) = parse_record(&buf[..offset])
            {
                let len = offset - rest.len();
                break (record, len);
            }
            let n = client_socket.read(&mut buf[offset..]).await.expect("read");
            assert!(n > 0, "socket closed before receiving manifest");
            offset += n;
        };

        // Check manifest arguments
        assert_eq!(manifest_record.arguments[0].name(), "moniker");
        assert_eq!(manifest_record.arguments[0].value(), Value::Text("foo".into()));
        assert_eq!(manifest_record.arguments[1].name(), "url");
        assert_eq!(manifest_record.arguments[1].value(), Value::Text("fuchsia-pkg://foo".into()));

        // 2. Read Log Record
        let (log_record, _log_len) = loop {
            if offset > manifest_len
                && let Ok((record, rest)) = parse_record(&buf[manifest_len..offset])
            {
                let len = offset - manifest_len - rest.len();
                break (record, len);
            }
            let n = client_socket.read(&mut buf[offset..]).await.expect("read");
            assert!(n > 0, "socket closed before receiving log record");
            offset += n;
        };

        // Verify header tag of first record (Manifest)
        let header1 = Header::read_from_bytes(&buf[0..8]).unwrap();
        let tag_id = header1.tag();
        assert_ne!(tag_id & LOG_CONTROL_BIT, 0, "Manifest should have LOG_CONTROL_BIT set");

        // Verify header tag of second record (Log)
        let header2 = Header::read_from_bytes(&buf[manifest_len..manifest_len + 8]).unwrap();
        assert_eq!(
            header2.tag() & LOG_CONTROL_BIT,
            0,
            "Log record should NOT have LOG_CONTROL_BIT set"
        );
        assert_eq!(
            header2.tag(),
            tag_id & !LOG_CONTROL_BIT,
            "Log record tag should match Manifest tag ID"
        );
        assert_eq!(header2.tag(), container_tag, "Log record tag ID should equal IOB tag ID");

        assert_eq!(log_record.arguments[2].value(), Value::Text("a".into()));
    }

    #[fuchsia::test]
    async fn log_stream_with_manifest_reused_tag() {
        use crate::logs::shared_buffer::create_ring_buffer;
        // Use a small buffer to facilitate rolling out logs.
        let repo = LogsRepository::new(
            create_ring_buffer(65536),
            std::iter::empty(),
            &Default::default(),
            fasync::Scope::new(),
        );

        let scope = fasync::Scope::new();
        let server = Arc::new(LogStreamServer::new(Arc::clone(&repo), scope));
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fdiagnostics::LogStreamMarker>();
        server.spawn(stream);

        let (client_socket, server_socket) = zx::Socket::create_stream();
        let mut client_socket = fasync::Socket::from_socket(client_socket);

        let opts = fdiagnostics::LogStreamOptions {
            subscribe_to_manifest: Some(true),
            mode: Some(StreamMode::SnapshotThenSubscribe),
            ..Default::default()
        };
        proxy.connect(server_socket, &opts).expect("connect");

        // 1. Setup Identity A
        let identity_a = Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./foo").unwrap(),
            "fuchsia-pkg://foo",
        ));
        let container_a = repo.get_log_container(Arc::clone(&identity_a));
        let tag_a = container_a.buffer().iob_tag();

        // 2. Ingest A
        container_a.ingest_message(make_message("msg_a", None, zx::BootInstant::from_nanos(1)));

        let mut buf = vec![0u8; 65536];
        let mut offset = 0;

        // Helper to read one record from the socket
        async fn read_one_record(
            socket: &mut fasync::Socket,
            buf: &mut [u8],
            offset: &mut usize,
        ) -> (diagnostics_log_encoding::Record<'static>, usize) {
            loop {
                if *offset > 0
                    && let Ok((record, rest)) = parse_record(&buf[..*offset])
                {
                    let len = *offset - rest.len();
                    let owned_record = record.into_owned();
                    // Shift buffer
                    buf.copy_within(len..*offset, 0);
                    *offset -= len;
                    // Return owned record to avoid lifetime issues
                    return (owned_record, len);
                }
                let n = socket.read(&mut buf[*offset..]).await.expect("read");
                assert!(n > 0, "socket closed unexpectedly");
                *offset += n;
            }
        }

        // 3. Read Manifest A and Log A
        let (manifest_a, _) = read_one_record(&mut client_socket, &mut buf, &mut offset).await;
        assert_eq!(manifest_a.arguments[0].value(), Value::Text("foo".into()));

        let (log_a, _) = read_one_record(&mut client_socket, &mut buf, &mut offset).await;
        assert_eq!(log_a.arguments[2].value(), Value::Text("msg_a".into()));

        // 4. Mark A inactive and release
        container_a.mark_stopped();
        drop(container_a);

        // 5. Force rollout A by ingesting filler logs
        let identity_filler = Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./filler").unwrap(),
            "fuchsia-pkg://filler",
        ));
        let container_filler = repo.get_log_container(Arc::clone(&identity_filler));

        let identity_b = Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./bar").unwrap(),
            "fuchsia-pkg://bar",
        ));

        let mut container_b;
        loop {
            // Ingest filler
            container_filler.ingest_message(make_message(
                "fill",
                None,
                zx::BootInstant::from_nanos(1),
            ));

            // Drain socket to ensure flow

            // Read available data
            loop {
                let mut temp_buf = [0u8; 1024];
                // Use poll_read to not block
                let read_fut = client_socket.read(&mut temp_buf);
                match futures::poll!(read_fut) {
                    std::task::Poll::Ready(Ok(n)) if n > 0 => {
                        // We just discard filler data for now, but we need to watch for B?
                        // No, B hasn't been created/ingested yet.
                        // We are just draining filler.
                    }
                    _ => break, // No more data or pending
                }
            }

            // Try to allocate B
            container_b = repo.get_log_container(Arc::clone(&identity_b));
            if container_b.buffer().iob_tag() == tag_a {
                break;
            }

            // Failed to reuse, clean up B
            container_b.mark_stopped();
            drop(container_b);

            // Yield to let cleanup tasks run
            fasync::Timer::new(std::time::Duration::from_millis(10)).await;
        }

        // 6. Ingest B
        container_b.ingest_message(make_message("msg_b", None, zx::BootInstant::from_nanos(2)));

        // 7. Read Manifest B + Log B
        // Note: Our buffer `buf` might contain leftover filler data or partial records.
        // But we discarded filler data in the loop above (into temp_buf).
        // `buf` and `offset` state from `read_one_record` was preserved.
        // Wait, `read_one_record` modifies `buf` and `offset` (shifts data).
        // So `buf` should be clean or contain partial data.

        // We need to read until we find Manifest B.
        loop {
            let (record, _) = read_one_record(&mut client_socket, &mut buf, &mut offset).await;

            // Is it Manifest B?
            if !record.arguments.is_empty()
                && record.arguments[0].name() == "moniker"
                && record.arguments[0].value() == Value::Text("bar".into())
            {
                // Found Manifest B!
                break;
            }
            // Otherwise it's filler log or Manifest Filler
        }

        // Next should be Log B
        let (log_b, _) = read_one_record(&mut client_socket, &mut buf, &mut offset).await;
        assert_eq!(log_b.arguments[2].value(), Value::Text("msg_b".into()));
    }

    #[test_case(ExtendRecordOpts::default(), "", "", 0 ; "no_additional_metadata")]
    #[test_case(
        ExtendRecordOpts { moniker: true, ..Default::default() },
        "UNKNOWN",
        "",
        0
        ; "with_moniker")]
    #[test_case(
        ExtendRecordOpts { component_url: true, ..Default::default() },
        "",
        "fuchsia-pkg://UNKNOWN",
        0
        ; "with_url")]
    #[test_case(
        ExtendRecordOpts { rolled_out: true, ..Default::default() },
        "",
        "",
        42
        ; "with_rolled_out")]
    #[test_case(
        ExtendRecordOpts { moniker: true, component_url: true, rolled_out: true, subscribe_to_manifest: false },
        "UNKNOWN",
        "fuchsia-pkg://UNKNOWN",
        42
        ; "with_all")]
    #[fuchsia::test]
    fn extend_record_with_metadata(
        opts: ExtendRecordOpts,
        expected_moniker: &str,
        expected_url: &str,
        expected_rolled_out: u64,
    ) {
        let mut buffer = Vec::new();
        extend_fxt_record(&ComponentIdentity::unknown(), 42, &opts, &mut buffer);

        if !opts.should_extend() {
            assert!(buffer.is_empty());
            return;
        }

        let moniker_len = u32::from_le_bytes(buffer[0..4].try_into().unwrap()) as usize;
        let component_url_len = u32::from_le_bytes(buffer[4..8].try_into().unwrap()) as usize;

        let rolled_out = u64::from_le_bytes(buffer[8..16].try_into().unwrap());
        if opts.rolled_out {
            assert_eq!(rolled_out, expected_rolled_out);
        } else {
            assert_eq!(rolled_out, 0);
        }

        let mut offset = 16;
        let moniker = std::str::from_utf8(&buffer[offset..offset + moniker_len]).unwrap();
        assert_eq!(moniker, expected_moniker);
        let moniker_padded_len = (moniker_len + 7) & !7;
        offset += moniker_padded_len;

        let url = std::str::from_utf8(&buffer[offset..offset + component_url_len]).unwrap();
        assert_eq!(url, expected_url);
        let component_url_padded_len = (component_url_len + 7) & !7;
        offset += component_url_padded_len;

        assert_eq!(offset, buffer.len());
    }
}
