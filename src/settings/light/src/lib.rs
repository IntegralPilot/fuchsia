// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod light_controller;
pub mod light_fidl_handler;
pub mod light_hardware_configuration;
pub mod types;

pub use light_hardware_configuration::build_light_default_settings;

use self::light_controller::LightError;
use self::light_fidl_handler::LightFidlHandler;
use self::light_hardware_configuration::LightHardwareConfiguration;

use anyhow::{anyhow, Context, Result};
use fuchsia_async as fasync;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::channel::oneshot;
use futures::StreamExt;
use settings_common::config::default_settings::DefaultSetting;
use settings_common::inspect::event::{
    ExternalEventPublisher, RequestType, ResponseType, SettingValuePublisher, UsagePublisher,
};
use settings_common::service_context::ServiceContext;
use settings_storage::fidl_storage::FidlStorage;
use settings_storage::storage_factory::StorageFactory;
use std::rc::Rc;
use types::LightInfo;

pub struct SetupResult {
    pub light_fidl_handler: LightFidlHandler,
    pub media_buttons_event_tx: UnboundedSender<settings_media_buttons::Event>,
    pub task: fuchsia_async::Task<()>,
}

pub async fn setup_light_api<F>(
    service_context: Rc<ServiceContext>,
    light_configuration: &mut DefaultSetting<LightHardwareConfiguration, &'static str>,
    storage_factory: Rc<F>,
    setting_value_publisher: SettingValuePublisher<LightInfo>,
    usage_publisher: UsagePublisher<LightInfo>,
    external_publisher: ExternalEventPublisher,
) -> Result<SetupResult>
where
    F: StorageFactory<Storage = FidlStorage>,
{
    use light_controller::LightController;

    let mut light_controller = LightController::new(
        service_context,
        light_configuration,
        storage_factory,
        setting_value_publisher.clone(),
        external_publisher,
    )
    .await
    .context("failed to construct light controller")?;
    let initial_value = light_controller
        .restore()
        .await
        .map_err(|e| anyhow!("failed to restore initial value: {e:?}"))?;
    let _ = setting_value_publisher.publish(&initial_value);

    let (light_fidl_handler, request_rx) =
        LightFidlHandler::new(&mut light_controller, usage_publisher.clone(), initial_value);
    let (media_buttons_event_tx, media_buttons_event_rx) = mpsc::unbounded();
    let inner_mb_event_rx = event_request_logger(media_buttons_event_rx, usage_publisher);
    let task = light_controller
        .handle(inner_mb_event_rx, request_rx)
        .await
        .map_err(|e| anyhow!("failed to start light controller task: {e:?}"))?;
    Ok(SetupResult { light_fidl_handler, media_buttons_event_tx, task })
}

type ResultSender = oneshot::Sender<Result<Option<()>, LightError>>;

fn event_request_logger(
    mut media_buttons_event_rx: UnboundedReceiver<settings_media_buttons::Event>,
    usage_publisher: UsagePublisher<LightInfo>,
) -> UnboundedReceiver<(settings_media_buttons::Event, ResultSender)> {
    let (inner_mb_event_tx, inner_mb_event_rx) = mpsc::unbounded();
    fasync::Task::local(async move {
        while let Some(event) = media_buttons_event_rx.next().await {
            let usage_responder =
                usage_publisher.request(format!("{event:?}"), RequestType::MediaButtons);
            let (tx, rx) = oneshot::channel::<Result<Option<()>, LightError>>();
            let _ = inner_mb_event_tx.unbounded_send((event, tx));
            if let Ok(res) = rx.await {
                usage_responder.respond(
                    format!("{res:?}"),
                    res.map(|res| {
                        if res.is_some() {
                            ResponseType::OkSome
                        } else {
                            ResponseType::OkNone
                        }
                    })
                    .unwrap_or_else(|e| ResponseType::from(&e)),
                );
            } else {
                usage_responder
                    .respond("Err(ControllerDied)".to_string(), ResponseType::UnexpectedError);
            }
        }
    })
    .detach();
    inner_mb_event_rx
}

#[cfg(test)]
mod test_fakes;
