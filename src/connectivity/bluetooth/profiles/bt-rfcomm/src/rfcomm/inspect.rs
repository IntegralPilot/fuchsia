// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_rfcomm::{DLCI, Role, ServerChannel};
use fuchsia_async as fasync;
use fuchsia_bluetooth::inspect::DataStreamInspect;
use fuchsia_inspect::{self as inspect, Property};
use fuchsia_inspect_derive::{AttachError, IValue, Inspect};
use windowed_stats::experimental::inspect::{InspectSender, InspectedTimeMatrix, TimeMatrixClient};
use windowed_stats::experimental::series::interpolation::ConstantSample;
use windowed_stats::experimental::series::statistic::Sum;
use windowed_stats::experimental::series::{SamplingProfile, TimeMatrix};

use crate::rfcomm::session::channel::FlowControlMode;
use crate::rfcomm::session::multiplexer::SessionParameters;

pub(crate) const FLOW_CONTROLLER: &str = "flow_controller";
pub(crate) const CREDIT_FLOW_CONTROL: &str = "Credit-Based";
const NO_FLOW_CONTROL: &str = "None";

/// Helper function that fulfills the role of the `Display` trait for the `Role` type.
fn role_to_display_str(role: Role) -> &'static str {
    match role {
        Role::Unassigned => "Unassigned",
        Role::Negotiating => "Negotiating",
        Role::Responder => "Responder",
        Role::Initiator => "Initiator",
    }
}

/// Tracks the data stream inspect stats for a channel.
/// Properties are tracked in both directions: data sent to the remote entity and data
/// received from the remote.
#[derive(Default)]
pub struct DuplexDataStreamInspect {
    inbound: DataStreamInspect,
    outbound: DataStreamInspect,

    /// Timeseries data for the inbound data path (bytes).
    rx_time_series: Option<InspectedTimeMatrix<u64>>,
    /// Timeseries data for the outbound data path (bytes).
    tx_time_series: Option<InspectedTimeMatrix<u64>>,
}

impl Inspect for &mut DuplexDataStreamInspect {
    fn iattach(self, parent: &inspect::Node, _name: impl AsRef<str>) -> Result<(), AttachError> {
        self.inbound.iattach(parent, "inbound_stream")?;
        self.outbound.iattach(parent, "outbound_stream")?;

        // Timeseries data is saved under the RX and TX stream nodes.
        let rx_client = TimeMatrixClient::new(self.inbound.node().clone_weak());
        let tx_client = TimeMatrixClient::new(self.outbound.node().clone_weak());

        // A balanced sampling profile optimizes for bursty traffic resolution and memory
        // efficiency over extended durations.
        let rx_matrix = TimeMatrix::<Sum<u64>, ConstantSample>::new(
            SamplingProfile::balanced(),
            ConstantSample::new(0u64),
        );
        let tx_matrix = TimeMatrix::<Sum<u64>, ConstantSample>::new(
            SamplingProfile::balanced(),
            ConstantSample::new(0u64),
        );

        self.rx_time_series = Some(rx_client.inspect_time_matrix("timeseries_bytes", rx_matrix));
        self.tx_time_series = Some(tx_client.inspect_time_matrix("timeseries_bytes", tx_matrix));

        Ok(())
    }
}

impl DuplexDataStreamInspect {
    pub fn start(&mut self) {
        self.inbound.start();
        self.outbound.start();
    }

    pub fn record_inbound_transfer(&mut self, bytes: usize, at: fasync::MonotonicInstant) {
        self.inbound.record_transferred(bytes, at);
        if let Some(matrix) = &mut self.rx_time_series {
            matrix.fold_or_log_error(bytes as u64);
        }
    }

    pub fn record_outbound_transfer(&mut self, bytes: usize, at: fasync::MonotonicInstant) {
        self.outbound.record_transferred(bytes, at);
        if let Some(matrix) = &mut self.tx_time_series {
            matrix.fold_or_log_error(bytes as u64);
        }
    }

