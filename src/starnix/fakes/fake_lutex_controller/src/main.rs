// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow};
use fidl_fuchsia_posix as fposix;
use fidl_fuchsia_starnix_binder as fbinder;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use futures::{FutureExt, StreamExt, TryFutureExt, TryStreamExt, pin_mut, select};
use log::{error, warn};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use zx;

enum Services {
    LutexController(fbinder::LutexControllerRequestStream),
}

#[derive(Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
struct LutexKey {
    koid: zx::Koid,
    offset: u64,
}

struct LutexWaiter {
    sender: Option<oneshot::Sender<()>>,
    mask: u32,
}

#[derive(Default)]
struct LutexState {
    state: Mutex<BTreeMap<LutexKey, Vec<LutexWaiter>>>,
}

impl LutexState {
    fn check_futex_value(vmo: &zx::Vmo, offset: u64, value: u32) -> Result<(), fposix::Errno> {
        // TODO: This read should be atomic.
        let mut buf = [0u8; 4];
        vmo.read(&mut buf, offset).map_err(|_| panic!("Impossible Error"))?;
        if u32::from_ne_bytes(buf) != value {
            return Err(fposix::Errno::Eagain);
        }
        Ok(())
    }

    async fn select_first<O>(f1: impl Future<Output = O>, f2: impl Future<Output = O>) -> O {
        let f1 = f1.fuse();
        let f2 = f2.fuse();
        pin_mut!(f1, f2);
        select! {
            f1 = f1 => f1,
            f2 = f2 => f2,
        }
    }

    async fn wait(
        &self,
        vmo: zx::Vmo,
        offset: u64,
        value: u32,
        mask: u32,
        deadline: Option<zx::MonotonicInstant>,
    ) -> Result<(), fposix::Errno> {
        let koid = vmo.info().map_err(|_| panic!("Impossible Error"))?.koid;
        let key = LutexKey { koid, offset };
        let (sender, receiver) = oneshot::channel::<()>();
        self.state.lock().entry(key).or_default().push(LutexWaiter { sender: Some(sender), mask });
        Self::check_futex_value(&vmo, offset, value)?;
        let receiver = receiver.map_err(|_| fposix::Errno::Etimedout);
        if let Some(deadline) = deadline {
            let timer = fasync::Timer::new(deadline - zx::MonotonicInstant::get())
                .map(|_| Err(fposix::Errno::Etimedout));
            Self::select_first(timer, receiver).await
        } else {
            receiver.await
        }
    }

    async fn wake(
        &self,
        vmo: zx::Vmo,
        offset: u64,
        count: u32,
        mask: u32,
    ) -> Result<u64, fposix::Errno> {
        let koid = vmo.info().map_err(|_| panic!("Impossible Error"))?.koid;
        let key = LutexKey { koid, offset };
        let mut woken = 0;
        self.state.lock().entry(key).or_default().retain_mut(|waiter| {
            if woken >= count || waiter.mask & mask == 0 {
                return true;
            }
            if let Some(sender) = waiter.sender.take() {
                if let Ok(()) = sender.send(()) {
                    woken += 1;
                }
            }
            false
        });
        Ok(woken as u64)
    }

    async fn requeue(
        &self,
        _first_vmo: zx::Vmo,
        _first_offset: u64,
        _second_vmo: Option<zx::Vmo>,
        _second_offset: u64,
        _wake_count: u32,
        _requeue_count: u32,
        _expected_value: Option<u32>,
    ) -> Result<u64, fposix::Errno> {
        // no current need for this impl
        panic!("not yet implemented");
    }
}

async fn serve_lutex_controller(
    state: Arc<LutexState>,
    stream: fbinder::LutexControllerRequestStream,
) -> Result<(), Error> {
    async fn handle_request(
        state: Arc<LutexState>,
        event: fbinder::LutexControllerRequest,
    ) -> Result<(), Error> {
        match event {
            fbinder::LutexControllerRequest::WaitBitset { payload, responder } => {
                let vmo = payload.vmo.ok_or_else(|| anyhow!("No vmo"))?;
                let offset = payload.offset.ok_or_else(|| anyhow!("No offset"))?;
                let value = payload.value.ok_or_else(|| anyhow!("No value"))?;
                let mask = payload.mask.unwrap_or(u32::MAX);
                let deadline = payload.deadline.map(zx::MonotonicInstant::from_nanos);
                responder
                    .send(state.wait(vmo, offset, value, mask, deadline).await)
                    .context("Unable to send LutexControllerRequest::WaitBitset response")
            }
            fbinder::LutexControllerRequest::WakeBitset { payload, responder } => {
                let vmo = payload.vmo.ok_or_else(|| anyhow!("No vmo"))?;
                let offset = payload.offset.ok_or_else(|| anyhow!("No offset"))?;
                let count = payload.count.ok_or_else(|| anyhow!("No count"))?;
                let mask = payload.mask.unwrap_or(u32::MAX);
                responder
                    .send(state.wake(vmo, offset, count, mask).await.map(|count| {
                        fbinder::WakeResponse {
                            count: Some(count as u64),
                            ..fbinder::WakeResponse::default()
                        }
                    }))
                    .context("Unable to send LutexControllerRequest::WaitBitset response")
            }
            fbinder::LutexControllerRequest::CmpRequeue { payload, responder } => {
                let first_vmo = payload.first_vmo.ok_or_else(|| anyhow!("no first_vmo"))?;
                let first_offset =
                    payload.first_offset.ok_or_else(|| anyhow!("no first_offset"))?;
                let second_vmo = payload.second_vmo;
                let second_offset =
                    payload.second_offset.ok_or_else(|| anyhow!("no second_offset"))?;
                let wake_count = payload.wake_count.ok_or_else(|| anyhow!("no wake_count"))?;
                let requeue_count =
                    payload.requeue_count.ok_or_else(|| anyhow!("no requeue_count"))?;
                let cmp_val = payload.cmp_val;
                responder
                    .send(
                        state
                            .requeue(
                                first_vmo,
                                first_offset,
                                second_vmo,
                                second_offset,
                                wake_count,
                                requeue_count,
                                cmp_val,
                            )
                            .await
                            .map(|count| fbinder::CmpRequeueResponse {
                                count: Some(count as u64),
                                ..fbinder::CmpRequeueResponse::default()
                            }),
                    )
                    .context("Unable to send LutexControllerRequest::Requeue response")
            }
            fbinder::LutexControllerRequest::_UnknownMethod { ordinal, .. } => {
                warn!("Unknown LutexController ordinal: {}", ordinal);
                Ok(())
            }
        }
    }
    stream
        .map(|result| result.context("failed fbinder::LutexController request"))
        .try_for_each_concurrent(None, move |event| handle_request(state.clone(), event))
        .await
}

#[fuchsia::main(logging_tags = ["fake_lutex_controller"])]
async fn main() -> Result<(), Error> {
    let state = Arc::new(LutexState::default());
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(Services::LutexController);
    fs.take_and_serve_directory_handle()?;

    fs.for_each_concurrent(None, move |request: Services| {
        let state = state.clone();
        async {
            match request {
                Services::LutexController(stream) => {
                    if let Err(e) = serve_lutex_controller(state, stream).await {
                        error!("Error when serving lutex controller: {e:?}");
                    }
                }
            }
        }
    })
    .await;

    Ok(())
}
