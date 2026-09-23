// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]

//! `log_listener` listens to messages from `fuchsia.logger.Log` and prints them to stdout and/or
//! writes them to disk.

use anyhow::Error;
use async_trait::async_trait;
use fidl_fuchsia_diagnostics::{LogSettingsMarker, StreamParameters};
use fidl_fuchsia_diagnostics_host::ArchiveAccessorMarker;
use fidl_fuchsia_sys2::RealmQueryMarker;
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_at_path};
use log_command as log_utils;
use log_command::{DefaultLogFormatter, LogEntry, Symbolize, Timestamp, WriterContainer};
use log_utils::{BootTimeAccessor, LogCommand, LogSubCommand};
use std::future::pending;
use std::io::Write;
use writer::{Format, JsonWriter};

/// Target-side symbolizer implementation.
/// Does nothing as no symbols are available on the target.
struct Symbolizer {}

impl Symbolizer {
    fn new() -> Self {
        Self {}
    }
}

#[async_trait(?Send)]
impl Symbolize for Symbolizer {
    async fn symbolize(&self, entry: LogEntry) -> Option<LogEntry> {
        Some(entry)
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let (sender, receiver) = zx::Socket::create_stream();
    let proxy = connect_to_protocol::<ArchiveAccessorMarker>().unwrap();
    let realm_proxy =
        connect_to_protocol_at_path::<RealmQueryMarker>("/svc/fuchsia.sys2.RealmQuery.root")
            .unwrap();
    let log_settings = connect_to_protocol::<LogSettingsMarker>().unwrap();
    let mut cmd: LogCommand = argh::from_env();
    let stream_mode = if matches!(cmd.sub_command, Some(LogSubCommand::Dump(..))) {
        fidl_fuchsia_diagnostics::StreamMode::Snapshot
    } else {
        cmd.since()
            .map(|value| {
                if value.is_now {
                    fidl_fuchsia_diagnostics::StreamMode::Subscribe
                } else {
                    fidl_fuchsia_diagnostics::StreamMode::SnapshotThenSubscribe
                }
            })
            .unwrap_or(fidl_fuchsia_diagnostics::StreamMode::SnapshotThenSubscribe)
    };
    let format = match cmd.encoding() {
        log_utils::LogEncoding::Json => fidl_fuchsia_diagnostics::Format::Json,
        log_utils::LogEncoding::Fxt => fidl_fuchsia_diagnostics::Format::Fxt,
    };
    proxy
        .stream_diagnostics(
            &StreamParameters {
                data_type: Some(fidl_fuchsia_diagnostics::DataType::Logs),
                stream_mode: Some(stream_mode),
                format: Some(format),
                client_selector_configuration: Some(
                    fidl_fuchsia_diagnostics::ClientSelectorConfiguration::SelectAll(true),
                ),
                ..Default::default()
            },
            sender,
        )
        .await?;
    let boot_ts = Timestamp::from_nanos(
        fuchsia_runtime::utc_time().into_nanos() - zx::BootInstant::get().into_nanos(),
    );
    let mut formatter = DefaultLogFormatter::<JsonWriter<LogEntry>>::new_from_args(
        &cmd,
        JsonWriter::new(if cmd.json() { Some(Format::Json) } else { None }),
    )?;
    formatter.expand_monikers(&realm_proxy).await?;
    for warning in cmd.validate_cmd_flags_with_warnings()? {
        writeln!(formatter.writer().stderr(), "{warning}")?;
    }
    cmd.maybe_set_interest(&log_settings, &realm_proxy).await?;
    if let Some(LogSubCommand::SetSeverity(options)) = &cmd.sub_command {
        if options.no_persist {
            // Block forever.
            pending::<()>().await;
        } else {
            // Interest persisted, exit.
            return Ok(());
        }
    }
    formatter.set_boot_timestamp(boot_ts);
    match cmd.encoding() {
        log_utils::LogEncoding::Json => {
            match log_utils::dump_logs_from_socket(
                flex_client::socket_to_async(receiver),
                &mut formatter,
                &Symbolizer::new(),
                true,
            )
            .await
            {
                Err(e) if !e.is_broken_pipe() => {
                    eprintln!("Error in log_listener reading logs from socket, exiting: {e:?}");
                    return Err(e.into());
                }
                _ => {}
            }
        }
        log_utils::LogEncoding::Fxt => {
            match log_utils::dump_fxt_logs_from_socket(
                flex_client::socket_to_async(receiver),
                &mut formatter,
                &Symbolizer::new(),
                true,
            )
            .await
            {
                Err(e) if !e.is_broken_pipe() => {
                    eprintln!("Error in log_listener reading logs from socket, exiting: {e:?}");
                    return Err(e.into());
                }
                _ => {}
            }
        }
    }

    match std::io::stdout().flush() {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => {
            eprintln!("Error in log_listener flushing stdout: {e:?}");
            return Err(e.into());
        }
        _ => {}
    }

    Ok(())
}