    #[cfg(test)]
    pub fn set_time_series_for_test(
        &mut self,
        rx: InspectedTimeMatrix<u64>,
        tx: InspectedTimeMatrix<u64>,
    ) {
        self.rx_time_series = Some(rx);
        self.tx_time_series = Some(tx);
    }
}

/// An inspect node that represents information about the current state of a Session Channel.
#[derive(Default, Debug, Inspect)]
pub struct SessionChannelInspect {
    /// The DLCI of this channel.
    dlci: inspect::UintProperty,
    /// Server channel number assigned to the RFCOMM channel.
    server_channel: inspect::UintProperty,
    /// The initial local credit amount (if applicable).
    initial_local_credits: IValue<Option<u64>>,
    /// The initial remote credit amount (if applicable).
    initial_remote_credits: IValue<Option<u64>>,
    /// The negotiated maximum frame size for this channel.
    max_packet_size: IValue<Option<u16>>,
    inspect_node: inspect::Node,
}

impl SessionChannelInspect {
    pub fn node(&self) -> &inspect::Node {
        &self.inspect_node
    }

    pub fn set_dlci(&mut self, dlci: DLCI) {
        self.dlci.set(u8::from(dlci) as u64);
        if let Ok(channel_number) = ServerChannel::try_from(dlci) {
            self.server_channel.set(u8::from(channel_number) as u64);
        }
    }

    pub fn set_parameters(&mut self, size: u16, flow_control: FlowControlMode) {
        self.max_packet_size.iset(Some(size));
        match flow_control {
            FlowControlMode::CreditBased(credits) => {
                self.initial_local_credits.iset(Some(credits.local() as u64));
                self.initial_remote_credits.iset(Some(credits.remote() as u64));
            }
            FlowControlMode::None => {
                self.initial_local_credits.iset(None);
                self.initial_remote_credits.iset(None);
            }
        }
    }
}

/// An inspect node that represents information about the current state of the Session Multiplexer.
#[derive(Default, Debug)]
pub struct SessionMultiplexerInspect {
    /// The current role of the multiplexer.
    role: inspect::StringProperty,
    /// The flow control parameter of the multiplexer.
    flow_control: inspect::StringProperty,
    /// The default maximum frame size parameter of the multiplexer.
    default_max_packet_size: inspect::UintProperty,
    inspect_node: inspect::Node,
}

impl Inspect for &mut SessionMultiplexerInspect {
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        self.inspect_node = parent.create_child(name.as_ref());
        self.role = self.inspect_node.create_string("role", role_to_display_str(Role::Unassigned));
        Ok(())
    }
}

impl SessionMultiplexerInspect {
    /// Set the role in inspect. This should only be called when the role changes.
    pub fn set_role(&mut self, role: Role) {
        self.role.set(role_to_display_str(role));
    }

    pub fn set_default_max_packet_size(&mut self, max_packet_size: u16) {
        self.default_max_packet_size =
            self.inspect_node.create_uint("default_max_packet_size", max_packet_size as u64);
    }

    pub fn set_session_parameters(&mut self, parameters: SessionParameters) {
        let flow_control =
            if parameters.credit_based_flow { CREDIT_FLOW_CONTROL } else { NO_FLOW_CONTROL };
        self.flow_control = self.inspect_node.create_string("flow_control", flow_control);
    }

    pub fn node(&self) -> &inspect::Node {
        &self.inspect_node
    }
}

/// An inspect node that represents information about the current state of the RFCOMM Session.
#[derive(Debug)]
pub struct SessionInspect {
    /// Whether the Session is currently connected to a peer.
    connected: inspect::StringProperty,
    inspect_node: inspect::Node,
}

impl Inspect for &mut SessionInspect {
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        self.inspect_node = parent.create_child(name.as_ref());
        self.connected = self.inspect_node.create_string("connected", "Connected");
        Ok(())
    }
}

impl SessionInspect {
    pub fn new() -> Self {
        Self {
            connected: inspect::StringProperty::default(),
            inspect_node: inspect::Node::default(),
        }
    }

    pub fn node(&self) -> &inspect::Node {
        &self.inspect_node
    }

    pub fn disconnect(&mut self) {
        self.connected.set("Disconnected");
    }
}

