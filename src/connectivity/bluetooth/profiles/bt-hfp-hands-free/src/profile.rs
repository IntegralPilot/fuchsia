// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use profile_client::ProfileClient;
use {fidl_fuchsia_bluetooth as fidl_bt, fidl_fuchsia_bluetooth_bredr as bredr};

use crate::config::HandsFreeFeatureSupport;
use crate::service_definition;

pub fn register_with_proxy(
    proxy: bredr::ProfileProxy,
    features: HandsFreeFeatureSupport,
) -> anyhow::Result<ProfileClient> {
    // Register the service advertisement for the Hands Free role.
    let service_definition = service_definition::hands_free(features);
    let mut profile = ProfileClient::advertise(
        proxy,
        vec![service_definition],
        fidl_bt::ChannelParameters::default(),
    )?;
    // Register a search for remote peers that support the Audio Gateway role
    profile.add_search(bredr::ServiceClassProfileIdentifier::HandsfreeAudioGateway, None)?;

    Ok(profile)
}

pub fn register_hands_free(
    features: HandsFreeFeatureSupport,
) -> anyhow::Result<(ProfileClient, bredr::ProfileProxy)> {
    let proxy = fuchsia_component::client::connect_to_protocol::<bredr::ProfileMarker>()
        .context("Failed to connect to Bluetooth Profile service")?;
    let profile_client = register_with_proxy(proxy.clone(), features)?;
    Ok((profile_client, proxy))
}
