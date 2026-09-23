// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_bluetooth::types::PeerId;
use futures::{Future, StreamExt};
use log::{debug, info, warn};
use std::collections::HashSet;
use {fidl_fuchsia_bluetooth as fidl_bt, fidl_fuchsia_bluetooth_bredr as bredr};

use crate::codec_id::CodecId;

use super::{ConnectError, Connection};

#[derive(Clone)]
pub struct Connector {
    proxy: bredr::ProfileProxy,
    controller_codecs: HashSet<CodecId>,
}

fn common_sco_params() -> bredr::ScoConnectionParameters {
    bredr::ScoConnectionParameters {
        air_frame_size: Some(60), // Chosen to match legacy usage.
        // IO parameters are to fit 16-bit PSM Signed audio input expected from the audio chip.
        io_coding_format: Some(fidl_bt::AssignedCodingFormat::LinearPcm),
        io_frame_size: Some(16),
        io_pcm_data_format: Some(fidl_fuchsia_hardware_audio::SampleFormat::PcmSigned),
        path: Some(bredr::DataPath::Offload),
        ..Default::default()
    }
}

/// If all eSCO parameters fail to setup a connection, these parameters are required to be
/// supported by all peers.  HFP 1.8 Section 5.7.1.
fn sco_params_fallback() -> bredr::ScoConnectionParameters {
    bredr::ScoConnectionParameters {
        parameter_set: Some(bredr::HfpParameterSet::D1),
        air_coding_format: Some(fidl_bt::AssignedCodingFormat::Cvsd),
        // IO bandwidth to match an 8khz audio rate.
        io_bandwidth: Some(16000),
        ..common_sco_params()
    }
}

fn params_with_data_path(
    sco_params: bredr::ScoConnectionParameters,
    in_band_sco: bool,
) -> bredr::ScoConnectionParameters {
    bredr::ScoConnectionParameters {
        path: in_band_sco.then_some(bredr::DataPath::Host).or(Some(bredr::DataPath::Offload)),
        ..sco_params
    }
}

// pub in this crate for tests
pub(crate) fn parameter_sets_for_codec(
    codec_id: CodecId,
    in_band_sco: bool,
) -> Vec<bredr::ScoConnectionParameters> {
    use bredr::HfpParameterSet::*;
    // Parameter sets from the HFP Spec, Section 5.7
    // Core spec 5.3 requires the air coding and io coding formats both be transparent or neither
    // should be.
    // The parameter sets returned also meet this criteria.
    match codec_id {
        CodecId::MSBC => {
            let (air_coding_format, io_bandwidth, io_coding_format) = if in_band_sco {
                (
                    Some(fidl_bt::AssignedCodingFormat::Transparent),
                    Some(16000),
                    Some(fidl_bt::AssignedCodingFormat::Transparent),
                )
            } else {
                // IO bandwidth to match an 16khz audio rate. (x2 for input + output)
                (
                    Some(fidl_bt::AssignedCodingFormat::Msbc),
                    Some(32000),
                    Some(fidl_bt::AssignedCodingFormat::LinearPcm),
                )
            };
            let params_fn = |set| bredr::ScoConnectionParameters {
                parameter_set: Some(set),
                air_coding_format,
                io_coding_format,
                io_bandwidth,
                ..params_with_data_path(common_sco_params(), in_band_sco)
            };
            // TODO(b/200305833): Disable MsbcT1 for now as it results in bad audio
            //vec![params_fn(T2), params_fn(T1)]
            vec![params_fn(T2)]
        }
        // CVSD parameter sets
        _ => {
            let (io_bandwidth, io_frame_size, io_coding_format) = if in_band_sco {
                (Some(16000), Some(8), Some(fidl_bt::AssignedCodingFormat::Cvsd))
            } else {
                (Some(16000), Some(16), Some(fidl_bt::AssignedCodingFormat::LinearPcm))
            };

            let params_fn = |set| bredr::ScoConnectionParameters {
                parameter_set: Some(set),
                io_bandwidth,
                io_frame_size,
                io_coding_format,
                ..params_with_data_path(sco_params_fallback(), in_band_sco)
            };
            vec![params_fn(S4), params_fn(S1), params_fn(D1)]
        }
    }
}

#[derive(Debug, Clone, PartialEq, Copy)]
enum InitiatorRole {
    Initiate,
    Accept,
}

