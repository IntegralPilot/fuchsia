// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cmp::min;
use std::collections::HashSet;

use fidl_fuchsia_hardware_haptics::{
    CompositeEffect, CompositeEffectWaveform, DeviceProxy, EffectStrength, ServiceMarker,
    SupportedCompositeEffectWaveform, SupportedEffect,
};
use fuchsia_component::client::Service;
use futures::future::join;

use test_util::{assert_geq, assert_lt};
use zx::{MonotonicDuration, MonotonicInstant};

// Test that `device` can play a vibration.
async fn test_play_vibration(device: &DeviceProxy, vibration_duration: MonotonicDuration) {
    let start = MonotonicInstant::get();
    device.play_vibration(vibration_duration.into_nanos()).await.unwrap().unwrap();

    // Verify that the device waits for the vibration to complete before completing the request.
    let actual_vibration_duration = MonotonicInstant::get() - start;
    assert_geq!(actual_vibration_duration, vibration_duration);
}

// Test that `device` can stop a vibration.
async fn test_stop_vibration(device: &DeviceProxy) {
    let vibration_duration = MonotonicDuration::from_seconds(10);
    let play_vibration_fut = async {
        let start = MonotonicInstant::get();
        // Play a vibration long enough so that a stop request can be sent before the vibration completes.
        device.play_vibration(vibration_duration.into_nanos()).await.unwrap().unwrap();
        (start, MonotonicInstant::get())
    };
    let stop_vibration_fut = async {
        let start = MonotonicInstant::get();
        device.stop_vibration().await.unwrap().unwrap();
        (start, MonotonicInstant::get())
    };

    // Play and stop a vibration.
    //
    // The future to play a vibration should be the first argument so that it gets executed first in
    // order to send the request to play a vibration before the request to stop the vibration gets
    // sent.
    let ((play_start, play_end), (stop_start, stop_end)) =
        join(play_vibration_fut, stop_vibration_fut).await;

    // Verify that the request to play vibrations stopped earlier than the provided vibration
    // duration.
    let actual_vibration_duration = play_end - play_start;
    assert_lt!(actual_vibration_duration, vibration_duration);

    // Verify that the request to stop vibrations was sent before the vibration completed.
    assert_lt!(stop_start, play_end);

    // Verify that the vibration completed before the request to stop vibrations completed.
    assert_lt!(play_end, stop_end);
}

// Test that `device` can play the supported effect `effect`.
async fn test_play_effect(device: &DeviceProxy, effect: &SupportedEffect) {
    for strength in [EffectStrength::Light, EffectStrength::Medium, EffectStrength::Strong] {
        let start = MonotonicInstant::get();
        device.play_effect(effect.effect, strength).await.unwrap().unwrap();

        // Verify that the device waits for the vibration to complete before completing the request.
        let actual_vibration_duration = MonotonicInstant::get() - start;
        let expected_vibration_duration = MonotonicDuration::from_nanos(effect.duration);
        assert_geq!(actual_vibration_duration, expected_vibration_duration);
    }
}

// Test that `device` can play a composite waveform composed of just one support effect waveform
// `waveform`.
async fn test_play_composite_waveform_single_effect(
    device: &DeviceProxy,
    effect_waveform: &SupportedCompositeEffectWaveform,
) {
    let composite_waveform =
        vec![CompositeEffect { delay: 0, waveform: effect_waveform.waveform, scale: 1.0 }];
    let start = MonotonicInstant::get();
    device.play_composite_waveform(&composite_waveform).await.unwrap().unwrap();

    // Verify that the device waits for the vibration to complete before completing the request.
    let actual_vibration_duration = MonotonicInstant::get() - start;
    let expected_vibration_duration = MonotonicDuration::from_nanos(effect_waveform.duration);
    assert_geq!(actual_vibration_duration, expected_vibration_duration);
}

