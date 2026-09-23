// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::{DecodePaddingMode, Engine as _};
use clap::Parser;
use fidl::client::QueryResponseFut;
use fidl::endpoints::create_request_stream;
use fidl_fuchsia_bluetooth::Appearance;
use fidl_fuchsia_bluetooth_le::{
    AdvertisedPeripheralMarker, AdvertisedPeripheralRequest, AdvertisedPeripheralRequestStream,
    AdvertisingData, AdvertisingModeHint, AdvertisingParameters, ConnectionOptions,
    ConnectionProxy, ManufacturerData, PeripheralError, PeripheralMarker, PeripheralProxy,
    ServiceData,
};
use fuchsia_bluetooth::assigned_numbers::find_service_uuid;
use fuchsia_bluetooth::types::Uuid;
use fuchsia_bluetooth::types::le::Peer;
use fuchsia_component::client::connect_to_protocol;
use futures::{StreamExt, select};

// TODO(armansito): Add ability to construct a valid fuchsia.bluetooth.Appearance from a string.
// Defines all the command line arguments accepted by the tool.
#[derive(Parser, Debug)]
#[command()]
struct Opt {
    #[arg(short = 'n', long = "name", help = "Advertised Device Name")]
    name: Option<String>,
    #[arg(
        short = 's',
        long = "service",
        help = "Advertised Service UUIDs. Multiple instances of the flag allowed."
    )]
    service_uuids: Vec<String>,
    #[arg(short = 'c', long = "connectable", help = "Advertise as connectable")]
    connectable: bool,
    #[arg(
        short = 'm',
        long = "mode-hint",
        value_parser = parse_mode_hint,
        help = "Advertising mode hint (\"fast\", \"slow\", \"very-fast\")"
    )]
    mode_hint: Option<AdvertisingModeHint>,
    #[arg(long = "appearance", help = "Advertised appearance as integer value")]
    appearance: Option<u16>,
    #[arg(
        short = 'u',
        help = "URIs included in the advertising packet. Multiple instances of the flag allowed."
    )]
    uris: Vec<String>,
    #[arg(
        long = "service-data",
        value_parser = parse_service_data,
        help = "Service data in the format '<service_uuid>:<string_data>'. \
                Multiple instances of the flag allowed."
    )]
    service_data: Vec<ServiceData>,
    #[arg(
        long = "binary-service-data",
        value_parser = parse_binary_service_data,
        help = "Service data in the format '<service_uuid>:<base64_data>'. \
                Multiple instances of the flag allowed."
    )]
    binary_service_data: Vec<ServiceData>,
    #[arg(
        long = "manufacturer-data",
        value_parser = parse_manufacturer_data,
        help = "Manufacturer specific data in the format '<company_id>:<string_data>'. \
                Multiple instances of the flag allowed."
    )]
    manufacturer_data: Vec<ManufacturerData>,
    #[arg(
        long = "binary-manufacturer-data",
        value_parser = parse_binary_manufacturer_data,
        help = "Manufacturer specific data in the format '<company_id>:<base64_data>'. \
                Multiple instances of the flag allowed."
    )]
    binary_manufacturer_data: Vec<ManufacturerData>,
}

fn parse_service_uuid(raw: &str) -> Result<Uuid, Error> {
    match find_service_uuid(raw) {
        Some(assigned_number) => Ok(Uuid::new16(assigned_number.number)),
        None => raw.parse::<Uuid>().map_err(|_| format_err!("invalid UUID: {}", raw)),
    }
}

fn parse_service_uuids(raw: Vec<String>) -> Result<Vec<Uuid>, Error> {
    raw.into_iter().map(|id| parse_service_uuid(&id)).collect()
}

/// Parse a raw string as a millisecond interval checking that it lies within the allowed range.
fn parse_mode_hint(raw: &str) -> Result<AdvertisingModeHint, Error> {
    match raw {
        "very-fast" => Ok(AdvertisingModeHint::VeryFast),
        "fast" => Ok(AdvertisingModeHint::Fast),
        "slow" => Ok(AdvertisingModeHint::Slow),
        other => Err(format_err!("invalid advertising mode hint: {}", other)),
    }
}

/// Parse ":" delimited pair from a string into a tuple pair of (id, utf8 encoded payload)
fn parse_data(raw: &str) -> Result<(&str, Vec<u8>), Error> {
    let elements: Vec<_> = raw.split(":").collect();
    if elements.len() != 2 {
        return Err(format_err!("Argument must be a ':' delimited pair."));
    }
    let id = elements[0];
    let payload = elements[1].to_string().into_bytes();
    Ok((id, payload))
}

