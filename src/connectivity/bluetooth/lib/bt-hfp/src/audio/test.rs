// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::codec_id::CodecId;
use crate::sco;
use fuchsia_bluetooth::types::PeerId;
use fuchsia_sync::Mutex;
use futures::StreamExt;
use futures::stream::BoxStream;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{Control, ControlEvent, Error};

struct TestControlInner {
    started: HashSet<PeerId>,
    connected: HashMap<PeerId, HashSet<CodecId>>,
    connections: HashMap<PeerId, sco::Connection>,
    event_sender: futures::channel::mpsc::Sender<ControlEvent>,
}

#[derive(Clone)]
pub struct TestControl {
    inner: Arc<Mutex<TestControlInner>>,
}

impl TestControl {
    pub fn unexpected_stop(&self, id: PeerId, error: Error) {
        let mut lock = self.inner.lock();
        let _ = lock.started.remove(&id);
        let _ = lock.connections.remove(&id);
        let _ = lock.event_sender.try_send(ControlEvent::Stopped { id, error: Some(error) });
    }

    pub fn is_started(&self, id: PeerId) -> bool {
        let lock = self.inner.lock();
        lock.started.contains(&id) && lock.connections.contains_key(&id)
    }

    pub fn is_connected(&self, id: PeerId) -> bool {
        let lock = self.inner.lock();
        lock.connected.contains_key(&id)
    }
}

impl Default for TestControl {
    fn default() -> Self {
        // Make a disconnected sender, we do not care about whether it succeeds.
        let (event_sender, _) = futures::channel::mpsc::channel(0);
        Self {
            inner: Arc::new(Mutex::new(TestControlInner {
                started: Default::default(),
                connected: Default::default(),
                connections: Default::default(),
                event_sender,
            })),
        }
    }
}

impl Control for TestControl {
    fn start(
        &mut self,
        id: PeerId,
        connection: sco::Connection,
        _codec: CodecId,
    ) -> Result<(), Error> {
        let mut lock = self.inner.lock();
        if !lock.started.insert(id) {
            return Err(Error::AlreadyStarted);
        }
        let _ = lock.connections.insert(id, connection);
        let _ = lock.event_sender.try_send(ControlEvent::Started { id });
        Ok(())
    }

    fn stop(&mut self, id: PeerId) -> Result<(), Error> {
        let mut lock = self.inner.lock();
        if !lock.started.remove(&id) {
            return Err(Error::NotStarted);
        }
        let _ = lock.connections.remove(&id);
        let _ = lock.event_sender.try_send(ControlEvent::Stopped { id, error: None });
        Ok(())
    }

    fn connect(&mut self, id: PeerId, supported_codecs: &[CodecId]) {
        let mut lock = self.inner.lock();
        let _ = lock.connected.insert(id, supported_codecs.iter().cloned().collect());
    }

    fn disconnect(&mut self, id: PeerId) {
        let _ = self.stop(id);
        let mut lock = self.inner.lock();
        let _ = lock.connected.remove(&id);
    }

    fn take_events(&self) -> BoxStream<'static, ControlEvent> {
        let mut lock = self.inner.lock();
        // Replace the sender.
        let (sender, receiver) = futures::channel::mpsc::channel(1);
        lock.event_sender = sender;
        receiver.boxed()
    }

    fn failed_request(&self, _request: ControlEvent, _error: Error) {
        // Nothing to do here for the moment
    }
}
