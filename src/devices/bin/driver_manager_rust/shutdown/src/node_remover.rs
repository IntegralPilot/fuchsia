// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;

#[async_trait(?Send)]
pub trait NodeRemover {
    async fn shutdown_all_drivers(&self);
    async fn shutdown_pkg_drivers(&self);
    fn set_on_removal_timeout_callback(&self, callback: Box<dyn Fn()>);
}
