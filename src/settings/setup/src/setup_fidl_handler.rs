// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::setup_controller::{Request, SetupController, SetupError};
use crate::types::{ConfigurationInterfaceFlags, SetupInfo};
use async_utils::hanging_get::server;
use fidl_fuchsia_settings::{
    Error as SettingsError, SetupRequest, SetupRequestStream, SetupSettings, SetupWatchResponder,
};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::channel::oneshot;
use settings_common::inspect::event::{
    HangingGetObserver, RequestType, ResponseType, UsagePublisher,
};

impl From<fidl_fuchsia_settings::ConfigurationInterfaces> for ConfigurationInterfaceFlags {
    fn from(interfaces: fidl_fuchsia_settings::ConfigurationInterfaces) -> Self {
        let mut flags = ConfigurationInterfaceFlags::empty();

        if interfaces.intersects(fidl_fuchsia_settings::ConfigurationInterfaces::ETHERNET) {
            flags |= ConfigurationInterfaceFlags::ETHERNET;
        }

        if interfaces.intersects(fidl_fuchsia_settings::ConfigurationInterfaces::WIFI) {
            flags |= ConfigurationInterfaceFlags::WIFI;
        }

        flags
    }
}

impl From<ConfigurationInterfaceFlags> for fidl_fuchsia_settings::ConfigurationInterfaces {
    fn from(flags: ConfigurationInterfaceFlags) -> Self {
        let mut interfaces = fidl_fuchsia_settings::ConfigurationInterfaces::empty();

        if flags.intersects(ConfigurationInterfaceFlags::ETHERNET) {
            interfaces |= fidl_fuchsia_settings::ConfigurationInterfaces::ETHERNET;
        }

        if flags.intersects(ConfigurationInterfaceFlags::WIFI) {
            interfaces |= fidl_fuchsia_settings::ConfigurationInterfaces::WIFI;
        }

        interfaces
    }
}

impl From<SetupInfo> for SetupSettings {
    fn from(info: SetupInfo) -> Self {
        let mut settings = SetupSettings::default();
        let interfaces =
            fidl_fuchsia_settings::ConfigurationInterfaces::from(info.configuration_interfaces);

        if !interfaces.is_empty() {
            settings.enabled_configuration_interfaces = Some(interfaces);
        }

        settings
    }
}

pub(super) type SubscriberObject = HangingGetObserver<SetupInfo, SetupWatchResponder>;
type InfoHangingFn = fn(&SetupInfo, SubscriberObject) -> bool;
pub(super) type InfoHangingGet = server::HangingGet<SetupInfo, SubscriberObject, InfoHangingFn>;
pub(super) type InfoPublisher = server::Publisher<SetupInfo, SubscriberObject, InfoHangingFn>;
pub(super) type InfoSubscriber = server::Subscriber<SetupInfo, SubscriberObject, InfoHangingFn>;

pub struct SetupFidlHandler {
    info_hanging_get: InfoHangingGet,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<SetupInfo>,
}

impl SetupFidlHandler {
    pub(super) fn new(
        setup_controller: &mut SetupController,
        usage_publisher: UsagePublisher<SetupInfo>,
        initial_value: SetupInfo,
    ) -> (Self, UnboundedReceiver<Request>) {
        let info_hanging_get = InfoHangingGet::new(initial_value, Self::hanging_get);
        setup_controller.register_publisher(info_hanging_get.new_publisher());
        let (controller_tx, controller_rx) = mpsc::unbounded();
        (Self { info_hanging_get, controller_tx, usage_publisher }, controller_rx)
    }

    fn hanging_get(info: &SetupInfo, observer: SubscriberObject) -> bool {
        let (usage_responder, responder) = observer.into_parts();
        usage_responder.respond(format!("{info:?}"), ResponseType::OkSome);
        if let Err(e) = responder.send(&SetupSettings::from(*info)) {
            log::warn!("Failed to respond to watch request: {e:?}");
            return false;
        }
        true
    }

    pub fn handle_stream(&mut self, mut stream: SetupRequestStream) {
        let request_handler = RequestHandler {
            subscriber: self.info_hanging_get.new_subscriber(),
            controller_tx: self.controller_tx.clone(),
            usage_publisher: self.usage_publisher.clone(),
        };
        fasync::Task::local(async move {
            while let Some(Ok(request)) = stream.next().await {
                request_handler.handle_request(request).await;
            }
        })
        .detach();
    }
}

#[derive(Debug)]
enum HandlerError {
    AlreadySubscribed,
    MissingConfig,
    ControllerStopped,
    Controller(SetupError),
}

impl From<&HandlerError> for ResponseType {
    fn from(error: &HandlerError) -> Self {
        match error {
            HandlerError::AlreadySubscribed => ResponseType::AlreadySubscribed,
            HandlerError::MissingConfig => ResponseType::InvalidArgument,
            HandlerError::ControllerStopped => ResponseType::UnexpectedError,
            HandlerError::Controller(e) => ResponseType::from(e),
        }
    }
}

struct RequestHandler {
    subscriber: InfoSubscriber,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<SetupInfo>,
}

impl RequestHandler {
    async fn handle_request(&self, request: SetupRequest) {
        match request {
            SetupRequest::Watch { responder } => {
                let usage_res = self.usage_publisher.request("Watch".to_string(), RequestType::Get);
                let observer = HangingGetObserver::new(usage_res, responder);
                if let Err(observer) = self.subscriber.register2(observer) {
                    let e = HandlerError::AlreadySubscribed;
                    let (usage_res, responder) = observer.into_parts();
                    usage_res.respond(format!("Err({e:?})"), ResponseType::from(&e));
                    drop(responder);
                }
            }
            SetupRequest::Set { settings, reboot_device, responder } => {
                let usage_res = self.usage_publisher.request(
                    format!("Set{{settings:{settings:?},reboot_device:{reboot_device:?}}}"),
                    RequestType::Set,
                );
                if let Err(e) = self.set(settings, reboot_device).await {
                    usage_res.respond(format!("Err({e:?}"), ResponseType::from(&e));
                    let _ = responder.send(Err(SettingsError::Failed));
                } else {
                    usage_res.respond("Ok(())".to_string(), ResponseType::OkNone);
                    let _ = responder.send(Ok(()));
                }
            }
        }
    }

    async fn set(&self, settings: SetupSettings, reboot_device: bool) -> Result<(), HandlerError> {
        let Some(enabled_config_interfaces) = settings.enabled_configuration_interfaces else {
            return Err(HandlerError::MissingConfig);
        };
        let (set_tx, set_rx) = oneshot::channel();
        self.controller_tx
            .unbounded_send(Request::Set(
                ConfigurationInterfaceFlags::from(enabled_config_interfaces),
                reboot_device,
                set_tx,
            ))
            .map_err(|_| HandlerError::ControllerStopped)?;
        set_rx
            .await
            .map_err(|_| HandlerError::ControllerStopped)
            .and_then(|res| res.map_err(HandlerError::Controller))
    }
}
