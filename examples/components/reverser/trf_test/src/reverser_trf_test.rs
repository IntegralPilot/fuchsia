// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_examples_reverser as freverser;
use trf_codegen_macro as trf;

#[trf::test(config(switch_case = false, use_replacer = false))]
pub async fn test_reverser_trf_basic(realm: &TestRealm) -> Result<(), Error> {
    let client = realm.connect_to_protocol::<freverser::ReverserMarker>().await?;

    let input = "Hello Fuchsia TRF!";
    let output = client.reverse(input).await?;
    assert_eq!(output, "!FRT aishcuF olleH");

    Ok(())
}

#[trf::test(config(switch_case = true, use_replacer = false))]
pub async fn test_reverser_trf_caps(realm: &TestRealm) -> Result<(), Error> {
    let client = realm.connect_to_protocol::<freverser::ReverserMarker>().await?;

    let input = "Hello Fuchsia TRF!";
    let output = client.reverse(input).await?;
    assert_eq!(output, "!frt AISHCUf OLLEh");

    Ok(())
}

#[trf::test(config(switch_case = false, use_replacer = true), mocks(MockReplacer))]
pub async fn test_reverser_trf_mock(realm: &TestRealm) -> Result<(), Error> {
    realm.set_replacement("TRF".to_string(), "Codegen".to_string()).await?;

    let client = realm.connect_to_protocol::<freverser::ReverserMarker>().await?;

    let input = "Hello Fuchsia TRF!";
    let output = client.reverse(input).await?;
    assert_eq!(output, "!negedoC aishcuF olleH");

    Ok(())
}

#[trf::test(config(switch_case = false, use_replacer = true), mocks(MockReplacer))]
pub async fn test_reverser_trf_greeting(realm: &TestRealm) -> Result<(), Error> {
    let client = realm.connect_to_protocol::<freverser::ReverserMarker>().await?;

    let input = "Hello Fuchsia TRF!";
    let output = client.reverse(input).await?;
    assert_eq!(output, "!FRT aishcuF olleH");
    realm.set_replacement("Hello".to_string(), "Greetings".to_string()).await?;
    let output = client.reverse(input).await?;
    assert_eq!(output, "!FRT aishcuF sgniteerG");

    Ok(())
}

/// Test using TRF lifecycle capabilities to stop and restart mock components.
#[trf::test(config(switch_case = false, use_replacer = true), mocks(MockReplacer))]
pub async fn test_reverser_trf_lifecycle(realm: &TestRealm) -> Result<(), Error> {
    realm.set_replacement("TRF".to_string(), "Lifecycle".to_string()).await?;

    let client = realm.connect_to_protocol::<freverser::ReverserMarker>().await?;

    let input = "Hello Fuchsia TRF!";
    let output = client.reverse(input).await?;
    assert_eq!(output, "!elcycefiL aishcuF olleH"); // Lifecycle reversed

    let mut events = realm.subscribe_to_lifecycle().expect("Failed to subscribe");
    realm.stop_component("mock_MockReplacer").await.expect("Failed to stop component");

    assert_eq!(
        events.next_event().await,
        Some(trf_generated_realm::LifecycleEvent::Stopped("mock_MockReplacer".to_string()))
    );

    let output2 = client.reverse(input).await?;
    assert_eq!(output2, "!FRT aishcuF olleH"); // Back to default because state was lost on restart

    // Verify it implicitly started
    assert!(realm.is_running("mock_MockReplacer").await.expect("Failed to check"));

    Ok(())
}
