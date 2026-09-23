// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::{
    default_media_stream_settings, AudioCoreRequest, AudioTest, DEFAULT_VOLUME_LEVEL,
    DEFAULT_VOLUME_MUTED,
};
use fidl_fuchsia_media::{AudioRenderUsage, AudioRenderUsage2};
use fidl_fuchsia_settings::{
    AudioProxy, AudioSettings, AudioSettings2, AudioStreamSettingSource, AudioStreamSettings,
    AudioStreamSettings2, Error, Volume,
};
use test_case::test_case;

mod common;
mod mock_audio_core_service;

/// A volume level of 0.7, which is different from the default of 0.5.
const CHANGED_VOLUME_LEVEL: f32 = DEFAULT_VOLUME_LEVEL + 0.2;

/// A volume level of 0.95, which is different from both CHANGED_VOLUME_LEVEL and the default.
const CHANGED_VOLUME_LEVEL_2: f32 = CHANGED_VOLUME_LEVEL + 0.25;

/// A mute state of true, which is different from the default of false.
const CHANGED_VOLUME_MUTED: bool = !DEFAULT_VOLUME_MUTED;

//
// In this set of integration tests -- unlike other audio-related integration tests such as
// those for earcons -- the set()/watch() test coverage is identical to that of set2()/watch2().
// Other integration test suites should use only set2() and watch2(), since the set() and watch()
// methods have been deprecated and will soon be removed.
//

fn to_audio_stream_settings2(settings: AudioStreamSettings) -> AudioStreamSettings2 {
    AudioStreamSettings2 {
        stream: Some(match settings.stream.unwrap() {
            AudioRenderUsage::Background => AudioRenderUsage2::Background,
            AudioRenderUsage::Communication => AudioRenderUsage2::Communication,
            AudioRenderUsage::Interruption => AudioRenderUsage2::Interruption,
            AudioRenderUsage::Media => AudioRenderUsage2::Media,
            AudioRenderUsage::SystemAgent => AudioRenderUsage2::SystemAgent,
        }),
        source: settings.source,
        user_volume: settings.user_volume,
        ..Default::default()
    }
}

fn default_media_stream_settings2() -> AudioStreamSettings2 {
    to_audio_stream_settings2(default_media_stream_settings())
}

