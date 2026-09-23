// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input_device::{InputDevice, InputHandler};
use crate::wire;
use anyhow::Error;
use async_trait::async_trait;
use async_utils::hanging_get::client::HangingGetStream;
use fidl_fuchsia_ui_pointer::{MouseDeviceInfo, MouseEvent, MouseSourceProxy, ViewParameters};
use futures::select;
use futures::stream::{Stream, StreamExt};
use virtio_device::mem::DriverMem;
use virtio_device::queue::{DescChain, DriverNotify};

pub struct MouseDevice<
    'a,
    'b,
    N: DriverNotify,
    M: DriverMem,
    Q: Stream<Item = DescChain<'a, 'b, N>> + Unpin,
> {
    input_device: InputDevice<'a, 'b, N, M, Q>,
    mouse_events_stream: HangingGetStream<MouseSourceProxy, Vec<MouseEvent>>,
    previously_pressed: std::collections::HashSet<u8>,
    view_parameters: Option<ViewParameters>,
    device_info: Option<MouseDeviceInfo>,
}

#[async_trait(?Send)]
impl<'a, 'b, N: DriverNotify, M: DriverMem, Q: Stream<Item = DescChain<'a, 'b, N>> + Unpin>
    InputHandler for MouseDevice<'a, 'b, N, M, Q>
{
    async fn run(&mut self) -> Result<(), Error> {
        loop {
            select! {
                evs = self.mouse_events_stream.select_next_some() => {
                    if let Ok(evs) = evs {
                        self.handle_mouse_events(evs)
                    }
                }
                _chain = self.input_device.statusq_message() => {
                    // New status message
                },
            }
        }
    }
}

// An affine transformation on R that maps the interval |input_range| to |output_range|.
// The point to be mapped |a| may lie out of the input_range. This mapping is not well defined if
// input_range is finite.
fn map_interval(
    input_range: std::ops::RangeInclusive<f32>,
    output_range: std::ops::RangeInclusive<f32>,
    p: f32,
) -> f32 {
    let (input_start, input_end) = input_range.into_inner();
    let (output_start, output_end) = output_range.into_inner();
    (p - input_start) * (output_end - output_start) / (input_end - input_start) + output_start
}

