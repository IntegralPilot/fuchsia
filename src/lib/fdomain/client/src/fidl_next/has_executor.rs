// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Channel;
use fidl_next::fuchsia_async::FuchsiaAsync;

impl fidl_next::RunsTransport<crate::Channel> for FuchsiaAsync {}

impl fidl_next::HasExecutor for Channel {
    type Executor = FuchsiaAsync;

    fn executor(&self) -> Self::Executor {
        FuchsiaAsync
    }
}