fn changed_media_stream_settings() -> AudioStreamSettings {
    AudioStreamSettings {
        stream: Some(AudioRenderUsage::Media),
        source: Some(AudioStreamSettingSource::User),
        user_volume: Some(Volume {
            level: Some(CHANGED_VOLUME_LEVEL),
            muted: Some(CHANGED_VOLUME_MUTED),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn changed_media_stream_settings2() -> AudioStreamSettings2 {
    to_audio_stream_settings2(changed_media_stream_settings())
}

fn changed_again_media_stream_settings() -> AudioStreamSettings {
    AudioStreamSettings {
        stream: Some(AudioRenderUsage::Media),
        source: Some(AudioStreamSettingSource::User),
        user_volume: Some(Volume {
            level: Some(CHANGED_VOLUME_LEVEL_2),
            muted: Some(CHANGED_VOLUME_MUTED),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn changed_again_media_stream_settings2() -> AudioStreamSettings2 {
    to_audio_stream_settings2(changed_again_media_stream_settings())
}

fn changed_system_agent_stream_settings() -> AudioStreamSettings {
    AudioStreamSettings {
        stream: Some(AudioRenderUsage::SystemAgent),
        source: Some(AudioStreamSettingSource::User),
        user_volume: Some(Volume {
            level: Some(CHANGED_VOLUME_LEVEL),
            muted: Some(CHANGED_VOLUME_MUTED),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn changed_accessibility_stream_settings2() -> AudioStreamSettings2 {
    AudioStreamSettings2 {
        stream: Some(AudioRenderUsage2::Accessibility),
        source: Some(AudioStreamSettingSource::User),
        user_volume: Some(Volume {
            level: Some(CHANGED_VOLUME_LEVEL),
            muted: Some(CHANGED_VOLUME_MUTED),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// Issue a collection of volume changes, using the fuchsia.settings.Audio/set() method.
async fn set_volume1(proxy: &AudioProxy, streams: Vec<AudioStreamSettings>) {
    let mut audio_settings = AudioSettings::default();
    audio_settings.streams = Some(streams);
    proxy.set(&audio_settings).await.expect("set completed").expect("set successful");
}

// Issue a collection of volume changes, using the fuchsia.settings.Audio/set2() method.
async fn set_volume2(proxy: &AudioProxy, streams: Vec<AudioStreamSettings2>) {
    let mut audio_settings = AudioSettings2::default();
    audio_settings.streams = Some(streams);
    proxy.set2(&audio_settings).await.expect("set completed").expect("set2 successful");
}

// Verifies that the provided AudioStreamSettings is found within the provided AudioSettings.
// Used when testing that audio_core has received changes made by a set() call.
fn verify_audio_stream(settings: &AudioSettings, stream: AudioStreamSettings) {
    let _ = settings
        .streams
        .as_ref()
        .expect("audio_settings contain streams")
        .iter()
        .find(|x| **x == stream)
        .unwrap_or_else(|| {
            panic!("contain matching {:#?} stream", stream.stream.as_ref().unwrap())
        });
}

// Verifies that the provided AudioStreamSettings2 is found within the provided AudioSettings2.
// Used when testing that audio_core has received changes made by a set2() call.
fn verify_audio_stream2(settings: &AudioSettings2, stream: AudioStreamSettings2) {
    let _ = settings
        .streams
        .as_ref()
        .expect("audio_settings2 contain streams")
        .iter()
        .find(|x| **x == stream)
        .unwrap_or_else(|| {
            panic!("contain matching {:#?} stream", stream.stream.as_ref().unwrap())
        });
}

// Verifies basic functionality of watch() and set().
#[fuchsia::test]
async fn test_audio() {
    let mut audio_test = AudioTest::create_and_init(&[AudioRenderUsage2::Media])
        .await
        .expect("setting up test realm");

    // Spawn a client for watch() calls, to verify that set() calls are observable by all clients.
    let watch_client = audio_test.connect_to_audio_marker();

    // Verify with initial watch() that the settings matches the default on start.
    let settings = watch_client.watch().await.expect("watch completed");
    verify_audio_stream(&settings, default_media_stream_settings());

    // Verify with watch() that we observe the changes made by a set() call.
    set_volume1(audio_test.proxy(), vec![changed_media_stream_settings()]).await;
    let settings = audio_test.proxy().watch().await.expect("watch completed");
    verify_audio_stream(&settings, changed_media_stream_settings());

    // Verify that audio_core received the changes to the audio settings.
    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::Media, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::Media, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    // Verify that a separate client receives these changes with a watch() call.
    let settings = watch_client.watch().await.expect("watch completed");
    verify_audio_stream(&settings, changed_media_stream_settings());

    audio_test.clean_up().await;
}

// Verifies basic functionality of watch2() and set2().
#[fuchsia::test]
async fn test_audio2() {
    let mut audio_test = AudioTest::create_and_init(&[AudioRenderUsage2::Media])
        .await
        .expect("setting up test realm");

    // Spawn a client for watch2() calls, to verify that set2() calls are observable by all clients.
    let watch_client = audio_test.connect_to_audio_marker();

    // Verify with initial watch2() that the settings matches the default on start.
    let settings = watch_client.watch2().await.expect("watch2 completed");
    verify_audio_stream2(&settings, default_media_stream_settings2());

    // Verify with watch2() that we observe the changes made by a set2() call.
    set_volume2(audio_test.proxy(), vec![changed_media_stream_settings2()]).await;
    let settings = audio_test.proxy().watch2().await.expect("watch2 completed");
    verify_audio_stream2(&settings, changed_media_stream_settings2());

    // Verify that audio_core received the changes to the audio settings.
    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::Media, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::Media, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    // Verify that a separate client receives these changes with a watch2() call.
    let settings = watch_client.watch2().await.expect("watch2 completed");
    verify_audio_stream2(&settings, changed_media_stream_settings2());

    audio_test.clean_up().await;
}

// Verifies basic interoperability between set2() and watch().
#[fuchsia::test]
async fn test_set2_works_with_watch() {
    let mut audio_test = AudioTest::create_and_init(&[AudioRenderUsage2::Media])
        .await
        .expect("setting up test realm");

    // Spawn a separate client for watch() calls to be used later, to verify that set2() calls are
    // observable by all clients.
    let watch_client = audio_test.connect_to_audio_marker();

    // Verify with initial watch() that the settings matches the default on start.
    let settings = watch_client.watch().await.expect("watch completed");
    verify_audio_stream(&settings, default_media_stream_settings());

    // Verify with watch() that we observe the changes made by a set() call.
    set_volume2(audio_test.proxy(), vec![changed_media_stream_settings2()]).await;
    let settings = audio_test.proxy().watch().await.expect("watch completed");
    verify_audio_stream(&settings, changed_media_stream_settings());

    // Verify that audio_core received the changes to the audio settings.
    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::Media, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::Media, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    // Verify that a separate client receives these changes with a watch() call.
    let settings = watch_client.watch().await.expect("watch completed");
    verify_audio_stream(&settings, changed_media_stream_settings());

    audio_test.clean_up().await;
}

// Verifies basic interoperability between set() and watch2().
#[fuchsia::test]
async fn test_set_works_with_watch2() {
    let mut audio_test = AudioTest::create_and_init(&[AudioRenderUsage2::Media])
        .await
        .expect("setting up test realm");

    // Spawn a separate client for watch2() calls to be used later, to verify that set() calls are
    // observable by all clients.
    let watch2_client = audio_test.connect_to_audio_marker();

    // Verify with initial watch2() that the settings matches the default on start.
    let settings = watch2_client.watch2().await.expect("watch2 completed");
    verify_audio_stream2(&settings, default_media_stream_settings2());

    // Verify with watch2() that we observe the changes made by a set() call.
    set_volume1(audio_test.proxy(), vec![changed_media_stream_settings()]).await;
    let settings = audio_test.proxy().watch2().await.expect("watch2 completed");
    verify_audio_stream2(&settings, changed_media_stream_settings2());

    // Verify that audio_core received the changes to the audio settings.
    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::Media, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::Media, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    // Verify that a separate client receives these changes with a watch2() call.
    let settings = watch2_client.watch2().await.expect("watch2 completed");
    verify_audio_stream2(&settings, changed_media_stream_settings2());

    audio_test.clean_up().await;
}

// Verifies that correct values are returned by watch() after two successive set() calls.
#[fuchsia::test]
async fn test_consecutive_volume_changes() {
    let mut audio_test = AudioTest::create_and_init(&[AudioRenderUsage2::Media])
        .await
        .expect("setting up test realm");

    // Spawn a client that we'll use for a watch() call later on to verify that set() calls are
    // observed by all clients.
    let watch_client = audio_test.connect_to_audio_marker();

    // Make a set() call and verify the value returned from watch() changes.
    set_volume1(audio_test.proxy(), vec![changed_media_stream_settings()]).await;
    let settings = audio_test.proxy().watch().await.expect("watch completed");
    verify_audio_stream(&settings, changed_media_stream_settings());

    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::Media, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::Media, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    // Make a second set() call and verify the value returned from watch() changes again.
    set_volume1(audio_test.proxy(), vec![changed_again_media_stream_settings()]).await;
    let settings = audio_test.proxy().watch().await.expect("watch completed");
    verify_audio_stream(&settings, changed_again_media_stream_settings());

    // There is no mute request here since the second set() call did not change that.
    audio_test
        .verify_audio_requests(&[AudioCoreRequest::SetVolume(
            AudioRenderUsage2::Media,
            CHANGED_VOLUME_LEVEL_2,
        )])
        .await
        .expect("second changed audio requests received");

    // Verify that a separate client receives the combined changed settings from a watch() call.
    let settings = watch_client.watch().await.expect("watch completed");
    verify_audio_stream(&settings, changed_again_media_stream_settings());

    audio_test.clean_up().await;
}

// Verifies that correct values are returned by watch2() after two successive set2() calls.
#[fuchsia::test]
async fn test_consecutive_volume_changes2() {
    let mut audio_test = AudioTest::create_and_init(&[AudioRenderUsage2::Media])
        .await
        .expect("setting up test realm");

    // Spawn a client that we'll use for a watch2() call later on to verify that set2() calls are
    // observed by all clients.
    let watch_client = audio_test.connect_to_audio_marker();

    // Make a set2() call and verify the value returned from watch2() changes.
    set_volume2(audio_test.proxy(), vec![changed_media_stream_settings2()]).await;
    let settings = audio_test.proxy().watch2().await.expect("watch2 completed");
    verify_audio_stream2(&settings, changed_media_stream_settings2());

    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::Media, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::Media, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    // Make a second set2() call and verify the value returned from watch2() changes again.
    set_volume2(audio_test.proxy(), vec![changed_again_media_stream_settings2()]).await;
    let settings = audio_test.proxy().watch2().await.expect("watch2 completed");
    verify_audio_stream2(&settings, changed_again_media_stream_settings2());

    // There is no mute request here since the second set2() call did not change that.
    audio_test
        .verify_audio_requests(&[AudioCoreRequest::SetVolume(
            AudioRenderUsage2::Media,
            CHANGED_VOLUME_LEVEL_2,
        )])
        .await
        .expect("second changed audio requests received");

    // Verify that a separate client receives the combined changed settings from a watch2() call.
    let settings = watch_client.watch2().await.expect("watch2 completed");
    verify_audio_stream2(&settings, changed_again_media_stream_settings2());

    audio_test.clean_up().await;
}

// Verifies that clients aren't notified by watch(), for duplicate audio changes made by set().
#[fuchsia::test]
async fn test_deduped_volume_changes() {
    let audio_test =
        AudioTest::create_and_init(&[AudioRenderUsage2::Accessibility, AudioRenderUsage2::Media])
            .await
            .expect("setting up test realm");

    {
        let set_client = audio_test.proxy();

        // Get the initial value.
        let _ = set_client.watch().await;
        let fut = set_client.watch();

        // Make a second identical request. This should do nothing.
        set_volume1(&set_client, vec![default_media_stream_settings()]).await;

        // Make a third, different request. This should show up in the watch.
        set_volume1(&set_client, vec![changed_again_media_stream_settings()]).await;

        let settings = fut.await.expect("watch completed");
        verify_audio_stream(&settings, changed_again_media_stream_settings());
    }

    audio_test.clean_up().await;
}

// Verifies that clients aren't notified by watch2(), for duplicate audio changes made by set2().
#[fuchsia::test]
async fn test_deduped_volume_changes2() {
    let audio_test =
        AudioTest::create_and_init(&[AudioRenderUsage2::Accessibility, AudioRenderUsage2::Media])
            .await
            .expect("setting up test realm");

    {
        let set_client = audio_test.proxy();

        // Get the initial value.
        let _ = set_client.watch2().await;
        let fut = set_client.watch2();

        // Make a second identical request. This should do nothing.
        set_volume2(&set_client, vec![default_media_stream_settings2()]).await;

        // Make a third, different request. This should show up in the watch2().
        set_volume2(&set_client, vec![changed_again_media_stream_settings2()]).await;

        let settings = fut.await.expect("watch2 completed");
        verify_audio_stream2(&settings, changed_again_media_stream_settings2());
    }

    audio_test.clean_up().await;
}

// Verifies that changing a stream volume with set() does not affect other stream volumes.
#[fuchsia::test]
async fn test_volume_not_overwritten() {
    let mut audio_test = AudioTest::create_and_init(&[
        AudioRenderUsage2::Media,
        AudioRenderUsage2::Background,
        AudioRenderUsage2::Accessibility,
    ])
    .await
    .expect("setting up test realm");

    // Change the media stream and verify a watch() call returns the updated value.
    set_volume1(audio_test.proxy(), vec![changed_media_stream_settings()]).await;
    let updated_settings = audio_test.proxy().watch().await.expect("watch completed");

    verify_audio_stream(&updated_settings, changed_media_stream_settings());

    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::Media, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::Media, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    // Change the background stream and verify a watch call returns the updated value.
    let changed_background_stream_settings = AudioStreamSettings {
        stream: Some(AudioRenderUsage::Background),
        source: Some(AudioStreamSettingSource::User),
        user_volume: Some(Volume { level: Some(0.3), muted: Some(true), ..Default::default() }),
        ..Default::default()
    };

    set_volume1(audio_test.proxy(), vec![changed_background_stream_settings.clone()]).await;
    let updated_settings = audio_test.proxy().watch().await.expect("watch completed");

    // Changing the background volume should not affect media volume.
    verify_audio_stream(&updated_settings, changed_media_stream_settings());
    verify_audio_stream(&updated_settings, changed_background_stream_settings.clone());

    audio_test.clean_up().await;
}

// Verifies that changing a stream volume with set2() does not affect other stream volumes.
#[fuchsia::test]
async fn test_volume_not_overwritten2() {
    let mut audio_test = AudioTest::create_and_init(&[
        AudioRenderUsage2::Media,
        AudioRenderUsage2::Background,
        AudioRenderUsage2::Accessibility,
    ])
    .await
    .expect("setting up test realm");

    // Change the media stream and verify a watch2() call returns the updated value.
    set_volume2(audio_test.proxy(), vec![changed_media_stream_settings2()]).await;
    let settings = audio_test.proxy().watch2().await.expect("watch2 completed");

    verify_audio_stream2(&settings, changed_media_stream_settings2());

    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::Media, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::Media, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    // Change the background stream and verify a watch2() call returns the updated value.
    let changed_background_stream_settings = AudioStreamSettings2 {
        stream: Some(AudioRenderUsage2::Background),
        source: Some(AudioStreamSettingSource::User),
        user_volume: Some(Volume { level: Some(0.3), muted: Some(true), ..Default::default() }),
        ..Default::default()
    };

    set_volume2(audio_test.proxy(), vec![changed_background_stream_settings.clone()]).await;
    let settings = audio_test.proxy().watch2().await.expect("watch2 completed");

    // Changing the background volume should not affect media volume.
    verify_audio_stream2(&settings, changed_media_stream_settings2());
    verify_audio_stream2(&settings, changed_background_stream_settings);

    audio_test.clean_up().await;
}

// Tests that the volume level gets rounded to two decimal places, when using set().
#[fuchsia::test]
async fn test_volume_rounding() {
    let mut audio_test = AudioTest::create_and_init(&[AudioRenderUsage2::SystemAgent])
        .await
        .expect("setting up test realm");

    // Set the volume to slightly different than CHANGED_VOLUME_LEVEL, but close enough that it
    // should be rounded away. With two decimal places, this delta should be less than 0.005.
    set_volume1(
        audio_test.proxy(),
        vec![AudioStreamSettings {
            stream: Some(AudioRenderUsage::SystemAgent),
            source: Some(AudioStreamSettingSource::User),
            user_volume: Some(Volume {
                level: Some(CHANGED_VOLUME_LEVEL + 0.0015),
                muted: Some(CHANGED_VOLUME_MUTED),
                ..Default::default()
            }),
            ..Default::default()
        }],
    )
    .await;

    let settings = audio_test.proxy().watch().await.expect("watch completed");
    verify_audio_stream(&settings, changed_system_agent_stream_settings());

    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::SystemAgent, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::SystemAgent, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    audio_test.clean_up().await;
}

// Tests that the volume level gets rounded to two decimal places, when using set2().
#[fuchsia::test]
async fn test_volume_rounding2() {
    let mut audio_test = AudioTest::create_and_init(&[AudioRenderUsage2::Accessibility])
        .await
        .expect("setting up test realm");

    // Set the volume to slightly different than CHANGED_VOLUME_LEVEL, but close enough that it
    // should be rounded away. With two decimal places, this delta should be less than 0.005.
    set_volume2(
        audio_test.proxy(),
        vec![AudioStreamSettings2 {
            stream: Some(AudioRenderUsage2::Accessibility),
            source: Some(AudioStreamSettingSource::User),
            user_volume: Some(Volume {
                level: Some(CHANGED_VOLUME_LEVEL + 0.0015),
                muted: Some(CHANGED_VOLUME_MUTED),
                ..Default::default()
            }),
            ..Default::default()
        }],
    )
    .await;

    let settings = audio_test.proxy().watch2().await.expect("watch2 completed");
    verify_audio_stream2(&settings, changed_accessibility_stream_settings2());

    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::Accessibility, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::Accessibility, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    audio_test.clean_up().await;
}

// Tests that the volume level gets rounded to two decimal places, when using set().
#[fuchsia::test]
async fn test_accessibility_ignored_by_legacy_watch() {
    let mut audio_test = AudioTest::create_and_init(&[
        AudioRenderUsage2::Accessibility,
        AudioRenderUsage2::SystemAgent,
    ])
    .await
    .expect("setting up test realm");

    {
        let set_client = audio_test.proxy();

        // Retrieve initial settings with watch(), so the subsequent watch() call will pend.
        let initial_settings1 = audio_test.proxy().watch().await.expect("watch completed");
        verify_audio_stream(&initial_settings1, default_media_stream_settings());

        // Establish a hanging watch(), to see whether an accessibility update will trigger it.
        let fut = set_client.watch();

        // Make a accessibility request. This should not trigger the watch().
        set_volume2(&set_client, vec![changed_accessibility_stream_settings2()]).await;

        // The easiest way to check whether the watch() was triggered is to then take an action that
        // we know WILL trigger it, and see if its response includes that update.
        // Make a system_agent request. This should trigger the watch().
        set_volume1(&set_client, vec![changed_system_agent_stream_settings()]).await;

        // Ensure that watch() only responded to the system_agent update. If it responded to the
        // (earlier) a11y update, then settings1 would contain system_agent default settings.
        let settings1 = fut.await.expect("watch completed");
        verify_audio_stream(&settings1, changed_system_agent_stream_settings());
    }

    // Verify that all volume changes went through, but watch() only notified of legacy ones.
    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::Accessibility, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::Accessibility, CHANGED_VOLUME_MUTED),
            AudioCoreRequest::SetVolume(AudioRenderUsage2::SystemAgent, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::SystemAgent, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    audio_test.clean_up().await;
}

// Verifies that invalid inputs return an error for set() calls.
#[test_case(AudioStreamSettings {
    stream: Some(AudioRenderUsage::Media),
    source: Some(AudioStreamSettingSource::User),
    user_volume: None,
    ..Default::default()
} ; "missing user volume")]
#[test_case(AudioStreamSettings {
    stream: Some(AudioRenderUsage::Media),
    source: Some(AudioStreamSettingSource::User),
    user_volume: Some(Volume {
        level: None,
        muted: None,
        ..Default::default()
    }),
..Default::default()
} ; "missing user volume and muted")]
#[test_case(AudioStreamSettings {
    stream: None,
    source: Some(AudioStreamSettingSource::User),
    user_volume: Some(Volume {
        level: Some(CHANGED_VOLUME_LEVEL),
        muted: Some(CHANGED_VOLUME_MUTED),
        ..Default::default()
    }),
    ..Default::default()
} ; "missing stream")]
#[test_case(AudioStreamSettings {
    stream: Some(AudioRenderUsage::Media),
    source: None,
    user_volume: Some(Volume {
        level: Some(CHANGED_VOLUME_LEVEL),
        muted: Some(CHANGED_VOLUME_MUTED),
        ..Default::default()
    }),
    ..Default::default()
} ; "missing source")]
#[fuchsia::test]
async fn invalid_missing_input_settings_tests(setting: AudioStreamSettings) {
    let mut audio_test = AudioTest::create_and_init(&[AudioRenderUsage2::Media])
        .await
        .expect("setting up test realm");

    let result = audio_test
        .proxy()
        .set(&AudioSettings { streams: Some(vec![setting]), ..Default::default() })
        .await
        .expect("set completed");
    assert_eq!(result, Err(Error::Failed));

    // The best way to test that audio core *didn't* receive an event is to
    // trigger another request and verify that it shows up next.
    set_volume1(audio_test.proxy(), vec![changed_media_stream_settings()]).await;

    // Verify that audio core received the changed audio settings.
    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::Media, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::Media, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    audio_test.clean_up().await;
}

// Verifies that invalid inputs return an error for set2() calls.
#[test_case(AudioStreamSettings2 {
    stream: Some(AudioRenderUsage2::Media),
    source: Some(AudioStreamSettingSource::User),
    user_volume: None,
    ..Default::default()
} ; "missing user volume")]
#[test_case(AudioStreamSettings2 {
    stream: Some(AudioRenderUsage2::Media),
    source: Some(AudioStreamSettingSource::User),
    user_volume: Some(Volume {
        level: None,
        muted: None,
        ..Default::default()
    }),
    ..Default::default()
} ; "missing user volume and muted")]
#[test_case(AudioStreamSettings2 {
    stream: None,
    source: Some(AudioStreamSettingSource::User),
    user_volume: Some(Volume {
        level: Some(CHANGED_VOLUME_LEVEL),
        muted: Some(CHANGED_VOLUME_MUTED),
        ..Default::default()
    }),
    ..Default::default()
} ; "missing stream")]
#[test_case(AudioStreamSettings2 {
    stream: Some(AudioRenderUsage2::Media),
    source: None,
    user_volume: Some(Volume {
        level: Some(CHANGED_VOLUME_LEVEL),
        muted: Some(CHANGED_VOLUME_MUTED),
        ..Default::default()
    }),
    ..Default::default()
} ; "missing source")]
#[fuchsia::test]
async fn invalid_missing_input_settings2_tests(setting: AudioStreamSettings2) {
    let mut audio_test = AudioTest::create_and_init(&[AudioRenderUsage2::Media])
        .await
        .expect("setting up test realm");

    let result = audio_test
        .proxy()
        .set2(&AudioSettings2 { streams: Some(vec![setting]), ..Default::default() })
        .await
        .expect("set completed");
    assert_eq!(result, Err(Error::Failed));

    // The best way to test that audio core *didn't* receive an event is to
    // trigger another request and verify that it shows up next.
    set_volume2(audio_test.proxy(), vec![changed_media_stream_settings2()]).await;

    // Verify that audio core received the changed audio settings.
    audio_test
        .verify_audio_requests(&[
            AudioCoreRequest::SetVolume(AudioRenderUsage2::Media, CHANGED_VOLUME_LEVEL),
            AudioCoreRequest::SetMute(AudioRenderUsage2::Media, CHANGED_VOLUME_MUTED),
        ])
        .await
        .expect("changed audio requests received");

    audio_test.clean_up().await;
}

