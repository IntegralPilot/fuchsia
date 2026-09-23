// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::display_controller::{DisplayController, DisplayError, Request};
use crate::types::{DisplayInfo, LowLightMode, SetDisplayInfo, Theme, ThemeMode, ThemeType};
use anyhow::{Error, anyhow};
use async_utils::hanging_get::server;
use fidl_fuchsia_settings::{
    DisplayRequest, DisplayRequestStream, DisplaySettings, DisplayWatchResponder,
    Error as SettingsError, LowLightMode as FidlLowLightMode, Theme as FidlTheme,
    ThemeMode as FidlThemeMode, ThemeType as FidlThemeType,
};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::channel::oneshot;
use settings_common::inspect::event::{
    HangingGetObserver, RequestType, ResponseType, UsagePublisher,
};

impl From<FidlThemeMode> for ThemeMode {
    fn from(fidl: FidlThemeMode) -> Self {
        ThemeMode::from_bits(FidlThemeMode::bits(&fidl))
            .expect("failed to convert FidlThemeMode to ThemeMode")
    }
}

impl From<ThemeMode> for FidlThemeMode {
    fn from(fidl: ThemeMode) -> Self {
        FidlThemeMode::from_bits(ThemeMode::bits(&fidl))
            .expect("failed to convert ThemeMode to FidlThemeMode")
    }
}

impl From<FidlLowLightMode> for LowLightMode {
    fn from(fidl_low_light_mode: FidlLowLightMode) -> Self {
        match fidl_low_light_mode {
            FidlLowLightMode::Disable => LowLightMode::Disable,
            FidlLowLightMode::DisableImmediately => LowLightMode::DisableImmediately,
            FidlLowLightMode::Enable => LowLightMode::Enable,
        }
    }
}

impl From<FidlThemeType> for ThemeType {
    fn from(fidl_theme_type: FidlThemeType) -> Self {
        match fidl_theme_type {
            FidlThemeType::Default => ThemeType::Default,
            FidlThemeType::Light => ThemeType::Light,
            FidlThemeType::Dark => ThemeType::Dark,
        }
    }
}

impl From<FidlTheme> for Theme {
    fn from(fidl_theme: FidlTheme) -> Self {
        Self {
            theme_type: fidl_theme.theme_type.map(Into::into),
            theme_mode: fidl_theme.theme_mode.map(Into::into).unwrap_or_else(ThemeMode::empty),
        }
    }
}