/// Parse ":" delimited pair from a string into a tuple pair of (id, raw byte payload) where the
/// payload was base64 encoded in the raw string.
fn parse_binary_data(raw: &str) -> Result<(&str, Vec<u8>), Error> {
    let elements: Vec<_> = raw.split(":").collect();
    if elements.len() != 2 {
        return Err(format_err!("Argument must be a ':' delimited pair."));
    }
    let id = elements[0];
    // Allow for payloads that do not have canonical padding.
    let base64_indifferent_padding = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    let payload = base64_indifferent_padding.decode(elements[1])?;
    Ok((id, payload))
}

fn parse_service_data(raw: &str) -> Result<ServiceData, Error> {
    let (raw_id, data) = parse_data(raw)?;
    let uuid = parse_service_uuid(raw_id)?.into();
    Ok(ServiceData { uuid, data })
}

fn parse_binary_service_data(raw: &str) -> Result<ServiceData, Error> {
    let (raw_id, data) = parse_binary_data(raw)?;
    let uuid = parse_service_uuid(raw_id)?.into();
    Ok(ServiceData { uuid, data })
}

fn parse_manufacturer_data(raw: &str) -> Result<ManufacturerData, Error> {
    let (raw_id, data) = parse_data(raw)?;
    let raw_id = if raw_id.starts_with("0x") { &raw_id[2..] } else { raw_id };
    let company_id = u16::from_str_radix(raw_id, 16)?;
    Ok(ManufacturerData { company_id, data })
}

fn parse_binary_manufacturer_data(raw: &str) -> Result<ManufacturerData, Error> {
    let (raw_id, data) = parse_binary_data(raw)?;
    let raw_id = if raw_id.starts_with("0x") { &raw_id[2..] } else { raw_id };
    let company_id = u16::from_str_radix(raw_id, 16)?;
    Ok(ManufacturerData { company_id, data })
}

/// Wrap a `Vec<T>` in an `Option`, returning `None` if the vector is empty and `Some(vec)` if the
/// vector is not empty.
fn optionalize<T>(vec: Vec<T>) -> Option<Vec<T>> {
    if vec.is_empty() { None } else { Some(vec) }
}

/// Start advertising and print status on success or construct error on failure
/// On success, returns a stream of connection requests and a future representing the advertising request. Error otherwise.
fn advertise(
    peripheral: &PeripheralProxy,
    parameters: AdvertisingParameters,
    service_names: &[String],
) -> Result<(AdvertisedPeripheralRequestStream, QueryResponseFut<Result<(), PeripheralError>>), Error>
{
    let (client_end, server_stream) = create_request_stream::<AdvertisedPeripheralMarker>();
    let name = match &parameters.data {
        Some(data) => data.name.clone(),
        None => None,
    };
    eprintln!("Advertising with name \"{:?}\" and services: {}", name, service_names.join(", "),);

    // advertise() only resolves on advertisement termination, so don't await here.
    let result_fut = peripheral.advertise(&parameters, client_end);
    Ok((server_stream, result_fut))
}

async fn await_connected(
    mut adv_peripheral_stream: AdvertisedPeripheralRequestStream,
    mut adv_result_fut: QueryResponseFut<Result<(), PeripheralError>>,
) -> Result<ConnectionProxy, Error> {
    let adv_peripheral_req = select! {
        item = adv_peripheral_stream.next() => match item {
            Some(Ok(request)) => request,
            Some(Err(err)) => return Err(format_err!("AdvertisedPeripheral disconnected with error: {:?}", err)),
            None => return Err(format_err!("AdvertisedPeripheral disconnected")),
        },
        result = adv_result_fut => match result {
            Ok(Ok(())) => return Err(format_err!("Advertise returned")),
            result => return Err(format_err!("Advertise returned with error: {:?}", result)),
        },
    };

    let AdvertisedPeripheralRequest::OnConnected { peer, connection, responder, .. } =
        adv_peripheral_req;
    responder.send()?;

    eprintln!("Connected to central: {}", Peer::try_from(peer)?);
    Ok(connection.into_proxy())
}

// This functions implements the main behavior of this tool which involves:
// - Advertising using the given parameters;
// - Awaiting for any incoming connection;
// - Waiting until the connection drops.
async fn listen(
    peripheral: &PeripheralProxy,
    parameters: AdvertisingParameters,
    service_names: &[String],
) -> Result<(), Error> {
    let (adv_peripheral_stream, adv_result_fut) =
        advertise(&peripheral, parameters, &service_names)?;

    // Wait for the next connection request or error.
    let connection = await_connected(adv_peripheral_stream, adv_result_fut).await?;

    // Wait until the connection drops.
    let mut conn_events = connection.take_event_stream();
    while let Some(_) = conn_events.next().await {}

    eprintln!("Central disconnected");
    Ok(())
}