// Verifies that the inputs to set() calls can be missing certain parts and still be valid.
#[test_case(AudioStreamSettings {
    stream: Some(AudioRenderUsage::Media),
    source: Some(AudioStreamSettingSource::User),
    user_volume: Some(Volume {
        level: None,
        muted: Some(CHANGED_VOLUME_MUTED),
        ..Default::default()
    }),
    ..Default::default()
},  AudioCoreRequest::SetMute(AudioRenderUsage2::Media, CHANGED_VOLUME_MUTED)
; "missing user volume")]
#[test_case(AudioStreamSettings {
    stream: Some(AudioRenderUsage::Media),
    source: Some(AudioStreamSettingSource::User),
    user_volume: Some(Volume {
        level: Some(CHANGED_VOLUME_LEVEL),
        muted: None,
        ..Default::default()
    }),
    ..Default::default()
}, AudioCoreRequest::SetVolume(AudioRenderUsage2::Media, CHANGED_VOLUME_LEVEL)
; "missing muted")]
#[fuchsia::test]
async fn valid_missing_input_settings_tests(
    setting: AudioStreamSettings,
    expected_request: AudioCoreRequest,
) {
    let mut audio_test = AudioTest::create_and_init(&[AudioRenderUsage2::Media])
        .await
        .expect("setting up test realm");

    let result = audio_test
        .proxy()
        .set(&AudioSettings { streams: Some(vec![setting]), ..Default::default() })
        .await
        .expect("set completed");

    // Verify that the valid request comes through.
    audio_test
        .verify_audio_requests(&[expected_request])
        .await
        .expect("changed audio requests received");

    assert_eq!(result, Ok(()));
    audio_test.clean_up().await;
}

