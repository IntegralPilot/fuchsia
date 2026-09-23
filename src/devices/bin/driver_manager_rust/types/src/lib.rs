// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_component_runner as frunner;
use futures::channel::mpsc;

mod bind_result;
mod bind_result_tracker;
mod conversions;
mod node_types;

pub use bind_result::*;
pub use bind_result_tracker::*;
pub use conversions::*;
pub use node_types::*;

pub struct StartedComponent {
    pub info: frunner::ComponentStartInfo,
    pub controller: fidl::endpoints::ServerEnd<frunner::ComponentControllerMarker>,
}

pub type StartRequest = mpsc::Sender<Result<StartedComponent, zx::Status>>;

pub type StartRequestReceiver = mpsc::Receiver<Result<StartedComponent, zx::Status>>;
