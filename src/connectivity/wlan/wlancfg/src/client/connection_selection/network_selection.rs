// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::types;
use std::collections::HashSet;

// Returns zero or more selected networks. Multiple selected networks indicates no preference
// between them.
pub fn select_networks(
    available_networks: HashSet<types::NetworkIdentifier>,
    network: &Option<types::NetworkIdentifier>,
) -> HashSet<types::NetworkIdentifier> {
    match network {
        Some(network) => HashSet::from([network.clone()]),
        None => {
            // TODO(https://fxbug.dev/42064323): Add network selection logic.
            // Currently, the connection selection is determined solely based on the BSS. All available
            // networks are allowed.
            available_networks
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;
    #[fuchsia::test]
    fn select_networks_selects_specified_network() {
        let ssid = "foo";
        let all_networks = vec![
            types::NetworkIdentifier {
                ssid: ssid.try_into().unwrap(),
                security_type: types::SecurityType::Wpa3,
            },
            types::NetworkIdentifier {
                ssid: ssid.try_into().unwrap(),
                security_type: types::SecurityType::Wpa2,
            },
        ];
        let all_network_set = HashSet::from_iter(all_networks.clone());

        // Specifying a network filters to just that network
        let desired_network = all_networks[0].clone();
        let selected_network =
            select_networks(all_network_set.clone(), &Some(desired_network.clone()));
        assert_eq!(selected_network, HashSet::from([desired_network]));

        // No specified network returns all networks
        assert_eq!(select_networks(all_network_set.clone(), &None), all_network_set);
    }
}
