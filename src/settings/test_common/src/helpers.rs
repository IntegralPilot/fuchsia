// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_ui_input::MediaButtonsEvent;
use fuchsia_async::TestExecutor;
use futures::task::Poll;
use std::future::Future;

/// Run the provided `future` via the `executor`.
pub fn move_executor_forward(
    executor: &mut TestExecutor,
    future: impl Future<Output = ()>,
    panic_msg: &str,
) {
    futures::pin_mut!(future);
    match executor.run_until_stalled(&mut future) {
        Poll::Ready(res) => res,
        _ => panic!("{}", panic_msg),
    }
}

/// Run the provided `future` via the `executor` and return the result of the future.
pub fn move_executor_forward_and_get<T>(
    executor: &mut TestExecutor,
    future: impl Future<Output = T>,
    panic_msg: &str,
) -> T {
    futures::pin_mut!(future);
    match executor.run_until_stalled(&mut future) {
        Poll::Ready(res) => res,
        _ => panic!("{}", panic_msg),
    }
}

pub fn clone_media_buttons_event_without_wake_lease(
    event: &MediaButtonsEvent,
) -> MediaButtonsEvent {
    MediaButtonsEvent {
        volume: event.volume,
        mic_mute: event.mic_mute,
        pause: event.pause,
        camera_disable: event.camera_disable,
        power: event.power,
        function: event.function,
        device_id: event.device_id,
        ..Default::default()
    }
}
