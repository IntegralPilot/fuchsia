// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use self::audio_controller::Request;
pub use self::audio_default_settings::{
    AudioInfoLoader, ModifiedCounters, build_audio_default_settings, create_default_audio_stream,
    create_default_modified_counters,
};
pub use self::stream_volume_control::StreamVolumeControl;
pub use self::types::AUDIO_STREAM_TYPE_COUNT;
pub mod audio_controller;
pub mod types;

mod audio_default_settings;
mod audio_fidl_handler;
mod stream_volume_control;

#[cfg(test)]
mod test_fakes;

/// Mod containing utility functions for audio-related functionality.
pub(crate) mod utils;

use self::audio_controller::AudioController;
use self::audio_fidl_handler::AudioFidlHandler;
use futures::channel::mpsc::{self, UnboundedSender};
use settings_common::inspect::event::{
    ExternalEventPublisher, SettingValuePublisher, UsagePublisher,
};
use settings_common::service_context::ServiceContext;
use settings_storage::device_storage::DeviceStorage;
use settings_storage::storage_factory::StorageFactory;
use std::rc::Rc;
use types::AudioInfo;

pub struct SetupResult {
    pub audio_fidl_handler: AudioFidlHandler,
    pub request_tx: UnboundedSender<Request>,
    pub task: fuchsia_async::Task<()>,
}

pub async fn setup_audio_api<F>(
    service_context: Rc<ServiceContext>,
    audio_info_loader: AudioInfoLoader,
    storage_factory: Rc<F>,
    setting_value_publisher: SettingValuePublisher<AudioInfo>,
    usage_publisher: UsagePublisher<AudioInfo>,
    external_publisher: ExternalEventPublisher,
) -> SetupResult
where
    F: StorageFactory<Storage = DeviceStorage>,
{
    let (request_tx, request_rx) = mpsc::unbounded();
    let mut audio_controller = AudioController::new(
        service_context,
        audio_info_loader,
        storage_factory,
        setting_value_publisher.clone(),
        external_publisher,
    )
    .await;
    let initial_value = audio_controller.restore().await;
    let _ = setting_value_publisher.publish(&initial_value);

    let audio_fidl_handler = AudioFidlHandler::new(
        &mut audio_controller,
        usage_publisher,
        request_tx.clone(),
        initial_value,
    );
    let task = audio_controller.handle(request_rx).await;
    SetupResult { audio_fidl_handler, request_tx, task }
}
