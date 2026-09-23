// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_rfcomm::frame::{Frame, UserData};
use bt_rfcomm::{DLCI, Role};
use fuchsia_async as fasync;
use fuchsia_bluetooth::types::Channel;
use fuchsia_inspect as inspect;
use fuchsia_inspect_derive::{AttachError, IValue, Inspect};
use futures::channel::{mpsc, oneshot};
use futures::future::{BoxFuture, Fuse, Shared};
use futures::{FutureExt, Sink, SinkExt, Stream, StreamExt, select};
use log::{info, trace, warn};
use std::collections::VecDeque;
use std::pin::pin;
use zx;

use crate::rfcomm::inspect::{DuplexDataStreamInspect, FLOW_CONTROLLER, SessionChannelInspect};
use crate::rfcomm::types::{Error, SignaledTask};

/// Upper bound for the number of credits we allow a remote device to have.
/// This is used to determine when to refresh the remote credit count to limit the number of
/// pending RX packets.
/// This value is arbitrarily chosen and not derived from the GSM or RFCOMM specifications.
// TODO(https://fxbug.dev/428738649): Consider setting this dynamically based on negotiated MTU.
const HIGH_CREDIT_WATERMARK: usize = 16;

/// Threshold indicating that the number of credits is low on a remote device.
/// This is used as an indicator to send an empty frame to replenish the remote's credits.
/// This value is arbitrarily chosen and not derived from the GSM or RFCOMM specifications.
// TODO(https://fxbug.dev/428738649): Consider setting this dynamically based on negotiated MTU.
const LOW_CREDIT_WATERMARK: usize = 4;

/// Reads data received from the remote peer and forwards to the flow controller to be sent
/// to the RFCOMM application.
async fn rx_task(
    dlci: DLCI,
    mut data_from_flow_controller: mpsc::Receiver<UserData>,
    mut application_writer: impl Sink<Vec<u8>, Error = zx::Status> + Send + Unpin + 'static,
) {
    trace!(dlci:%; "RFCOMM RX task started");

    while let Some(data) = data_from_flow_controller.next().await {
        // The flow controller task should forward user data. Attempt to write it to the
        // application.
        if let Err(e) = application_writer.send(data.information).await {
            warn!(dlci:%; "Failed to send data to the application: {e:?}");
            break;
        }
    }

    trace!(dlci:%; "RFCOMM RX task finished");
}

/// Reads data from the RFCOMM application and forwards to the flow controller to be sent
/// to the remote peer.
async fn tx_task(
    dlci: DLCI,
    max_packet_size: usize,
    mut client_reader: impl Stream<Item = Result<Vec<u8>, zx::Status>> + Send + Unpin + 'static,
    mut tx_to_flow_controller_sender: mpsc::Sender<UserData>,
) {
    trace!(dlci:%; "RFCOMM TX task started");
    while let Some(result) = client_reader.next().await {
        match result {
            Ok(bytes) => {
                if bytes.len() > max_packet_size {
                    warn!(dlci:%; "Received data size {} exceeds max packet size {}", bytes.len(), max_packet_size);
                }
                let user_data = UserData { information: bytes };
                if let Err(e) = tx_to_flow_controller_sender.send(user_data).await {
                    // Can occur if the main flow controller task terminates unexpectedly.
                    warn!(dlci:%; "Error sending TX data to flow controller: {e:?}");
                    break;
                }
            }
            Err(e) => {
                warn!(dlci:%; "Failed to read from client channel: {e:?}");
                break;
            }
        }
    }
    trace!(dlci:%; "RFCOMM TX task finished");
}

/// An interface for implementing a data proxy between a remote Bluetooth peer and an RFCOMM
/// application. Flow controllers typically manage the RX and TX data paths for an active RFCOMM
/// channel. The amount of internal bookkeeping depends on the type of flow controller.
trait FlowController: Send + 'static {
    /// Start the RFCOMM channel relay.
    /// `tx_to_flow_controller_receiver` is the internal channel containing data received from the
    /// application intended for the peer.
    /// `data_from_peer_receiver` is the incoming stream of packets from the peer.
    /// `flow_controller_to_rx_sender` is the internal channel containing data received from the
    /// peer intended for the application.
    /// `data_to_peer_sender` is used to send packets to the peer.
    fn run(
        self: Box<Self>,
        tx_to_flow_controller_receiver: mpsc::Receiver<UserData>,
        data_from_peer_receiver: mpsc::UnboundedReceiver<FlowControlledData>,
        flow_controller_to_rx_sender: mpsc::Sender<UserData>,
        data_to_peer_sender: mpsc::Sender<Frame>,
    ) -> BoxFuture<'static, ()>;
}

/// Tracks the credits for each device connected to a given RFCOMM channel.
#[derive(Debug, Default, Inspect)]
pub struct Credits {
    /// The local credits that we currently have. Determines the number of frames
    /// we can send to the remote peer.
    #[inspect(rename = "local_credits")]
    local: IValue<usize>,
    /// The remote peer's current credit count. Determines the number of frames the
    /// remote may send to us.
    #[inspect(rename = "remote_credits")]
    remote: IValue<usize>,
}

impl Credits {
    pub fn new(local: usize, remote: usize) -> Self {
        Self { local: local.into(), remote: remote.into() }
    }

    /// Makes a copy of the current credit counts because IValue<T> does not derive Clone.
    fn as_new(&self) -> Self {
        Self::new(*self.local, *self.remote)
    }

    pub fn local(&self) -> usize {
        *self.local
    }

    pub fn remote(&self) -> usize {
        *self.remote
    }

    /// Increases the local credit amount by `credits`.
    fn replenish_local(&mut self, credits: usize) {
        self.local.iset(*self.local + credits);
    }

    /// Increases the remote credit amount by `credits`.
    fn replenish_remote(&mut self, credits: usize) {
        self.remote.iset(*self.remote + credits);
    }

    /// Reduces the local credit count by 1.
    fn decrement_local(&mut self) {
        self.local.iset(self.local.checked_sub(1).unwrap_or_default());
    }

    /// Reduces the remote credit count by 1.
    fn decrement_remote(&mut self) {
        self.remote.iset(self.remote.checked_sub(1).unwrap_or_default());
    }
}

/// A flow controller that indiscriminantly relays frames between a remote peer and an RFCOMM
/// requesting client. There is no flow control.
struct SimpleController {
    role: Role,
    dlci: DLCI,
    /// Inspect object for tracking total bytes sent/received & the current transfer speeds.
    stream_inspect: DuplexDataStreamInspect,
    /// The inspect node for this controller.
    inspect: inspect::Node,
}

impl Inspect for &mut SimpleController {
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        self.inspect = parent.create_child(name.as_ref());
        self.inspect.record_string("controller_type", "simple");
        self.stream_inspect.iattach(&self.inspect, "data_stream")?;
        self.stream_inspect.start();
        Ok(())
    }
}

impl SimpleController {
    fn new(role: Role, dlci: DLCI) -> Self {
        Self {
            role,
            dlci,
            stream_inspect: DuplexDataStreamInspect::default(),
            inspect: inspect::Node::default(),
        }
    }
}

impl FlowController for SimpleController {
    fn run(
        self: Box<Self>,
        mut tx_to_flow_controller_receiver: mpsc::Receiver<UserData>,
        mut data_from_peer_receiver: mpsc::UnboundedReceiver<FlowControlledData>,
        mut flow_controller_to_rx_sender: mpsc::Sender<UserData>,
        mut data_to_peer_sender: mpsc::Sender<Frame>,
    ) -> BoxFuture<'static, ()> {
        async move {
            trace!(dlci:% = self.dlci; "Starting simple flow controller task");
            loop {
                select! {
                    app_data = tx_to_flow_controller_receiver.select_next_some() => {
                        let user_data_frame =
                            Frame::make_user_data_frame(self.role, self.dlci, app_data, None);
                        if let Err(e) = data_to_peer_sender.send(user_data_frame).await {
                            warn!(dlci:% = self.dlci; "Couldn't forward application data to peer: {e:?}");
                            break;
                        };
                    }
                    peer_data = data_from_peer_receiver.select_next_some() => {
                        if let Err(e) = flow_controller_to_rx_sender.send(peer_data.user_data).await {
                            warn!(dlci:% = self.dlci; "Couldn't forward peer data to application: {e:?}");
                            break;
                        }
                    }
                    complete => break,
                }
            }
            trace!(dlci:% = self.dlci; "Simple flow controller task finished");
        }
        .boxed()
    }
}

/// Handles the credit-based flow control scheme defined in RFCOMM 6.5.
///
/// Provides an API to send and receive frames between the remote peer and a local
/// RFCOMM application. Uses a credit based system to control the flow of data. In general,
/// when credit-based flow control is in use, each frame contains an extra credit octet,
/// signaling the amount of credits the sending device is "giving" to the receiving device.
///
/// When the remote's credit count is low (i.e. the remote will soon need more credits
/// before they can send frames), we preemptively send an empty user-data frame with a
/// replenished credit amount. See the last paragraph of RFCOMM 6.5 which states:
/// "...always allowed to send frames containing no user data (length field = 0)".
/// This flow controller implements backpressure by withholding remote credits if the RFCOMM
/// application is slow to process data.
// TODO(https://fxbug.dev/371346522): Consider adding inspect for the RX & TX queues.
struct CreditFlowController {
    role: Role,
    dlci: DLCI,
    /// The current credit count for this controller.
    credits: Credits,
    /// Data from the peer intended for the application that is waiting to be sent.
    /// For backpressure on the RX datapath.
    rx_queue: VecDeque<UserData>,
    /// Data from the application intended for the remote peer that is awaiting local credits.
    /// For backpressure on the TX datapath.
    tx_queue: VecDeque<UserData>,
    /// Inspect object for tracking total bytes sent/received & the current transfer speeds.
    stream_inspect: DuplexDataStreamInspect,
    /// The inspect node for this controller.
    inspect: inspect::Node,
}

