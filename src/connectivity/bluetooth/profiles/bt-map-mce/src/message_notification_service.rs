// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use bt_map::packets::event_report::*;
use bt_map::{Error, MapSupportedFeatures};
use bt_obex::header::{Header, HeaderIdentifier, HeaderSet};
use bt_obex::operation::{OpCode, ResponseCode as OperationResponseCode};
use bt_obex::server::*;
use fidl::endpoints::{ClientEnd, Proxy};
use fidl_fuchsia_bluetooth_map::{
    AccessorSetNotificationRegistrationResponder, Notification, NotificationRegistrationMarker,
    NotificationRegistrationNewEventReportRequest, NotificationRegistrationProxy,
};
use fuchsia_async as fasync;
use fuchsia_bluetooth::profile::ProtocolDescriptor;
use fuchsia_bluetooth::types::Channel;
use fuchsia_sync::Mutex;
use futures::StreamExt;
use futures::channel::mpsc::{Sender, UnboundedReceiver, UnboundedSender};
use futures::channel::{mpsc, oneshot};
use futures::future::{FutureExt, TryFutureExt};
use log::{info, warn};
use objects::Parser;
use std::collections::HashMap;
use std::io::Cursor;
use std::pin::pin;
use uuid::{Uuid, uuid};

use crate::profile::transport_type_from_protocol;

const MAS_INSTANCE_ID_TAG_ID: MasInstanceId = 0x0F;
/// MAP v1.4.2 Section 6.3 Table 6.5.
const MNS_TARGET_UUID: Uuid = uuid!("bb582b41-420c-11db-b0de-0800200c9a66");

pub type MasInstanceId = u8;

/// Thread-safe representation of the Message Notification Service session and its
/// initialization state.
#[derive(Default)]
pub struct Session {
    state: Mutex<State>,
}

/// Signal used to send to active Accessor to perform actions based on
/// current Session status.
pub enum SessionSignal {
    // Client no longer wishes to to receive notifications from the
    // current session. Unregister notifications from all relevant
    // MAS instances.
    UnregisterNotifications,
    // Underlying OBEX server connection has been disconnected.
    // Session should terminate.
    Terminate,
}

pub struct SessionMaterial {
    relayer_proxy: NotificationRegistrationProxy,
    responder: AccessorSetNotificationRegistrationResponder,
    signal_sender: Sender<SessionSignal>,
}

// UIDs of MAS instances (aka Repositories) that are not yet registered
// for notifications.
#[derive(Default)]
enum State {
    #[default]
    NotInitialized,
    // MNS session was initiated and we're pending for connection request
    // from the remote MSE peer.
    Requested {
        // Map of MAS instances for which we want to establish a MNS
        // session for.
        mas_instances: HashMap<MasInstanceId, MapSupportedFeatures>,
        session_material: SessionMaterial,
    },
    // The MNS connection process has started and is awaiting the OBEX connection.
    Connecting {
        mas_instances: HashMap<MasInstanceId, MapSupportedFeatures>,
    },
    // OBEX connection to the remote MSE peer was established and
    // the session is serving event reports from the peer to the client
    // through the FIDL protocol.
    Running {
        mas_instances: HashMap<MasInstanceId, MapSupportedFeatures>,
        // The background task that manages the MNS session. This task handles
        // the OBEX connection, relays event reports to the FIDL client, and
        // manages session termination. The task is dropped when the state
        // changes, which cancels the task and cleans up the session.
        _task: fasync::Task<()>,
    },
}

impl Session {
    /// Initializes a [Session] from an Accessor client's request
    /// to register for notfiications. On success, [Session] should be
    /// in [State::Requested] state.
    /// If the initialization fails, `responder` would be consumed to
    /// respond to register notification request with an error.
    pub fn initialize(
        &self,
        mas_instances: HashMap<MasInstanceId, MapSupportedFeatures>,
        relayer_client: ClientEnd<NotificationRegistrationMarker>,
        responder: AccessorSetNotificationRegistrationResponder,
        signal_sender: Sender<SessionSignal>,
    ) -> Result<(), Error> {
        let mut lock = self.state.lock();
        let State::NotInitialized = *lock else {
            let _ = responder.send(Err(Error::InvalidMnsState.into()));
            return Err(Error::InvalidMnsState);
        };
        if mas_instances.is_empty() {
            return Err(Error::InvalidParameters);
        }
        let relayer_proxy = relayer_client.into_proxy();
        *lock = State::Requested {
            mas_instances,
            session_material: SessionMaterial { relayer_proxy, responder, signal_sender },
        };
        Ok(())
    }

