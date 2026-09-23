// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Unit test utilities for clients of the `fuchsia.hardware.display` FIDL API.

use display_types::INVALID_DISP_ID;
use fidl::endpoints::{ClientEnd, ServerEnd};
use fidl_fuchsia_hardware_display::{
    self as display, CoordinatorListenerMarker, CoordinatorListenerProxy, CoordinatorMarker,
    CoordinatorRequestStream, VsyncAckCookie,
};
use fidl_fuchsia_hardware_display_types as display_types;
use itertools::Itertools;
use std::collections::HashMap;
use thiserror::Error;

/// Errors that can be returned by `MockCoordinator`.
#[derive(Error, Debug)]
pub enum MockCoordinatorError {
    /// Duplicate IDs were given to a function that expects objects with unique IDs. For example,
    /// MockCoordinator has been assigned multiple displays with clashing IDs.
    #[error("duplicate IDs provided")]
    DuplicateIds,

    /// Error from the underlying FIDL bindings or channel transport.
    #[error("FIDL error: {0}")]
    FidlError(#[from] fidl::Error),
}

/// MockCoordinatorError Result type alias.
pub type Result<T> = std::result::Result<T, MockCoordinatorError>;

/// `MockCoordinator` implements the server-end of the `fuchsia.hardware.display.Coordinator`
/// protocol. It minimally reproduces the display coordinator driver state machine to respond to
/// FIDL messages in a predictable and configurable manner.
pub struct MockCoordinator {
    #[allow(unused)]
    coordinator_stream: CoordinatorRequestStream,
    listener_proxy: CoordinatorListenerProxy,

    displays: HashMap<DisplayId, display::Info>,
}

// TODO(https://fxbug.dev/42080268): Instead of defining a separate DisplayId, we should
// use the same DisplayId from display_utils instead.
#[derive(Eq, Hash, Ord, PartialOrd, PartialEq)]
struct DisplayId(u64);

impl MockCoordinator {
    /// Bind a new `MockCoordinator` to the server end of a FIDL channel.
    pub fn new(
        coordinator_server_end: ServerEnd<CoordinatorMarker>,
        listener_client_end: ClientEnd<CoordinatorListenerMarker>,
    ) -> Result<MockCoordinator> {
        let coordinator_stream = coordinator_server_end.into_stream();
        let listener_proxy = listener_client_end.into_proxy();
        Ok(MockCoordinator { coordinator_stream, listener_proxy, displays: HashMap::new() })
    }

    /// Replace the list of available display devices with the given collection and send a
    /// `fuchsia.hardware.display.Coordinator.OnDisplaysChanged` event reflecting the changes.
    ///
    /// All the new displays will be reported as added while previously present displays will
    /// be reported as removed, regardless of their content.
    ///
    /// Returns an error if `displays` contains entries with repeated display IDs.
    pub fn assign_displays(&mut self, displays: Vec<display::Info>) -> Result<()> {
        let mut map = HashMap::new();
        if !displays.into_iter().all(|info| map.insert(DisplayId(info.id.value), info).is_none()) {
            return Err(MockCoordinatorError::DuplicateIds);
        }

        let added: Vec<_> = map
            .iter()
            .sorted_by(|a, b| Ord::cmp(&a.0, &b.0))
            .map(|(_, info)| info.clone())
            .collect();
        let removed: Vec<display_types::DisplayId> =
            self.displays.iter().map(|(_, info)| info.id).collect();
        self.displays = map;
        self.listener_proxy.on_displays_changed(&added, &removed)?;
        Ok(())
    }

