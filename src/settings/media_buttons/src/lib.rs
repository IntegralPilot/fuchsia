// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_ui_input::MediaButtonsEvent;

/// Setting service internal representation of hw media buttons. Used to send
/// OnButton events in the service.
#[derive(PartialEq, Eq, Copy, Clone, Debug)]
pub struct MediaButtons {
    pub mic_mute: Option<bool>,
    pub camera_disable: Option<bool>,
}

impl MediaButtons {
    fn new() -> Self {
        Self { mic_mute: None, camera_disable: None }
    }

    pub fn set_mic_mute(&mut self, mic_mute: Option<bool>) {
        self.mic_mute = mic_mute;
    }

    pub fn set_camera_disable(&mut self, camera_disable: Option<bool>) {
        self.camera_disable = camera_disable;
    }
}

impl From<MediaButtonsEvent> for MediaButtons {
    fn from(event: MediaButtonsEvent) -> Self {
        let mut buttons = MediaButtons::new();

        if let Some(mic_mute) = event.mic_mute {
            buttons.set_mic_mute(Some(mic_mute));
        }
        if let Some(camera_disable) = event.camera_disable {
            buttons.set_camera_disable(Some(camera_disable));
        }

        buttons
    }
}

#[derive(PartialEq, Clone, Debug)]
pub enum Event {
    OnButton(MediaButtons),
}

impl From<MediaButtons> for Event {
    fn from(button_types: MediaButtons) -> Self {
        Self::OnButton(button_types)
    }
}

// Only used for tests
/// Builder to simplify construction of fidl_fuchsia_ui_input::MediaButtonsEvent.
/// # Example usage:
/// ```
/// MediaButtonsEventBuilder::new().set_mic_mute(true).build();
/// ```
pub struct MediaButtonsEventBuilder {
    mic_mute: bool,
    camera_disable: bool,
}

impl MediaButtonsEventBuilder {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        // Create with defaults.
        Self { mic_mute: false, camera_disable: false }
    }

    pub fn build(self) -> MediaButtonsEvent {
        MediaButtonsEvent {
            mic_mute: Some(self.mic_mute),
            pause: Some(false),
            camera_disable: Some(self.camera_disable),
            ..Default::default()
        }
    }

    pub fn set_mic_mute(mut self, mic_mute: bool) -> Self {
        self.mic_mute = mic_mute;
        self
    }

    pub fn set_camera_disable(mut self, camera_disable: bool) -> Self {
        self.camera_disable = camera_disable;
        self
    }
}
