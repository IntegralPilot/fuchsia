// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use bt_rfcomm::ServerChannel;
use bt_rfcomm::profile::{is_rfcomm_protocol, server_channel_from_protocol};
use fuchsia_bluetooth::profile::{DataElement, Psm, ServiceDefinition};
use std::collections::HashSet;

/// Returns true if `service` requests RFCOMM at most once across its primary
/// and additional protocol descriptor lists.
///
/// Standard Bluetooth SIG profiles specify at most one RFCOMM channel per ServiceDefinition.
/// `bt-rfcomm` allocates exactly one ServerChannel per ServiceDefinition.
///
/// TODO(https://fxbug.dev/534889939): If a future service requires multiple RFCOMM channels
/// within a single ServiceDefinition, extend `ServiceGroup` and this function to allocate
/// and assign multiple `ServerChannel`s per `ServiceDefinition`.
pub fn check_service_definition(service: &ServiceDefinition) -> Result<bool, Error> {
    let primary_rfcomm = usize::from(is_rfcomm_protocol(&service.protocol_descriptor_list));
    let additional_rfcomm = service
        .additional_protocol_descriptor_lists
        .iter()
        .filter(|list| is_rfcomm_protocol(list))
        .count();

    match primary_rfcomm + additional_rfcomm {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(format_err!("ServiceDefinition requests multiple RFCOMM channels")),
    }
}

/// Updates `service` with `server_channel` if the service requests RFCOMM.
///
/// Updates the RFCOMM protocol descriptor in either the primary protocol descriptor list
/// or additional protocol descriptor lists. Assumes `service` is valid and requests RFCOMM
/// exactly once.
pub fn update_svc_def_with_server_channel(
    service: &mut ServiceDefinition,
    server_channel: ServerChannel,
) -> Result<(), Error> {
    if !check_service_definition(service)? {
        return Err(format_err!("Non-RFCOMM service definition provided"));
    }

    for desc in service.protocol_descriptor_list.iter_mut() {
        if desc.protocol == fidl_fuchsia_bluetooth_bredr::ProtocolIdentifier::Rfcomm {
            desc.params = vec![DataElement::Uint8(server_channel.into())];
            return Ok(());
        }
    }

    for list in service.additional_protocol_descriptor_lists.iter_mut() {
        for desc in list.iter_mut() {
            if desc.protocol == fidl_fuchsia_bluetooth_bredr::ProtocolIdentifier::Rfcomm {
                desc.params = vec![DataElement::Uint8(server_channel.into())];
                return Ok(());
            }
        }
    }

    Ok(())
}

/// Returns the service definitions that are in `new` but not in `current`.
pub fn service_def_difference(
    current: &Vec<ServiceDefinition>,
    new: &Vec<ServiceDefinition>,
) -> Vec<ServiceDefinition> {
    let current_set: std::collections::HashSet<&ServiceDefinition> = current.iter().collect();
    new.iter().filter(|&definition| !current_set.contains(definition)).cloned().collect()
}

/// Returns true if the provided `service` is valid and requests RFCOMM.
pub fn is_rfcomm_service_definition(service: &ServiceDefinition) -> bool {
    check_service_definition(service).unwrap_or(false)
}

/// Returns `Ok(true)` if all services are valid and at least one requests RFCOMM.
/// Returns `Ok(false)` if all services are valid and none request RFCOMM.
/// Returns `Err(...)` if any service requests RFCOMM multiple times.
pub fn service_definitions_request_rfcomm(
    services: &Vec<ServiceDefinition>,
) -> Result<bool, Error> {
    let mut requests_rfcomm = false;
    for service in services {
        if check_service_definition(service)? {
            requests_rfcomm = true;
        }
    }
    Ok(requests_rfcomm)
}

/// Returns the server channels specified in `services`. It's possible that
/// none of the `services` request a ServerChannel in which case the returned set
/// will be empty.
pub fn server_channels_from_service_definitions(
    services: &Vec<ServiceDefinition>,
) -> HashSet<ServerChannel> {
    services
        .iter()
        .flat_map(|def| {
            let mut channels = HashSet::new();
            if let Some(sc) = server_channel_from_protocol(&def.protocol_descriptor_list) {
                let _ = channels.insert(sc);
            }
            for list in &def.additional_protocol_descriptor_lists {
                if let Some(sc) = server_channel_from_protocol(list) {
                    let _ = channels.insert(sc);
                }
            }
            channels
        })
        .collect()
}

