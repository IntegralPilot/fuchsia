// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_map::{
    Error, MAP_SUPPORTED_FEATURES_TAG_ID, MapSupportedFeatures, NOTIFICATION_STATUS_TAG_ID,
};
use bt_obex::ObexError;
use bt_obex::client::*;
use bt_obex::header::{Header, HeaderSet};
use fidl_fuchsia_bluetooth_bredr as bredr;
use fidl_fuchsia_bluetooth_map as fidl_map;
use fuchsia_bluetooth::types::Channel;
use fuchsia_sync::Mutex;
use log::trace;
use uuid::{Uuid, uuid};

use crate::profile::{MasConfig, mns_supported_features};

/// MAP v1.4.2 Section 6.3 Table 6.5.
const MAS_TARGET: Uuid = uuid!("bb582b40-420c-11db-b0de-0800200c9a66");
const NOTIFICATION_TYPE: &str = "x-bt/MAP-NotificationRegistration";
/// As per MAP v1.4.2 Section 5.2.4, to avoid a PUT with an empty body leading to a 'delete',
/// put a filler byte in the body/end of body header.
const PUT_FILLER_BYTES: &'static [u8] = &[0x30];

/// Manages the OBEX client connection to the peer's MAS server.
pub struct MasInstance {
    obex_client: ObexClient,
    mas_config: MasConfig,
    // Whether or not SetNotificationRegistration is turned 'on'
    notification: Mutex<bool>,
}

impl MasInstance {
    /// Given the OBEX connection channel and the MAS configuration,
    /// creates a new MAS instance.
    pub fn create(channel: Channel, mas_config: MasConfig) -> Self {
        let type_ = match mas_config.connection_params() {
            &bredr::ConnectParameters::L2cap(_) => TransportType::L2cap,
            &bredr::ConnectParameters::Rfcomm(_) => TransportType::Rfcomm,
        };
        Self {
            obex_client: ObexClient::new(channel, type_),
            mas_config,
            notification: Default::default(),
        }
    }

    pub fn id(&self) -> u8 {
        self.mas_config.instance_id()
    }

    pub fn features(&self) -> MapSupportedFeatures {
        self.mas_config.features()
    }

    pub fn notification_registered(&self) -> bool {
        *self.notification.lock()
    }

    /// Returns whether or not there is an ongoing OBEX connection.
    pub fn is_obex_connected(&self) -> bool {
        self.obex_client.is_connected()
    }

    /// Returns whether or not the underlying transport is connected.
    pub fn is_transport_connected(&self) -> bool {
        self.obex_client.is_transport_connected()
    }

    /// Make OBEX connection to the MAS server.
    // See MAP v1.4.2 section 6.4.1.
    pub async fn connect(&self) -> Result<(), Error> {
        if self.is_obex_connected() {
            trace!("OBEX connection already established for MAS instance {}", self.id());
            return Ok(());
        }

        // Target UUID based on MAP v1.4.2 Section 6.3.
        let mut app_params = vec![];
        if self
            .mas_config
            .features()
            .contains(MapSupportedFeatures::MAPSUPPORTEDFEATURES_IN_CONNECT_REQUEST)
        {
            app_params.push((
                MAP_SUPPORTED_FEATURES_TAG_ID,
                mns_supported_features().bits().to_be_bytes().to_vec(),
            ));
        }
        let headers = HeaderSet::from_headers(vec![
            Header::Target(MAS_TARGET.as_bytes().to_vec()),
            Header::ApplicationParameters(app_params.into()),
        ])?;

        let _ = self.obex_client.connect(headers).await?;
        if self.obex_client.connection_id().is_none() {
            return Err(Error::Obex(ObexError::other(
                "Failed to get connection ID from OBEX connection",
            )));
        }
        Ok(())
    }

