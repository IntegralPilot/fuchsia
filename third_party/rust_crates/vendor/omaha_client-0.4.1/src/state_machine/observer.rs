// Copyright 2019 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

use crate::{
    common::{ProtocolState, UpdateCheckSchedule},
    installer::ProgressObserver,
    protocol::response::Response,
    state_machine::{State, UpdateCheckError, update_check},
};
use futures::{channel::mpsc, future::BoxFuture, prelude::*};

/// Events emitted by the state machine.
#[derive(Debug)]
pub enum StateMachineEvent {
    StateChange(State),
    ScheduleChange(UpdateCheckSchedule),
    ProtocolStateChange(ProtocolState),
    UpdateCheckResult(Result<update_check::Response, UpdateCheckError>),
    InstallProgressChange(InstallProgress),
    OmahaServerResponse(Response),
    InstallerError(Option<Box<dyn std::error::Error + Send + 'static>>),
}

#[derive(Debug)]
pub struct InstallProgress {
    pub progress: f32,
}

pub(super) struct StateMachineProgressObserver(pub(super) mpsc::Sender<InstallProgress>);

impl ProgressObserver for StateMachineProgressObserver {
    fn receive_progress(
        &self,
        _operation: Option<&str>,
        progress: f32,
        _total_size: Option<u64>,
        _size_so_far: Option<u64>,
    ) -> BoxFuture<'_, ()> {
        async move {
            let _ = self.0.clone().send(InstallProgress { progress }).await;
        }
        .boxed()
    }
}
