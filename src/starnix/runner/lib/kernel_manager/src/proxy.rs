// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::suspend::WakeSources;
use anyhow::{Error, anyhow};
use fuchsia_async as fasync;
use futures::FutureExt;
use log::warn;
use starnix_sync::{LockDepMutex, WakeSourcesLock};
use std::cell::RefCell;
use std::mem::MaybeUninit;
use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;

/// `ChannelProxy` is used to proxy messages on a `zx::Channel` between the Starnix
/// container and the outside world. This allows the Starnix runner to wake the container
/// on incoming messages.
///
/// [platform component] <-- remote_channel --> [Starnix runner] <-- container_channel --> [Starnix container]
pub struct ChannelProxy {
    /// The channel that is connected to the container component.
    pub container_channel: zx::Channel,

    /// The channel that is connected to a peer outside of the container component.
    pub remote_channel: zx::Channel,

    /// The number of unhandled messages on this proxy. If non-zero, the container is still
    /// processing one of the incoming messages and the container should not be suspended.
    pub message_counter: zx::Counter,

    /// Human readable name for the thing that is being proxied.
    pub name: String,
}

// `WaitReturn` is used to indicate which proxy endpoint caused the wait to complete.
#[derive(Debug)]
enum WaitReturn {
    Container,
    Remote,
}

/// The Zircon role name that is applied to proxy threads.
const PROXY_ROLE_NAME: &str = "fuchsia.starnix.runner.proxy";

/// Starts a thread that listens for new proxies and runs `start_proxy` on each.
pub fn run_proxy_thread(
    new_proxies: async_channel::Receiver<(
        ChannelProxy,
        Arc<LockDepMutex<WakeSources, WakeSourcesLock>>,
    )>,
) {
    let _ = std::thread::Builder::new().name("proxy_thread".to_string()).spawn(move || {
        if let Err(e) = fuchsia_scheduler::set_role_for_this_thread(PROXY_ROLE_NAME) {
            warn!(e:%; "failed to set thread role");
        }
        let mut executor = fasync::LocalExecutor::default();
        executor.run_singlethreaded(async move {
            let tasks = fasync::Scope::new();
            let bounce_bytes = Rc::new(RefCell::new(
                [MaybeUninit::uninit(); zx::sys::ZX_CHANNEL_MAX_MSG_BYTES as usize],
            ));
            let bounce_handles = Rc::new(RefCell::new(
                [const { MaybeUninit::uninit() }; zx::sys::ZX_CHANNEL_MAX_MSG_HANDLES as usize],
            ));
            while let Ok((proxy, events)) = new_proxies.recv().await {
                let bytes_clone = bounce_bytes.clone();
                let handles_clone = bounce_handles.clone();
                tasks.spawn_local(start_proxy(proxy, events, bytes_clone, handles_clone));
            }
        });
    });
}