/// An inspect node that represents the RFCOMM server.
#[derive(Default, Debug)]
pub struct RfcommServerInspect {
    inspect_node: inspect::Node,
    peers: inspect::Node,
}

impl Inspect for &mut RfcommServerInspect {
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        self.inspect_node = parent.create_child(name.as_ref());
        self.peers = self.inspect_node.create_child("peers");
        Ok(())
    }
}

impl RfcommServerInspect {
    pub fn peers(&self) -> &inspect::Node {
        &self.peers
    }

    pub fn node(&self) -> &inspect::Node {
        &self.inspect_node
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_async::DurationExt;
    use fuchsia_inspect_derive::WithInspect;
    use windowed_stats::experimental::testing::{MockTimeMatrixClient, TimeMatrixCall};

    use crate::rfcomm::session::channel::Credits;

    #[fuchsia::test]
    async fn session_inspect_tree() {
        let inspect = inspect::Inspector::default();

        let mut session_inspect =
            SessionInspect::new().with_inspect(inspect.root(), "session").unwrap();

        // Default inspect tree.
        assert_data_tree!(inspect, root: {
            session: {
                connected: "Connected",
            }
        });

        // Inspect when disconnected.
        session_inspect.disconnect();
        assert_data_tree!(inspect, root: {
            session: {
                connected: "Disconnected",
            }
        });
    }

    #[fuchsia::test]
    async fn session_multiplexer_inspect_tree() {
        let inspect = inspect::Inspector::default();

        let mut multiplexer = SessionMultiplexerInspect::default()
            .with_inspect(inspect.root(), "multiplexer")
            .unwrap();

        // Default inspect tree.
        assert_data_tree!(inspect, root: {
            multiplexer: {
                role: "Unassigned",
            }
        });

        // Inspect with a different role and parameters.
        let parameters = SessionParameters { credit_based_flow: true };
        multiplexer.set_role(Role::Initiator);
        multiplexer.set_session_parameters(parameters);
        multiplexer.set_default_max_packet_size(99);
        assert_data_tree!(inspect, root: {
            multiplexer: {
                role: "Initiator",
                flow_control: CREDIT_FLOW_CONTROL,
                default_max_packet_size: 99u64,
            }
        });
    }

    #[fuchsia::test]
    async fn session_channel_inspect_tree() {
        let inspect = inspect::Inspector::default();
        let mut channel =
            SessionChannelInspect::default().with_inspect(inspect.root(), "channel").unwrap();

        // Default inspect tree.
        assert_data_tree!(inspect, root: {
            channel: {
                dlci: 0u64,
                server_channel: 0u64,
            }
        });

        // Inspect with a DLCI.
        channel.set_dlci(DLCI::try_from(8).unwrap());
        assert_data_tree!(inspect, root: {
            channel: {
                dlci: 8u64,
                server_channel: 4u64,
            }
        });

        // Parameters are set with credit flow control.
        channel.set_parameters(1024, FlowControlMode::CreditBased(Credits::new(10, 19)));
        assert_data_tree!(inspect, root: {
            channel: {
                dlci: 8u64,
                server_channel: 4u64,
                initial_local_credits: 10u64,
                initial_remote_credits: 19u64,
                max_packet_size: 1024u64,
            }
        });

        // Parameters are changed with no flow control, don't expect credits in the updated tree
        channel.set_parameters(500, FlowControlMode::None);
        assert_data_tree!(inspect, root: {
            channel: {
                dlci: 8u64,
                server_channel: 4u64,
                max_packet_size: 500u64,
            }
        });
    }

    #[fuchsia::test]
    fn duplex_data_stream_inspect_tree_updates_when_changed() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(1_234_567));

        let inspect = inspect::Inspector::default();
        let mut stream =
            DuplexDataStreamInspect::default().with_inspect(inspect.root(), "stream").unwrap();
        // Default inspect tree.
        assert_data_tree!(@executor exec, inspect, root: {
            inbound_stream: {
                bytes_per_second_current: 0u64,
                streaming_secs: 0u64,
                total_bytes: 0u64,
                timeseries_bytes: contains {},
            },
            outbound_stream: {
                bytes_per_second_current: 0u64,
                streaming_secs: 0u64,
                total_bytes: 0u64,
                timeseries_bytes: contains {},
            },
        });

