// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use std::fmt::Display;

pub trait SendResultExt<T>: Sized {
    fn format_send_err(self) -> Result<T, anyhow::Error>;

    fn format_send_err_with_context<C>(self, context: C) -> Result<T, anyhow::Error>
    where
        C: Display;
}

impl<T> SendResultExt<T> for Result<T, fidl::Error> {
    fn format_send_err(self) -> Result<T, anyhow::Error> {
        self.map_err(|error| format_err!("Failed to respond: {}", error))
    }

    fn format_send_err_with_context<C>(self, context: C) -> Result<T, anyhow::Error>
    where
        C: Display,
    {
        self.map_err(|error| format_err!("Failed to respond ({}): {}", context, error))
    }
}