/// Starts a task that proxies messages between `proxy.container_channel` and
/// `proxy.remote_channel`. The task will exit when either of the channels' peer is closed, or
/// if `proxy.resume_event`'s peer is closed.
///
/// When the task exits, `proxy.resume_event` will be removed from `wake_sources`.
async fn start_proxy(
    proxy: ChannelProxy,
    wake_sources: Arc<LockDepMutex<WakeSources, WakeSourcesLock>>,
    bounce_bytes: Rc<RefCell<[MaybeUninit<u8>; zx::sys::ZX_CHANNEL_MAX_MSG_BYTES as usize]>>,
    bounce_handles: Rc<
        RefCell<[MaybeUninit<zx::NullableHandle>; zx::sys::ZX_CHANNEL_MAX_MSG_HANDLES as usize]>,
    >,
) {
    let proxy_name = proxy.name.as_str();
    trace_instant("starnix_runner:start_proxy:loop:enter", proxy_name);

    'outer: loop {
        // Wait on messages from both the container and remote channel endpoints.
        let mut container_wait = pin!(
            fasync::OnSignals::new(
                proxy.container_channel.as_handle_ref(),
                zx::Signals::CHANNEL_READABLE | zx::Signals::CHANNEL_PEER_CLOSED,
            )
            .fuse()
        );
        let mut remote_wait = pin!(
            fasync::OnSignals::new(
                proxy.remote_channel.as_handle_ref(),
                zx::Signals::CHANNEL_READABLE | zx::Signals::CHANNEL_PEER_CLOSED,
            )
            .fuse()
        );

        let (signals, finished_wait) = {
            trace_duration("starnix_runner:start_proxy:wait_for_messages", proxy_name);
            let result = futures::select! {
                res = container_wait => {
                    trace_instant("starnix_runner:start_proxy:container_readable", proxy_name);
                    res.map(|s| (s, WaitReturn::Container))
                },
                res = remote_wait => {
                    trace_instant("starnix_runner:start_proxy:remote_readable", proxy_name);
                    res.map(|s| (s, WaitReturn::Remote))
                },
            };

            match result {
                Ok(result) => result,
                Err(e) => {
                    trace_instant("starnix_runner:start_proxy:result:error", proxy_name);
                    log::warn!("Failed to wait on proxied channels in runner: {:?}", e);
                    break 'outer;
                }
            }
        };

        // Forward messages in both directions. Only messages that are entering the container
        // should signal `proxy.resume_event`, since those are the only messages that should
        // wake the container if it's suspended.
        let name = proxy.name.as_str();
        let result = match finished_wait {
            WaitReturn::Container => forward_message(
                &signals,
                &proxy.container_channel,
                &proxy.remote_channel,
                None,
                &mut bounce_bytes.borrow_mut(),
                &mut bounce_handles.borrow_mut(),
                name,
            ),
            WaitReturn::Remote => forward_message(
                &signals,
                &proxy.remote_channel,
                &proxy.container_channel,
                Some(&proxy.message_counter),
                &mut bounce_bytes.borrow_mut(),
                &mut bounce_handles.borrow_mut(),
                name,
            ),
        };

        if result.is_err() {
            log::warn!(
                "Proxy failed to forward message {} kernel: {}; {:?}",
                match finished_wait {
                    WaitReturn::Container => "from",
                    WaitReturn::Remote => "to",
                },
                name,
                result,
            );
            break 'outer;
        }
    }

    trace_instant("starnix_runner:start_proxy:loop:exit", proxy_name);
    if let Ok(koid) = proxy.message_counter.koid() {
        wake_sources.lock().remove(&koid);
    }
}

/// Forwards any pending messages on `read_channel` to `write_channel`, if the `wait_item.pending`
/// contains `CHANNEL_READABLE`.
///
/// If `message_counter` is `Some`, it will be incremented by one when writing the message to the
/// write_channel.

fn forward_message(
    signals: &zx::Signals,
    read_channel: &zx::Channel,
    write_channel: &zx::Channel,
    message_counter: Option<&zx::Counter>,
    bytes: &mut [MaybeUninit<u8>; zx::sys::ZX_CHANNEL_MAX_MSG_BYTES as usize],
    handles: &mut [MaybeUninit<zx::NullableHandle>; zx::sys::ZX_CHANNEL_MAX_MSG_HANDLES as usize],
    name: &str,
) -> Result<(), Error> {
    trace_duration("starnix_runner:forward_message", name);

    if signals.contains(zx::Signals::CHANNEL_READABLE) {
        let (actual_bytes, actual_handles) = {
            match read_channel.read_uninit(bytes, handles) {
                zx::ChannelReadResult::Ok(r) => r,
                _ => return Err(anyhow!("Failed to read from channel")),
            }
        };

        if let Some(counter) = message_counter {
            counter.add(1).expect("Failed to add to the proxy's message counter");
            trace_instant("starnix_runner:forward_message:counter_incremented", name);
        }

        write_channel.write(actual_bytes, actual_handles)?;
    }

    // It is important to check for peer closed after readable, in order to flush any
    // remaining messages in the proxied channel.
    if signals.contains(zx::Signals::CHANNEL_PEER_CLOSED) {
        Err(anyhow!("Proxy peer was closed"))
    } else {
        Ok(())
    }
}

fn trace_duration(event: &'static str, name: &str) {
    fuchsia_trace::duration!("power", event, "name" => name);
}

fn trace_instant(event: &'static str, name: &str) {
    fuchsia_trace::instant!(
        "power",
        event,
        fuchsia_trace::Scope::Process,
        "name" => name
    );
}

#[cfg(test)]
mod test {
    use super::{ChannelProxy, fasync, start_proxy};
    use std::cell::RefCell;
    use std::mem::MaybeUninit;
    use std::rc::Rc;

