// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use crate::att::l2cap::{
    FixedCid, L2CapChannel, L2CapChannelRx, L2CapChannelTx, L2CapEstablishChannelError,
    L2CapLogicalLink, L2CapRecvError, L2CapSendError,
};
use core::mem::MaybeUninit;
use futures::{FutureExt, pin_mut, select_biased};
use sapphire_async::executor::BoundedExecutor;
use sapphire_async::testing::TestExecutor;
use std::sync::Arc;
use std::task::Poll;

use sapphire_async::mpsc::{Mpsc, MpscCfg, RecvError, Rx, SendError, Tx};
use sapphire_async::notification::Notification;
use sapphire_collections::storage::ArrayStorage;
use sapphire_sync::mutex::Mutex;
use sapphire_sync::mutex::raw::SingleThreadMutex;

pub struct MockL2CapMpscCfg;
impl MpscCfg for MockL2CapMpscCfg {
    type Buffer = ArrayStorage<16>;
    type Mtx = SingleThreadMutex;
}

#[derive(Clone)]
pub struct MockTx {
    mpsc_tx: Tx<Arc<Mpsc<Vec<u8>, MockL2CapMpscCfg>>>,
    shared: Arc<Mutex<SingleThreadMutex, L2capSharedState>>,
}

pub struct MockRx {
    mpsc_rx: Rx<Arc<Mpsc<Vec<u8>, MockL2CapMpscCfg>>>,
    shared: Arc<Mutex<SingleThreadMutex, L2capSharedState>>,
}

fn mock_channel(shared: Arc<Mutex<SingleThreadMutex, L2capSharedState>>) -> (MockTx, MockRx) {
    let channel = Mpsc::<Vec<u8>, MockL2CapMpscCfg>::new();
    let (tx, rx) = channel.split_to_arc();
    (MockTx { mpsc_tx: tx, shared: shared.clone() }, MockRx { mpsc_rx: rx, shared })
}

impl L2CapChannelTx for MockTx {
    async fn send(&mut self, sdu: &[u8]) -> Result<(), L2CapSendError> {
        let closed_notify = {
            let state = self.shared.lock();
            if state.closed {
                return Err(L2CapSendError::LinkClosed);
            }
            state.closed_notify.clone()
        };

        let send_fut = self.mpsc_tx.send(sdu.to_vec()).fuse();
        let closed_fut = closed_notify.wait().fuse();

        pin_mut!(send_fut);
        pin_mut!(closed_fut);

        select_biased! {
            res = send_fut => {
                match res {
                    Ok(()) => Ok(()),
                    Err((_, SendError::Closed)) => Err(L2CapSendError::LinkClosed),
                }
            }
            _ = closed_fut => {
                Err(L2CapSendError::LinkClosed)
            }
        }
    }
}

impl L2CapChannelRx for MockRx {
    async fn recv<'a>(
        &mut self,
        buffer: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], L2CapRecvError> {
        let closed_notify = {
            let state = self.shared.lock();
            if state.closed {
                return Err(L2CapRecvError::LinkClosed);
            }
            state.closed_notify.clone()
        };

        let recv_fut = self.mpsc_rx.recv().fuse();
        let closed_fut = closed_notify.wait().fuse();

        pin_mut!(recv_fut);
        pin_mut!(closed_fut);

        select_biased! {
            res = recv_fut => {
                match res {
                    Ok(sdu) => {
                        if buffer.len() < sdu.len() {
                            return Err(L2CapRecvError::BufferTooSmall);
                        }
                        let initialized = buffer[..sdu.len()].write_copy_of_slice(&sdu);
                        Ok(initialized)
                    }
                    Err(RecvError::Closed) => Err(L2CapRecvError::LinkClosed),
                }
            }
            _ = closed_fut => {
                Err(L2CapRecvError::LinkClosed)
            }
        }
    }
}

/// Internal structural layout tracking rendezvous state for a single L2CAP fixed channel slot.
struct ChannelSlot {
    /// Sibling handles handed down to the application upon successful rendezvous resolution.
    app_channel: Option<L2CapChannel<MockTx, MockRx>>,
    /// Sibling handles handed down to the test driver upon successful rendezvous resolution.
    test_channel: Option<(MockTx, MockRx)>,

    /// Flag indicating link disruption or logical closure.
    closed: bool,
}

struct SharedSlot {
    slot: Mutex<SingleThreadMutex, ChannelSlot>,
    claim_channel_notification: Notification<SingleThreadMutex>,
    expect_channel_claimed_notification: Notification<SingleThreadMutex>,
}

