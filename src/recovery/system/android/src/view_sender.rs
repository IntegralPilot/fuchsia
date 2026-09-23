// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use carnelian::{make_message, AppSender, MessageTarget, ViewKey};

/// AppSender wrapper that always send messages to the view.
#[derive(Clone)]
pub struct ViewSender {
    app_sender: AppSender,
    view_key: ViewKey,
}

impl ViewSender {
    pub fn new(app_sender: AppSender, view_key: ViewKey) -> Self {
        Self { app_sender, view_key }
    }

    /// Send a message to the view.
    pub fn queue_message(&self, message: impl std::any::Any) {
        self.app_sender.queue_message(MessageTarget::View(self.view_key), make_message(message));
    }

    /// Request that a frame be rendered at the next appropriate time.
    pub fn request_render(&self) {
        self.app_sender.request_render(self.view_key);
    }
}