    /// Set the [Session] as not initialized.
    pub fn clean_up(&self) {
        let mut lock = self.state.lock();
        let _ = std::mem::replace(&mut *lock, State::NotInitialized);
    }

    /// Returns the MAS instances that the notification session is
    /// initialized to look out for notifications for.
    /// If the [Session] isn't initialized, returns empty map.
    pub fn mas_instances(&self) -> HashMap<u8, MapSupportedFeatures> {
        let lock = self.state.lock();
        match &(*lock) {
            State::NotInitialized => HashMap::default(),
            State::Requested { mas_instances, .. } => mas_instances.clone(),
            State::Running { mas_instances, .. } => mas_instances.clone(),
            State::Connecting { mas_instances } => mas_instances.clone(),
        }
    }

    /// Accepts incoming L2CAP/RFCOMM connection from the remote MSE peer to connect to the
    /// MNS server.
    /// On success, [Session] should be in [State::Running] and should have started processing
    /// incoming OBEX requests and returns the future for the active MNS server future.
    /// On failure, the MNS session should be terminated.
    pub async fn run(
        &self,
        protocol: Vec<ProtocolDescriptor>,
        channel: Channel,
    ) -> Result<(), Error> {
        // Atomically transition from Requested -> Connecting.
        let (mas_instances, session_material) = {
            let mut lock = self.state.lock();
            let prev_state = std::mem::replace(&mut *lock, State::NotInitialized);
            let State::Requested { mas_instances, session_material } = prev_state else {
                *lock = prev_state;
                return Err(Error::InvalidMnsState);
            };
            *lock = State::Connecting { mas_instances: mas_instances.clone() };
            (mas_instances, session_material)
        };

        let SessionMaterial { responder, relayer_proxy, mut signal_sender } = session_material;

        let transport_type = transport_type_from_protocol(&protocol)?;
        let (handler, mut notification_receiver) = ServerHandler::new();

        let obex_server_task = match handler.start_obex(channel, transport_type).await {
            Ok(task) => task,
            Err(e) => {
                let _ = responder.send(Err(fidl_fuchsia_bluetooth_map::Error::Unknown));
                return Err(e);
            }
        };

        // Session should process the obex server task as well as notifications that we receive from the peer.
        let session_task = fasync::Task::spawn(async move {
            let mut obex_server_fut = obex_server_task.fuse();
            let mut fidl_closed_fut = pin!(relayer_proxy.on_closed().fuse());
            info!("MNS session task started");
            loop {
                futures::select! {
                    res = obex_server_fut => {
                        info!(res:?; "Underlying OBEX connection terminated");
                        break;
                    }
                    item = notification_receiver.select_next_some() => {
                        let _ = relayer_proxy.new_event_report(&NotificationRegistrationNewEventReportRequest {
                                notification: Some(item.0),
                                received: Some(item.1.into_zx().into_nanos()),
                                ..Default::default()
                            })
                            .await
                            .inspect_err(|err| warn!(err:%; "Failed to send new event report"));
                    }
                    // FIDL server end closed which means the client no longer needs the service.
                    _ = fidl_closed_fut => {
                        if let Err(e) = signal_sender.try_send(SessionSignal::UnregisterNotifications) {
                            warn!(e:?; "Failed to send session signal");
                        }
                    }
                }
            }
            info!("MNS session task completed");
            if let Err(e) = signal_sender.try_send(SessionSignal::Terminate) {
                warn!(e:?; "Failed to send session signal");
            }
        });

        let _ = responder.send(Ok(()));

        let mut lock = self.state.lock();
        let prev_state =
            std::mem::replace(&mut *lock, State::Running { mas_instances, _task: session_task });

        match prev_state {
            State::Connecting { .. } => {}
            _ => {
                *lock = prev_state;
                return Err(Error::InvalidMnsState);
            }
        }
        Ok(())
    }
}

/// Implements the [ObexServerHandler] to handle OBEX requests from remote
/// MNS OBEX client.
struct ServerHandler {
    notification_sender: UnboundedSender<(Notification, fasync::MonotonicInstant)>,
    // Expected to be `Some<T>` until the OBEX CONNECT operation is completed, after which it will
    // be `None`.
    obex_connection_notifier: Option<oneshot::Sender<Result<(), Error>>>,
}