impl Inspect for &mut CreditFlowController {
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        self.inspect = parent.create_child(name.as_ref());
        self.inspect.record_string("controller_type", "credit_flow");
        self.credits.iattach(&self.inspect, "credits")?;
        self.stream_inspect.iattach(&self.inspect, "data_stream")?;
        self.stream_inspect.start();
        Ok(())
    }
}

impl CreditFlowController {
    fn new(role: Role, dlci: DLCI, initial_credits: Credits) -> Self {
        trace!(dlci:%, initial_credits:?; "Creating credit flow controller");
        Self {
            role,
            dlci,
            rx_queue: VecDeque::with_capacity(HIGH_CREDIT_WATERMARK),
            tx_queue: VecDeque::with_capacity(HIGH_CREDIT_WATERMARK),
            credits: initial_credits,
            stream_inspect: DuplexDataStreamInspect::default(),
            inspect: inspect::Node::default(),
        }
    }

    /// Returns true if there are sufficient credits to send at least one queued packet.
    fn can_send_queued(&self) -> bool {
        self.credits.local() > 0 && !self.tx_queue.is_empty()
    }

    fn is_rx_empty(&self) -> bool {
        self.rx_queue.is_empty()
    }

    /// Returns true if the RX datapath is congested and cannot accept more data.
    fn is_rx_congested(&self) -> bool {
        self.rx_queue.len() >= HIGH_CREDIT_WATERMARK
    }

    /// Returns true if the TX datapath is congested and cannot accept more data.
    fn is_tx_congested(&self) -> bool {
        self.tx_queue.len() >= HIGH_CREDIT_WATERMARK
    }

    fn remote_credit_count_low(&self) -> bool {
        self.credits.remote() <= LOW_CREDIT_WATERMARK
    }

    /// Returns the number of credits to replenish the remote with. Returns None if the remote
    /// has sufficient credits and does not need any more.
    fn remote_replenish_amount(&self) -> Option<u8> {
        if self.is_rx_congested() {
            return None;
        }

        if !self.remote_credit_count_low() {
            return None;
        }

        // The number of issued credits is contingent on space available in the RX queue.
        let capacity = HIGH_CREDIT_WATERMARK.saturating_sub(self.rx_queue.len());
        let granted = capacity.saturating_sub(self.credits.remote());
        if granted == 0 { None } else { granted.try_into().ok() }
    }

    /// Returns a user Frame with the replenished remote credit count.
    /// NOTE: Assumes there is at least one local credit to send the frame.
    fn make_frame(&mut self, data: UserData) -> Frame {
        let data_size = data.information.len();
        let credits_to_replenish = self.remote_replenish_amount();
        let frame = Frame::make_user_data_frame(self.role, self.dlci, data, credits_to_replenish);
        // User data frames containing a nonempty payload require a local credit to be sent.
        if data_size != 0 {
            self.credits.decrement_local();
            self.stream_inspect
                .record_outbound_transfer(data_size, fasync::MonotonicInstant::now());
        }
        // Update our bookkeeping for the remote credit count with what we gave to the peer.
        self.credits.replenish_remote(credits_to_replenish.map_or(0, Into::into));
        frame
    }

    /// Attempts to drain the TX queue and returns as many frames as there are credits for. If there
    /// are no pending frames, then potentially returns an empty frame to replenish remote credits.
    pub fn process_tx_queue(&mut self) -> Vec<Frame> {
        // Attempt to replenish the remote credit count if there are no queued frames.
        // Otherwise, credits will be issued with any outgoing TX packets.
        let maybe_credits = self.remote_replenish_amount();
        if !self.can_send_queued() && maybe_credits.is_some() {
            trace!(dlci:% = self.dlci; "Replenishing remote with {maybe_credits:?} credits");
            return vec![self.make_frame(UserData::empty())];
        }

        let mut frames = Vec::new();
        while self.can_send_queued() {
            let data = self.tx_queue.pop_front().expect("queue nonempty");
            // Internally updates the local credit count.
            frames.push(self.make_frame(data));
        }
        frames
    }

    /// Attempts to drain the next available packet in the RX queue.
    pub fn process_rx_queue(&mut self) -> Option<UserData> {
        self.rx_queue.pop_front()
    }

    /// Processes an outgoing user data packet to be sent to the remote peer.
    pub fn handle_outgoing_frame(&mut self, user_data: UserData) {
        self.tx_queue.push_back(user_data);
    }

    /// Processes an incoming frame received from the remote peer.
    pub fn handle_incoming_frame(&mut self, frame: FlowControlledData) {
        let FlowControlledData { user_data, credits } = frame;
        let num_bytes = user_data.information.len();

        self.credits.replenish_local(credits.map_or(0, Into::into));
        if num_bytes == 0 {
            trace!(dlci:% = self.dlci; "Remote replenished local with {credits:?} credits");
            return;
        }

        // If provided, stage the user data and update the remote credit count.
        self.rx_queue.push_back(user_data);
        self.credits.decrement_remote();
        self.stream_inspect.record_inbound_transfer(num_bytes, fasync::MonotonicInstant::now());
    }
}

impl FlowController for CreditFlowController {
    fn run(
        mut self: Box<Self>,
        mut tx_to_flow_controller_receiver: mpsc::Receiver<UserData>,
        mut data_from_peer_receiver: mpsc::UnboundedReceiver<FlowControlledData>,
        mut flow_controller_to_rx_sender: mpsc::Sender<UserData>,
        mut data_to_peer_sender: mpsc::Sender<Frame>,
    ) -> BoxFuture<'static, ()> {
        let dlci = self.dlci;
        let mut send_frames_to_peer_fn = move |frames: Vec<Frame>| {
            for frame in frames {
                if let Err(e) = data_to_peer_sender.try_send(frame) {
                    warn!(dlci:%; "Couldn't forward application data to peer: {e:?}");
                    return true;
                }
            }
            false
        };

        async move {
            trace!(dlci:%; "Starting credit-based flow controller task");

            loop {
                // Only send the flow controller's RX packets if the RX task has space.
                let mut rx_task_ready_fut = pin!(if self.is_rx_empty() {
                    Fuse::terminated()
                } else {
                    futures::future::poll_fn(|cx| flow_controller_to_rx_sender.poll_ready(cx))
                        .fuse()
                });

                // Stop processing TX packets if the flow controller's TX buffer is full.
                let mut tx_task_ready_fut = pin!(if self.is_tx_congested() {
                    Fuse::terminated()
                } else {
                    tx_to_flow_controller_receiver.next().fuse()
                });

                select! {
                    peer_data = data_from_peer_receiver.select_next_some().fuse() => {
                        // Process the received frame for the user data payload and replenished credits.
                        self.handle_incoming_frame(peer_data);

                        // Attempt to relay any TX frames that were blocked on local credits.
                        if send_frames_to_peer_fn(self.process_tx_queue()) {
                            break;
                        }
                    },
                    app_data_result = tx_task_ready_fut => {
                        let Some(user_data) = app_data_result else {
                            trace!(dlci:%; "TX task closed");
                            break;
                        };

                        // Attempt to relay the frame to the remote peer. The data will be staged if
                        // there are insufficient local credits.
                        self.handle_outgoing_frame(user_data);
                        if send_frames_to_peer_fn(self.process_tx_queue()) {
                            break;
                        }
                    },
                    result = rx_task_ready_fut => {
                        if let Err(e) = result {
                            warn!(dlci:%; "RX task closed: {e:?}");
                            break;
                        }
                        // Relay the payload to the RX task to be forwarded to the application.
                        // `rx_task_ready_fut` guarantees that there is space in the channel
                        let data = self.process_rx_queue().expect("checked RX queue nonempty");
                        let _ = flow_controller_to_rx_sender.try_send(data);

                        // Potentially refresh remote credits as there is space in the RX queue.
                        if send_frames_to_peer_fn(self.process_tx_queue()) {
                            break;
                        }
                    }
                    complete => break,
                }
            }
            trace!(dlci:%; "Credit-based flow controller task finished");
        }
        .boxed()
    }
}

impl Drop for CreditFlowController {
    fn drop(&mut self) {
        info!(dlci:% = self.dlci; "CreditFlowController closed. Dropping {} RX frames and {} TX fames", self.rx_queue.len(), self.tx_queue.len());
    }
}

/// The flow control method for the channel.
#[derive(Debug)]
pub enum FlowControlMode {
    /// Credit-based flow control with the provided initial credits.
    CreditBased(Credits),
    /// No flow control is being used.
    None,
}

impl FlowControlMode {
    /// Makes a copy of the FlowControlMode.
    fn as_new(&self) -> Self {
        match self {
            Self::CreditBased(credits) => Self::CreditBased(credits.as_new()),
            Self::None => Self::None,
        }
    }

    pub fn initial_remote_credits(&self) -> Option<u8> {
        match self {
            Self::CreditBased(credits) => Some(credits.remote() as u8),
            Self::None => None,
        }
    }
}

/// User data frames with optional credits to be used for flow control.
pub struct FlowControlledData {
    pub user_data: UserData,
    pub credits: Option<u8>,
}

impl FlowControlledData {
    #[cfg(test)]
    pub fn new(data: Vec<u8>, credits: u8) -> Self {
        Self { user_data: UserData { information: data }, credits: Some(credits) }
    }

    #[cfg(test)]
    pub fn new_no_credits(data: Vec<u8>) -> Self {
        Self { user_data: UserData { information: data }, credits: None }
    }
}