    /// Send notification register ON/OFF on a MAS instance.
    // See MAP v1.4.2 section 6.4.1.
    pub async fn set_register_notification(&self, on: bool) -> Result<(), Error> {
        if self.notification_registered() == on {
            return Ok(());
        }

        if !self.mas_config.features().contains(
            MapSupportedFeatures::NOTIFICATION_REGISTRATION | MapSupportedFeatures::NOTIFICATION,
        ) {
            return Err(Error::NotSupported);
        }

        // If we're not connected, first connect.
        if !self.is_obex_connected() {
            let _ = self.connect().await?;
        }

        // See MAP v1.4.2 section 5.2.
        let app_params = vec![(NOTIFICATION_STATUS_TAG_ID, vec![on as u8])];
        let headers = HeaderSet::from_headers(vec![
            Header::Type(NOTIFICATION_TYPE.to_string().into()),
            Header::ApplicationParameters(app_params.into()),
        ])?;

        let put_op = self.obex_client.put()?;
        let _ = put_op.write_final(PUT_FILLER_BYTES, headers).await?;
        *self.notification.lock() = on;

        Ok(())
    }
}

impl From<&MasInstance> for fidl_map::MasInstance {
    fn from(value: &MasInstance) -> Self {
        fidl_map::MasInstance {
            id: Some(value.id()),
            supported_message_types: value
                .mas_config
                .supported_message_types()
                .iter()
                .map(|t| (*t).into())
                .reduce(|acc, t| acc | t),
            supports_notification: Some(
                value.mas_config.features().contains(MapSupportedFeatures::SUPPORTS_NOTIFICATION),
            ),
            ..Default::default()
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use async_test_helpers::expect_stream_item;
    use async_utils::PollExt;
    use bt_channel_test_support::{Transport, create_test_channels};
    use bt_map::MessageType;
    use bt_obex::header::HeaderIdentifier;
    use bt_obex::operation::{OpCode, RequestPacket, ResponseCode, ResponsePacket};
    use fuchsia_async as fasync;
    use futures::{SinkExt, StreamExt};
    use std::pin::pin;
    use test_case::test_case;

    use packet_encoding::{Decodable, Encodable};

    // Version = 1.0, Flags = 0, Max packet = 0xffff.
    const CONNECTION_RESPONSE_DATA: [u8; 4] = [0x10, 0x00, 0xff, 0xff];

    const CONNECTION_ID: u32 = 0x1;

    pub(crate) async fn send_ok_response(
        channel: &mut Channel,
        headers: Vec<Header>,
        data: Vec<u8>,
    ) {
        let response =
            ResponsePacket::new(ResponseCode::Ok, data, HeaderSet::from_headers(headers).unwrap());
        let mut response_buf = vec![0; response.encoded_len()];
        response.encode(&mut response_buf[..]).unwrap();
        let _ = channel.send(response_buf).await.unwrap();
    }

    /// Returns a [MasConfig] with specified features and id of 1, support for SMS CDMA message
    /// type and connection over L2CAP.
    fn test_mas_config(features: MapSupportedFeatures) -> MasConfig {
        MasConfig::new(
            1,
            "Test MAS",
            vec![MessageType::SmsCdma],
            bredr::ConnectParameters::L2cap(bredr::L2capParameters {
                psm: Some(1004),
                parameters: None,
                ..Default::default()
            }),
            features,
        )
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn connect(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (local, mut remote) = create_test_channels(transport);
        let test_config = test_mas_config(MapSupportedFeatures::NOTIFICATION_REGISTRATION);

        let mas_instance = MasInstance::create(local, test_config);
        assert!(!mas_instance.is_obex_connected());

        // Test OBEX CONNECT operation.
        {
            let conn_fut = mas_instance.connect();
            let mut conn_fut = pin!(conn_fut);
            exec.run_until_stalled(&mut conn_fut).expect_pending("waiting for response");

            let request_raw = expect_stream_item(&mut exec, &mut remote).expect("request");
            let request = RequestPacket::decode(&request_raw[..]).expect("can decode request");

            assert_eq!(request.code(), &OpCode::Connect);
            let expected_headers = HeaderSet::from_headers(vec![
                Header::Target(MAS_TARGET.as_bytes().to_vec()),
                Header::ApplicationParameters(vec![].into()),
            ])
            .unwrap();
            assert_eq!(request.headers(), &expected_headers);

            let mut response_fut = pin!(send_ok_response(
                &mut remote,
                vec![Header::ConnectionId(CONNECTION_ID.try_into().unwrap())],
                CONNECTION_RESPONSE_DATA.to_vec(),
            ));
            exec.run_until_stalled(&mut response_fut).expect("response should complete");
            let _ = exec.run_until_stalled(&mut conn_fut).expect("complete").expect("no error");
        }

        assert!(mas_instance.is_obex_connected());

        // Connecting again should resolve immediately.
        {
            let conn_fut = mas_instance.connect();
            let mut conn_fut = pin!(conn_fut);
            let _ = exec.run_until_stalled(&mut conn_fut).expect("complete").expect("no error");
        }
        exec.run_until_stalled(&mut remote.next())
            .expect_pending("should not have received request");
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn connect_with_map_supported_features(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (local, mut remote) = create_test_channels(transport);
        let test_config = test_mas_config(
            MapSupportedFeatures::NOTIFICATION_REGISTRATION
                | MapSupportedFeatures::MAPSUPPORTEDFEATURES_IN_CONNECT_REQUEST,
        );

        let mas_instance = MasInstance::create(local, test_config);
        assert!(!mas_instance.is_obex_connected());

        // Test connect.
        {
            let conn_fut = mas_instance.connect();
            let mut conn_fut = pin!(conn_fut);
            exec.run_until_stalled(&mut conn_fut).expect_pending("waiting for response");

            let request_raw = expect_stream_item(&mut exec, &mut remote).expect("request");
            let request = RequestPacket::decode(&request_raw[..]).expect("can decode request");

            assert_eq!(request.code(), &OpCode::Connect);
            let expected_headers = HeaderSet::from_headers(vec![
                Header::Target(MAS_TARGET.as_bytes().to_vec()),
                Header::ApplicationParameters(
                    vec![(MAP_SUPPORTED_FEATURES_TAG_ID, (0x243u32).to_be_bytes().to_vec())].into(),
                ),
            ])
            .unwrap();
            assert_eq!(request.headers(), &expected_headers);

            let mut response_fut = pin!(send_ok_response(
                &mut remote,
                vec![Header::ConnectionId(CONNECTION_ID.try_into().unwrap())],
                CONNECTION_RESPONSE_DATA.to_vec(),
            ));
            exec.run_until_stalled(&mut response_fut).expect("response should complete");

            let _ = exec.run_until_stalled(&mut conn_fut).expect("complete").expect("no error");
        }

        assert!(mas_instance.is_obex_connected());
    }

    /// Create a MasInstance for testing purposes with OBEX connection already set up.
    /// Used for testing non-connect OBEX operations.
    #[track_caller]
    fn mas_instance_with_obex_connection(
        exec: &mut fasync::TestExecutor,
        transport: Transport,
    ) -> (MasInstance, Channel) {
        let (local, mut remote) = create_test_channels(transport);
        let mas_instance = MasInstance::create(
            local,
            test_mas_config(
                MapSupportedFeatures::NOTIFICATION_REGISTRATION
                    | MapSupportedFeatures::NOTIFICATION,
            ),
        );

        {
            let conn_fut = mas_instance.connect();
            let mut conn_fut = pin!(conn_fut);
            exec.run_until_stalled(&mut conn_fut).expect_pending("waiting for response");

            let _ = expect_stream_item(exec, &mut remote).unwrap();
            let mut response_fut = pin!(send_ok_response(
                &mut remote,
                vec![Header::ConnectionId(CONNECTION_ID.try_into().unwrap())],
                CONNECTION_RESPONSE_DATA.to_vec(),
            ));
            exec.run_until_stalled(&mut response_fut).expect("response should complete");

            let _ = exec.run_until_stalled(&mut conn_fut).expect("complete").expect("no error");
        }
        assert_eq!(
            mas_instance.obex_client.connection_id(),
            Some(CONNECTION_ID.try_into().unwrap())
        );
        assert!(mas_instance.is_obex_connected());
        (mas_instance, remote)
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn set_notifications(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mas_instance, mut remote) = mas_instance_with_obex_connection(&mut exec, transport);
        assert!(!mas_instance.notification_registered());

        // Test turning on notifications.
        {
            let notification_fut = mas_instance.set_register_notification(true);
            let mut notification_fut = pin!(notification_fut);
            exec.run_until_stalled(&mut notification_fut).expect_pending("waiting for response");

            let request_raw = expect_stream_item(&mut exec, &mut remote).expect("request");
            let request = RequestPacket::decode(&request_raw[..]).expect("can decode request");

            assert_eq!(request.code(), &OpCode::PutFinal);
            let headers = request.headers();
            assert!(headers.contains_header(&HeaderIdentifier::ConnectionId));
            assert_eq!(
                headers.get(&HeaderIdentifier::Type),
                Some(&Header::Type(NOTIFICATION_TYPE.to_string().into()))
            );
            assert_eq!(
                headers.get(&HeaderIdentifier::ApplicationParameters),
                Some(&Header::ApplicationParameters(
                    vec![(NOTIFICATION_STATUS_TAG_ID, (0x1u8).to_be_bytes().to_vec(),)].into()
                ))
            );

            let mut response_fut = pin!(send_ok_response(&mut remote, vec![], vec![]));
            exec.run_until_stalled(&mut response_fut).expect("response should complete");

            let _ =
                exec.run_until_stalled(&mut notification_fut).expect("complete").expect("no error");
        }
        assert!(mas_instance.notification_registered());

        // Test turning off notifications.
        {
            let notification_fut = mas_instance.set_register_notification(false);
            let mut notification_fut = pin!(notification_fut);
            exec.run_until_stalled(&mut notification_fut).expect_pending("waiting for response");

            let request_raw = expect_stream_item(&mut exec, &mut remote).expect("request");
            let request = RequestPacket::decode(&request_raw[..]).expect("can decode request");

            assert_eq!(request.code(), &OpCode::PutFinal);
            let headers = request.headers();
            assert_eq!(
                headers.get(&HeaderIdentifier::ApplicationParameters),
                Some(&Header::ApplicationParameters(
                    vec![(NOTIFICATION_STATUS_TAG_ID, (0x0u8).to_be_bytes().to_vec(),)].into()
                ))
            );

            let mut response_fut = pin!(send_ok_response(&mut remote, vec![], vec![]));
            exec.run_until_stalled(&mut response_fut).expect("response should complete");

            let _ =
                exec.run_until_stalled(&mut notification_fut).expect("complete").expect("no error");
        }
        assert!(!mas_instance.notification_registered());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn set_notifications_connect_first(transport: Transport) {
        // If obex was not connected, we should attempt to connect first.
        let mut exec = fasync::TestExecutor::new();
        let (local, mut remote) = create_test_channels(transport);
        let mas_instance = MasInstance::create(
            local,
            test_mas_config(
                MapSupportedFeatures::NOTIFICATION_REGISTRATION
                    | MapSupportedFeatures::NOTIFICATION,
            ),
        );
        assert!(!mas_instance.is_obex_connected());

        {
            let notification_fut = mas_instance.set_register_notification(true);
            let mut notification_fut = pin!(notification_fut);

            exec.run_until_stalled(&mut notification_fut).expect_pending("should be pending");

            // We should have sent connect request.
            let request_raw = expect_stream_item(&mut exec, &mut remote).expect("request");
            let request = RequestPacket::decode(&request_raw[..]).expect("can decode request");

            assert_eq!(request.code(), &OpCode::Connect);

            {
                let mut response_fut = pin!(send_ok_response(
                    &mut remote,
                    vec![Header::ConnectionId(CONNECTION_ID.try_into().unwrap())],
                    CONNECTION_RESPONSE_DATA.to_vec(),
                ));
                exec.run_until_stalled(&mut response_fut).expect("response should complete");
            }

            exec.run_until_stalled(&mut notification_fut).expect_pending("waiting for response");

            // After connect, we finally send set notification registration request.
            let request_raw = expect_stream_item(&mut exec, &mut remote).expect("request");
            let request = RequestPacket::decode(&request_raw[..]).expect("can decode request");
            assert_eq!(request.code(), &OpCode::PutFinal);
        }
    }
}
