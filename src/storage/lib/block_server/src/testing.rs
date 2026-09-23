// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::callback_interface::{Interface, Request, Session, SessionManager};
use crate::verifier::Verifier;
use crate::{BlockInfo, DeviceInfo};
use std::borrow::Cow;
use std::sync::Arc;

pub struct MockInterface {
    pub request_sender: std::sync::mpsc::Sender<Request>,
}

impl MockInterface {
    pub fn new(request_sender: std::sync::mpsc::Sender<Request>) -> Self {
        Self { request_sender }
    }
}

impl Interface for MockInterface {
    type Orchestrator = SessionManager<Self>;

    fn get_info(&self) -> Cow<'_, DeviceInfo> {
        Cow::Owned(DeviceInfo::Block(BlockInfo { block_count: 1024, ..Default::default() }))
    }

    fn spawn_session(&self, session: Arc<Session<Self>>) {
        std::thread::spawn(move || {
            session.run();
        });
    }

    fn on_requests(&self, requests: &[Request]) {
        for request in requests {
            self.request_sender.send(request.clone()).unwrap();
        }
    }

    fn on_open_mapper_session(
        &self,
        _mapping_vmo: &zx::Vmo,
        delivery_queue: zx::Vmo,
    ) -> Result<Verifier, zx::Status> {
        Ok(Verifier::new(delivery_queue))
    }
}