// Test that `device` can play a composite waveform composed of all of its supported effect
// waveforms.
async fn test_play_composite_waveform_all_effects(
    device: &DeviceProxy,
    max_composite_waveform_effect_count: usize,
    effect_waveforms: &[SupportedCompositeEffectWaveform],
) {
    let max_window_size = min(effect_waveforms.len(), max_composite_waveform_effect_count);
    for i in (0..effect_waveforms.len()).step_by(max_window_size) {
        let j = min(i + max_window_size, effect_waveforms.len());
        let window = &effect_waveforms[i..j];
        let mut composite_waveform: Vec<CompositeEffect> = vec![];
        let mut expected_vibration_duration = MonotonicDuration::default();
        for effect_waveform in window {
            composite_waveform.push(CompositeEffect {
                delay: 0,
                waveform: effect_waveform.waveform,
                scale: 1.0,
            });
            expected_vibration_duration += MonotonicDuration::from_nanos(effect_waveform.duration);
        }

        let start = MonotonicInstant::get();
        device.play_composite_waveform(&composite_waveform).await.unwrap().unwrap();

        // Verify that the device waits for the vibration to complete before completing the request.
        let actual_vibration_duration = MonotonicInstant::get() - start;
        assert_geq!(actual_vibration_duration, expected_vibration_duration);
    }
}

// Return all immediately available haptics devices.
async fn get_haptics_devices() -> Vec<DeviceProxy> {
    Service::open(ServiceMarker)
        .unwrap()
        .enumerate()
        .await
        .unwrap()
        .into_iter()
        .map(|instance| instance.connect_to_device().unwrap())
        .collect()
}

// Verify that the haptics device can provide a correctly structured set of properties related to
// haptics.
#[fuchsia::test]
async fn test_haptics_properties() {
    for device in get_haptics_devices().await {
        let (_, _, _, supported_composite_effect_waveforms, max_composite_waveform_effect_count, _) =
            device.get_properties().await.unwrap().unwrap();

        // Verify that the list of supported composite effects does not contain duplicate waveforms.
        {
            let mut waveform_set: HashSet<CompositeEffectWaveform> =
                supported_composite_effect_waveforms
                    .iter()
                    .map(|waveform| waveform.waveform)
                    .collect();
            for waveform in &supported_composite_effect_waveforms {
                assert!(
                    waveform_set.remove(&waveform.waveform),
                    "List of supported composite effect waveforms contains duplicates of {:?}",
                    waveform.waveform
                );
            }
        }

        // Verify that the no-op composite effect waveform has a duration of 0ms.
        if let Some(noop_effect) = supported_composite_effect_waveforms
            .iter()
            .find(|effect| effect.waveform == CompositeEffectWaveform::Noop)
        {
            assert_eq!(noop_effect.duration, 0);
        }

        assert!(
            !(max_composite_waveform_effect_count == 0
                && supported_composite_effect_waveforms.is_empty()),
            "Haptics device cannot play composite waveforms because its max composite waveform effect \
        count is 0 but its haptics properties claim that the device supports playing composite \
        effect waveforms"
        );
    }
}

// Verify that the haptics device can play various types of vibrations.
//
// All vibration playback testing must be done sequentially and not in parallel because haptics
// devices are not allowed to play multiple vibrations at the same time.
#[fuchsia::test]
async fn test_haptics_playback() {
    for device in get_haptics_devices().await {
        let (
            _,
            _,
            supported_effects,
            supported_composite_effect_waveforms,
            max_composite_waveform_effect_count,
            _,
        ) = device.get_properties().await.unwrap().unwrap();

        // Verify that the haptics device can set the amplitude of future vibrations.
        device.set_amplitude(0.1).await.unwrap().unwrap();

        // Verify that the haptics device can play a vibration with a duration of 0ms.
        test_play_vibration(&device, MonotonicDuration::from_millis(0)).await;

        // Verify that the haptics device can play a short vibration.
        test_play_vibration(&device, MonotonicDuration::from_millis(20)).await;

        // Verify that the haptics device can play a long vibration.
        test_play_vibration(&device, MonotonicDuration::from_millis(500)).await;

        // Verify that the haptics device can stop vibrations.
        test_stop_vibration(&device).await;

        // Verify that the haptics device can play its supported effects.
        for effect in supported_effects {
            test_play_effect(&device, &effect).await;
        }

        // Verify that the haptics device can play its supported composite effect waveforms.
        for waveform in &supported_composite_effect_waveforms {
            test_play_composite_waveform_single_effect(&device, waveform).await;
        }

        // Verify that the haptics device can play a composite waveform comprised of all of its
        // supported composite effect waveforms.
        if max_composite_waveform_effect_count > 0 {
            test_play_composite_waveform_all_effects(
                &device,
                max_composite_waveform_effect_count as usize,
                &supported_composite_effect_waveforms,
            )
            .await;
        }
    }
}