/// The processing task associated with an established SessionChannel.
struct EstablishedChannelTask {
    /// The active processing task managing the RX and TX datapaths for the RFCOMM channel.
    _task: fasync::Task<()>,
    /// Used to relay user data received from the peer to the active data relay `_task`.
    data_from_peer_sender: mpsc::UnboundedSender<FlowControlledData>,
    /// Indicates the termination status of the `_task`.
    terminated: Shared<BoxFuture<'static, ()>>,
}

/// Parameters associated with the RFCOMM channel. These are negotiated between the local device
/// and remote peer.
#[derive(Debug)]
struct SessionChannelParameters {
    /// The maximum packet size that can be sent on the channel.
    max_packet_size: u16,
    /// The flow control mode for the channel.
    flow_control: FlowControlMode,
}

/// The RFCOMM Channel associated with a DLCI for an RFCOMM Session.
/// The local end of the channel is held by the `bt-rfcomm` component and is used a means to forward
/// user data received from the remote peer.
/// The remote end of the channel is held by a profile or application that requires RFCOMM
/// functionality.
///
/// Typical usage looks like:
///   let (local, remote) = Channel::create();
///   let mut session_channel = SessionChannel::new(role, dlci);
///   session_channel.set_parameters(parameters);
///   session_channel.establish(..);
///   // Give remote to some client requesting RFCOMM.
///   pass_channel_to_rfcomm_client(remote);
///
///   session_channel.receive_user_data(FlowControlledData { user_data_buf1, credits1 }).await;
///   session_channel.receive_user_data(FlowControlledData { user_data_buf2, credits2 }).await;
pub struct SessionChannel {
    /// The DLCI associated with this channel.
    dlci: DLCI,
    /// The role assigned to the local RFCOMM session.
    role: Role,
    /// The parameters for this channel.
    parameters: Option<SessionChannelParameters>,
    /// The processing task that supports the RX & TX datapaths for the established channel.
    established_task: Option<EstablishedChannelTask>,
    /// The inspect node for this channel.
    inspect: SessionChannelInspect,
}

impl Inspect for &mut SessionChannel {
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        self.inspect.iattach(parent, name.as_ref())?;
        self.inspect.set_dlci(self.dlci);
        Ok(())
    }
}

impl SessionChannel {
    pub fn new(dlci: DLCI, role: Role) -> Self {
        Self {
            dlci,
            role,
            parameters: None,
            established_task: None,
            inspect: SessionChannelInspect::default(),
        }
    }

    /// Returns true if this SessionChannel has been established. Namely, `self.establish()`
    /// has been called there is an active processing task.
    pub fn is_established(&self) -> bool {
        self.finished().now_or_never().is_none()
    }

    /// Returns true if the parameters for this SessionChannel have been negotiated.
    pub fn parameters_negotiated(&self) -> bool {
        self.parameters.is_some()
    }

    pub fn max_packet_size(&self) -> Option<u16> {
        self.parameters.as_ref().map(|p| p.max_packet_size)
    }

    /// Sets the parameters for this channel.
    /// Returns an Error if the channel has already been established, as the flow control mode and
    /// max packet size cannot be changed after the channel has been opened and established.
    pub fn set_parameters(
        &mut self,
        max_packet_size: u16,
        flow_control: FlowControlMode,
    ) -> Result<(), Error> {
        if self.is_established() {
            return Err(Error::ChannelAlreadyEstablished(self.dlci));
        }

        self.parameters =
            Some(SessionChannelParameters { max_packet_size, flow_control: flow_control.as_new() });
        self.inspect.set_parameters(max_packet_size, flow_control);
        Ok(())
    }

    /// Establishes a credit-based flow control session for the provided RFCOMM channel.
    /// Returns a Task representing the active processing loops for the RX and TX datapaths.
    fn establish_with_controller(
        role: Role,
        dlci: DLCI,
        flow_controller: Box<dyn FlowController>,
        application_channel: Channel,
        mut frame_sender: mpsc::Sender<Frame>,
        data_from_peer_receiver: mpsc::UnboundedReceiver<FlowControlledData>,
        termination_sender: oneshot::Sender<()>,
    ) -> fasync::Task<()> {
        let max_packet_size = application_channel.max_tx_size();
        let (client_writer, client_reader) = application_channel.split();
        // TX Processing Task -> Flow Controller Task: Used to send TX data to the peer via the
        // flow controller.
        let (tx_to_flow_controller_sender, tx_to_flow_controller_receiver) = mpsc::channel(0);
        // Flow Controller Task -> RX Processing Task: Used to send RX data to the application.
        let (flow_controller_to_rx_sender, flow_controller_to_rx_receiver) = mpsc::channel(0);

        let tx_fut = tx_task(dlci, max_packet_size, client_reader, tx_to_flow_controller_sender);
        let rx_fut = rx_task(dlci, flow_controller_to_rx_receiver, client_writer);

        // The central flow control task that communicates with the RX and TX datapaths.
        let flow_controller_fut = flow_controller.run(
            tx_to_flow_controller_receiver,
            data_from_peer_receiver,
            flow_controller_to_rx_sender,
            frame_sender.clone(),
        );
        let data_relay_fut = async move {
            select! {
                _ = tx_fut.fuse() => {},
                _ = rx_fut.fuse() => {},
                _ = flow_controller_fut.fuse() => {},
            };
            // The RFCOMM channel tasks have terminated. This can be because the application closed
            // it, or the peer closed it, or an internal error. Regardless, attempt to send a
            // Disconnect frame to indicate that this channel no longer established.
            let disc = Frame::make_disc_command(role, dlci);
            let _ = frame_sender.send(disc).await;
            // RFCOMM channel has closed. Notify any listeners.
            let _ = termination_sender.send(());
        };
        fasync::Task::spawn(data_relay_fut)
    }

    /// Starts the processing task over the provided RFCOMM `channel`.
    /// While unusual, it is OK to call establish() multiple times. The currently active
    /// processing task will be terminated and a new task will be created with the provided
    /// `channel`.
    ///
    /// Returns an error if the channel parameters have not been negotiated.
    pub fn establish(
        &mut self,
        channel: Channel,
        data_to_peer_sender: mpsc::Sender<Frame>,
    ) -> Result<(), Error> {
        let (data_from_peer_sender, data_from_peer_receiver) = mpsc::unbounded();
        let (termination_sender, termination_receiver) = oneshot::channel();

        let channel_parameters =
            self.parameters.as_ref().ok_or(Error::ChannelParametersNotNegotiated(self.dlci))?;
        let flow_controller: Box<dyn FlowController> = match &channel_parameters.flow_control {
            FlowControlMode::None => {
                let mut simple_controller = SimpleController::new(self.role, self.dlci);
                let _ = simple_controller.iattach(self.inspect.node(), FLOW_CONTROLLER);
                Box::new(simple_controller)
            }
            FlowControlMode::CreditBased(credits) => {
                let mut credit_flow_controller =
                    CreditFlowController::new(self.role, self.dlci, credits.as_new());
                let _ = credit_flow_controller.iattach(self.inspect.node(), FLOW_CONTROLLER);
                Box::new(credit_flow_controller)
            }
        };

        let _task = Self::establish_with_controller(
            self.role,
            self.dlci,
            flow_controller,
            channel,
            data_to_peer_sender,
            data_from_peer_receiver,
            termination_sender,
        );
        let terminated = termination_receiver.map(|_| ()).boxed().shared();
        self.established_task =
            Some(EstablishedChannelTask { _task, data_from_peer_sender, terminated });
        trace!(dlci:% = self.dlci; "Started SessionChannel processing task");
        Ok(())
    }

    /// Receive a user data payload from the remote peer, which will be queued to be sent
    /// to the local profile client.
    pub async fn receive_user_data(&mut self, data: FlowControlledData) -> Result<(), Error> {
        if !self.is_established() {
            return Err(Error::ChannelNotEstablished(self.dlci));
        }

        // This unwrap is safe because the task is guaranteed to be set if the channel
        // has been established.
        let queue = &mut self.established_task.as_mut().unwrap().data_from_peer_sender;
        queue.send(data).await.map_err(|e| anyhow::format_err!("{e:?}").into())
    }
}

