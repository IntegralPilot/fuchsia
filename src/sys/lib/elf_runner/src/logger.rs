// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_types::NamespacePath;
use diagnostics_log::{Publisher, PublisherOptions};
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_logger as flogger;
use fuchsia_component::client::connect::connect_to_named_protocol_at_dir_root;
use futures::FutureExt;
use log::warn;
use namespace::Namespace;
use std::future::Future;
use std::sync::LazyLock;

static SVC_DIRECTORY_PATH: LazyLock<NamespacePath> = LazyLock::new(|| "/svc".parse().unwrap());

pub fn create_namespace_logger(
    ns: &Namespace,
) -> Option<impl Future<Output = Option<Publisher>> + Clone + use<>> {
    let svc_dir = ns.get(&SVC_DIRECTORY_PATH)?;
    if let Ok(logsink) =
        connect_to_named_protocol_at_dir_root(svc_dir, flogger::LogSinkMarker::PROTOCOL_NAME)
    {
        Some(
            async move {
                Publisher::new_async(PublisherOptions::default().use_log_sink(logsink)).await.ok()
            }
            .shared(),
        )
    } else {
        warn!("LogSink unavailable");
        None
    }
}

/// Object capable of writing a stream of bytes.
pub trait LogWriter: Send {
    async fn write(&mut self, bytes: &[u8]);
}

pub struct SyslogWriter<L> {
    logger: L,
    level: OutputLevel,
}

#[derive(Copy, Clone)]
pub enum OutputLevel {
    Info,
    Warn,
}

impl From<OutputLevel> for log::Level {
    fn from(level: OutputLevel) -> log::Level {
        match level {
            OutputLevel::Info => log::Level::Info,
            OutputLevel::Warn => log::Level::Warn,
        }
    }
}

impl<L> SyslogWriter<L> {
    pub fn new(logger: L, level: OutputLevel) -> Self {
        Self { logger, level }
    }
}

impl<L: log::Log> LogWriter for SyslogWriter<L> {
    async fn write(&mut self, bytes: &[u8]) {
        let msg = String::from_utf8_lossy(&bytes);
        self.logger.log(
            &log::Record::builder().level(self.level.into()).args(format_args!("{msg}")).build(),
        );
    }
}