impl<'a, 'b, N: DriverNotify, M: DriverMem, Q: Stream<Item = DescChain<'a, 'b, N>> + Unpin>
    MouseDevice<'a, 'b, N, M, Q>
{
    pub fn new(input_device: InputDevice<'a, 'b, N, M, Q>, mouse_source: MouseSourceProxy) -> Self {
        let mouse_events_stream =
            HangingGetStream::new_with_fn_ptr(mouse_source, MouseSourceProxy::watch);
        Self {
            input_device,
            mouse_events_stream,
            previously_pressed: std::collections::HashSet::new(),
            view_parameters: None,
            device_info: None,
        }
    }

    fn handle_mouse_events(&mut self, events: Vec<MouseEvent>) {
        for ev in events {
            let evs = self.translate_mouse_event(ev);
            if evs.is_empty() {
                // There weren't any identified mouse events.
                continue;
            }
            if let Err(e) = self.input_device.write_events_to_queue(evs.as_ref()) {
                log::warn!("Failed to write mouse event to queue with err: {e:?}");
                break;
            }
        }
    }

    fn translate_mouse_event(
        &mut self,
        event: fidl_fuchsia_ui_pointer::MouseEvent,
    ) -> Vec<wire::VirtioInputEvent> {
        let mut translation = vec![];

        if event.view_parameters.is_some() {
            self.view_parameters = event.view_parameters;
        }

        if let Some(d) = event.device_info {
            let buttons = d
                .buttons
                .as_ref()
                .expect("MouseDeviceInfo is expected to provide a list of button identifiers");
            if buttons.len() > 3 {
                log::warn!(
                    concat!(
                        "The following mouse buttons were reported by the device: {:?}, ",
                        "but only the first 3 will be used"
                    ),
                    buttons
                );
            }
            self.device_info = Some(d);
        }

        if let Some(ref pointer_sample) = event.pointer_sample {
            // Absolute position of the pointer in the viewport coordinate space
            if let Some(mut pointer_abs) = pointer_sample.position_in_viewport {
                // The input controller configures the range of abs coordinates as 0 - 64K for each
                // axis. Once we receive view parameters, we can rescale to the desired output
                // range.
                if let Some(view_params) = &self.view_parameters {
                    pointer_abs[0] = map_interval(
                        view_params.view.min[0]..=view_params.view.max[0],
                        0.0..=(u16::MAX as f32),
                        pointer_abs[0],
                    );
                    pointer_abs[1] = map_interval(
                        view_params.view.min[1]..=view_params.view.max[1],
                        0.0..=(u16::MAX as f32),
                        pointer_abs[1],
                    );
                }

                // This cast rounds towards 0 and saturates at the type's numeric limits.
                let pointer_abs = pointer_abs.map(|f| f as u32);
                translation.push(wire::VirtioInputEvent {
                    type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                    code: wire::VIRTIO_INPUT_EV_ABS_X.into(),
                    value: pointer_abs[0].into(),
                });
                translation.push(wire::VirtioInputEvent {
                    type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                    code: wire::VIRTIO_INPUT_EV_ABS_Y.into(),
                    value: pointer_abs[1].into(),
                });
            }

            if let Some(relative_scroll) = pointer_sample.scroll_v {
                let relative_scroll =
                    relative_scroll.clamp(i32::MIN.into(), i32::MAX.into()) as i32;
                translation.push(wire::VirtioInputEvent {
                    type_: wire::VIRTIO_INPUT_EV_REL.into(),
                    code: wire::VIRTIO_INPUT_EV_REL_WHEEL.into(),
                    value: (relative_scroll as u32).into(),
                });
            }

            let currently_pressed = pointer_sample
                .pressed_buttons
                .as_ref()
                .map(|pressed| std::collections::HashSet::from_iter(pressed.iter().copied()))
                .unwrap_or_else(std::collections::HashSet::new);

            // Take the first 3 button identifiers (listed in priority order) to be left-click,
            // right-click and middle-click respectively.
            if let Some(Some(button_set)) =
                self.device_info.as_ref().map(|info| info.buttons.as_ref())
            {
                for (button_idx, button_id) in button_set.iter().enumerate().take(3) {
                    if self.previously_pressed.contains(button_id)
                        && !currently_pressed.contains(button_id)
                    {
                        translation.push(wire::VirtioInputEvent {
                            type_: wire::VIRTIO_INPUT_EV_KEY.into(),
                            code: (wire::BTN_LEFT + (button_idx as u16)).into(),
                            value: wire::VIRTIO_INPUT_EV_KEY_RELEASED.into(),
                        });
                    } else if !self.previously_pressed.contains(button_id)
                        && currently_pressed.contains(button_id)
                    {
                        translation.push(wire::VirtioInputEvent {
                            type_: wire::VIRTIO_INPUT_EV_KEY.into(),
                            code: (wire::BTN_LEFT + (button_idx as u16)).into(),
                            value: wire::VIRTIO_INPUT_EV_KEY_PRESSED.into(),
                        });
                    } else {
                        // Don't need to do anything for the other two cases where the button hasn't changed state.
                    }
                }
            }

            self.previously_pressed = currently_pressed;

            // SYN events are markers to separate events. If there were no events translated we
            // should not send it.
            if !translation.is_empty() {
                translation.push(wire::SYNC_EVENT.clone());
            }
        }

        translation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use async_utils::PollExt;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_ui_pointer::{MousePointerSample, MouseSourceMarker, Rectangle};
    use fuchsia_async as fasync;
    use futures::FutureExt;
    use virtio_device::fake_queue::{ChainBuilder, IdentityDriverMem, TestQueue};
    use virtio_device::util::DescChainStream;
    use zerocopy::FromBytes;

    #[fuchsia::test]
    async fn test_map_interval() {
        // Testing various expected properties of this affine transformation
        macro_rules! assert_rescale {
            ($A: ident, $B: ident; $in:expr => $out: expr) => {
                assert_eq!(map_interval($A.clone(), $B.clone(), $in), $out)
            };
        }

        // Range grows and translates
        let a = 0.0..=1.0;
        let b = 100.0..=200.0;
        assert_rescale!(a, b; 0.0 => 100.0);
        assert_rescale!(a, b; 0.5 => 150.0);
        assert_rescale!(a, b; 1.0 => 200.0);
        assert_rescale!(a, b; -0.5 => 50.0);
        assert_rescale!(a, b; 1.5 => 250.0);

        // Range shrinks and translates
        let a = 1.0..=5.0;
        let b = 0.0..=1.0;
        assert_rescale!(a, b; 1.0 => 0.0);
        assert_rescale!(a, b; 2.0 => 0.25);
        assert_rescale!(a, b; 5.0 => 1.0);
        assert_rescale!(a, b; 0.0 => -0.25);

        // Mapping into {p} results in p
        let a = 0.0..=1.0;
        let b = 2.0..=2.0;
        assert_rescale!(a, b; 0.0 => 2.0);
        assert_rescale!(a, b; 0.5 => 2.0);
        assert_rescale!(a, b; 1.0 => 2.0);
        assert_rescale!(a, b; 1.5 => 2.0);
        assert_rescale!(a, b; -1.5 => 2.0);

        // Mapping from {p} results in a non-finite value since there can be no surjection and hence
        // no bijection from a set of smaller cardinality to larger. Technically this could be made
        // well-defined for the case of {p} to {q}, but there is not yet a need for this degenerate
        // case.
        let a = 1.0..=1.0;
        let b = 0.0..=100.0;
        assert!(!map_interval(a.clone(), b.clone(), 1.0).is_finite());

        // Backwards ranges still represent a valid linear transformation.
        let a = -1.0..=1.0;
        let b = 100.0..=0.0;
        assert_rescale!(a, b; 0.0 => 50.0);
        assert_rescale!(a, b; 1.0 => 0.0);
        assert_rescale!(a, b; -1.0 => 100.0);
        assert_rescale!(a, b; 0.5 => 25.0);
    }

    macro_rules! init_mouse_device {
        ($mem:ident, $event_queue:ident, $status_queue:ident, $device:ident, $requests:ident, $mouse_device:ident) => {
            let $mem = IdentityDriverMem::new();
            let mut $event_queue = TestQueue::new(32, &$mem);
            let $status_queue = TestQueue::new(32, &$mem);
            let $device = InputDevice::new(
                &$mem,
                DescChainStream::new(&$event_queue.queue),
                Some(DescChainStream::new(&$status_queue.queue)),
            );

            let (mouse_source_proxy, mut $requests) =
                create_proxy_and_stream::<MouseSourceMarker>();

            let mut $mouse_device = MouseDevice::new($device, mouse_source_proxy);
        };
    }

    fn read_returned<T: FromBytes>(range: (u64, u32)) -> T {
        let (data, len) = range;
        let slice =
            unsafe { std::slice::from_raw_parts::<u8>(data as usize as *const u8, len as usize) };
        T::read_from_bytes(slice).expect("Failed to read result from returned chain")
    }

    fn mouse_test_helper(
        input_events: Vec<MouseEvent>,
        expected_events: &[wire::VirtioInputEvent],
    ) {
        let mut executor = fasync::TestExecutor::new();
        init_mouse_device!(
            mem,
            event_queue,
            status_queue,
            device,
            mouse_source_request_stream,
            mouse_device
        );

        // Add enough descriptors for all the expected output events. If there aren't enough
        // descriptors when the device is processing the inputs it will simply drop the events.
        for _ in 0..expected_events.len() {
            event_queue
                .fake_queue
                .publish(
                    ChainBuilder::new()
                        .writable(std::mem::size_of::<wire::VirtioInputEvent>() as u32, &mem)
                        .build(),
                )
                .unwrap();
        }

        let mut run_mouse_device = mouse_device.run().fuse();

        let mut send_mouse_event = Box::pin(async {
            let request = mouse_source_request_stream
                .next()
                .await
                .context("MouseSourceRequestStream terminated unexpectedly")?
                .context("MouseSourceRequestStream reported a fidl error")?;
            let method_name = request.method_name();
            request
                .into_watch()
                .context(format!("Received unexpected request {method_name}"))?
                .send(input_events)
                .context("Failed to send MouseSource.Watch response")?;
            Ok::<(), anyhow::Error>(())
        })
        .fuse();

        // These must be polled together to ensure that the mouse device issues its Watch() request
        // and the send_mouse_event block responds to it!
        executor.run_singlethreaded(async { select! {
            _ = run_mouse_device => { panic!("run_mouse_device exited unexpectedly"); }
            result = send_mouse_event => result.expect("send_mouse_event returned with an error"),
        }});

        // Allow the mouse device to process the incoming MouseSource.Watch response.
        executor
            .run_until_stalled(&mut run_mouse_device)
            .expect_pending("run_mouse_device has exited unexpectedly");

        for expected_event in expected_events {
            let returned = event_queue.fake_queue.next_used().unwrap();
            let mut iter = returned.data_iter();
            let translated_event = read_returned::<wire::VirtioInputEvent>(iter.next().unwrap());
            assert_eq!(translated_event, *expected_event);
            assert!(iter.next().is_none());
        }

        assert!(event_queue.fake_queue.next_used().is_none());
    }

    fn default_view_parameters() -> Option<ViewParameters> {
        Some(ViewParameters {
            view: Rectangle { min: [0.0, 0.0], max: [100.0, 100.0] },
            viewport: Rectangle { min: [0.0, 0.0], max: [100.0, 100.0] },
            // identity matrix
            viewport_to_view_transform: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        })
    }

    fn default_device_info() -> Option<MouseDeviceInfo> {
        Some(MouseDeviceInfo { id: Some(3), buttons: Some(vec![1, 2, 3]), ..Default::default() })
    }

    #[test]
    fn test_mouse_empty_input_event() {
        let input_events = vec![];
        let expected_events = [];
        mouse_test_helper(input_events, &expected_events);
    }

    #[test]
    fn test_mouse_point_and_left_button_down_in_single_event() {
        let input_events = vec![MouseEvent {
            timestamp: Some(0),
            view_parameters: default_view_parameters(),
            device_info: default_device_info(),
            pointer_sample: Some(MousePointerSample {
                // Note: Currently the implementation makes no attempt to track ids. If that ever
                // changes this test will fail because of the below line.
                device_id: Some(5),
                position_in_viewport: Some([50.0, 50.0]),
                pressed_buttons: Some(vec![1]),
                ..Default::default()
            }),
            ..Default::default()
        }];
        let expected_events = [
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                code: wire::VIRTIO_INPUT_EV_ABS_X.into(),
                value: ((u16::MAX / 2) as u32).into(),
            },
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                code: wire::VIRTIO_INPUT_EV_ABS_Y.into(),
                value: ((u16::MAX / 2) as u32).into(),
            },
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_KEY.into(),
                code: wire::BTN_LEFT.into(),
                value: wire::VIRTIO_INPUT_EV_KEY_PRESSED.into(),
            },
            wire::SYNC_EVENT.clone(),
        ];
        mouse_test_helper(input_events, &expected_events);
    }

    #[test]
    fn test_mouse_point_and_click_across_multiple_events() {
        let input_events = vec![
            MouseEvent {
                timestamp: Some(0),
                view_parameters: default_view_parameters(),
                ..Default::default()
            },
            MouseEvent {
                timestamp: Some(1),
                device_info: default_device_info(),
                ..Default::default()
            },
            MouseEvent {
                timestamp: Some(2),
                pointer_sample: Some(MousePointerSample {
                    device_id: Some(3),
                    position_in_viewport: Some([50.0, 50.0]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            MouseEvent {
                timestamp: Some(3),
                pointer_sample: Some(MousePointerSample {
                    device_id: Some(3),
                    pressed_buttons: Some(vec![1]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            MouseEvent {
                timestamp: Some(4),
                pointer_sample: Some(MousePointerSample {
                    device_id: Some(3),
                    pressed_buttons: None,
                    ..Default::default()
                }),
                ..Default::default()
            },
        ];
        let expected_events = [
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                code: wire::VIRTIO_INPUT_EV_ABS_X.into(),
                value: ((u16::MAX / 2) as u32).into(),
            },
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                code: wire::VIRTIO_INPUT_EV_ABS_Y.into(),
                value: ((u16::MAX / 2) as u32).into(),
            },
            wire::SYNC_EVENT.clone(),
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_KEY.into(),
                code: wire::BTN_LEFT.into(),
                value: wire::VIRTIO_INPUT_EV_KEY_PRESSED.into(),
            },
            wire::SYNC_EVENT.clone(),
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_KEY.into(),
                code: wire::BTN_LEFT.into(),
                value: wire::VIRTIO_INPUT_EV_KEY_RELEASED.into(),
            },
            wire::SYNC_EVENT.clone(),
        ];
        mouse_test_helper(input_events, &expected_events);
    }

    #[test]
    fn test_mouse_move() {
        let input_events = vec![
            MouseEvent {
                timestamp: Some(0),
                view_parameters: default_view_parameters(),
                device_info: default_device_info(),
                pointer_sample: Some(MousePointerSample {
                    device_id: Some(3),
                    position_in_viewport: Some([50.0, 50.0]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            MouseEvent {
                timestamp: Some(1),
                pointer_sample: Some(MousePointerSample {
                    device_id: Some(3),
                    position_in_viewport: Some([25.0, 75.0]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ];
        let expected_events = [
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                code: wire::VIRTIO_INPUT_EV_ABS_X.into(),
                value: ((u16::MAX / 2) as u32).into(),
            },
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                code: wire::VIRTIO_INPUT_EV_ABS_Y.into(),
                value: ((u16::MAX / 2) as u32).into(),
            },
            wire::SYNC_EVENT.clone(),
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                code: wire::VIRTIO_INPUT_EV_ABS_X.into(),
                value: ((u16::MAX / 4) as u32).into(),
            },
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                code: wire::VIRTIO_INPUT_EV_ABS_Y.into(),
                value: ((u16::MAX as u32 * 3 / 4) as u32).into(),
            },
            wire::SYNC_EVENT.clone(),
        ];
        mouse_test_helper(input_events, &expected_events);
    }

    #[test]
    fn test_mouse_right_click() {
        let input_events = vec![
            MouseEvent {
                timestamp: Some(0),
                view_parameters: default_view_parameters(),
                device_info: default_device_info(),
                pointer_sample: Some(MousePointerSample {
                    device_id: Some(3),
                    pressed_buttons: Some(vec![2]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            MouseEvent {
                timestamp: Some(1),
                pointer_sample: Some(MousePointerSample {
                    device_id: Some(3),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ];
        let expected_events = [
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_KEY.into(),
                code: wire::BTN_RIGHT.into(),
                value: wire::VIRTIO_INPUT_EV_KEY_PRESSED.into(),
            },
            wire::SYNC_EVENT.clone(),
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_KEY.into(),
                code: wire::BTN_RIGHT.into(),
                value: wire::VIRTIO_INPUT_EV_KEY_RELEASED.into(),
            },
            wire::SYNC_EVENT.clone(),
        ];
        mouse_test_helper(input_events, &expected_events);
    }

    #[test]
    fn test_mouse_click_and_drag() {
        let input_events = vec![
            MouseEvent {
                timestamp: Some(0),
                view_parameters: default_view_parameters(),
                device_info: default_device_info(),
                pointer_sample: Some(MousePointerSample {
                    device_id: Some(3),
                    position_in_viewport: Some([0.0, 0.0]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            MouseEvent {
                timestamp: Some(3),
                pointer_sample: Some(MousePointerSample {
                    device_id: Some(3),
                    pressed_buttons: Some(vec![1]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            MouseEvent {
                timestamp: Some(5),
                view_parameters: default_view_parameters(),
                device_info: default_device_info(),
                pointer_sample: Some(MousePointerSample {
                    device_id: Some(3),
                    position_in_viewport: Some([50.0, 50.0]),
                    pressed_buttons: Some(vec![1]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            MouseEvent {
                timestamp: Some(8),
                pointer_sample: Some(MousePointerSample {
                    device_id: Some(3),
                    pressed_buttons: None,
                    ..Default::default()
                }),
                ..Default::default()
            },
        ];
        let expected_events = [
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                code: wire::VIRTIO_INPUT_EV_ABS_X.into(),
                value: 0.into(),
            },
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                code: wire::VIRTIO_INPUT_EV_ABS_Y.into(),
                value: 0.into(),
            },
            wire::SYNC_EVENT.clone(),
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_KEY.into(),
                code: wire::BTN_LEFT.into(),
                value: wire::VIRTIO_INPUT_EV_KEY_PRESSED.into(),
            },
            wire::SYNC_EVENT.clone(),
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                code: wire::VIRTIO_INPUT_EV_ABS_X.into(),
                value: ((u16::MAX / 2) as u32).into(),
            },
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_ABS.into(),
                code: wire::VIRTIO_INPUT_EV_ABS_Y.into(),
                value: ((u16::MAX / 2) as u32).into(),
            },
            wire::SYNC_EVENT.clone(),
            wire::VirtioInputEvent {
                type_: wire::VIRTIO_INPUT_EV_KEY.into(),
                code: wire::BTN_LEFT.into(),
                value: wire::VIRTIO_INPUT_EV_KEY_RELEASED.into(),
            },
            wire::SYNC_EVENT.clone(),
        ];
        mouse_test_helper(input_events, &expected_events);
    }
}