struct L2capSharedState {
    slots: std::collections::HashMap<FixedCid, Arc<SharedSlot>>,
    closed: bool,
    closed_notify: Arc<Notification<SingleThreadMutex>>,
}

/// The main mock coordinator retained by the test harness to expect channel mappings,
/// send mock packets, and intercept application SDUs.
#[derive(Clone)]
pub struct MockL2cap {
    shared: Arc<Mutex<SingleThreadMutex, L2capSharedState>>,
}

/// The active logical link handle representation handed down to application code.
/// Implements `L2CapLogicalLink`.
#[derive(Clone)]
pub struct MockL2CapLink {
    shared: Arc<Mutex<SingleThreadMutex, L2capSharedState>>,
}

impl MockL2cap {
    /// Creates a new Mock L2CAP coordinator setup.
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(L2capSharedState {
                slots: std::collections::HashMap::new(),
                closed: false,
                closed_notify: Arc::new(Notification::new()),
            })),
        }
    }

    /// Extracts the logical link end that implements the production `L2CapLogicalLink` trait.
    pub fn l2cap(&self) -> MockL2CapLink {
        MockL2CapLink { shared: self.shared.clone() }
    }

    /// Simulates sudden closure or disruption of the entire logical connection link.
    pub fn close(&self) {
        let mut shared = self.shared.lock();
        shared.closed = true;
        shared.closed_notify.notify_all();
        for slot_shared in shared.slots.values() {
            let mut slot = slot_shared.slot.lock();
            slot.closed = true;
            slot_shared.claim_channel_notification.notify_all();
            slot_shared.expect_channel_claimed_notification.notify_all();
        }
    }

    /// Blocking expectation that application code claims the targeted fixed channel ID.
    /// Returns the associated mock transport endpoints for test-driver interaction.
    pub async fn expect_channel_claimed(
        &self,
        cid: FixedCid,
    ) -> Result<(MockTx, MockRx), L2CapEstablishChannelError> {
        let slot_arc = {
            let mut shared = self.shared.lock();
            if shared.closed {
                return Err(L2CapEstablishChannelError::LinkClosed);
            }
            shared
                .slots
                .entry(cid)
                .or_insert_with(|| {
                    let (app_tx, test_rx) = mock_channel(self.shared.clone());
                    let (test_tx, app_rx) = mock_channel(self.shared.clone());
                    Arc::new(SharedSlot {
                        slot: Mutex::new(ChannelSlot {
                            app_channel: Some(L2CapChannel { sender: app_tx, receiver: app_rx }),
                            test_channel: Some((test_tx, test_rx)),
                            closed: false,
                        }),
                        claim_channel_notification: Notification::new(),
                        expect_channel_claimed_notification: Notification::new(),
                    })
                })
                .clone()
        };

        let mut slot = slot_arc.slot.lock();
        if slot.closed {
            return Err(L2CapEstablishChannelError::LinkClosed);
        }

        let channel = slot.test_channel.take().ok_or(L2CapEstablishChannelError::AlreadyInUse)?;
        slot_arc.claim_channel_notification.notify_all();

        let test_channel_res = slot_arc
            .expect_channel_claimed_notification
            .when(slot, |slot| {
                if slot.closed {
                    Poll::Ready(Err(L2CapEstablishChannelError::LinkClosed))
                } else if slot.app_channel.is_none() {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            })
            .await;

        match test_channel_res {
            Ok(()) => Ok(channel),
            Err(e) => Err(e),
        }
    }
}

impl L2CapLogicalLink for MockL2CapLink {
    type Tx = MockTx;
    type Rx = MockRx;