// Verifies that the inputs to set2() calls can be missing certain parts and still be valid.
#[test_case(AudioStreamSettings2 {
    stream: Some(AudioRenderUsage2::Media),
    source: Some(AudioStreamSettingSource::User),
    user_volume: Some(Volume {
        level: None,
        muted: Some(CHANGED_VOLUME_MUTED),
        ..Default::default()
    }),
    ..Default::default()
},  AudioCoreRequest::SetMute(AudioRenderUsage2::Media, CHANGED_VOLUME_MUTED)
; "missing user volume")]
#[test_case(AudioStreamSettings2 {
    stream: Some(AudioRenderUsage2::Media),
    source: Some(AudioStreamSettingSource::User),
    user_volume: Some(Volume {
        level: Some(CHANGED_VOLUME_LEVEL),
        muted: None,
        ..Default::default()
    }),
    ..Default::default()
}, AudioCoreRequest::SetVolume(AudioRenderUsage2::Media, CHANGED_VOLUME_LEVEL)
; "missing muted")]
#[fuchsia::test]
async fn valid_missing_input_settings2_tests(
    setting: AudioStreamSettings2,
    expected_request: AudioCoreRequest,
) {
    let mut audio_test = AudioTest::create_and_init(&[AudioRenderUsage2::Media])
        .await
        .expect("setting up test realm");

    let result = audio_test
        .proxy()
        .set2(&AudioSettings2 { streams: Some(vec![setting]), ..Default::default() })
        .await
        .expect("set completed");

    // Verify that the valid request comes through.
    audio_test
        .verify_audio_requests(&[expected_request])
        .await
        .expect("changed audio requests received");

    assert_eq!(result, Ok(()));
    audio_test.clean_up().await;
}