    fn run_proxy_for_test(proxy: ChannelProxy) -> fasync::Task<()> {
        let bounce_bytes = Rc::new(RefCell::new(
            [MaybeUninit::uninit(); zx::sys::ZX_CHANNEL_MAX_MSG_BYTES as usize],
        ));
        let bounce_handles = Rc::new(RefCell::new(
            [const { MaybeUninit::uninit() }; zx::sys::ZX_CHANNEL_MAX_MSG_HANDLES as usize],
        ));
        fasync::Task::local(start_proxy(proxy, Default::default(), bounce_bytes, bounce_handles))
    }

    #[fuchsia::test]
    async fn test_peer_closed_kernel() {
        let (local_client, local_server) = zx::Channel::create();
        let (remote_client, remote_server) = zx::Channel::create();
        let message_counter = zx::Counter::create();

        let channel_proxy = ChannelProxy {
            container_channel: local_server,
            remote_channel: remote_client,
            message_counter,
            name: "test".to_string(),
        };
        let _task = run_proxy_for_test(channel_proxy);

        std::mem::drop(local_client);

        fasync::OnSignals::new(remote_server, zx::Signals::CHANNEL_PEER_CLOSED).await.unwrap();
    }

    #[fuchsia::test]
    async fn test_peer_closed_remote() {
        let (local_client, local_server) = zx::Channel::create();
        let (remote_client, remote_server) = zx::Channel::create();
        let message_counter = zx::Counter::create();

        let channel_proxy = ChannelProxy {
            container_channel: local_server,
            remote_channel: remote_client,
            message_counter,
            name: "test".to_string(),
        };
        let _task = run_proxy_for_test(channel_proxy);

        std::mem::drop(remote_server);

        fasync::OnSignals::new(local_client, zx::Signals::CHANNEL_PEER_CLOSED).await.unwrap();
    }

    #[fuchsia::test]
    async fn test_counter_sequential() {
        let (_local_client, local_server) = zx::Channel::create();
        let (remote_client, remote_server) = zx::Channel::create();
        let message_counter = zx::Counter::create();
        let local_message_counter = message_counter
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .expect("Failed to duplicate counter");

        let channel_proxy = ChannelProxy {
            container_channel: local_server,
            remote_channel: remote_client,
            message_counter,
            name: "test".to_string(),
        };
        let _task = run_proxy_for_test(channel_proxy);

        // Send a message and make sure counter is incremented.
        fasync::OnSignals::new(&local_message_counter, zx::Signals::COUNTER_NON_POSITIVE)
            .await
            .unwrap();
        assert!(remote_server.write(&[0x0, 0x1, 0x2], &mut []).is_ok());
        fasync::OnSignals::new(&local_message_counter, zx::Signals::COUNTER_POSITIVE)
            .await
            .unwrap();

        // Decrement the counter, simulating a read, and make sure it goes back down to zero.
        local_message_counter.add(-1).expect("Failed add");
        fasync::OnSignals::new(&local_message_counter, zx::Signals::COUNTER_NON_POSITIVE)
            .await
            .unwrap();
        assert!(remote_server.write(&[0x0, 0x1, 0x2], &mut []).is_ok());
        fasync::OnSignals::new(&local_message_counter, zx::Signals::COUNTER_POSITIVE)
            .await
            .unwrap();
    }

    #[fuchsia::test]
    async fn test_counter_multiple() {
        let (_local_client, local_server) = zx::Channel::create();
        let (remote_client, remote_server) = zx::Channel::create();
        let message_counter = zx::Counter::create();
        let local_message_counter = message_counter
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .expect("Failed to duplicate counter");

        let channel_proxy = ChannelProxy {
            container_channel: local_server,
            remote_channel: remote_client,
            message_counter,
            name: "test".to_string(),
        };
        let _task = run_proxy_for_test(channel_proxy);

        assert!(remote_server.write(&[0x0, 0x1, 0x2], &mut []).is_ok());
        assert!(remote_server.write(&[0x0, 0x1, 0x2], &mut []).is_ok());
        assert!(remote_server.write(&[0x0, 0x1, 0x2], &mut []).is_ok());
        fasync::OnSignals::new(&local_message_counter, zx::Signals::COUNTER_POSITIVE)
            .await
            .unwrap();
        assert_eq!(local_message_counter.read().expect("Failed to read counter"), 3);
    }
}