impl Connector {
    pub fn build(proxy: bredr::ProfileProxy, controller_codecs: HashSet<CodecId>) -> Self {
        Self { proxy, controller_codecs }
    }

    async fn setup_sco_connection(
        profile_proxy: bredr::ProfileProxy,
        peer_id: PeerId,
        role: InitiatorRole,
        params: Vec<bredr::ScoConnectionParameters>,
    ) -> Result<Connection, ConnectError> {
        let (connection_proxy, server) =
            fidl::endpoints::create_proxy::<bredr::ScoConnectionMarker>();
        profile_proxy.connect_sco(bredr::ProfileConnectScoRequest {
            peer_id: Some(peer_id.into()),
            initiator: Some(role == InitiatorRole::Initiate),
            params: Some(params),
            connection: Some(server),
            ..Default::default()
        })?;

        match connection_proxy.take_event_stream().next().await {
            Some(Ok(bredr::ScoConnectionEvent::OnConnectionComplete { payload })) => {
                match payload {
                    bredr::ScoConnectionOnConnectionCompleteRequest::ConnectedParams(params) => {
                        let params =
                            params.try_into().map_err(|_| ConnectError::InvalidArguments)?;
                        Ok(Connection { peer_id, params, proxy: connection_proxy })
                    }
                    bredr::ScoConnectionOnConnectionCompleteRequest::Error(err) => Err(err.into()),
                    _ => {
                        warn!("Received unknown ScoConnectionOnConnectionCompleteRequest");
                        Err(ConnectError::Canceled)
                    }
                }
            }
            Some(Ok(bredr::ScoConnectionEvent::_UnknownEvent { .. })) => {
                warn!("Received unknown ScoConnectionEvent");
                Err(ConnectError::Canceled)
            }
            Some(Err(e)) => Err(e.into()),
            None => Err(ConnectError::Canceled),
        }
    }

    fn parameters_for_codecs(&self, codecs: Vec<CodecId>) -> Vec<bredr::ScoConnectionParameters> {
        codecs
            .into_iter()
            .map(|id| parameter_sets_for_codec(id, !self.controller_codecs.contains(&id)))
            .flatten()
            .collect()
    }

    pub fn connect(
        &self,
        peer_id: PeerId,
        codecs: Vec<CodecId>,
    ) -> impl Future<Output = Result<Connection, ConnectError>> + 'static {
        let params = self.parameters_for_codecs(codecs);
        info!(peer_id:%, params:?; "Initiating SCO connection");

        let proxy = self.proxy.clone();
        async move {
            for param in params {
                let result = Self::setup_sco_connection(
                    proxy.clone(),
                    peer_id,
                    InitiatorRole::Initiate,
                    vec![param.clone()],
                )
                .await;
                match &result {
                    // Return early if there is a FIDL issue, or we succeeded.
                    Err(ConnectError::Fidl { .. }) | Ok(_) => return result,
                    // Otherwise continue to try the next params.
                    Err(e) => {
                        debug!(peer_id:%, param:?, e:?; "Connection failed, trying next set..")
                    }
                }
            }
            info!(peer_id:%; "Exhausted SCO connection parameters");
            Err(ConnectError::Failed)
        }
    }

    pub fn accept(
        &self,
        peer_id: PeerId,
        codecs: Vec<CodecId>,
    ) -> impl Future<Output = Result<Connection, ConnectError>> + 'static {
        let params = self.parameters_for_codecs(codecs);
        info!(peer_id:%, params:?; "Accepting SCO connection");

        let proxy = self.proxy.clone();
        Self::setup_sco_connection(proxy, peer_id, InitiatorRole::Accept, params)
    }
}

#[cfg(test)]
pub mod test {
    use super::*;

    use fidl_fuchsia_bluetooth_bredr::HfpParameterSet;

    #[track_caller]
    pub fn connection_for_codec(
        peer_id: PeerId,
        codec_id: CodecId,
        in_band: bool,
    ) -> (Connection, bredr::ScoConnectionRequestStream) {
        let sco_params = parameter_sets_for_codec(codec_id, in_band).pop().unwrap();
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<bredr::ScoConnectionMarker>();
        let connection = Connection::build(peer_id, sco_params, proxy);
        (connection, stream)
    }