impl ServerHandler {
    /// Creates a new OBEX server handler for Message Notification Server and an [UnboundedReceiver]
    /// for notifications received through the PUT operation from the peer.
    fn new() -> (Self, UnboundedReceiver<(Notification, fasync::MonotonicInstant)>) {
        let (notification_sender, notification_receiver) = mpsc::unbounded();
        (Self { notification_sender, obex_connection_notifier: None }, notification_receiver)
    }

    async fn start_obex(
        mut self,
        channel: Channel,
        transport_type: TransportType,
    ) -> Result<fasync::Task<Result<(), Error>>, Error> {
        // Create a one-shot channel for OBEX connection establishment.
        let (connect_sender, connect_receiver) = oneshot::channel();

        // Start OBEX server in the background.
        self.obex_connection_notifier = Some(connect_sender);
        let obex_server = ObexServer::new(channel, transport_type, Box::new(self));

        let obex_server_task = fasync::Task::local(obex_server.run().map_err(|e| Error::Obex(e)));
        match connect_receiver.await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(Error::other(e)),
            Err(e) => return Err(Error::other(e)),
        };
        Ok(obex_server_task)
    }
}

#[async_trait]
impl ObexServerHandler for ServerHandler {
    async fn connect(&mut self, headers: HeaderSet) -> Result<HeaderSet, ObexOperationError> {
        // We only expect one OBEX CONNECT.
        let Some(conn_notifier) = self.obex_connection_notifier.take() else {
            warn!("Unexpected incoming MNS connection request. Ignoring...");
            return Err(new_operation_error(
                OperationResponseCode::Forbidden,
                "MNS Obex connection already exists",
            ));
        };

        let err = new_operation_error(
            OperationResponseCode::BadRequest,
            &format!("Invalid headers: {headers:?}"),
        );
        let Some(Header::Target(val)) = headers.get(&HeaderIdentifier::Target) else {
            let _ = conn_notifier.send(Err(Error::Obex(bt_obex::ObexError::operation(
                OpCode::Connect,
                "Missing target header",
            ))));
            return Err(err);
        };

        // Target UUID based on MAP v1.4.2 Section 6.3.
        if Uuid::from_slice(val.as_slice()) != Ok(MNS_TARGET_UUID) {
            let _ = conn_notifier.send(Err(Error::Obex(bt_obex::ObexError::operation(
                OpCode::Connect,
                "Invalid target value",
            ))));
            return Err(err);
        }

        // See MAP v1.4.2 Section 6.4.1 for response headers of Connect operation.
        // We omit adding ConnectionId header since it'll be automatically
        // handled by the ObexServer.
        let response_headers =
            HeaderSet::from_headers(vec![Header::Who(MNS_TARGET_UUID.into_bytes().to_vec())])
                .unwrap();

        // Notify that the connection was established successfully.
        let _ = conn_notifier.send(Ok(()));
        Ok(response_headers)
    }

    async fn disconnect(&mut self, _headers: HeaderSet) -> HeaderSet {
        // The session lifecycle, including disconnection, is managed by the `session_task` in
        // `Session::run`. No explicit cleanup is needed here.
        HeaderSet::new()
    }

    async fn set_path(
        &mut self,
        _headers: HeaderSet,
        _backup: bool,
        _create: bool,
    ) -> Result<HeaderSet, ObexOperationError> {
        Err(new_operation_error(
            OperationResponseCode::MethodNotAllowed,
            "Set Path not yet implemented",
        ))
    }

    async fn get_info(&mut self, _headers: HeaderSet) -> Result<HeaderSet, ObexOperationError> {
        Err(new_operation_error(
            OperationResponseCode::MethodNotAllowed,
            "Get Info not yet implemented",
        ))
    }

    async fn get_data(
        &mut self,
        _headers: HeaderSet,
    ) -> Result<(Vec<u8>, HeaderSet), ObexOperationError> {
        Err(new_operation_error(
            OperationResponseCode::MethodNotAllowed,
            "Get Data not yet implemented",
        ))
    }