    /// Sends a single OnVsync event to the client. The vsync event will appear to be sent from the
    /// given `display_id` even if a corresponding fake display has not been assigned by a call to
    /// `assign_displays`.
    // TODO(https://fxbug.dev/42080268): Currently we cannot use display_utils::DisplayId
    // here due to circular dependency. Instead of passing a raw u64 value, we
    // should use a generic and strong-typed DisplayId.
    pub fn emit_vsync_event(
        &self,
        display_id_value: u64,
        stamp: display::ConfigStamp,
    ) -> Result<()> {
        self.listener_proxy
            .on_vsync(
                &display_types::DisplayId { value: display_id_value },
                zx::MonotonicInstant::get(),
                &stamp,
                &VsyncAckCookie { value: INVALID_DISP_ID },
            )
            .map_err(MockCoordinatorError::from)
    }
}

/// Create a Zircon channel and return both endpoints with the server end bound to a
/// `MockCoordinator`.
///
/// NOTE: This function instantiates FIDL bindings and thus requires a fuchsia-async executor to
/// have been created beforehand.
pub fn create_proxy_and_mock()
-> Result<(display::CoordinatorProxy, display::CoordinatorListenerRequestStream, MockCoordinator)> {
    let (coordinator_proxy, coordinator_server) =
        fidl::endpoints::create_proxy::<CoordinatorMarker>();
    let (listener_client, listener_requests) =
        fidl::endpoints::create_request_stream::<CoordinatorListenerMarker>();
    Ok((
        coordinator_proxy,
        listener_requests,
        MockCoordinator::new(coordinator_server, listener_client)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Result};
    use fidl_fuchsia_hardware_display as display;
    use futures::{TryStreamExt, future};

    async fn wait_for_displays_changed_event(
        listener_requests: &mut display::CoordinatorListenerRequestStream,
    ) -> Result<(Vec<display::Info>, Vec<display_types::DisplayId>)> {
        let mut stream = listener_requests.try_filter_map(|event| match event {
            display::CoordinatorListenerRequest::OnDisplaysChanged {
                added,
                removed,
                control_handle: _,
            } => future::ok(Some((added, removed))),
            _ => future::ok(None),
        });
        stream.try_next().await?.context("failed to listen to coordinator listener requests")
    }

    #[fuchsia::test]
    async fn assign_displays_fails_with_duplicate_display_ids() {
        let displays = vec![
            display::Info {
                id: display_types::DisplayId { value: 1 },
                modes: Vec::new(),
                pixel_format: Vec::new(),
                manufacturer_name: "Foo".to_string(),
                monitor_name: "what".to_string(),
                monitor_serial: "".to_string(),
                horizontal_size_mm: 0,
                vertical_size_mm: 0,
                using_fallback_size: false,
                max_layer_count: 1,
            },
            display::Info {
                id: display_types::DisplayId { value: 1 },
                modes: Vec::new(),
                pixel_format: Vec::new(),
                manufacturer_name: "Bar".to_string(),
                monitor_name: "who".to_string(),
                monitor_serial: "".to_string(),
                horizontal_size_mm: 0,
                vertical_size_mm: 0,
                using_fallback_size: false,
                max_layer_count: 1,
            },
        ];

        let (_proxy, _listener_requests, mut mock) =
            create_proxy_and_mock().expect("failed to create MockCoordinator");
        let result = mock.assign_displays(displays);
        assert!(result.is_err());
    }

    #[fuchsia::test]
    async fn assign_displays_displays_added() -> Result<()> {
        let displays = vec![
            display::Info {
                id: display_types::DisplayId { value: 1 },
                modes: Vec::new(),
                pixel_format: Vec::new(),
                manufacturer_name: "Foo".to_string(),
                monitor_name: "what".to_string(),
                monitor_serial: "".to_string(),
                horizontal_size_mm: 0,
                vertical_size_mm: 0,
                using_fallback_size: false,
                max_layer_count: 1,
            },
            display::Info {
                id: display_types::DisplayId { value: 2 },
                modes: Vec::new(),
                pixel_format: Vec::new(),
                manufacturer_name: "Bar".to_string(),
                monitor_name: "who".to_string(),
                monitor_serial: "".to_string(),
                horizontal_size_mm: 0,
                vertical_size_mm: 0,
                using_fallback_size: false,
                max_layer_count: 1,
            },
        ];

        let (_proxy, mut listener_requests, mut mock) =
            create_proxy_and_mock().expect("failed to create MockCoordinator");
        mock.assign_displays(displays.clone())?;

        let (added, removed) = wait_for_displays_changed_event(&mut listener_requests).await?;
        assert_eq!(added, displays);
        assert_eq!(removed, vec![]);

        Ok(())
    }

    #[fuchsia::test]
    async fn assign_displays_displays_removed() -> Result<()> {
        let displays = vec![display::Info {
            id: display_types::DisplayId { value: 1 },
            modes: Vec::new(),
            pixel_format: Vec::new(),
            manufacturer_name: "Foo".to_string(),
            monitor_name: "what".to_string(),
            monitor_serial: "".to_string(),
            horizontal_size_mm: 0,
            vertical_size_mm: 0,
            using_fallback_size: false,
            max_layer_count: 1,
        }];

        let (_proxy, mut listener_requests, mut mock) =
            create_proxy_and_mock().expect("failed to create MockCoordinator");
        mock.assign_displays(displays)?;

        let _ = wait_for_displays_changed_event(&mut listener_requests).await?;

        // Remove all displays.
        mock.assign_displays(vec![])?;
        let (added, removed) = wait_for_displays_changed_event(&mut listener_requests).await?;
        assert_eq!(added, vec![]);
        assert_eq!(removed, vec![display_types::DisplayId { value: 1 }]);

        Ok(())
    }
}
