// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_lock::OnceCell;
use async_trait::async_trait;
use fidl::endpoints::RequestStream;
use fidl_fuchsia_process::HandleInfo;
use fuchsia_runtime::HandleType;
use futures::TryStreamExt;
use futures::future::{BoxFuture, FutureExt};
use log::warn;
use runner::component::{Controllable, StopInfo};
use std::sync::Arc;
use zx::Koid;
use {fidl_fuchsia_examples_colocated as fcolocated, fuchsia_async as fasync};

/// [`ColocatedProgram `] represents an instance of a program run by the
/// colocated runner. Its state is held in this struct and its behavior
/// is run in the `task`.
pub struct ColocatedProgram {
    task: Option<fasync::Task<()>>,
    terminated: Arc<OnceCell<()>>,
    vmo_koid: Koid,
}

impl ColocatedProgram {
    pub fn new(numbered_handles: Vec<HandleInfo>) -> Result<Self, anyhow::Error> {
        let vmo = zx::Vmo::create(1024)?;
        let vmo_koid = vmo.koid()?;

        let terminated = Arc::new(OnceCell::new());
        let terminated_clone = terminated.clone();
        let guard = scopeguard::guard((), move |()| {
            _ = terminated_clone.set_blocking(());
        });
        let task = async move {
            // We will notify others of termination when this guard is dropped,
            // which happens when this task is dropped.
            let _guard = guard;

            // Signal to the outside world that the pages have been allocated.
            let handle_info = numbered_handles
                .into_iter()
                .filter(|info| match fuchsia_runtime::HandleInfo::try_from(info.id) {
                    Ok(handle_info) => {
                        handle_info == fuchsia_runtime::HandleInfo::new(HandleType::User0, 0)
                    }
                    Err(_) => false,
                })
                .next()
                .unwrap();

            let channel = zx::Channel::from(handle_info.handle);
            let mut request_stream = fcolocated::ColocatedRequestStream::from_channel(
                fasync::Channel::from_channel(channel),
            );
            while let Some(request) = request_stream.try_next().await.unwrap() {
                match request {
                    fcolocated::ColocatedRequest::GetVmos { responder } => {
                        responder.send(&[vmo_koid.raw_koid()]).unwrap();
                    }
                    fcolocated::ColocatedRequest::_UnknownMethod { .. } => {
                        panic!("Unknown method");
                    }
                }
            }

            // Sleep forever.
            std::future::pending().await
        };
        let task = fasync::Task::spawn(task);
        Ok(Self { task: Some(task), terminated, vmo_koid })
    }

    /// Returns a future that will resolve when the program is terminated.
    pub fn wait_for_termination<'a>(&self) -> BoxFuture<'a, StopInfo> {
        let terminated = self.terminated.clone();
        async move {
            terminated.wait().await;
            StopInfo::from_ok(None)
        }
        .boxed()
    }

    /// Returns the koid of the program's VMO, so the runner can report its memory as attributed to
    /// this component.
    pub fn get_vmo_koid(&self) -> Koid {
        self.vmo_koid
    }
}

#[async_trait]
impl Controllable for ColocatedProgram {
    async fn kill(&mut self) {
        warn!("Timed out stopping ColocatedProgram");
        self.stop().await
    }

    fn stop<'a>(&mut self) -> BoxFuture<'a, ()> {
        let task = self.task.take();
        async {
            if let Some(task) = task {
                _ = task.abort();
            }
        }
        .boxed()
    }
}