    /// Checks if the put request is the SendEvent from MAP v1.4.3
    /// Section 5.1.
    async fn put(&mut self, data: Vec<u8>, headers: HeaderSet) -> Result<(), ObexOperationError> {
        let received_time = fasync::MonotonicInstant::now();
        // Check connection ID.
        if !headers.contains_header(&HeaderIdentifier::ConnectionId) {
            return Err(new_operation_error(
                OperationResponseCode::NotAcceptable,
                "Connection ID header missing",
            ));
        };
        let app_params =
            headers.get(&HeaderIdentifier::ApplicationParameters).ok_or_else(|| {
                new_operation_error(
                    OperationResponseCode::NotAcceptable,
                    "Application Parameters header missing",
                )
            })?;
        let Header::ApplicationParameters(params) = app_params else { unreachable!() };

        // Parse MAS instance ID.
        let Some((_tag, value)) = params.iter().find(|&&(t, _)| t == MAS_INSTANCE_ID_TAG_ID) else {
            return Err(new_operation_error(
                OperationResponseCode::NotAcceptable,
                "Missing MAS instance ID application parameters",
            ));
        };
        if value.len() != 1 {
            return Err(new_operation_error(
                OperationResponseCode::NotAcceptable,
                "Invalid MAS instance ID value",
            ));
        }
        let mas_instance_id = value[0];

        let event_report = EventReport::parse(Cursor::new(data)).map_err(|e| {
            new_operation_error(OperationResponseCode::NotAcceptable, &format!("{e:?}"))
        })?;

        let maybe_notification: Result<Notification, _> = (&event_report).try_into();
        let Ok(mut notification) = maybe_notification else {
            warn!(e:% = maybe_notification.err().unwrap();  "Failed to convert event report to notification: {event_report:?}");
            return Ok(());
        };

        // Send the notification to the FIDL server end.
        notification.mas_instance_id = Some(mas_instance_id);
        let _ = self
            .notification_sender
            .unbounded_send((notification, received_time))
            .inspect_err(|err| warn!(err:%; "Failed to relay notification"));
        Ok(())
    }

