// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use discovery::emulator_watcher::EmulatorWatcher;
use discovery::fastboot_file_watcher::FastbootWatcher;
use discovery::gce_watcher::GceWatcher;
use discovery::query::TargetInfoQuery;
use discovery::{
    DiscoveryBuilder, DiscoverySources, TargetEvent, TargetHandle, TargetStream, TargetStreamConfig,
};
use ffx_config::EnvironmentContext;
use ffx_diagnostics::NotificationType;
use fho::{FfxContext, Result};
use fidl_fuchsia_developer_ffx as ffx;
use futures::channel::mpsc::{self, UnboundedSender};
use manual_targets::watcher::ManualTargetEvent;
use std::future::Future;
use std::path::PathBuf;
use usb_fastboot_discovery::FastbootEvent;

pub struct NotifierMessage {
    pub ty: NotificationType,
    pub msg: String,
}

/// A trait for resolving targets for diagnostics. Intends to be used where the caller requests
/// a stream to be constructed with a specific notifier, and the caller then joins on the stream
/// and the incoming information sent by the stream discovery methods.
pub trait DiagnosticsResolver {
    /// Creates a new resolver with the given discovery sources and notifier sender.
    fn from_sources_and_notifier_sender(
        sources: DiscoverySources,
        notifier_sender: UnboundedSender<NotifierMessage>,
    ) -> Self;

    /// Converts the resolver into Vec<TargetHandle> of discovered devices.
    fn discovered_targets(
        self,
        query: TargetInfoQuery,
        ctx: &EnvironmentContext,
    ) -> impl Future<Output = Result<Vec<TargetHandle>>>;
}

pub struct SingleTargetResolver {
    sources: DiscoverySources,
    notifier_sender: UnboundedSender<NotifierMessage>,
}

impl DiagnosticsResolver for SingleTargetResolver {
    fn from_sources_and_notifier_sender(
        sources: DiscoverySources,
        notifier_sender: UnboundedSender<NotifierMessage>,
    ) -> Self {
        Self { sources, notifier_sender }
    }

    async fn discovered_targets(
        self,
        query: TargetInfoQuery,
        ctx: &EnvironmentContext,
    ) -> Result<Vec<TargetHandle>> {
        let stream = build_discovery_stream(&ctx, self.sources, self.notifier_sender.clone())?;
        let discoverer = DiscoveryBuilder::default().build_with_stream(ctx, stream);
        discoverer
            .discover_devices(query.clone())
            .await
            .with_user_message(|| format!("failed to discovery devices for query: {query:?}"))
    }
}

trait NotifierSenderExt {
    fn info(&self, msg: impl Into<String>);
    // When used there will be more info here.
}

impl NotifierSenderExt for UnboundedSender<NotifierMessage> {
    fn info(&self, msg: impl Into<String>) {
        let _ =
            self.unbounded_send(NotifierMessage { ty: NotificationType::Info, msg: msg.into() });
    }
}

pub(crate) fn build_discovery_stream(
    ctx: &EnvironmentContext,
    sources: DiscoverySources,
    notifier_sender: UnboundedSender<NotifierMessage>,
) -> Result<TargetStream> {
    let emu_instance_root: PathBuf =
        ctx.get(ffx_config::keys::EMU_INSTANCE_ROOT_DIR).with_user_message(|| {
            format!("unable to get `{}`", ffx_config::keys::EMU_INSTANCE_ROOT_DIR)
        })?;
    let fastboot_file_path: Option<PathBuf> = ctx.get(ffx_config::keys::FASTBOOT_FILE_PATH).ok();
    let gce_instance_root: Option<PathBuf> = ctx.get(ffx_config::keys::GCE_INSTANCE_ROOT_DIR).ok();

    let mut config = TargetStreamConfig::new();
    let (sender, queue) = mpsc::unbounded();

    if sources.contains(DiscoverySources::MDNS) {
        let mdns_sender = sender.clone();
        let ns_clone = notifier_sender.clone();
        config.set_mdns_event_handler(move |res: ffx::MdnsEventType| {
            ns_clone.info(format!(
                "Got MDNS event: {}",
                ffx_diagnostics_formatting::format_mdns_event(&res)
            ));
            let event = TargetEvent::try_from(res).ok();
            if let Some(event) = event {
                let _ = mdns_sender.unbounded_send(event);
            }
        })
    }

    if sources.contains(DiscoverySources::USB_FASTBOOT) {
        let fastboot_sender = sender.clone();
        let ns_clone = notifier_sender.clone();
        config.set_fastboot_event_handler(move |res: FastbootEvent| {
            ns_clone.info(format!("Got Fastboot event: {res:?}"));
            let event = res.into();
            let _ = fastboot_sender.unbounded_send(event);
        })
    }

    if sources.contains(DiscoverySources::MANUAL) {
        let manual_targets_sender = sender.clone();
        config.set_manual_event_handler(move |res: ManualTargetEvent| {
            notifier_sender.info(format!("Got manual event: {res:?}"));
            let event = res.into();
            let _ = manual_targets_sender.unbounded_send(event);
        })
    }

    if sources.contains(DiscoverySources::EMULATOR) {
        config.set_emulator_watcher(EmulatorWatcher::new(emu_instance_root, sender.clone()).bug()?)
    }

    if sources.contains(DiscoverySources::FASTBOOT_FILE) {
        if let Some(fastboot_devices_file) = fastboot_file_path {
            config.set_fastboot_file_watcher(
                FastbootWatcher::new(fastboot_devices_file, sender.clone()).bug()?,
            )
        }
    }

    if sources.contains(DiscoverySources::GCE) {
        if let Some(gce_root) = gce_instance_root {
            config.set_gce_watcher(GceWatcher::new(gce_root, sender).bug()?)
        }
    }

    Ok(TargetStream::new(ctx, config, queue))
}
