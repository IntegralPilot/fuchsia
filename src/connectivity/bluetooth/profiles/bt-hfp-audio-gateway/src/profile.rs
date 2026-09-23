// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use fidl_fuchsia_bluetooth as fidl_bt;
use fidl_fuchsia_bluetooth_bredr as bredr;
use profile_client::ProfileClient;

use crate::config::AudioGatewayFeatureSupport;
use crate::service_definitions;

fn register(
    proxy: bredr::ProfileProxy,
    features: AudioGatewayFeatureSupport,
) -> anyhow::Result<ProfileClient> {
    // Register the service advertisement for the Audio Gateway role implemented by us.
    let service_definition = service_definitions::audio_gateway(features);
    let mut profile = ProfileClient::advertise(
        proxy,
        vec![service_definition],
        fidl_bt::ChannelParameters::default(),
    )?;

    // Register a search for remote peers that support the Hands Free role.
    profile.add_search(bredr::ServiceClassProfileIdentifier::Handsfree, None)?;

    Ok(profile)
}

/// Register the Audio Gateway profile. Returns a `ProfileClient` to interact
/// with the `bredr.Profile` and the ProfileProxy on success or Error on failure.
pub fn register_audio_gateway(
    features: AudioGatewayFeatureSupport,
) -> anyhow::Result<(ProfileClient, bredr::ProfileProxy)> {
    let proxy = fuchsia_component::client::connect_to_protocol::<bredr::ProfileMarker>()
        .context("Failed to connect to Bluetooth Profile service")?;
    let profile_client = register(proxy.clone(), features)?;
    Ok((profile_client, proxy))
}

#[cfg(test)]
pub(crate) mod test_server {
    use super::*;
    use fidl_fuchsia_bluetooth_bredr as bredr;
    use futures::StreamExt;

    /// Register a new Profile object, and create an associated test server.
    pub(crate) fn setup_profile_and_test_server()
    -> (ProfileClient, bredr::ProfileProxy, LocalProfileTestServer) {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<bredr::ProfileMarker>();

        let profile = register(proxy.clone(), Default::default()).expect("register profile");
        (profile, proxy, stream.into())
    }

    use fuchsia_bluetooth::types::Channel;

    /// Holds all the server side resources associated with a `Profile`'s connection to
    /// fuchsia.bluetooth.bredr.Profile. Provides helper methods for common test related tasks.
    /// Some fields are optional because they are not populated until the Profile has completed
    /// registration.
    pub(crate) struct LocalProfileTestServer {
        pub stream: bredr::ProfileRequestStream,
        pub receiver: Option<bredr::ConnectionReceiverProxy>,
        pub results: Option<bredr::SearchResultsProxy>,
        pub connections: Vec<Channel>,
    }

    impl From<bredr::ProfileRequestStream> for LocalProfileTestServer {
        fn from(stream: bredr::ProfileRequestStream) -> Self {
            Self { stream, receiver: None, results: None, connections: vec![] }
        }
    }

    impl LocalProfileTestServer {
        /// Returns true if the `Profile` has registered an `Advertise` and `Search` request.
        fn is_registration_complete(&self) -> bool {
            self.receiver.is_some() && self.results.is_some()
        }

        /// Run through the registration process of a new `Profile`.
        pub async fn complete_registration(&mut self) {
            while let Some(request) = self.stream.next().await {
                match request {
                    Ok(bredr::ProfileRequest::Advertise { payload, responder, .. }) => {
                        if self.is_registration_complete() {
                            panic!("unexpected second advertise request");
                        }
                        self.receiver = Some(payload.receiver.unwrap().into_proxy());
                        let _ = responder.send(Ok(&bredr::ProfileAdvertiseResponse {
                            services: payload.services.clone(),
                            ..Default::default()
                        }));
                        if self.is_registration_complete() {
                            break;
                        }
                    }
                    Ok(bredr::ProfileRequest::Search { payload, .. }) => {
                        if self.is_registration_complete() {
                            panic!("unexpected second search request");
                        }
                        self.results = Some(payload.results.unwrap().into_proxy());
                        if self.is_registration_complete() {
                            break;
                        }
                    }
                    _ => panic!("unexpected result on profile request stream: {:?}", request),
                }
            }
        }
    }
}