/// Returns a set of PSMs specified by a list of `services`.
pub fn psms_from_service_definitions(services: &Vec<ServiceDefinition>) -> HashSet<Psm> {
    services.iter().fold(HashSet::new(), |mut psms, service| {
        psms.extend(&service.psm_set());
        psms
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use fidl_fuchsia_bluetooth_bredr as bredr;
    use fuchsia_bluetooth::profile::{Attribute, ProtocolDescriptor};

    #[test]
    fn update_empty_service_definition_is_error() {
        let server_channel = ServerChannel::try_from(10).unwrap();
        let mut def = ServiceDefinition::default();

        // Empty definition doesn't request RFCOMM - shouldn't be updated.
        let result = update_svc_def_with_server_channel(&mut def, server_channel);
        assert_matches!(result, Err(_));

        let expected = ServiceDefinition::default();
        assert_eq!(def, expected);
    }

    #[test]
    fn update_non_rfcomm_service_definition_is_error() {
        let server_channel = ServerChannel::try_from(8).unwrap();
        let mut def = ServiceDefinition {
            protocol_descriptor_list: vec![ProtocolDescriptor {
                protocol: bredr::ProtocolIdentifier::L2Cap,
                params: vec![],
            }],
            ..ServiceDefinition::default()
        };
        let expected = def.clone();

        // Only L2CAP definition cannot be updated with RFCOMM.
        let result = update_svc_def_with_server_channel(&mut def, server_channel);
        assert_matches!(result, Err(_));
        // The original `def` should be unchanged.
        assert_eq!(def, expected);
    }

    #[test]
    fn update_service_definition_with_rfcomm() {
        let server_channel = ServerChannel::try_from(10).unwrap();
        let mut def = ServiceDefinition {
            protocol_descriptor_list: vec![
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::L2Cap, params: vec![] },
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::Rfcomm, params: vec![] },
            ],
            ..ServiceDefinition::default()
        };

        // Normal case - definition is requesting RFCOMM. It should be updated with the
        // server channel.
        let result = update_svc_def_with_server_channel(&mut def, server_channel);
        assert_matches!(result, Ok(_));

        let expected = ServiceDefinition {
            protocol_descriptor_list: vec![
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::L2Cap, params: vec![] },
                ProtocolDescriptor {
                    protocol: bredr::ProtocolIdentifier::Rfcomm,
                    params: vec![DataElement::Uint8(10)],
                },
            ],
            ..ServiceDefinition::default()
        };
        assert_eq!(def, expected);
    }

    #[test]
    fn update_obex_service_definition_with_rfcomm() {
        let server_channel = ServerChannel::try_from(12).unwrap();
        let mut def = ServiceDefinition {
            protocol_descriptor_list: vec![
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::L2Cap, params: vec![] },
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::Rfcomm, params: vec![] },
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::Obex, params: vec![] },
            ],
            ..ServiceDefinition::default()
        };

        // Definition is requesting RFCOMM, but OBEX is the "highest" protocol level, not RFCOMM.
        let result = update_svc_def_with_server_channel(&mut def, server_channel);
        assert_matches!(result, Ok(_));

        // We expect the RFCOMM descriptor to be updated and the OBEX descriptor should still be
        // preserved.
        let expected = ServiceDefinition {
            protocol_descriptor_list: vec![
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::L2Cap, params: vec![] },
                ProtocolDescriptor {
                    protocol: bredr::ProtocolIdentifier::Rfcomm,
                    params: vec![DataElement::Uint8(12)],
                },
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::Obex, params: vec![] },
            ],
            ..ServiceDefinition::default()
        };
        assert_eq!(def, expected);
    }

    #[test]
    fn service_definition_difference() {
        let mut current = vec![];
        let mut new = vec![];
        assert_eq!(service_def_difference(&current, &new), vec![]);

        let def1 = ServiceDefinition {
            protocol_descriptor_list: vec![
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::L2Cap, params: vec![] },
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::Rfcomm, params: vec![] },
            ],
            ..ServiceDefinition::default()
        };

        new = vec![def1.clone()];
        assert_eq!(service_def_difference(&current, &new), vec![def1.clone()]);

        let def2 = ServiceDefinition {
            protocol_descriptor_list: vec![
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::L2Cap, params: vec![] },
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::Rfcomm, params: vec![] },
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::Obex, params: vec![] },
            ],
            ..ServiceDefinition::default()
        };
        new.push(def2.clone());
        assert_eq!(service_def_difference(&current, &new), vec![def1.clone(), def2.clone()]);

        current.push(def1);
        assert_eq!(service_def_difference(&current, &new), vec![def2.clone()]);

        current.push(def2);
        assert_eq!(service_def_difference(&current, &new), vec![]);
    }

    #[test]
    fn psm_from_service_definitions() {
        // Service 1 is only L2CAP.
        let def1 = ServiceDefinition {
            protocol_descriptor_list: vec![ProtocolDescriptor {
                protocol: bredr::ProtocolIdentifier::L2Cap,
                params: vec![DataElement::Uint16(21)],
            }],
            additional_protocol_descriptor_lists: vec![
                vec![ProtocolDescriptor {
                    protocol: bredr::ProtocolIdentifier::L2Cap,
                    params: vec![DataElement::Uint16(23)],
                }],
                vec![ProtocolDescriptor {
                    protocol: bredr::ProtocolIdentifier::Avdtp,
                    params: vec![DataElement::Uint16(0x0103)],
                }],
            ],
            ..ServiceDefinition::default()
        };
        // Service 2 is RFCOMM + L2CAP (OBEX).
        let def2 = ServiceDefinition {
            protocol_descriptor_list: vec![
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::L2Cap, params: vec![] },
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::Rfcomm, params: vec![] },
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::Obex, params: vec![] },
            ],
            additional_attributes: vec![Attribute {
                id: 0x0200,
                element: DataElement::Uint16(2000),
            }],
            ..ServiceDefinition::default()
        };

        let psms = psms_from_service_definitions(&vec![def1, def2]);

        // Expect to contain all of the PSMs that are specified in the record. Unallocated PSMs
        // (e.g. RFCOMM) aren't included.
        let expected_psms = HashSet::from([Psm::new(21), Psm::new(23), Psm::new(2000)]);
        assert_eq!(psms, expected_psms);
    }

    #[test]
    fn check_service_definition_test() {
        let mut def = ServiceDefinition {
            protocol_descriptor_list: vec![ProtocolDescriptor {
                protocol: bredr::ProtocolIdentifier::L2Cap,
                params: vec![],
            }],
            ..ServiceDefinition::default()
        };
        assert_matches!(check_service_definition(&def), Ok(false));

        def.protocol_descriptor_list.push(ProtocolDescriptor {
            protocol: bredr::ProtocolIdentifier::Rfcomm,
            params: vec![DataElement::Uint8(0)],
        });
        assert_matches!(check_service_definition(&def), Ok(true));

        def.additional_protocol_descriptor_lists = vec![vec![ProtocolDescriptor {
            protocol: bredr::ProtocolIdentifier::Rfcomm,
            params: vec![DataElement::Uint8(30)],
        }]];
        assert_matches!(check_service_definition(&def), Err(_));
    }

    #[test]
    fn update_service_definition_in_additional_protocol_list() {
        let server_channel = ServerChannel::try_from(14).unwrap();
        let mut def = ServiceDefinition {
            protocol_descriptor_list: vec![ProtocolDescriptor {
                protocol: bredr::ProtocolIdentifier::L2Cap,
                params: vec![],
            }],
            additional_protocol_descriptor_lists: vec![vec![
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::L2Cap, params: vec![] },
                ProtocolDescriptor {
                    protocol: bredr::ProtocolIdentifier::Rfcomm,
                    params: vec![DataElement::Uint8(25)],
                },
            ]],
            ..ServiceDefinition::default()
        };

        assert!(is_rfcomm_service_definition(&def));

        let result = update_svc_def_with_server_channel(&mut def, server_channel);
        assert_matches!(result, Ok(_));

        let expected = ServiceDefinition {
            protocol_descriptor_list: vec![ProtocolDescriptor {
                protocol: bredr::ProtocolIdentifier::L2Cap,
                params: vec![],
            }],
            additional_protocol_descriptor_lists: vec![vec![
                ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::L2Cap, params: vec![] },
                ProtocolDescriptor {
                    protocol: bredr::ProtocolIdentifier::Rfcomm,
                    params: vec![DataElement::Uint8(14)],
                },
            ]],
            ..ServiceDefinition::default()
        };
        assert_eq!(def, expected);
    }

    #[test]
    fn server_channels_from_all_service_definition_lists() {
        let def1 = ServiceDefinition {
            protocol_descriptor_list: vec![ProtocolDescriptor {
                protocol: bredr::ProtocolIdentifier::Rfcomm,
                params: vec![DataElement::Uint8(10)],
            }],
            ..ServiceDefinition::default()
        };
        let def2 = ServiceDefinition {
            protocol_descriptor_list: vec![ProtocolDescriptor {
                protocol: bredr::ProtocolIdentifier::L2Cap,
                params: vec![],
            }],
            additional_protocol_descriptor_lists: vec![vec![ProtocolDescriptor {
                protocol: bredr::ProtocolIdentifier::Rfcomm,
                params: vec![DataElement::Uint8(12)],
            }]],
            ..ServiceDefinition::default()
        };

        let channels = server_channels_from_service_definitions(&vec![def1, def2]);
        let expected = HashSet::from([
            ServerChannel::try_from(10).unwrap(),
            ServerChannel::try_from(12).unwrap(),
        ]);
        assert_eq!(channels, expected);
    }
}
