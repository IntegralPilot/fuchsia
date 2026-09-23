// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{CancelableTask, LedWatcher, Update};
use crate::light_sensor::led_watcher::LedState;
use crate::light_sensor::test_utils::{
    LED1_NAME, LED2_NAME, SetupData, close_enough, setup_proxies_and_data,
};
use fidl_fuchsia_settings::{LightGroup as LightGroupFidl, LightRequest, LightState, LightValue};
use fidl_fuchsia_ui_brightness::ControlRequest;
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use std::cell::RefCell;
use std::rc::Rc;
use std::task::Poll;

#[fuchsia::test(allow_stalls = false)]
async fn led_watcher_updates_brightness_on_watch() {
    let SetupData {
        light: (light_proxy, mut light_requests),
        control: (brightness_proxy, mut brightness_requests),
        light_groups,
    } = setup_proxies_and_data();

    let (mut brightness_request_tx, mut brightness_request_rx) = mpsc::channel(0);
    let server_tasks = fasync::Task::local(async move {
        let mut lights = vec![];
        loop {
            futures::select! {
                request = light_requests.select_next_some() => lights.push(request),
                request = brightness_requests.select_next_some() => {
                    brightness_request_tx.try_send(request).expect("should be able to send");
                }
                complete => break,
            }
        }
    });

    let (update_tx, mut update_rx) = mpsc::channel(0);
    let led_watcher = LedWatcher::new_with_sender(light_groups, update_tx);
    let (cancelation_tx, cancelation_rx) = oneshot::channel();

    let inspector = fuchsia_inspect::Inspector::default();
    let test_node = inspector.root().create_child("test_node");

    let (watcher_handle, task) = led_watcher.handle_light_groups_and_brightness_watch(
        light_proxy,
        brightness_proxy,
        cancelation_rx,
        test_node.create_bool("light_proxy_receives_initial_response", false),
        test_node.create_bool("brightness_proxy_receives_initial_response", false),
    );

    // Ensure default is set to zero.
    let backlight_brightness = watcher_handle.backlight_brightness();
    assert_eq!(backlight_brightness, 0.0);

    let brightness_request = brightness_request_rx
        .next()
        .await
        .expect("should be running")
        .expect("should not be fidl error");

    const NEW_BRIGHTNESS: f32 = 0.5;
    if let ControlRequest::WatchCurrentBrightness { responder } = brightness_request {
        responder.send(NEW_BRIGHTNESS).expect("should be running");
    } else {
        panic!("Unexpected control call: {:?}", brightness_request);
    }

    loop {
        let update = update_rx.next().await.expect("should be running");
        if let Update::Brightness = update {
            break;
        }
    }

    // Now try to get updated brightness.
    let backlight_brightness = watcher_handle.backlight_brightness();
    assert_eq!(backlight_brightness, NEW_BRIGHTNESS);
    cancelation_tx.send(()).expect("should be running");
    task.await;
    server_tasks.await;
}

#[fuchsia::test(allow_stalls = false)]
async fn led_watcher_updates_light_groups_on_watch() {
    let SetupData {
        light: (light_proxy, mut light_requests),
        control: (brightness_proxy, mut brightness_requests),
        light_groups,
    } = setup_proxies_and_data();

    let (mut light_request_tx, mut light_request_rx) = mpsc::channel(0);
    let server_tasks = fasync::Task::local(async move {
        let mut brightnesses = vec![];
        loop {
            futures::select! {
                request = light_requests.select_next_some() => {
                    light_request_tx.try_send(request).expect("should be able to send");
                }
                request = brightness_requests.select_next_some() => brightnesses.push(request),
                complete => break,
            }
        }
    });

    let (update_tx, mut update_rx) = mpsc::channel(0);
    let led_watcher = LedWatcher::new_with_sender(light_groups, update_tx);
    let (cancelation_tx, cancelation_rx) = oneshot::channel();

    let inspector = fuchsia_inspect::Inspector::default();
    let test_node = inspector.root().create_child("test_node");

    let (watcher_handle, task) = led_watcher.handle_light_groups_and_brightness_watch(
        light_proxy,
        brightness_proxy,
        cancelation_rx,
        test_node.create_bool("light_proxy_receives_initial_response", false),
        test_node.create_bool("brightness_proxy_receives_initial_response", false),
    );

    // Ensure default is disabled.
    let light_groups = watcher_handle.light_groups();
    assert!(light_groups.get(LED1_NAME).unwrap().brightness.is_none());
    assert!(light_groups.get(LED2_NAME).unwrap().brightness.is_none());

    let light_request = light_request_rx
        .next()
        .await
        .expect("should be running")
        .expect("should not be fidl error");

    const NEW_LED1_VAL: f32 = 0.6;
    const NEW_LED2_VAL: f32 = 0.8;
    let new_light_groups = &[
        LightGroupFidl {
            name: Some(String::from(LED1_NAME)),
            enabled: Some(true),
            lights: Some(vec![LightState {
                value: Some(LightValue::Brightness(NEW_LED1_VAL as f64)),
                ..Default::default()
            }]),
            ..Default::default()
        },
        LightGroupFidl {
            name: Some(String::from(LED2_NAME)),
            enabled: Some(true),
            lights: Some(vec![LightState {
                value: Some(LightValue::Brightness(NEW_LED2_VAL as f64)),
                ..Default::default()
            }]),
            ..Default::default()
        },
    ];
    if let LightRequest::WatchLightGroups { responder } = light_request {
        responder.send(new_light_groups).expect("should be running");
    } else {
        panic!("Unexpected light call: {:?}", light_request);
    }

    loop {
        let update = update_rx.next().await.expect("should be running");
        if let Update::LightGroups = update {
            break;
        }
    }

    // Now try to get updated light groups.
    let light_groups = watcher_handle.light_groups();
    assert!(close_enough(light_groups.get(LED1_NAME).unwrap().brightness.unwrap(), NEW_LED1_VAL));
    assert!(close_enough(light_groups.get(LED2_NAME).unwrap().brightness.unwrap(), NEW_LED2_VAL));
    cancelation_tx.send(()).expect("should be running");
    task.await;
    server_tasks.await;
}

#[fuchsia::test]
fn cancelable_task_triggers_controlled_cancelation() {
    let (cancelation_tx, mut cancelation_rx) = oneshot::channel();
    let mut executor = fasync::TestExecutor::new();

    let done = Rc::new(RefCell::new(false));
    let mut task = fasync::Task::local({
        let done = Rc::clone(&done);
        async move {
            let mut never = futures::future::pending::<()>();
            loop {
                futures::select! {
                    _ = cancelation_rx => break,
                    _ = never => {},
                }
            }
            *done.borrow_mut() = true;
        }
    });
    // Run until stalled so we know the task is in the select block.
    if let Poll::Ready(_) = executor.run_until_stalled(&mut task) {
        panic!("Should not have completed");
    }

    // Pass the task to the ctor.
    let cancelable_task = CancelableTask::new(cancelation_tx, task);

    // Canceling should
    let cancelation_future = cancelable_task.cancel();
    futures::pin_mut!(cancelation_future);
    if let Poll::Pending = executor.run_until_stalled(&mut cancelation_future) {
        panic!("Should not have stalled after triggering cancelation");
    }

    // Verify we exited the loop in the task.
    assert!(*done.borrow());
}