    async fn delete(&mut self, headers: HeaderSet) -> Result<(), ObexOperationError> {
        // Unlike other handler methods, we don't return an error and just ignore delete requests
        // since underlying OBEX operation is still PUT so it's not totally unexpected.
        warn!(
            "Received unexpected OBEX DELETE operation for headers: {headers:?}. Ignoring request"
        );
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use async_test_helpers::expect_stream_item;
    use async_utils::PollExt;
    use bt_channel_test_support::{Transport, create_test_channels};
    use bt_obex::operation::{RequestPacket, ResponsePacket};
    use fidl::endpoints::{create_proxy_and_stream, create_request_stream};
    use fidl_fuchsia_bluetooth_bredr as bredr;
    use fidl_fuchsia_bluetooth_map::{
        AccessorMarker, AccessorRequest, AccessorSetNotificationRegistrationRequest,
        NotificationRegistrationMarker, NotificationRegistrationRequest,
        NotificationRegistrationRequestStream,
    };
    use fuchsia_async as fasync;
    use fuchsia_bluetooth::profile::DataElement;
    use futures::SinkExt;
    use futures::channel::mpsc::Receiver;
    use objects::Builder;
    use packet_encoding::Encodable;
    use std::pin::pin;
    use test_case::test_case;

    // Version = 1.0, Flags = 0, Max packet = 0xffff.
    const CONNECTION_RESPONSE_DATA: [u8; 4] = [0x10, 0x00, 0xff, 0xff];
    const TEST_MAS_ID_1: u8 = 1;
    const TEST_MAS_ID_2: u8 = 2;

    /// Sends the `packet` over the provided `channel`.
    #[track_caller]
    pub(crate) fn send_packet<T>(exec: &mut fasync::TestExecutor, channel: &mut Channel, packet: T)
    where
        T: Encodable,
        <T as Encodable>::Error: std::fmt::Debug,
    {
        let mut buf = vec![0; packet.encoded_len()];
        packet.encode(&mut buf[..]).expect("can encode packet");
        let write_fut = channel.send(buf.to_vec());
        exec.run_until_stalled(&mut pin!(write_fut))
            .expect("write to channel success")
            .expect("write successful");
    }

    // Starts the MNS session and returns the high-level AccessorProxy
    // Accessor server request stream, remote peer end of the OBEX connection,
    // and the repository notifier client that can be used for testing.
    // The MNS is registered for two MASes (MAS with ID 1 and MAS with ID 2).
    fn run_mns_session(
        exec: &mut fasync::TestExecutor,
        transport: Transport,
    ) -> (Channel, NotificationRegistrationRequestStream, Session, Receiver<SessionSignal>) {
        // Set up fake client request for notifications.
        let (accessor_proxy, mut accessor_requests) = create_proxy_and_stream::<AccessorMarker>();
        let (relayer_client, relayer_request_stream) =
            create_request_stream::<NotificationRegistrationMarker>();
        let register_fut = accessor_proxy.set_notification_registration(
            AccessorSetNotificationRegistrationRequest {
                mas_instance_ids: Some(vec![TEST_MAS_ID_1, TEST_MAS_ID_2]),
                server: Some(relayer_client),
                ..Default::default()
            },
        );
        let (session_signal_sender, session_signal_receiver) = mpsc::channel(1);

        let mut register_fut = pin!(register_fut);

        let request = expect_stream_item(exec, &mut accessor_requests).unwrap();
        let AccessorRequest::SetNotificationRegistration { payload, responder } = request else {
            panic!("Should have received register for notifications request");
        };

        let mas_instances = HashMap::from([
            (
                TEST_MAS_ID_1,
                MapSupportedFeatures::NOTIFICATION
                    | MapSupportedFeatures::NOTIFICATION_REGISTRATION,
            ),
            (
                TEST_MAS_ID_2,
                MapSupportedFeatures::NOTIFICATION
                    | MapSupportedFeatures::NOTIFICATION_REGISTRATION,
            ),
        ]);
        let session = Session::default();
        let _ = session
            .initialize(mas_instances, payload.server.unwrap(), responder, session_signal_sender)
            .expect("should initialize");

        let _ = exec.run_until_stalled(&mut register_fut).expect_pending("should be pending");

        // Test out Session start.
        let (local, mut remote) = create_test_channels(transport);
        {
            let run_fut = session.run(
                vec![ProtocolDescriptor {
                    protocol: bredr::ProtocolIdentifier::L2Cap,
                    params: vec![DataElement::Uint16(1234)],
                }],
                local,
            );
            let mut run_fut = pin!(run_fut);

            // Should be pending since OBEX connection wasn't established yet.
            let _ = exec.run_until_stalled(&mut run_fut).expect_pending("should be pending");

            // Fake an incoming OBEX connection from remote server.
            let connect_packet = RequestPacket::new(
                OpCode::Connect,
                CONNECTION_RESPONSE_DATA.to_vec(),
                HeaderSet::from_headers(vec![Header::Target(MNS_TARGET_UUID.as_bytes().to_vec())])
                    .unwrap(),
            );
            send_packet(exec, &mut remote, connect_packet);

            exec.run_until_stalled(&mut run_fut).expect("should be ready").expect("no error");

            let response_raw = expect_stream_item(exec, &mut remote).expect("response");
            let response = ResponsePacket::decode(&response_raw[..], OpCode::Connect)
                .expect("can decode request");
            assert_eq!(
                response.headers().get(&HeaderIdentifier::Who),
                Some(&Header::Who(MNS_TARGET_UUID.as_bytes().to_vec()))
            );
            assert_eq!(response.code(), &OperationResponseCode::Ok);
        };

        // Register notification future isn't resolved until we actually run the server.
        let _ = exec
            .run_until_stalled(&mut register_fut)
            .expect("should be ready")
            .expect("no fidl error")
            .expect("success");

        (remote, relayer_request_stream, session, session_signal_receiver)
    }

    #[track_caller]
    fn new_msg_event_report_packet(repo_id: u8, msg_handle: u64) -> RequestPacket {
        let event_report = EventReport::v1_0(vec![
            EventAttribute::Type(Type::NewMessage),
            EventAttribute::Handle(msg_handle),
            EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
            EventAttribute::MessageType(bt_map::MessageType::SmsCdma),
        ])
        .unwrap();
        let mut buf = Vec::new();
        event_report.build(&mut buf).unwrap();

        RequestPacket::new(
            OpCode::PutFinal,
            vec![],
            HeaderSet::from_headers(vec![
                Header::ConnectionId(1.try_into().unwrap()),
                Header::Type("x-bt/MAP-event-report".to_string().into()),
                Header::ApplicationParameters(
                    vec![(0x0F /* MASInstanceID */, vec![repo_id])].into(),
                ),
                Header::EndOfBody(buf),
            ])
            .unwrap(),
        )
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn new_event_report(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut remote, mut relayer_stream, _session, mut signal_receiver) =
            run_mns_session(&mut exec, transport);

        // Mimic incoming event report from remote MNS client.
        send_packet(&mut exec, &mut remote, new_msg_event_report_packet(TEST_MAS_ID_1, 1234));
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Should have sent notifications.
        let relayer_request =
            expect_stream_item(&mut exec, &mut relayer_stream).expect("should be ready");
        let NotificationRegistrationRequest::NewEventReport { payload, responder } =
            relayer_request
        else {
            panic!("expected new event report request");
        };
        assert_eq!(
            payload.notification,
            Some(Notification {
                type_: Some(fidl_fuchsia_bluetooth_map::NotificationType::NewMessage),
                mas_instance_id: Some(TEST_MAS_ID_1),
                message_handle: Some(1234),
                folder: Some("TELECOM/MSG/INBOX".to_string()),
                message_type: Some(fidl_fuchsia_bluetooth_map::MessageType::SMS_CDMA),
                ..Default::default()
            }),
        );
        assert!(responder.send().is_ok());

        // 2 more messages from remote peer.
        send_packet(&mut exec, &mut remote, new_msg_event_report_packet(TEST_MAS_ID_1, 2345));
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        send_packet(&mut exec, &mut remote, new_msg_event_report_packet(TEST_MAS_ID_2, 1234));
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Server end should have received 2 requests for new event report.
        let relayer_request =
            expect_stream_item(&mut exec, &mut relayer_stream).expect("should be ready");
        let NotificationRegistrationRequest::NewEventReport { payload: _, responder } =
            relayer_request
        else {
            panic!("expected new event report request");
        };
        assert!(responder.send().is_ok());

        let relayer_request =
            expect_stream_item(&mut exec, &mut relayer_stream).expect("should be ready");
        let NotificationRegistrationRequest::NewEventReport { payload: _, responder } =
            relayer_request
        else {
            panic!("expected new event report request");
        };
        assert!(responder.send().is_ok());

        // No more messages.
        let mut next_message = relayer_stream.next();
        let _ = exec.run_until_stalled(&mut next_message).expect_pending("should be pending");

        // We shouldn't have received any signals.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        let mut signal_fut = signal_receiver.next();
        let _ = exec.run_until_stalled(&mut signal_fut).expect_pending("no outstanding signals");
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn notification_registration_client_close(transport: Transport) {
        let mut exec: fuchsia_async::TestExecutor = fasync::TestExecutor::new();

        let (_remote, notifier_stream, _session, mut signal_receiver) =
            run_mns_session(&mut exec, transport);

        // Mimic client dropping the NotificationRegistration stream.
        drop(notifier_stream);
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Should have received signal to unregister from notifications.
        let mut signal_fut = signal_receiver.next();
        let Some(SessionSignal::UnregisterNotifications) =
            exec.run_until_stalled(&mut signal_fut).expect("should have received signal")
        else {
            panic!("unexpected signal received");
        };

        // No more signals.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        let mut signal_fut = signal_receiver.next();
        let _ = exec.run_until_stalled(&mut signal_fut).expect_pending("no outstanding signals");
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn obex_disconnect(transport: Transport) {
        let mut exec: fuchsia_async::TestExecutor = fasync::TestExecutor::new();

        let (mut remote, _notifier_stream, _session, mut signal_receiver) =
            run_mns_session(&mut exec, transport);

        // Mimic a OBEX DISCONNECT request from remote MNS client.
        send_packet(
            &mut exec,
            &mut remote,
            RequestPacket::new(OpCode::Disconnect, vec![], HeaderSet::new()),
        );

        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Should have received signal to terminate session.
        let mut signal_fut = signal_receiver.next();
        let Some(SessionSignal::Terminate) =
            exec.run_until_stalled(&mut signal_fut).expect("should have received signal")
        else {
            panic!("unexpected signal received");
        };

        // No more signals can be expected from this session.
        let mut signal_fut = signal_receiver.next();
        let None = exec.run_until_stalled(&mut signal_fut).expect("not pending") else {
            panic!("unexpected signal sender status");
        };
    }
}