#[fuchsia::main(logging = false)]
async fn main() -> Result<(), Error> {
    // Extract arguments and perform additional transformation of incoming arguments
    let Opt {
        name,
        service_uuids,
        connectable,
        mode_hint,
        appearance,
        uris,
        mut service_data,
        binary_service_data,
        mut manufacturer_data,
        binary_manufacturer_data,
    } = Opt::parse();

    let appearance = match appearance {
        Some(a) => Appearance::from_primitive(a),
        None => None,
    };
    let service_uuids = parse_service_uuids(service_uuids)?;
    let service_names: Vec<_> = service_uuids.iter().map(Uuid::to_string).collect();
    service_data.extend(binary_service_data);
    manufacturer_data.extend(binary_manufacturer_data);

    let conn_opts = if connectable {
        Some(ConnectionOptions {
            bondable_mode: Some(true),
            service_filter: None,
            ..Default::default()
        })
    } else {
        None
    };
    // unchanging advertising data used for the lifetime of the program
    let params = AdvertisingParameters {
        data: Some(AdvertisingData {
            name,
            appearance,
            tx_power_level: None,
            service_uuids: optionalize(service_uuids.iter().map(Into::into).collect()),
            service_data: optionalize(service_data),
            manufacturer_data: optionalize(manufacturer_data),
            uris: optionalize(uris),
            ..Default::default()
        }),
        scan_response: None,
        mode_hint,
        connectable: None,
        connection_options: conn_opts,
        ..Default::default()
    };

    let peripheral = connect_to_protocol::<PeripheralMarker>()
        .context("failed to connect to bluetooth peripheral service")?;
    listen(&peripheral, params, &service_names).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::{ServerEnd, create_endpoints, create_proxy, create_proxy_and_stream};
    use fidl_fuchsia_bluetooth_le::{
        AdvertisedPeripheralProxy, ConnectionMarker, Peer, PeripheralRequest,
        PeripheralRequestStream,
    };
    use fuchsia_bluetooth::types::le::{ManufacturerData, ServiceData};
    use futures::{TryStreamExt, join};

    #[test]
    fn test_parse_service_uuid() {
        let uuid = parse_service_uuid("180d").expect("failed to parse UUID");
        assert_eq!(Uuid::new16(0x180d), uuid);
    }

    #[test]
    fn test_parse_service_uuid_error() {
        let uuid = parse_service_uuid("💩");
        assert!(uuid.is_err());
    }

    #[test]
    fn test_parse_service_uuids() {
        let uuids = parse_service_uuids(vec!["180d".to_string()]).expect("failed to parse UUID");
        assert_eq!(vec![Uuid::new16(0x180d)], uuids);
    }

    #[test]
    fn test_parse_service_uuids_error() {
        let uuids = parse_service_uuids(vec!["💩".to_string()]);
        assert!(uuids.is_err());
    }

    #[test]
    fn test_parse_mode_hint() {
        let mode = parse_mode_hint("very-fast").expect("failed to parse mode hint");
        assert_eq!(AdvertisingModeHint::VeryFast, mode);
        let mode = parse_mode_hint("fast").expect("failed to parse mode hint");
        assert_eq!(AdvertisingModeHint::Fast, mode);
        let mode = parse_mode_hint("slow").expect("failed to parse mode hint");
        assert_eq!(AdvertisingModeHint::Slow, mode);
        let mode = parse_mode_hint("💩");
        assert!(mode.is_err());
    }

    #[test]
    fn test_parse_service_data() {
        let data = parse_service_data("180d:hello").expect("failed to parse data");
        let data: ServiceData = data.into();
        let expected = ServiceData {
            uuid: Uuid::new16(0x180d),
            data: vec!['h' as u8, 'e' as u8, 'l' as u8, 'l' as u8, 'o' as u8],
        };
        assert_eq!(expected, data);
    }

    #[test]
    fn test_parse_service_data_error() {
        let data = parse_service_data("180dhello");
        assert!(data.is_err());
        let data = parse_service_data("💩:hello");
        assert!(data.is_err());
    }

    #[test]
    fn test_parse_binary_service_data() {
        let data = parse_binary_service_data("180d:cG9vcA").expect("failed to parse data");
        let data: ServiceData = data.into();
        let expected = ServiceData {
            uuid: Uuid::new16(0x180d),
            data: vec!['p' as u8, 'o' as u8, 'o' as u8, 'p' as u8],
        };
        assert_eq!(expected, data);
    }

    #[test]
    fn test_parse_binary_service_data_error() {
        let data = parse_binary_service_data("180dcG9vcA");
        assert!(data.is_err());
        let data = parse_binary_service_data("💩:cG9vcA");
        assert!(data.is_err());
    }

    #[test]
    fn test_parse_manufacturer_data() {
        let data = parse_manufacturer_data("180d:hello").expect("failed to parse data");
        let data: ManufacturerData = data.into();
        let expected = ManufacturerData {
            company_id: 0x180d,
            data: vec!['h' as u8, 'e' as u8, 'l' as u8, 'l' as u8, 'o' as u8],
        };
        assert_eq!(expected, data);
    }

    #[test]
    fn test_parse_manufacturer_data_error() {
        let data = parse_manufacturer_data("180dhello");
        assert!(data.is_err());
        let data = parse_manufacturer_data("💩:hello");
        assert!(data.is_err());
    }

    #[test]
    fn test_parse_binary_manufacturer_data() {
        let data = parse_binary_manufacturer_data("180d:cG9vcA").expect("failed to parse data");
        let data: ManufacturerData = data.into();
        let expected = ManufacturerData {
            company_id: 0x180d,
            data: vec!['p' as u8, 'o' as u8, 'o' as u8, 'p' as u8],
        };
        assert_eq!(expected, data);
    }

    #[test]
    fn test_parse_binary_manufacturer_data_error() {
        let data = parse_binary_manufacturer_data("180dcG9vcA");
        assert!(data.is_err());
        let data = parse_binary_manufacturer_data("💩:cG9vcA");
        assert!(data.is_err());
    }

    struct MockPeripheral {
        _stream: PeripheralRequestStream,
        _adv_peripheral: AdvertisedPeripheralProxy,
        adv_params: Option<AdvertisingParameters>,
    }

    // Emulate the Peripheral service until advertising is started, emulate a connection and return
    // all handles.
    async fn emulate_peripheral(
        mut stream: PeripheralRequestStream,
    ) -> Result<(MockPeripheral, ServerEnd<ConnectionMarker>), Error> {
        match stream.try_next().await? {
            None => Err(format_err!("le.Peripheral connection failed")),
            Some(PeripheralRequest::Advertise {
                parameters,
                advertised_peripheral,
                responder,
                ..
            }) => {
                let (conn_client_end, conn_server_end) = create_endpoints::<ConnectionMarker>();
                let peer = Peer {
                    id: Some(fidl_fuchsia_bluetooth::PeerId { value: 1 }),
                    connectable: Some(true),
                    rssi: None,
                    advertising_data: None,
                    ..Default::default()
                };

                let proxy = advertised_peripheral.into_proxy();
                let _ = proxy.on_connected(&peer, conn_client_end).await;

                responder.send(Ok(()))?;

                let mock = MockPeripheral {
                    _stream: stream,
                    _adv_peripheral: proxy,
                    adv_params: Some(parameters),
                };
                Ok((mock, conn_server_end))
            }
            Some(x) => Err(format_err!("le.Peripheral method {:?} not implemented", x)),
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_listen() {
        let (proxy, stream) = create_proxy_and_stream::<PeripheralMarker>();

        let input_parameters = AdvertisingParameters {
            data: None,
            scan_response: None,
            mode_hint: Some(AdvertisingModeHint::Slow),
            connectable: None,
            connection_options: Some(ConnectionOptions {
                bondable_mode: Some(true),
                service_filter: None,
                ..Default::default()
            }),
            ..Default::default()
        };
        let listen_task = listen(&proxy, input_parameters, &[]);
        let emulate_task = async {
            let (mock_data, connection) =
                emulate_peripheral(stream).await.expect("le.Peripheral emulation failed");
            drop(connection);
            Ok(mock_data) as Result<MockPeripheral, anyhow::Error>
        };
        let (mock_data, listen) = join!(emulate_task, listen_task);

        assert!(listen.is_ok());
        let mock_data = mock_data.expect("emulate task failed");
        let adv_params = mock_data.adv_params.expect("advertising was not enabled!");
        assert_eq!(AdvertisingModeHint::Slow, adv_params.mode_hint.unwrap());
        let received_connection_options = adv_params.connection_options.unwrap();
        assert!(received_connection_options.bondable_mode.unwrap());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_listen_peripheral_server_closes_immediately() {
        let (proxy, server) = create_proxy::<PeripheralMarker>();

        let input_parameters = AdvertisingParameters {
            data: None,
            scan_response: None,
            mode_hint: Some(AdvertisingModeHint::Slow),
            connectable: None,
            connection_options: Some(ConnectionOptions {
                bondable_mode: Some(true),
                service_filter: None,
                ..Default::default()
            }),
            ..Default::default()
        };

        drop(server);
        let result = listen(&proxy, input_parameters, &[]).await;
        assert!(result.is_err());
    }
}
