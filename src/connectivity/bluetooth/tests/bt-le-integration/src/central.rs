// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Error};
use bt_test_harness::emulator::{add_le_peer, default_le_peer};
use bt_test_harness::low_energy_central::CentralHarness;
use fidl_fuchsia_hardware_bluetooth::{AdvertisingData, PeerSetLeAdvertisementRequest};
use fuchsia_async::{DurationExt, TimeoutExt};
use fuchsia_bluetooth::constants::INTEGRATION_TIMEOUT;
use fuchsia_bluetooth::expectation::asynchronous::{ExpectableExt, ExpectableStateExt};
use fuchsia_bluetooth::types::Address;
use futures::TryFutureExt;

mod expect {
    use bt_test_harness::low_energy_central::{CentralState, ScanStateChange};
    use fuchsia_bluetooth::expectation::Predicate;
    use fuchsia_bluetooth::types::le::RemoteDevice;

    pub fn scan_enabled() -> Predicate<CentralState> {
        Predicate::equal(Some(ScanStateChange::ScanEnabled)).over_value(
            |state: &CentralState| state.scan_state_changes.last().cloned(),
            ".scan_state_changes.last()",
        )
    }
    pub fn scan_disabled() -> Predicate<CentralState> {
        Predicate::equal(Some(ScanStateChange::ScanDisabled)).over_value(
            |state: &CentralState| state.scan_state_changes.last().cloned(),
            ".scan_state_changes.last()",
        )
    }
    pub fn device_found(expected_name: &str) -> Predicate<CentralState> {
        let expected_name = expected_name.to_string();
        let has_expected_name = Predicate::equal(Some(expected_name)).over_value(
            |peer: &RemoteDevice| {
                peer.advertising_data.as_ref().and_then(|ad| ad.name.as_ref().cloned())
            },
            ".advertising_data.name",
        );

        Predicate::any(has_expected_name)
            .over(|state: &CentralState| &state.remote_devices, ".remote_devices")
    }
}

async fn start_scan(central: &CentralHarness) -> Result<(), Error> {
    let fut = central
        .aux()
        .central
        .start_scan(None)
        .map_err(|e| e.into())
        .on_timeout(INTEGRATION_TIMEOUT.after_now(), move || Err(format_err!("Timed out")));
    let status = fut.await.unwrap();
    if let Some(e) = status.error {
        return Err(format_err!("error during scan {e:?}"));
    }
    Ok(())
}

#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-le-integration-tests#meta/bt-le-integration-tests-component.cm"
)]
async fn test_enable_scan(central: CentralHarness) {
    let address = Address::Random([1, 0, 0, 0, 0, 0]);
    let fut = add_le_peer(central.aux().as_ref(), default_le_peer(&address), None);
    let peer = fut.await.unwrap();
    let request = PeerSetLeAdvertisementRequest {
        le_address: Some(address.into()),
        advertisement: Some(AdvertisingData {
            data: Some(vec![
                // Flags field set to "general discoverable"
                0x02, 0x01, 0x02, // Complete local name set to "Fake"
                0x05, 0x09, 'F' as u8, 'a' as u8, 'k' as u8, 'e' as u8,
            ]),
            __source_breaking: fidl::marker::SourceBreaking,
        }),
        scan_response: Some(AdvertisingData {
            data: None,
            __source_breaking: fidl::marker::SourceBreaking,
        }),
        __source_breaking: fidl::marker::SourceBreaking,
    };
    let _ = peer.set_le_advertisement(&request).await.unwrap();

    start_scan(&central).await.unwrap();
    let _ = central
        .when_satisfied(
            expect::scan_enabled().and(expect::device_found("Fake")),
            INTEGRATION_TIMEOUT,
        )
        .await
        .unwrap();
}

#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-le-integration-tests#meta/bt-le-integration-tests-component.cm"
)]
async fn test_enable_and_disable_scan(central: CentralHarness) {
    start_scan(&central).await.unwrap();
    let _ = central.when_satisfied(expect::scan_enabled(), INTEGRATION_TIMEOUT).await.unwrap();
    let _ = central.aux().central.stop_scan().unwrap();
    let _ = central.when_satisfied(expect::scan_disabled(), INTEGRATION_TIMEOUT).await.unwrap();
}