    #[fuchsia::test]
    fn codec_parameters() {
        let _exec = fuchsia_async::TestExecutor::new();
        let all_codecs = vec![CodecId::MSBC, CodecId::CVSD];

        // Out-of-band SCO.
        let test_profile_server::TestProfileServerEndpoints { proxy: profile_svc, .. } =
            test_profile_server::TestProfileServer::new(None, None);
        let sco = Connector::build(profile_svc.clone(), all_codecs.iter().cloned().collect());
        let res = sco.parameters_for_codecs(all_codecs.clone());
        assert_eq!(res.len(), 4);
        assert_eq!(
            res,
            vec![
                bredr::ScoConnectionParameters {
                    parameter_set: Some(HfpParameterSet::T2),
                    air_coding_format: Some(fidl_bt::AssignedCodingFormat::Msbc),
                    io_bandwidth: Some(32000),
                    path: Some(bredr::DataPath::Offload),
                    ..common_sco_params()
                },
                bredr::ScoConnectionParameters {
                    parameter_set: Some(HfpParameterSet::S4),
                    path: Some(bredr::DataPath::Offload),
                    ..sco_params_fallback()
                },
                bredr::ScoConnectionParameters {
                    parameter_set: Some(HfpParameterSet::S1),
                    path: Some(bredr::DataPath::Offload),
                    ..sco_params_fallback()
                },
                bredr::ScoConnectionParameters {
                    path: Some(bredr::DataPath::Offload),
                    ..sco_params_fallback()
                },
            ]
        );

        // All In-band SCO.
        let sco = Connector::build(profile_svc.clone(), HashSet::new());
        let res = sco.parameters_for_codecs(all_codecs.clone());
        assert_eq!(res.len(), 4);
        assert_eq!(
            res,
            vec![
                bredr::ScoConnectionParameters {
                    parameter_set: Some(HfpParameterSet::T2),
                    air_coding_format: Some(fidl_bt::AssignedCodingFormat::Transparent),
                    io_bandwidth: Some(16000),
                    io_coding_format: Some(fidl_bt::AssignedCodingFormat::Transparent),
                    path: Some(bredr::DataPath::Host),
                    ..common_sco_params()
                },
                bredr::ScoConnectionParameters {
                    parameter_set: Some(HfpParameterSet::S4),
                    io_coding_format: Some(fidl_bt::AssignedCodingFormat::Cvsd),
                    io_frame_size: Some(8),
                    path: Some(bredr::DataPath::Host),
                    ..sco_params_fallback()
                },
                bredr::ScoConnectionParameters {
                    parameter_set: Some(HfpParameterSet::S1),
                    io_coding_format: Some(fidl_bt::AssignedCodingFormat::Cvsd),
                    io_frame_size: Some(8),
                    path: Some(bredr::DataPath::Host),
                    ..sco_params_fallback()
                },
                bredr::ScoConnectionParameters {
                    io_coding_format: Some(fidl_bt::AssignedCodingFormat::Cvsd),
                    io_frame_size: Some(8),
                    path: Some(bredr::DataPath::Host),
                    ..sco_params_fallback()
                },
            ]
        );

        // Mix of in-band and offloaded SCO
        let only_cvsd_set = [CodecId::CVSD].iter().cloned().collect();
        let sco = Connector::build(profile_svc.clone(), only_cvsd_set);
        let res = sco.parameters_for_codecs(all_codecs);
        assert_eq!(res.len(), 4);
        assert_eq!(
            res,
            vec![
                bredr::ScoConnectionParameters {
                    parameter_set: Some(HfpParameterSet::T2),
                    air_coding_format: Some(fidl_bt::AssignedCodingFormat::Transparent),
                    io_bandwidth: Some(16000),
                    io_coding_format: Some(fidl_bt::AssignedCodingFormat::Transparent),
                    path: Some(bredr::DataPath::Host),
                    ..common_sco_params()
                },
                bredr::ScoConnectionParameters {
                    parameter_set: Some(HfpParameterSet::S4),
                    path: Some(bredr::DataPath::Offload),
                    ..sco_params_fallback()
                },
                bredr::ScoConnectionParameters {
                    parameter_set: Some(HfpParameterSet::S1),
                    path: Some(bredr::DataPath::Offload),
                    ..sco_params_fallback()
                },
                bredr::ScoConnectionParameters {
                    path: Some(bredr::DataPath::Offload),
                    ..sco_params_fallback()
                },
            ]
        );
    }
}
