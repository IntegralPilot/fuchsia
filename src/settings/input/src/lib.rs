// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod input_controller;
pub mod input_device_configuration;
mod input_fidl_handler;
pub mod types;

use self::input_controller::InputController;
pub use self::input_device_configuration::build_input_default_settings;
use self::input_fidl_handler::InputFidlHandler;
use crate::input_device_configuration::InputConfiguration;
use anyhow::{Context, Result};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::channel::oneshot;
use input_controller::InputError;
use settings_common::config::default_settings::DefaultSetting;
use settings_common::inspect::event::{
    ExternalEventPublisher, RequestType, ResponseType, SettingValuePublisher, UsagePublisher,
};
use settings_common::service_context::ServiceContext;
use settings_storage::device_storage::DeviceStorage;
use settings_storage::storage_factory::StorageFactory;
use std::rc::Rc;
use types::InputInfo;

pub struct SetupResult {
    pub input_fidl_handler: InputFidlHandler,
    pub camera_watcher_event_tx: UnboundedSender<bool>,
    pub media_buttons_event_tx: UnboundedSender<settings_media_buttons::Event>,
    pub task: fuchsia_async::Task<()>,
}

pub async fn setup_input_api<F>(
    service_context: Rc<ServiceContext>,
    input_configuration: &mut DefaultSetting<InputConfiguration, &'static str>,
    storage_factory: Rc<F>,
    setting_value_publisher: SettingValuePublisher<InputInfo>,
    usage_publisher: UsagePublisher<InputInfo>,
    external_publisher: ExternalEventPublisher,
) -> Result<SetupResult>
where
    F: StorageFactory<Storage = DeviceStorage>,
{
    let mut input_controller = InputController::new(
        service_context,
        input_configuration,
        storage_factory,
        setting_value_publisher.clone(),
        external_publisher,
    )
    .await
    .context("initializing input controller")?;
    let initial_value = input_controller.restore().await.context("Failed to restore input")?;
    let _ = setting_value_publisher.publish(&initial_value);

    let (input_fidl_handler, request_rx) =
        InputFidlHandler::new(&mut input_controller, usage_publisher.clone(), initial_value);
    let (camera_watcher_event_tx, camera_watcher_event_rx) = mpsc::unbounded();
    let inner_camera_event_rx =
        event_request_logger(camera_watcher_event_rx, usage_publisher.clone(), RequestType::Camera);
    let (media_buttons_event_tx, media_buttons_event_rx) = mpsc::unbounded();
    let inner_media_buttons_event_rx =
        event_request_logger(media_buttons_event_rx, usage_publisher.clone(), RequestType::Camera);
    let task = input_controller
        .handle(inner_camera_event_rx, inner_media_buttons_event_rx, request_rx)
        .await;
    Ok(SetupResult { input_fidl_handler, camera_watcher_event_tx, media_buttons_event_tx, task })
}

type ResultSender = oneshot::Sender<Result<Option<()>, InputError>>;

fn event_request_logger<T>(
    mut event_rx: UnboundedReceiver<T>,
    usage_publisher: UsagePublisher<InputInfo>,
    request_type: RequestType,
) -> UnboundedReceiver<(T, ResultSender)>
where
    T: std::fmt::Debug + 'static,
{
    let (inner_event_tx, inner_event_rx) = mpsc::unbounded();
    fasync::Task::local(async move {
        while let Some(event) = event_rx.next().await {
            let usage_responder = usage_publisher.request(format!("{event:?}"), request_type);
            let (tx, rx) = oneshot::channel::<Result<Option<()>, InputError>>();
            let _ = inner_event_tx.unbounded_send((event, tx));
            if let Ok(res) = rx.await {
                usage_responder.respond(
                    format!("{res:?}"),
                    res.map(|res| {
                        if res.is_some() { ResponseType::OkSome } else { ResponseType::OkNone }
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
    inner_event_rx
}

#[cfg(test)]
mod input_device_limit_tests;
#[cfg(test)]
mod input_test_environment;
#[cfg(test)]
mod input_tests;