        stream.start();
        // Both nodes should have same start_time.
        assert_data_tree!(@executor exec, inspect, root: {
            inbound_stream: {
                bytes_per_second_current: 0u64,
                start_time: 1_234_567i64,
                streaming_secs: 0u64,
                total_bytes: 0u64,
                timeseries_bytes: contains {},
            },
            outbound_stream: {
                bytes_per_second_current: 0u64,
                start_time: 1_234_567i64,
                streaming_secs: 0u64,
                total_bytes: 0u64,
                timeseries_bytes: contains {},
            },
        });

        exec.set_fake_time(zx::MonotonicDuration::from_seconds(1).after_now());
        // An inbound transfer should have no impact on the outbound stats.
        stream.record_inbound_transfer(500, fasync::MonotonicInstant::now());
        assert_data_tree!(@executor exec, inspect, root: {
            inbound_stream: {
                bytes_per_second_current: 500u64,
                start_time: 1_234_567i64,
                streaming_secs: 1u64,
                total_bytes: 500u64,
                timeseries_bytes: contains {},
            },
            outbound_stream: {
                bytes_per_second_current: 0u64,
                start_time: 1_234_567i64,
                streaming_secs: 0u64,
                total_bytes: 0u64,
                timeseries_bytes: contains {},
            },
        });

        exec.set_fake_time(zx::MonotonicDuration::from_seconds(1).after_now());
        stream.record_outbound_transfer(250, fasync::MonotonicInstant::now());
        assert_data_tree!(@executor exec, inspect, root: {
            inbound_stream: {
                bytes_per_second_current: 500u64, // 500 bytes in 1 second
                start_time: 1_234_567i64,
                streaming_secs: 1u64,
                total_bytes: 500u64,
                timeseries_bytes: contains {},
            },
            outbound_stream: {
                bytes_per_second_current: 125u64, // 250 bytes in 2 seconds
                start_time: 1_234_567i64,
                streaming_secs: 2u64,
                total_bytes: 250u64,
                timeseries_bytes: contains {},
            },
        });
    }

    #[fuchsia::test]
    fn time_series_recording() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(1_000_000));
        let inspect = inspect::Inspector::default();

        let mut stream =
            DuplexDataStreamInspect::default().with_inspect(inspect.root(), "stream").unwrap();
        stream.start();

        let mock_client = MockTimeMatrixClient::new();

        let rx_matrix = TimeMatrix::<Sum<u64>, ConstantSample>::new(
            SamplingProfile::balanced(),
            ConstantSample::new(0u64),
        );
        let tx_matrix = TimeMatrix::<Sum<u64>, ConstantSample>::new(
            SamplingProfile::balanced(),
            ConstantSample::new(0u64),
        );

        let mock_rx = mock_client.inspect_time_matrix("rx_test_bytes", rx_matrix);
        let mock_tx = mock_client.inspect_time_matrix("tx_test_bytes", tx_matrix);
        stream.set_time_series_for_test(mock_rx, mock_tx);

        stream.record_inbound_transfer(500, fasync::MonotonicInstant::now());
        exec.set_fake_time(zx::MonotonicDuration::from_seconds(1).after_now());
        stream.record_outbound_transfer(250, fasync::MonotonicInstant::now());

        let mut log = mock_client.drain_calls();
        let rx_calls = log.drain::<u64>("rx_test_bytes");
        let tx_calls = log.drain::<u64>("tx_test_bytes");

        assert_eq!(rx_calls.len(), 1);
        assert_matches!(rx_calls[0], TimeMatrixCall::Fold(timed) if *timed.inner() == 500);

        assert_eq!(tx_calls.len(), 1);
        assert_matches!(tx_calls[0], TimeMatrixCall::Fold(timed) if *timed.inner() == 250);

        assert_data_tree!(@executor exec, inspect, root: {
            inbound_stream: contains {
                total_bytes: 500u64,
            },
            outbound_stream: contains {
                total_bytes: 250u64,
            },
        });
    }
}