impl From<DisplayInfo> for DisplaySettings {
    fn from(info: DisplayInfo) -> Self {
        fidl_fuchsia_settings::DisplaySettings {
            auto_brightness: Some(info.auto_brightness),
            adjusted_auto_brightness: Some(info.auto_brightness_value),
            brightness_value: Some(info.manual_brightness_value),
            screen_enabled: Some(info.screen_enabled),
            low_light_mode: Some(match info.low_light_mode {
                LowLightMode::Enable => FidlLowLightMode::Enable,
                LowLightMode::Disable => FidlLowLightMode::Disable,
                LowLightMode::DisableImmediately => FidlLowLightMode::DisableImmediately,
            }),
            theme: Some(FidlTheme {
                theme_type: match info.theme {
                    Some(Theme { theme_type: Some(theme_type), .. }) => match theme_type {
                        ThemeType::Unknown => None,
                        ThemeType::Default => Some(FidlThemeType::Default),
                        ThemeType::Light => Some(FidlThemeType::Light),
                        ThemeType::Dark => Some(FidlThemeType::Dark),
                    },
                    _ => None,
                },
                theme_mode: match info.theme {
                    Some(Theme { theme_mode, .. }) if !theme_mode.is_empty() => {
                        Some(FidlThemeMode::from(theme_mode))
                    }
                    _ => None,
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

fn to_request(settings: DisplaySettings) -> Result<SetDisplayInfo, Error> {
    let set_display_info = SetDisplayInfo {
        manual_brightness_value: settings.brightness_value,
        auto_brightness_value: settings.adjusted_auto_brightness,
        auto_brightness: settings.auto_brightness,
        screen_enabled: settings.screen_enabled,
        low_light_mode: settings.low_light_mode.map(Into::into),
        theme: settings.theme.map(Into::into),
    };
    match set_display_info {
        // No values being set is invalid
        SetDisplayInfo {
            manual_brightness_value: None,
            auto_brightness_value: None,
            auto_brightness: None,
            screen_enabled: None,
            low_light_mode: None,
            theme: None,
        } => Err(anyhow!("No values set")),
        _ => Ok(set_display_info),
    }
}

pub(crate) type SubscriberObject = HangingGetObserver<DisplayInfo, DisplayWatchResponder>;
type HangingGetFn = fn(&DisplayInfo, SubscriberObject) -> bool;
pub(crate) type HangingGet = server::HangingGet<DisplayInfo, SubscriberObject, HangingGetFn>;
pub(crate) type Publisher = server::Publisher<DisplayInfo, SubscriberObject, HangingGetFn>;
pub(crate) type Subscriber = server::Subscriber<DisplayInfo, SubscriberObject, HangingGetFn>;

pub struct DisplayFidlHandler {
    hanging_get: HangingGet,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<DisplayInfo>,
}

impl DisplayFidlHandler {
    pub(crate) fn new<T>(
        display_controller: &mut DisplayController<T>,
        usage_publisher: UsagePublisher<DisplayInfo>,
        initial_value: DisplayInfo,
    ) -> (Self, UnboundedReceiver<Request>) {
        let hanging_get = HangingGet::new(initial_value, Self::hanging_get);
        display_controller.register_publisher(hanging_get.new_publisher());
        let (controller_tx, controller_rx) = mpsc::unbounded();
        (Self { hanging_get, controller_tx, usage_publisher }, controller_rx)
    }

    fn hanging_get(info: &DisplayInfo, observer: SubscriberObject) -> bool {
        let (usage_responder, responder) = observer.into_parts();
        usage_responder.respond(format!("{info:?}"), ResponseType::OkSome);
        if let Err(e) = responder.send(&DisplaySettings::from(*info)) {
            log::warn!("Failed to respond to watch request: {e:?}");
            return false;
        }
        true
    }

    pub fn handle_stream(&mut self, mut stream: DisplayRequestStream) {
        let request_handler = RequestHandler {
            subscriber: self.hanging_get.new_subscriber(),
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
    InvalidArgument(
        // Error used by Debug impl for inspect logs.
        #[allow(dead_code)] Error,
    ),
    ControllerStopped,
    Controller(DisplayError),
}

impl From<&HandlerError> for ResponseType {
    fn from(error: &HandlerError) -> Self {
        match error {
            HandlerError::AlreadySubscribed => ResponseType::AlreadySubscribed,
            HandlerError::InvalidArgument(_) => ResponseType::InvalidArgument,
            HandlerError::ControllerStopped => ResponseType::UnexpectedError,
            HandlerError::Controller(e) => ResponseType::from(e),
        }
    }
}

struct RequestHandler {
    subscriber: Subscriber,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<DisplayInfo>,
}

impl RequestHandler {
    async fn handle_request(&self, request: DisplayRequest) {
        match request {
            DisplayRequest::Watch { responder } => {
                let usage_res = self.usage_publisher.request("Watch".to_string(), RequestType::Get);
                let observer = HangingGetObserver::new(usage_res, responder);
                if let Err(observer) = self.subscriber.register2(observer) {
                    let e = HandlerError::AlreadySubscribed;
                    let (usage_res, responder) = observer.into_parts();
                    usage_res.respond(format!("Err({e:?})"), ResponseType::from(&e));
                    drop(responder);
                }
            }
            DisplayRequest::Set { settings, responder } => {
                let usage_res = self
                    .usage_publisher
                    .request(format!("Set{{settings:{settings:?}}}"), RequestType::Set);
                if let Err(e) = self.set(settings).await {
                    usage_res.respond(format!("Err({e:?}"), ResponseType::from(&e));
                    let _ = responder.send(Err(SettingsError::Failed));
                } else {
                    usage_res.respond("Ok(())".to_string(), ResponseType::OkNone);
                    let _ = responder.send(Ok(()));
                }
            }
        }
    }

    async fn set(&self, settings: DisplaySettings) -> Result<(), HandlerError> {
        let (set_tx, set_rx) = oneshot::channel();
        let info = to_request(settings).map_err(HandlerError::InvalidArgument)?;
        self.controller_tx
            .unbounded_send(Request::Set(info, set_tx))
            .map_err(|_| HandlerError::ControllerStopped)?;
        set_rx
            .await
            .map_err(|_| HandlerError::ControllerStopped)
            .and_then(|res| res.map_err(HandlerError::Controller))
    }
}