impl SignaledTask for SessionChannel {
    fn finished(&self) -> BoxFuture<'static, ()> {
        self.established_task.as_ref().map_or_else(
            || futures::future::ready(()).boxed(),
            |task| task.terminated.clone().boxed(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use async_test_helpers::run_while;
    use async_utils::PollExt;
    use bt_channel_test_support::{Transport, create_test_channels};
    use bt_rfcomm::frame::{FrameData, UIHData};
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_async::DurationExt;
    use fuchsia_inspect_derive::WithInspect;
    use fuchsia_sync::Mutex;
    use futures::task::Poll;
    use std::pin::{Pin, pin};
    use std::sync::Arc;
    use std::task::{Context, Waker};
    use test_case::test_case;

    use crate::rfcomm::test_util::{
        assert_user_data_frame, expect_frame, expect_user_data_frame, poll_stream,
    };

    fn establish_channel(
        session_channel: &mut SessionChannel,
        transport: Transport,
    ) -> (Channel, mpsc::Receiver<Frame>) {
        let (frame_sender, frame_receiver) = mpsc::channel(0);
        let (local, remote) = create_test_channels(transport);
        session_channel.establish(local, frame_sender).unwrap();
        (remote, frame_receiver)
    }

    /// Creates and establishes a SessionChannel. If provided, sets the flow control mode to
    /// `flow_control`.
    /// Returns the SessionChannel, the client end of the fuchsia_bluetooth::Channel, and a frame
    /// receiver that can be used to verify outgoing (i.e to the remote peer) frames being sent
    /// correctly.
    fn create_and_establish(
        role: Role,
        dlci: DLCI,
        flow_control: FlowControlMode,
        transport: Transport,
    ) -> (inspect::Inspector, SessionChannel, Channel, mpsc::Receiver<Frame>) {
        let inspect = inspect::Inspector::default();
        let mut session_channel =
            SessionChannel::new(dlci, role).with_inspect(inspect.root(), "channel_").unwrap();
        assert!(!session_channel.is_established());
        // Arbitrary max packet size since tests don't do any size specific things.
        assert!(session_channel.set_parameters(1000, flow_control).is_ok());

        let (remote, frame_receiver) = establish_channel(&mut session_channel, transport);
        assert!(session_channel.is_established());

        (inspect, session_channel, remote, frame_receiver)
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn establish_fails_if_parameters_not_set(transport: Transport) {
        let _exec = fasync::TestExecutor::new();
        let dlci = DLCI::try_from(8).unwrap();
        let mut session_channel = SessionChannel::new(dlci, Role::Responder);
        let (local, _remote) = create_test_channels(transport);
        let (frame_sender, _frame_receiver) = mpsc::channel(0);

        // Should fail because parameters haven't been set.
        assert_matches!(
            session_channel.establish(local, frame_sender),
            Err(Error::ChannelParametersNotNegotiated(_))
        );
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn set_parameters_after_termination(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let inspect = inspect::Inspector::default();
        let dlci = DLCI::try_from(8).unwrap();
        let mut session_channel = SessionChannel::new(dlci, Role::Responder)
            .with_inspect(inspect.root(), "channel_")
            .unwrap();
        let (local, remote) = create_test_channels(transport);
        let (frame_sender, mut frame_receiver) = mpsc::channel(0);

        // Set parameters and establish.
        let max_packet_size = 1000;
        assert!(session_channel.set_parameters(max_packet_size, FlowControlMode::None).is_ok());
        assert!(session_channel.establish(local, frame_sender).is_ok());
        assert!(session_channel.is_established());

        // The SessionChannel task should still be running.
        let mut finished = session_channel.finished();
        exec.run_until_stalled(&mut finished).expect_pending("task is active");

        // To terminate, we drop `remote` end which is held by some application.
        drop(remote);
        // Expect the outgoing DISCONNECT frame and the task should finish.
        let _frame = exec
            .run_until_stalled(&mut frame_receiver.next())
            .expect("future completed")
            .expect("frame received");
        exec.run_until_stalled(&mut finished).expect("task finished");
        assert!(!session_channel.is_established());

        // New, different parameters can be set.
        let new_max_packet_size = 512;
        let new_flow_control = FlowControlMode::CreditBased(Credits::new(5, 5));
        assert!(session_channel.set_parameters(new_max_packet_size, new_flow_control).is_ok());

        // Establish again.
        let (local2, _remote2) = create_test_channels(transport);
        let (frame_sender2, _frame_receiver2) = mpsc::channel(0);
        assert!(session_channel.establish(local2, frame_sender2).is_ok());
        assert!(session_channel.is_established());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn establish_succeeds_with_parameters(transport: Transport) {
        let _exec = fasync::TestExecutor::new();
        let dlci = DLCI::try_from(8).unwrap();
        let mut session_channel = SessionChannel::new(dlci, Role::Responder);
        let (local, _remote) = create_test_channels(transport);
        let (frame_sender, _frame_receiver) = mpsc::channel(0);

        assert!(session_channel.set_parameters(1024, FlowControlMode::None).is_ok());
        assert!(session_channel.establish(local, frame_sender).is_ok());
        assert!(session_channel.is_established());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn establish_channel_and_send_data_with_no_flow_control(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let role = Role::Responder;
        let dlci = DLCI::try_from(8).unwrap();
        let (_inspect, mut session_channel, mut client, mut outgoing_frames) =
            create_and_establish(role, dlci, FlowControlMode::None, transport);
        // Trying to change the flow control mode after establishment should fail.
        assert!(
            session_channel
                .set_parameters(400, FlowControlMode::CreditBased(Credits::default()))
                .is_err()
        );

        let data_received_by_client = client.next();
        let mut data_received_by_client = pin!(data_received_by_client);
        exec.run_until_stalled(&mut data_received_by_client).expect_pending("shouldn't have data");

        poll_stream(&mut exec, &mut outgoing_frames).expect_pending("shouldn't have frames");

        // Receive user data to be relayed to the profile-client - no credits.
        let user_data = vec![0x01, 0x02, 0x03];
        {
            let mut receive_fut = Box::pin(
                session_channel
                    .receive_user_data(FlowControlledData::new_no_credits(user_data.clone())),
            );
            assert_matches!(exec.run_until_stalled(&mut receive_fut), Poll::Ready(Ok(_)));
        }

        // Data should be relayed to the client.
        match exec.run_until_stalled(&mut data_received_by_client) {
            Poll::Ready(Some(Ok(buf))) => {
                assert_eq!(buf, user_data);
            }
            x => panic!("Expected ready with data but got: {:?}", x),
        }

        // Client responds with some user data.
        let client_data = vec![0xff, 0x00, 0xaa, 0x0bb];
        assert!(client.write(&client_data).is_ok());

        // Data should be processed by the SessionChannel, packed into an RFCOMM frame, and relayed
        // to peer.
        expect_user_data_frame(
            &mut exec,
            &mut outgoing_frames,
            UserData { information: client_data },
            None, // No credits since no flow control.
        );

        // Profile client no longer needs the RFCOMM channel - expect a Disc frame.
        drop(client);
        expect_frame(&mut exec, &mut outgoing_frames, FrameData::Disconnect, Some(dlci));
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(!session_channel.is_established());
    }

    /// Tests the SessionChannel relay with default flow control parameters
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn session_channel_with_default_credit_flow_control(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let role = Role::Responder;
        let dlci = DLCI::try_from(8).unwrap();
        let (_inspect, mut session_channel, mut client, mut outgoing_frames) = create_and_establish(
            role,
            dlci,
            FlowControlMode::CreditBased(Credits::default()),
            transport,
        );

        exec.run_until_stalled(&mut client.next()).expect_pending("no application data");
        poll_stream(&mut exec, &mut outgoing_frames).expect_pending("no outgoing frames");

        // Receive user data from remote peer be relayed to the profile-client. The peer issues 0
        // extra credits.
        let user_data = vec![0x01, 0x02, 0x03];
        {
            let mut receive_fut = Box::pin(
                session_channel.receive_user_data(FlowControlledData::new(user_data.clone(), 0)),
            );
            assert_matches!(exec.run_until_stalled(&mut receive_fut), Poll::Ready(Ok(_)));
        }

        // Because the remote's credit count is low (0, the default), we expect to immediately send
        // an outgoing empty data frame to replenish it.
        expect_user_data_frame(
            &mut exec,
            &mut outgoing_frames,
            UserData::empty(),
            Some(15), // HIGH_CREDIT_WATERMARK - 1 (pending RX frame)
        );
        // The data should be relayed to the client.
        let received_data2 = exec
            .run_until_stalled(&mut client.select_next_some())
            .expect("stream ready")
            .expect("received data");
        assert_eq!(received_data2, user_data);

        // Application wants to send data to the remote peer. We have no local credits so it should
        // be queued for later.
        let client_data = vec![0xaa, 0xcc, 0xee, 0xff, 0x00, 0x11, 0x12];
        assert!(client.write(&client_data).is_ok());
        poll_stream(&mut exec, &mut outgoing_frames).expect_pending("no outgoing frame");

        // Remote peer sends more user data with a refreshed credit amount.
        let user_data2 = vec![0x10, 0x01, 0x20, 0x02, 0x30, 0x03];
        {
            let mut receive_fut = Box::pin(
                session_channel.receive_user_data(FlowControlledData::new(user_data2.clone(), 50)),
            );
            assert_matches!(exec.run_until_stalled(&mut receive_fut), Poll::Ready(Ok(_)));
        }

        // The previously queued TX frame should be finally sent due to the credit refresh.
        expect_user_data_frame(
            &mut exec,
            &mut outgoing_frames,
            UserData { information: client_data },
            None, // Remote credit count is sufficiently high (15)
        );
        // The RX frame should also be forwarded to the application layer.
        let received_data2 = exec
            .run_until_stalled(&mut client.select_next_some())
            .expect("stream ready")
            .expect("received data");
        assert_eq!(received_data2, user_data2);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn session_channel_with_negotiated_credit_flow_control(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let role = Role::Responder;
        let dlci = DLCI::try_from(8).unwrap();
        let (_inspect, mut session_channel, mut client, mut outgoing_frames) = create_and_establish(
            role,
            dlci,
            FlowControlMode::CreditBased(Credits::new(
                12, // Arbitrary
                6,  // Arbitrary
            )),
            transport,
        );

        exec.run_until_stalled(&mut client.next()).expect_pending("shouldn't have data");
        poll_stream(&mut exec, &mut outgoing_frames).expect_pending("no frame");

        // Peer sends data to us. We expect it to be relayed to the upper layer profile.
        let user_data = vec![0x01, 0x02, 0x03];
        {
            let mut receive_fut =
                Box::pin(session_channel.receive_user_data(FlowControlledData::new(user_data, 6)));
            assert_matches!(exec.run_until_stalled(&mut receive_fut), Poll::Ready(Ok(_)));
            // Data should be relayed to the client.
            assert!(exec.run_until_stalled(&mut client.next()).is_ready());
            // Don't expect any outgoing empty frame to refresh remote credit count (5) as it is
            // still above the LOW_CREDIT_WATERMARK.
            poll_stream(&mut exec, &mut outgoing_frames).expect_pending("no frame");
        }

        // Client responds with some user data.
        let client_data = vec![0xff, 0x00, 0xaa];
        assert!(client.write(&client_data).is_ok());

        // Data should be processed by the SessionChannel, packed into an RFCOMM frame, and relayed
        // using the `frame_sender`.
        expect_user_data_frame(
            &mut exec,
            &mut outgoing_frames,
            UserData { information: client_data },
            None, // Remote credit count is sufficient (5), no credit refresh
        );

        // Peer sends more data. It should be relayed to the application. Because the remote credit
        // count is low, we expect to issue an empty frame to the peer with replenished credits.
        let user_data2 = vec![5, 6, 7, 8, 9, 10, 11, 12, 13];
        {
            let mut receive_fut = Box::pin(
                session_channel.receive_user_data(FlowControlledData::new_no_credits(user_data2)),
            );
            assert_matches!(exec.run_until_stalled(&mut receive_fut), Poll::Ready(Ok(_)));
            // Data should be relayed to the client.
            assert!(exec.run_until_stalled(&mut client.next()).is_ready());
            expect_user_data_frame(
                &mut exec,
                &mut outgoing_frames,
                UserData::empty(),
                Some(11), // MAX - 1 (RX queue size) - 4 (current remote credits)
            );
        }
    }

    /// Exercises the credit flow control path for normal `CreditFlowController` operation (low
    /// throughput).
    #[fuchsia::test]
    fn credit_flow_controller_normal_send_and_receive() {
        let _exec = fasync::TestExecutor::new();
        let credits = Credits::new(2, 4);
        let dlci = DLCI::try_from(11).unwrap();
        let mut flow_controller = CreditFlowController::new(Role::Initiator, dlci, credits);

        // Receive data from the peer with no credits (RX).
        let data1 = vec![0xaa, 0xbb, 0xcc, 0xdd, 0xee];
        flow_controller.handle_incoming_frame(FlowControlledData::new_no_credits(data1.clone()));
        let tx_frames = flow_controller.process_tx_queue();
        // Because the remote credit count is low, we expect to return an empty packet with a credit
        // refresh of 16 (max) - 1 (RX queue size) - 3 (current remote credits) = 12.
        assert_eq!(tx_frames.len(), 1);
        assert_user_data_frame(&tx_frames[0], UserData::empty(), Some(12));
        // The RX packet should be staged and the controller should have plenty of space.
        assert!(!flow_controller.is_rx_congested());
        let rx_user_data = flow_controller.process_rx_queue().expect("user data queued");
        assert_eq!(rx_user_data.information, data1);

        // Application attempts to write 4 packets. There are only 2 credits available so 2 packets
        // should be queued for later.
        let data2 = UserData { information: vec![0x1, 0x2, 0x3] };
        let data3 = UserData { information: vec![0x1, 0x1, 0x1, 0x1] };
        let data4 = UserData { information: vec![0x9, 0x8, 0x7] };
        let data5 = UserData { information: vec![0x4] };
        // Remote credit count is at 15 which is well above the low watermark.
        flow_controller.handle_outgoing_frame(data2.clone());
        let tx_frame2 = flow_controller.process_tx_queue();
        assert_user_data_frame(&tx_frame2[0], data2, None);
        flow_controller.handle_outgoing_frame(data3.clone());
        let tx_frame3 = flow_controller.process_tx_queue();
        assert_user_data_frame(&tx_frame3[0], data3, None);
        flow_controller.handle_outgoing_frame(data4.clone());
        assert!(flow_controller.process_tx_queue().is_empty());
        flow_controller.handle_outgoing_frame(data5.clone());
        assert!(flow_controller.process_tx_queue().is_empty());
        assert!(!flow_controller.is_tx_congested());

        // Peer gives us a single credit refresh. Expect a TX packet to be ready to be sent.
        flow_controller.handle_incoming_frame(FlowControlledData::new(vec![], 1));
        let tx_frame4 = flow_controller.process_tx_queue();
        assert_eq!(tx_frame4.len(), 1);
        // No credits issued because the remote credit count is still sufficiently high (15).
        assert_user_data_frame(&tx_frame4[0], data4, None);

        // Peer sends a few more packets with no credit refreshes, the single TX packet should
        // still be queued.
        let data6 = vec![0xaa, 0xbb, 0xcc, 0xdd, 0xee];
        for _i in 0..3 {
            flow_controller
                .handle_incoming_frame(FlowControlledData::new_no_credits(data6.clone()));
            let tx_frames = flow_controller.process_tx_queue();
            assert_eq!(tx_frames.len(), 0);
        }

        // Peer finally gives us more credits. Last TX packet should be sent.
        flow_controller.handle_incoming_frame(FlowControlledData::new(vec![], 5));
        let tx_frame5 = flow_controller.process_tx_queue();
        assert_eq!(tx_frame5.len(), 1);
        // No credits issued because the remote credit count is still sufficiently high (12).
        assert_user_data_frame(&tx_frame5[0], data5, None);
    }

    #[fuchsia::test]
    fn credit_flow_controller_high_throughput_backpressure() {
        let _exec = fasync::TestExecutor::new();
        // No local credits.
        let credits = Credits::new(0, 7);
        let dlci = DLCI::try_from(11).unwrap();
        let mut flow_controller = CreditFlowController::new(Role::Initiator, dlci, credits);
        assert!(!flow_controller.is_tx_congested());
        assert!(!flow_controller.is_rx_congested());

        // Application attempts to write many packets at once. All of them should be queued because
        // we have no local credits.
        let random_packet = vec![0x00, 0x02, 0x04, 0x06, 0x08, 0x10, 0x12];
        for _i in 0..HIGH_CREDIT_WATERMARK {
            flow_controller.handle_outgoing_frame(UserData { information: random_packet.clone() });
            assert!(flow_controller.process_tx_queue().is_empty());
        }
        // The entire TX buffer is full, we expect the controller to be in a congested state now.
        assert!(flow_controller.is_tx_congested());
        assert!(!flow_controller.is_rx_congested());

        // Can process RX frames as normal even if TX is congested.
        let peer_data = vec![1, 2, 3, 4, 5, 6, 7];
        flow_controller
            .handle_incoming_frame(FlowControlledData::new_no_credits(peer_data.clone()));
        assert!(flow_controller.process_tx_queue().is_empty());
        flow_controller
            .handle_incoming_frame(FlowControlledData::new_no_credits(peer_data.clone()));
        assert!(flow_controller.process_tx_queue().is_empty());
        // Another RX frame puts the remote credit count low. We expect to replenish even though TX
        // is congested. 16 (Max) - 3 (RX queue size) - 4 (current credit count)
        flow_controller.handle_incoming_frame(FlowControlledData::new_no_credits(peer_data));
        let tx_packet = flow_controller.process_tx_queue();
        assert_eq!(tx_packet.len(), 1);
        assert_user_data_frame(&tx_packet[0], UserData::empty(), Some(9));
        assert!(!flow_controller.is_rx_congested());

        // Peer now has 13 credits. It can fill up the buffer to put the controller in a congested
        // state.
        let random_peer_packet = vec![0, 5, 10, 15, 20];
        for _i in 0..13 {
            flow_controller.handle_incoming_frame(FlowControlledData::new_no_credits(
                random_peer_packet.clone(),
            ));
            assert!(flow_controller.process_tx_queue().is_empty());
        }
        // Flow controller is now congested in both directions. Both sides have 0 credits.
        assert!(flow_controller.is_rx_congested());
        assert!(flow_controller.is_tx_congested());

        // Peer finally refreshes our credit count. Should send as many frames as credits.
        flow_controller.handle_incoming_frame(FlowControlledData::new(vec![], 10));
        let tx_packets = flow_controller.process_tx_queue();
        assert_eq!(tx_packets.len(), 10);
        assert!(!flow_controller.is_tx_congested());
        assert!(flow_controller.is_rx_congested());

        // Slow application finally reads from the flow controller.
        for _i in 0..10 {
            assert!(flow_controller.process_rx_queue().is_some());
        }
        // There are still 6 queued RX packets, but we are no longer congested.
        assert!(!flow_controller.is_rx_congested());
        // Peer refreshes our credit count again. Should drain the rest of the TX packets (6).
        flow_controller.handle_incoming_frame(FlowControlledData::new(vec![], 10));
        let tx_packets = flow_controller.process_tx_queue();
        assert_eq!(tx_packets.len(), 6);
        for i in 0..6 {
            // The very first TX frame we send, we should refresh the remote with credits since RX is no
            // longer congested.
            // 16 (Max) - 6 (current RX queue) - 0 (current remote count) = 10
            let expected_credits = if i == 0 { Some(10) } else { None };
            assert_user_data_frame(
                &tx_packets[i],
                UserData { information: random_packet.clone() },
                expected_credits,
            );
        }
    }

    #[fuchsia::test]
    fn credit_flow_controller_run_sends_credit_update_when_rx_is_relieved() {
        let mut exec = fasync::TestExecutor::new();

        let dlci = DLCI::try_from(5).unwrap();
        let role = Role::Initiator;
        let credits = Credits::new(0, HIGH_CREDIT_WATERMARK);
        let controller = Box::new(CreditFlowController::new(role, dlci, credits));

        let (_tx_to_flow_controller_sender, tx_to_flow_controller_receiver) = mpsc::channel(0);
        let (data_from_peer_sender, data_from_peer_receiver) = mpsc::unbounded();
        let (flow_controller_to_rx_sender, mut flow_controller_to_rx_receiver) = mpsc::channel(0);
        let (data_to_peer_sender, mut data_to_peer_receiver) = mpsc::channel(100);

        let mut run_fut = controller.run(
            tx_to_flow_controller_receiver,
            data_from_peer_receiver,
            flow_controller_to_rx_sender,
            data_to_peer_sender,
        );

        // Peer sends enough packets to fill the RX queue.
        for i in 0..HIGH_CREDIT_WATERMARK {
            let data = FlowControlledData::new_no_credits(vec![i as u8]);
            data_from_peer_sender.unbounded_send(data).expect("remote peer can send");
        }

        // The run loop should process all incoming peer packets, and fill the RX queue.
        // It will then get stuck trying to send the first packet to the application, since
        // the application is not reading from the channel.
        exec.run_until_stalled(&mut run_fut).expect_pending("run loop active");
        // Expect an initial credit refresh because 1 packet was drained from the RX queue but not
        // delivered to the application.
        expect_user_data_frame(&mut exec, &mut data_to_peer_receiver, UserData::empty(), Some(1));

        // Application immediately sends another packet with its received credit.
        let data = FlowControlledData::new_no_credits(vec![16]);
        data_from_peer_sender.unbounded_send(data).expect("remote peer can send");
        // Don't expect a credit refresh because the RX path is completely congested.
        exec.run_until_stalled(&mut data_to_peer_receiver.next())
            .expect_pending("No outgoing frame");

        // Application finally reads the first packet - should free up space in the internal flow
        // controller RX queue.
        let received = exec
            .run_until_stalled(&mut flow_controller_to_rx_receiver.select_next_some())
            .expect("user data");
        assert_eq!(received, UserData { information: vec![0] });
        exec.run_until_stalled(&mut run_fut).expect_pending("run loop active");
        // Because the RX path is no longer congested, and the remote has 0 credits, we expect
        // an empty frame to be sent to replenish credits. Since only one packet was processed, we
        // expect to replenish the peer with one credit.
        expect_user_data_frame(&mut exec, &mut data_to_peer_receiver, UserData::empty(), Some(1));
    }

    #[fuchsia::test]
    fn credit_flow_controller_run_does_not_read_tx_task_when_congested() {
        let mut exec = fasync::TestExecutor::new();

        let dlci = DLCI::try_from(5).unwrap();
        let role = Role::Initiator;
        // No local credits initially, so any outgoing packets will be queued.
        let credits = Credits::new(0, 0);
        let controller = Box::new(CreditFlowController::new(role, dlci, credits));

        let (mut tx_to_flow_controller_sender, tx_to_flow_controller_receiver) = mpsc::channel(0);
        let (data_from_peer_sender, data_from_peer_receiver) = mpsc::unbounded();
        let (flow_controller_to_rx_sender, _flow_controller_to_rx_receiver) = mpsc::channel(0);
        let (data_to_peer_sender, _data_to_peer_receiver) = mpsc::channel(100);

        let mut run_fut = controller.run(
            tx_to_flow_controller_receiver,
            data_from_peer_receiver,
            flow_controller_to_rx_sender,
            data_to_peer_sender,
        );

        // Send enough packets from the application to fill the TX queue.
        for i in 0..HIGH_CREDIT_WATERMARK {
            let data = UserData { information: vec![i as u8] };
            let mut send_fut = pin!(tx_to_flow_controller_sender.send(data));
            exec.run_until_stalled(&mut send_fut).expect_pending("send waiting");
            exec.run_until_stalled(&mut run_fut).expect_pending("run loop active");
            let send_result = exec.run_until_stalled(&mut send_fut).expect("send finished");
            assert_matches!(send_result, Ok(_));
        }

        // The flow controller's tx_queue is full and should no longer be trying to read data from
        // the TX task.
        // Trying to send another packet should not complete.
        let data = UserData { information: vec![99] };
        let mut send_fut = tx_to_flow_controller_sender.send(data);
        exec.run_until_stalled(&mut send_fut).expect_pending("send blocked");
        exec.run_until_stalled(&mut run_fut).expect_pending("run loop still active");
        exec.run_until_stalled(&mut send_fut).expect_pending("send still blocked");

        // Peer issues an empty frame with a single credit. The flow controller should update and
        // attempt to send a queued TX packet. It is no longer congested and the previous write from
        // the TX task should complete.
        let empty_data = FlowControlledData::new(vec![], 1);
        data_from_peer_sender.unbounded_send(empty_data).expect("can send data to main task");
        exec.run_until_stalled(&mut run_fut).expect_pending("run loop still active");
        let send_result = exec.run_until_stalled(&mut send_fut).expect("send complete");
        assert_matches!(send_result, Ok(_));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn finished_signal_resolves_when_session_channel_dropped(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (_inspect, session_channel, _client, _outgoing_frames) = create_and_establish(
            Role::Initiator,
            DLCI::try_from(8).unwrap(),
            FlowControlMode::None,
            transport,
        );

        // Finished signal is pending while the channel is active.
        let mut finished = session_channel.finished();
        assert_matches!(exec.run_until_stalled(&mut finished), Poll::Pending);

        // SessionChannel is dropped (usually due to peer disconnection). Polling the finished
        // signal thereafter is OK - should resolve immediately.
        drop(session_channel);
        assert_matches!(exec.run_until_stalled(&mut finished), Poll::Ready(_));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn finished_signal_resolves_when_client_disconnects(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let dlci = DLCI::try_from(19).unwrap();
        let (_inspect, session_channel, _client, mut outgoing_frames) = create_and_establish(
            Role::Responder,
            dlci,
            FlowControlMode::CreditBased(Credits::new(0, 0)),
            transport,
        );

        // Finished signal is pending while the channel is active.
        let mut finished = session_channel.finished();
        assert_matches!(exec.run_until_stalled(&mut finished), Poll::Pending);

        // Client closes its end of the RFCOMM channel - expect the outgoing Disconnect frame.
        drop(_client);
        expect_frame(&mut exec, &mut outgoing_frames, FrameData::Disconnect, Some(dlci));
        // Polling the finished signal thereafter is OK - should resolve.
        assert_matches!(exec.run_until_stalled(&mut finished), Poll::Ready(_));
        assert!(!session_channel.is_established());

        // Checking again should resolve immediately.
        let mut finished = session_channel.finished();
        assert_matches!(exec.run_until_stalled(&mut finished), Poll::Ready(_));
    }

    #[fuchsia::test]
    fn finished_signal_before_establishment_resolves_immediately() {
        let mut exec = fasync::TestExecutor::new();

        let session_channel = SessionChannel::new(DLCI::try_from(19).unwrap(), Role::Initiator);
        // Checking termination multiple times is OK.
        assert!(!session_channel.is_established());
        let mut finished = session_channel.finished();
        assert_matches!(exec.run_until_stalled(&mut finished), Poll::Ready(_));
        assert!(!session_channel.is_established());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn finish_signal_resolves_with_multiple_establishments(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let dlci = DLCI::try_from(19).unwrap();
        let (_inspect, mut session_channel, _client, _outgoing_frames) = create_and_establish(
            Role::Responder,
            dlci,
            FlowControlMode::CreditBased(Credits::new(0, 0)),
            transport,
        );

        let mut finished = session_channel.finished();
        assert_matches!(exec.run_until_stalled(&mut finished), Poll::Pending);

        // Establishment occurs again.
        let (_remote, _frame_receiver) = establish_channel(&mut session_channel, transport);

        // Finished signal from previous establishment should resolve.
        assert_matches!(exec.run_until_stalled(&mut finished), Poll::Ready(_));
        // New signal should be pending still.
        let mut finished2 = session_channel.finished();
        assert_matches!(exec.run_until_stalled(&mut finished2), Poll::Pending);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    async fn simple_controller_send_and_receive(transport: Transport) {
        let dlci = DLCI::try_from(15).unwrap();
        let role = Role::Initiator;
        let controller = Box::new(SimpleController::new(role, dlci));

        let (mut application, local) = create_test_channels(transport);
        let (data_to_peer_sender, mut data_to_peer_receiver) = mpsc::channel(0);
        let (mut data_from_peer_sender, data_from_peer_receiver) = mpsc::unbounded();
        let (termination_sender, termination_receiver) = oneshot::channel();

        let _simple_controller_task = SessionChannel::establish_with_controller(
            role,
            dlci,
            controller,
            local,
            data_to_peer_sender,
            data_from_peer_receiver,
            termination_sender,
        );

        // Peer can send data. Should be relayed to the application immediately.
        let data1 = vec![0x00, 0x02, 0x04, 0x06];
        data_from_peer_sender
            .send(FlowControlledData::new_no_credits(data1.clone()))
            .await
            .expect("can send data");
        let received1 = application.select_next_some().await.expect("data received");
        assert_eq!(received1, data1);

        // Application can send data. Should be relayed to the peer immediately.
        let data2 = vec![0x5, 0x4, 0x3, 0x2, 0x1, 0x0];
        application.send(data2.clone()).await.expect("succesful write");
        let received2 = data_to_peer_receiver.select_next_some().await;
        assert_eq!(
            received2.data,
            FrameData::UnnumberedInfoHeaderCheck(UIHData::User(UserData { information: data2 }))
        );

        // Peer can send multiple chunks of data at a time.
        let data_chunks = [vec![0x22, 0x33, 0x44], vec![0x55, 0x66, 0x77], vec![0x88, 0x99, 0x00]];
        for chunk in &data_chunks {
            data_from_peer_sender
                .send(FlowControlledData::new_no_credits(chunk.clone()))
                .await
                .expect("can send data");
        }
        // Should be received by the application.
        for chunk in data_chunks {
            let received_chunk = application.select_next_some().await.expect("data received");
            assert_eq!(received_chunk, chunk);
        }

        // Application no longer needs the channel. We expect an outgoing Disconnect frame and
        // channel closure.
        drop(application);
        let received3 = data_to_peer_receiver.select_next_some().await;
        assert_eq!(received3.data, FrameData::Disconnect);
        // Any termination listeners should be notified.
        let () = termination_receiver.await.expect("termination signaled");
    }

    /// A mock `AsyncWrite` implementation that can simulate a full buffer.
    #[derive(Clone)]
    struct MockApplication(Arc<Mutex<MockApplicationInner>>);

    struct MockApplicationInner {
        write_buffer: VecDeque<Vec<u8>>,
        read_buffer: VecDeque<Vec<u8>>,
        /// The maximum capacity of the buffer.
        capacity: usize,
        /// The waker for the task that is pending on writing to this writer.
        write_waker: Option<Waker>,
        /// The waker for the task that is pending on reading from this reader.
        read_waker: Option<Waker>,
        /// Indicates whether the next operation should return error.
        should_error: bool,
    }

    impl MockApplication {
        fn new(capacity: usize) -> Self {
            Self(Arc::new(Mutex::new(MockApplicationInner {
                write_buffer: VecDeque::new(),
                read_buffer: VecDeque::new(),
                capacity,
                write_waker: None,
                read_waker: None,
                should_error: false,
            })))
        }

        pub fn write_buffer(&self) -> Vec<u8> {
            let copied = self.0.lock().write_buffer.clone();
            copied.into_iter().flatten().collect()
        }

        /// Empties the write buffer and wakes the writer waker.
        pub fn clear_and_wake_writer(&self) {
            let mut locked = self.0.lock();
            locked.write_buffer.clear();
            if let Some(waker) = locked.write_waker.take() {
                waker.wake();
            }
        }

        pub fn set_and_wake_reader(&self, data: Vec<u8>) {
            let mut locked = self.0.lock();
            locked.read_buffer.push_back(data);
            if let Some(waker) = locked.read_waker.take() {
                waker.wake();
            }
        }

        fn set_error(&self) {
            let mut locked = self.0.lock();
            locked.should_error = true;
            if let Some(waker) = locked.write_waker.take() {
                waker.wake();
            }
            if let Some(waker) = locked.read_waker.take() {
                waker.wake();
            }
        }
    }

    impl Sink<Vec<u8>> for MockApplication {
        type Error = zx::Status;

        fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            let mut locked = self.0.lock();
            if locked.should_error {
                return Poll::Ready(Err(zx::Status::PEER_CLOSED));
            }

            let write_buffer_len: usize = locked.write_buffer.iter().map(|v| v.len()).sum();
            if write_buffer_len >= locked.capacity {
                locked.write_waker = Some(cx.waker().clone());
                return Poll::Pending;
            }
            Poll::Ready(Ok(()))
        }

        fn start_send(self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
            let mut locked = self.0.lock();
            locked.write_buffer.push_back(item);
            Ok(())
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            let locked = self.0.lock();
            if locked.should_error {
                return Poll::Ready(Err(zx::Status::PEER_CLOSED));
            }
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            let locked = self.0.lock();
            if locked.should_error {
                return Poll::Ready(Err(zx::Status::PEER_CLOSED));
            }
            Poll::Ready(Ok(()))
        }
    }

    impl Stream for MockApplication {
        type Item = Result<Vec<u8>, zx::Status>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let mut locked = self.0.lock();
            if locked.should_error {
                return Poll::Ready(Some(Err(zx::Status::PEER_CLOSED)));
            }

            if let Some(data) = locked.read_buffer.pop_front() {
                if data.is_empty() {
                    return Poll::Ready(None);
                }
                return Poll::Ready(Some(Ok(data)));
            }

            locked.read_waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }

    #[fuchsia::test]
    fn rx_task_data_path_and_termination() {
        let mut exec = fasync::TestExecutor::new();

        let dlci = DLCI::try_from(5).unwrap();
        let (mut sender, receiver) = mpsc::channel(0);
        let writer_capacity = 10;
        let writer = MockApplication::new(writer_capacity);
        let mut rx_task_fut = pin!(rx_task(dlci, receiver, writer.clone()));

        // The task should be pending initially as there is no data.
        exec.run_until_stalled(&mut rx_task_fut).expect_pending("RX Task active");

        // Send some data. It should be written to the mock writer.
        let data1 = UserData { information: vec![1, 2, 3, 4, 5] };
        let mut send_fut1 = pin!(sender.send(data1));
        let (result1, mut rx_task_fut) = run_while(&mut exec, &mut rx_task_fut, &mut send_fut1);
        assert_matches!(result1, Ok(_));
        exec.run_until_stalled(&mut rx_task_fut).expect_pending("RX Task active");
        assert_eq!(writer.write_buffer().as_slice(), &[1, 2, 3, 4, 5]);

        // Send more data to fill up the writer's buffer.
        let data2 = UserData { information: vec![6, 7, 8, 9, 10] };
        let mut send_fut2 = pin!(sender.send(data2));
        let (result2, mut rx_task_fut) = run_while(&mut exec, &mut rx_task_fut, &mut send_fut2);
        assert_matches!(result2, Ok(_));
        exec.run_until_stalled(&mut rx_task_fut).expect_pending("RX Task active");
        // The writer buffer should be full.
        assert_eq!(writer.write_buffer().as_slice(), &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);

        // Sending more data should over the channel should succeed and cause the `send` in
        // the RX task to hang as it is full.
        let data3 = UserData { information: vec![11] };
        let mut send_fut3 = pin!(sender.send(data3.clone()));
        let (result3, mut rx_task_fut) = run_while(&mut exec, &mut rx_task_fut, &mut send_fut3);
        assert_matches!(result3, Ok(_));
        exec.run_until_stalled(&mut rx_task_fut).expect_pending("RX Task active");
        // `data3` shouldn't have been written
        assert_eq!(writer.write_buffer().as_slice(), &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);

        // Sending more data should not complete as the RX task is currently blocked.
        let data4 = UserData { information: vec![12, 13, 14, 15] };
        let mut send_fut4 = pin!(sender.send(data4.clone()));
        exec.run_until_stalled(&mut send_fut4).expect_pending("send blocked");
        exec.run_until_stalled(&mut rx_task_fut).expect_pending("RX Task active");
        exec.run_until_stalled(&mut send_fut4).expect_pending("send blocked");
        // `data4` shouldn't have been written
        assert_eq!(writer.write_buffer().as_slice(), &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);

        // Clear the writer buffer and wake the writer. Expect both `data3` and `data4` to be
        // written now that there is space for both.
        writer.clear_and_wake_writer();
        exec.run_until_stalled(&mut rx_task_fut).expect_pending("RX Task active");
        assert_eq!(writer.write_buffer().as_slice(), &[11, 12, 13, 14, 15]);

        // Close the sender channel. This should cause the `rx_task` to terminate because the
        // stream of data from the flow controller has ended.
        drop(sender);
        exec.run_until_stalled(&mut rx_task_fut).expect("RX Task finished");
    }

    #[fuchsia::test]
    fn rx_task_terminates_on_writer_error() {
        let mut exec = fasync::TestExecutor::new();

        let dlci = DLCI::try_from(5).unwrap();
        let (mut sender, receiver) = mpsc::channel(0);
        let writer_capacity = 10;
        let writer = MockApplication::new(writer_capacity);
        let mut rx_task_fut = pin!(rx_task(dlci, receiver, writer.clone()));
        exec.run_until_stalled(&mut rx_task_fut).expect_pending("RX Task active");

        // Make the next write fail.
        writer.set_error();

        let data = UserData { information: vec![1, 2, 3, 4, 5] };
        let mut send_fut = pin!(sender.send(data));
        exec.run_until_stalled(&mut send_fut).expect_pending("send blocked");
        exec.run_until_stalled(&mut rx_task_fut).expect("RX Task terminated");
        let result = exec.run_until_stalled(&mut send_fut).expect("send finished");
        assert_matches!(result, Ok(_));
    }

    #[fuchsia::test]
    fn tx_task_data_path_and_termination() {
        let mut exec = fasync::TestExecutor::new();
        let dlci = DLCI::try_from(5).unwrap();
        let (sender, mut receiver) = mpsc::channel(0);
        let reader = MockApplication::new(10);
        let mut tx_task_fut =
            pin!(tx_task(dlci, /*max_packet_size=*/ 1000, reader.clone(), sender));
        exec.run_until_stalled(&mut tx_task_fut).expect_pending("TX Task active");

        // Simulate the application writing data - should be received by the receiver.
        let data1 = vec![1, 2, 3, 4, 5];
        reader.set_and_wake_reader(data1.clone());
        exec.run_until_stalled(&mut tx_task_fut).expect_pending("TX Task active");
        let received1 =
            exec.run_until_stalled(&mut receiver.select_next_some()).expect("data received");
        assert_eq!(received1.information, data1);

        // Application writes multiple data packets before TX task can process it.
        let data2 = vec![6, 7, 8];
        reader.set_and_wake_reader(data2.clone());
        let data3 = vec![9, 10, 11, 12, 13];
        reader.set_and_wake_reader(data3.clone());
        let data4 = vec![14, 15, 16, 17, 18, 19, 20];
        reader.set_and_wake_reader(data4.clone());
        exec.run_until_stalled(&mut tx_task_fut).expect_pending("TX Task active");
        let received2 =
            exec.run_until_stalled(&mut receiver.select_next_some()).expect("data received");
        assert_eq!(received2.information, data2);
        exec.run_until_stalled(&mut tx_task_fut).expect_pending("TX Task active");
        let received3 =
            exec.run_until_stalled(&mut receiver.select_next_some()).expect("data received");
        assert_eq!(received3.information, data3);
        exec.run_until_stalled(&mut tx_task_fut).expect_pending("TX Task active");
        let received4 =
            exec.run_until_stalled(&mut receiver.select_next_some()).expect("data received");
        assert_eq!(received4.information, data4);

        // Application closes the channel (signaled by 0 byte write) - TX task should terminate.
        reader.set_and_wake_reader(vec![]);
        exec.run_until_stalled(&mut tx_task_fut).expect("TX Task finished");
    }

    #[fuchsia::test]
    fn tx_task_terminates_on_reader_error() {
        let mut exec = fasync::TestExecutor::new();
        let dlci = DLCI::try_from(5).unwrap();
        let (sender, _receiver) = mpsc::channel(0);
        let reader = MockApplication::new(10);
        let mut tx_task_fut =
            pin!(tx_task(dlci, /*max_packet_size=*/ 1000, reader.clone(), sender));
        exec.run_until_stalled(&mut tx_task_fut).expect_pending("TX Task active");

        // Next read should trigger an error.
        reader.set_error();
        reader.set_and_wake_reader(vec![1]); // Add data to trigger a read.
        // The task should terminate because of the read error.
        exec.run_until_stalled(&mut tx_task_fut).expect("TX Task finished");
    }

    #[fuchsia::test]
    fn tx_task_terminates_on_flow_controller_close() {
        let mut exec = fasync::TestExecutor::new();
        let dlci = DLCI::try_from(5).unwrap();
        let (sender, receiver) = mpsc::channel(0);
        let reader = MockApplication::new(10);
        let mut tx_task_fut =
            pin!(tx_task(dlci, /*max_packet_size=*/ 1000, reader.clone(), sender));

        // The task should be pending initially.
        exec.run_until_stalled(&mut tx_task_fut).expect_pending("TX Task active");

        // Flow control terminates by closing the internal mpsc channel.
        drop(receiver);

        reader.set_and_wake_reader(vec![1]); // Add data to trigger a read.
        // The task should terminate because it cannot send to the closed channel.
        exec.run_until_stalled(&mut tx_task_fut).expect("TX Task finished");
    }

    #[fuchsia::test]
    async fn set_parameters_updates_inspect() {
        let inspect = inspect::Inspector::default();
        let dlci = DLCI::try_from(8).unwrap();
        let mut session_channel = SessionChannel::new(dlci, Role::Responder)
            .with_inspect(inspect.root(), "channel_")
            .unwrap();

        // Initial state - no parameters.
        assert_data_tree!(inspect, root: {
            channel_: {
                dlci: 8u64,
                server_channel: 4u64,
            }
        });

        // Set parameters.
        let max_packet_size = 1024;
        let flow_control = FlowControlMode::CreditBased(Credits::new(10, 10));
        assert!(session_channel.set_parameters(max_packet_size, flow_control).is_ok());

        // Inspect should be updated.
        assert_data_tree!(inspect, root: {
            channel_: {
                dlci: 8u64,
                server_channel: 4u64,
                max_packet_size: 1024u64,
                initial_local_credits: 10u64,
                initial_remote_credits: 10u64,
            }
        });
    }

    #[fuchsia::test]
    async fn session_channel_inspect_updates_with_new_flow_control() {
        let inspect = inspect::Inspector::default();

        // Set up a channel with inspect.
        let dlci = DLCI::try_from(8).unwrap();
        let mut channel = SessionChannel::new(dlci, Role::Initiator)
            .with_inspect(inspect.root(), "channel_")
            .unwrap();
        // Default tree.
        assert_data_tree!( inspect, root: {
            channel_: {
                dlci: 8u64,
                server_channel: 4u64,
            },
        });

        let flow_control = FlowControlMode::CreditBased(Credits::new(
            12, // Arbitrary
            15, // Arbitrary
        ));
        assert!(channel.set_parameters(980, flow_control).is_ok());
        // Inspect tree should be updated with the initial credits.
        assert_data_tree!(inspect, root: {
            channel_: {
                dlci: 8u64,
                server_channel: 4u64,
                initial_local_credits: 12u64,
                initial_remote_credits: 15u64,
                max_packet_size: 980u64,
            },
        });
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn session_channel_inspect_updates_when_established(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(7_000_000));

        // Set up and establish a channel with inspect.
        let dlci = DLCI::try_from(8).unwrap();
        let flow_control = FlowControlMode::CreditBased(Credits::new(
            12, // Arbitrary
            15, // Arbitrary
        ));
        let (inspect, _channel, _client, mut outgoing_frames) = create_and_establish(
            Role::Initiator,
            DLCI::try_from(8).unwrap(),
            flow_control,
            transport,
        );
        // Upon establishment, the inspect tree should have a `flow_controller` node.
        assert_data_tree!(@executor exec, inspect, root: {
            channel_: contains {
                flow_controller: {
                    controller_type: "credit_flow",
                    local_credits: 12u64, // Local same as initial local amount.
                    remote_credits: 15u64, // Remote same as initial remote amount.
                    inbound_stream: {
                        bytes_per_second_current: 0u64,
                        start_time: 7_000_000i64,
                        streaming_secs: 0u64,
                        total_bytes: 0u64,
                        timeseries_bytes: contains {},
                    },
                    outbound_stream: {
                        bytes_per_second_current: 0u64,
                        start_time: 7_000_000i64,
                        streaming_secs: 0u64,
                        total_bytes: 0u64,
                        timeseries_bytes: contains {},
                    },
                },
            },
        });

        // Client closes its end of the RFCOMM channel - expect the outgoing Disconnect frame.
        drop(_client);
        expect_frame(&mut exec, &mut outgoing_frames, FrameData::Disconnect, Some(dlci));
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        // The flow controller node should be removed indicating the inactiveness of the channel.
        assert_data_tree!(@executor exec, inspect, root: {
            channel_: {
                dlci: 8u64,
                server_channel: 4u64,
                max_packet_size: 1000u64, // Default in `create_and_establish`
                initial_local_credits: 12u64,
                initial_remote_credits: 15u64,
            },
        });
    }

    #[fuchsia::test]
    fn credit_flow_controller_inspect_updates_after_data_exchange() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(987_000_000));

        let initial_credits = Credits::new(50, 70);
        let inspect = inspect::Inspector::default();
        let mut flow_controller = CreditFlowController::new(
            Role::Initiator,             // Random fixed Role.
            DLCI::try_from(19).unwrap(), // Random fixed user DLCI.
            initial_credits,
        )
        .with_inspect(inspect.root(), FLOW_CONTROLLER)
        .unwrap();

        // Initial inspect topology
        assert_data_tree!(@executor exec, inspect, root: {
            flow_controller: {
                controller_type: "credit_flow",
                local_credits: 50u64,
                remote_credits: 70u64,
                inbound_stream: {
                    bytes_per_second_current: 0u64,
                    start_time: 987_000_000i64,
                    streaming_secs: 0u64,
                    total_bytes: 0u64,
                    timeseries_bytes: contains {},
                },
                outbound_stream: {
                    bytes_per_second_current: 0u64,
                    start_time: 987_000_000i64,
                    streaming_secs: 0u64,
                    total_bytes: 0u64,
                    timeseries_bytes: contains {},
                },
            },
        });

        // After some time, the peer sends us user data with credits.
        exec.set_fake_time(zx::MonotonicDuration::from_seconds(1).after_now());
        let data1 = vec![0x12, 0x34, 0x56, 0x78, 0x90];
        flow_controller.handle_incoming_frame(FlowControlledData::new(data1, 10));
        assert_eq!(flow_controller.process_tx_queue(), vec![]);
        // Flow controller inspect node should be updated.
        assert_data_tree!(@executor exec, inspect, root: {
            flow_controller: {
                controller_type: "credit_flow",
                local_credits: 60u64, // 50 (initial) + 10 (Replenished in `data1`)
                remote_credits: 69u64, // 70 (initial) - 1 (Received frame)
                inbound_stream: {
                    bytes_per_second_current: 5u64,
                    start_time: 987_000_000i64,
                    streaming_secs: 1u64,
                    total_bytes: 5u64, // Received 5 bytes from peer.
                    timeseries_bytes: contains {},
                },
                outbound_stream: {
                    bytes_per_second_current: 0u64,
                    start_time: 987_000_000i64,
                    streaming_secs: 0u64,
                    total_bytes: 0u64,
                    timeseries_bytes: contains {},
                },
            },
        });

        // After some time, the application responds with data.
        exec.set_fake_time(zx::MonotonicDuration::from_seconds(2).after_now());
        let data2 = UserData { information: vec![0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22] };
        flow_controller.handle_outgoing_frame(data2.clone());
        assert!(!flow_controller.process_tx_queue().is_empty());
        // Flow controller inspect node should be updated.
        assert_data_tree!(@executor exec, inspect, root: {
            flow_controller: {
                controller_type: "credit_flow",
                local_credits: 59u64, // 60 (previous amount) - 1 (sent frame)
                remote_credits: 69u64, // 69 (previous amount unchanged)
                inbound_stream: { // Unchanged
                    bytes_per_second_current: 5u64,
                    start_time: 987_000_000i64,
                    streaming_secs: 1u64,
                    total_bytes: 5u64,
                    timeseries_bytes: contains {},
                },
                outbound_stream: {
                    bytes_per_second_current: 2u64, // 8 (bytes) / 3 (seconds)
                    start_time: 987_000_000i64,
                    streaming_secs: 3u64,
                    total_bytes: 8u64, // Sent 8 bytes to peer.
                    timeseries_bytes: contains {},
                },
            },
        });
    }
}