    async fn claim_fixed_channel(
        &mut self,
        channel: FixedCid,
    ) -> Result<L2CapChannel<Self::Tx, Self::Rx>, L2CapEstablishChannelError> {
        let slot_arc = {
            let mut shared = self.shared.lock();
            if shared.closed {
                return Err(L2CapEstablishChannelError::LinkClosed);
            }
            shared
                .slots
                .entry(channel)
                .or_insert_with(|| {
                    let (app_tx, test_rx) = mock_channel(self.shared.clone());
                    let (test_tx, app_rx) = mock_channel(self.shared.clone());
                    Arc::new(SharedSlot {
                        slot: Mutex::new(ChannelSlot {
                            app_channel: Some(L2CapChannel { sender: app_tx, receiver: app_rx }),
                            test_channel: Some((test_tx, test_rx)),
                            closed: false,
                        }),
                        claim_channel_notification: Notification::new(),
                        expect_channel_claimed_notification: Notification::new(),
                    })
                })
                .clone()
        };

        let mut slot = slot_arc.slot.lock();
        if slot.closed {
            return Err(L2CapEstablishChannelError::LinkClosed);
        }

        let channel = slot.app_channel.take().ok_or(L2CapEstablishChannelError::AlreadyInUse)?;
        slot_arc.expect_channel_claimed_notification.notify_all();

        let app_channel_res = slot_arc
            .claim_channel_notification
            .when(slot, |slot| {
                if slot.closed {
                    Poll::Ready(Err(L2CapEstablishChannelError::LinkClosed))
                } else if slot.test_channel.is_none() {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            })
            .await;

        match app_channel_res {
            Ok(()) => Ok(channel),
            Err(e) => Err(e),
        }
    }
}

/// Public test helper to set up a loopback L2CAP channel pair using the Mock coordinator.
pub fn setup_mock_channel() -> (L2CapChannel<MockTx, MockRx>, MockTx, MockRx) {
    let shared = MockL2cap::new().shared;
    let (app_tx, test_rx) = mock_channel(shared.clone());
    let (test_tx, app_rx) = mock_channel(shared);
    let app_channel = L2CapChannel { sender: app_tx, receiver: app_rx };
    (app_channel, test_tx, test_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_l2cap_rendezvous_claim_first() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (mut app_channel, mut peer_tx, mut peer_rx) = setup_mock_channel();

            // 3. Drive dynamic loopback using concurrent spawned tasks to verify asynchrony
            let mut send_handle = executor.spawn(async move {
                let test_sdu = b"rendezvous test SDU";
                app_channel.sender.send(test_sdu).await.unwrap();
            });

            let mut recv_handle = executor.spawn(async move {
                let test_sdu = b"rendezvous test SDU";
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let rx_slice = peer_rx.recv(&mut rx_buf).await.unwrap();
                assert_eq!(rx_slice, test_sdu);
            });

            executor.run_until_stalled();
            assert!(send_handle.is_finished());
            assert!(recv_handle.is_finished());
        });
    }

    #[test]
    fn test_mock_channel_send_recv() {
        let send_packet = b"hello attenuation";

        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, _test_tx, mut rx) = setup_mock_channel();
            let mut tx = app_channel.sender;

            // 1. Spawn a concurrent receiver task awaiting bytes
            let mut recv_handle = executor.spawn(async move {
                let mut buf = [MaybeUninit::uninit(); 32];
                rx.recv(&mut buf).await.expect("recv succeeds").to_vec()
            });

            executor.run_until_stalled();
            // Receiver must be suspended awaiting bytes (queue is empty)
            assert!(!recv_handle.is_finished());

            // 2. Spawn a concurrent sender task to transmit the SDU
            let mut send_handle = executor.spawn(async move {
                tx.send(send_packet).await.expect("send succeeds");
            });

            // Drive executor: sender transmits, triggers notification, receiver wakes & resolves!
            executor.run_until_stalled();

            assert!(send_handle.is_finished());
            assert!(recv_handle.is_finished());
            let received = recv_handle.get().unwrap();
            assert_eq!(received, send_packet);
        });
    }

    #[test]
    fn test_mock_channel_recv_and_close() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let l2cap_mock = MockL2cap::new();
            let (app_tx, _test_rx) = mock_channel(l2cap_mock.shared.clone());
            let (_test_tx, app_rx) = mock_channel(l2cap_mock.shared.clone());
            let app_channel = L2CapChannel { sender: app_tx, receiver: app_rx };

            // 1. Spawn a concurrent receiver task awaiting bytes on the app channel
            let mut recv_handle = executor.spawn(async move {
                let mut app_rx = app_channel.receiver;
                let mut buf = [MaybeUninit::uninit(); 32];
                app_rx.recv(&mut buf).await.unwrap_err()
            });

            executor.run_until_stalled();
            assert!(!recv_handle.is_finished());

            // 2. Forcibly close the logical link via MockL2cap
            l2cap_mock.close();

            executor.run_until_stalled();
            assert!(recv_handle.is_finished());
            let err = recv_handle.get().unwrap();
            assert_eq!(err, L2CapRecvError::LinkClosed);
        });
    }
}
