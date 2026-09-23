// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_bluetooth::profile::ValidScoConnectionParameters;
use fuchsia_inspect as inspect;
use fuchsia_inspect_derive::{IValue, Unit};
use futures::FutureExt;
use futures::future::{BoxFuture, Fuse, FusedFuture, Future, Shared};
use std::fmt;
use vigil::Vigil;

use crate::a2dp;
use crate::sco::{ConnectError, Connection};

#[derive(Debug)]
pub struct Active {
    pub params: ValidScoConnectionParameters,
    on_closed: Shared<BoxFuture<'static, ()>>,
    // Stored (but unused) here to keep A2DP in a paused state.
    _pause_token: a2dp::PauseToken,
}

impl Active {
    pub fn new(connection: &Connection, pause_token: a2dp::PauseToken) -> Self {
        let on_closed = connection.on_closed().boxed().shared();
        Self { params: connection.params.clone(), on_closed, _pause_token: pause_token }
    }

    pub fn on_closed(&self) -> impl Future<Output = ()> + 'static {
        self.on_closed.clone()
    }
}

impl Unit for Active {
    type Data = <Connection as Unit>::Data;
    fn inspect_create(&self, parent: &inspect::Node, name: impl AsRef<str>) -> Self::Data {
        self.params.inspect_create(parent, name)
    }

    fn inspect_update(&self, data: &mut Self::Data) {
        self.params.inspect_update(data)
    }
}

#[derive(Default)]
pub enum State {
    /// No call is in progress.
    #[default]
    Inactive,
    /// A call has been made active, and we are negotiating codecs before setting up the SCO
    /// connection. This state prevents a race in the AG where the call has been made active but
    /// SCO not yet set up, and the AG peer task, seeing that the connection is not Active,
    /// attempts to set up the SCO connection a second time,
    SettingUp,
    /// The HF has closed the remote SCO connection so we are waiting for the call to be set
    /// transferred to AG. This state prevents a race in the AG where the SCO connection has been
    /// torn down but the call not yet set to inactive by the call manager, so the peer task
    /// attempts to mark the call as inactive a second time.
    TearingDown,
    /// A call is transferred to the AG and we are waiting for the HF to initiate a SCO connection.
    AwaitingRemote(BoxFuture<'static, Result<Connection, ConnectError>>),
    /// A call is active and so is the SCO connection.
    Active(Vigil<Active>),
}

impl State {
    pub fn is_active(&self) -> bool {
        match self {
            Self::Active(_) => true,
            _ => false,
        }
    }

    pub fn on_connected<'a>(
        &'a mut self,
    ) -> impl Future<Output = Result<Connection, ConnectError>> + FusedFuture + 'a {
        match self {
            Self::AwaitingRemote(fut) => fut.fuse(),
            _ => Fuse::terminated(),
        }
    }

    pub fn on_closed<'a>(&'a self) -> impl Future<Output = ()> + FusedFuture + 'static {
        match self {
            Self::Active(connection) => connection.on_closed().fuse(),
            _ => Fuse::terminated(),
        }
    }
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            State::Inactive => write!(f, "Inactive"),
            State::SettingUp => write!(f, "SettingUp"),
            State::TearingDown => write!(f, "TearingDown"),
            State::AwaitingRemote(_) => write!(f, "AwaitingRemote"),
            State::Active(active) => write!(f, "Active({:?})", active),
        }
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match &self {
            State::Inactive => "Inactive",
            State::SettingUp => "SettingUp",
            State::TearingDown => "TearingDown",
            State::AwaitingRemote(_) => "AwaitingRemote",
            State::Active(_) => "Active",
        };
        write!(f, "{}", state)
    }
}

impl Unit for State {
    type Data = inspect::Node;
    fn inspect_create(&self, parent: &inspect::Node, name: impl AsRef<str>) -> Self::Data {
        let mut node = parent.create_child(String::from(name.as_ref()));
        self.inspect_update(&mut node);
        node
    }

    fn inspect_update(&self, data: &mut Self::Data) {
        data.clear_recorded();
        data.record_string("status", &format!("{}", self));
        if let State::Active(active) = &self {
            let node = active.inspect_create(data, "parameters");
            data.record(node);
        }
    }
}

pub type InspectableState = IValue<State>;

#[cfg(test)]
mod tests {
    use super::*;

    use diagnostics_assertions::assert_data_tree;
    use fuchsia_bluetooth::types::PeerId;
    use fuchsia_inspect_derive::WithInspect;
    use {fidl_fuchsia_bluetooth as fidl_bt, fidl_fuchsia_bluetooth_bredr as bredr};

    #[fuchsia::test]
    async fn sco_state_inspect_tree() {
        let inspect = inspect::Inspector::default();

        let mut state =
            InspectableState::default().with_inspect(inspect.root(), "sco_connection").unwrap();
        // Default inspect tree.
        assert_data_tree!(inspect, root: {
            sco_connection: {
                status: "Inactive",
            }
        });

        state.iset(State::SettingUp);
        assert_data_tree!(inspect, root: {
            sco_connection: {
                status: "SettingUp",
            }
        });

        state.iset(State::AwaitingRemote(Box::pin(async { Err(ConnectError::Failed) })));
        assert_data_tree!(inspect, root: {
            sco_connection: {
                status: "AwaitingRemote",
            }
        });

        let params = bredr::ScoConnectionParameters {
            parameter_set: Some(bredr::HfpParameterSet::D1),
            air_coding_format: Some(fidl_bt::AssignedCodingFormat::Cvsd),
            air_frame_size: Some(60),
            io_bandwidth: Some(16000),
            io_coding_format: Some(fidl_bt::AssignedCodingFormat::LinearPcm),
            io_frame_size: Some(16),
            io_pcm_data_format: Some(fidl_fuchsia_hardware_audio::SampleFormat::PcmSigned),
            io_pcm_sample_payload_msb_position: Some(1),
            path: Some(bredr::DataPath::Offload),
            ..Default::default()
        };
        let (sco_proxy, _sco_stream) =
            fidl::endpoints::create_proxy_and_stream::<bredr::ScoConnectionMarker>();
        let connection = Connection::build(PeerId(1), params, sco_proxy);
        let vigil = Vigil::new(Active::new(&connection, None));
        state.iset(State::Active(vigil));
        assert_data_tree!(inspect, root: {
            sco_connection: {
                status: "Active",
                parameters: {
                    parameter_set: "D1",
                    air_coding_format: "Cvsd",
                    air_frame_size: 60u64,
                    io_bandwidth: 16000u64,
                    io_coding_format: "LinearPcm",
                    io_frame_size: 16u64,
                    io_pcm_data_format: "PcmSigned",
                    io_pcm_sample_payload_msb_position: 1u64,
                    path: "Offload",
                },
            }
        });

        state.iset(State::TearingDown);
        assert_data_tree!(inspect, root: {
            sco_connection: {
                status: "TearingDown",
            }
        });
    }
}
