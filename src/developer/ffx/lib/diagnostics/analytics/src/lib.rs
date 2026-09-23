// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use discovery::{TargetHandle, TargetState};
use std::collections::BTreeMap;
use std::future::Future;

/// An enum used for our analytics to have a well-defined way of specifying a point of failure.
/// We can assume that all previous steps that would precede our checks would have succeeded.
// TODO(468117635): Move these into their related libraries.
pub enum PointOfFailure<'a> {
    /// A state possible if we discover a device but the device is not in the correct state.
    /// e.g. ffx_target::Resolution::from_target_handle(handle) fails.
    TargetHandleInBadState {
        state: TargetState,
    },
    /// This state occurs when the target handle is being used to connect to SSH but the state
    /// isn't correct wrt networking.
    TargetDoesntSupportNetworking {
        state: TargetState,
    },
    /// The target address could not be converted to a scoped socket addr because the link local
    /// IPv6 scope wasn't valid.
    TargetAddressBadScope,
    UnableToBuildFDomainCommand,

    /////////// RCS ERRORS ////////
    /// Failed to open HW info component. Contains a stringified FDomain error.
    FailedToOpenHWInfoComponent {
        moniker: &'static str,
        protocol: &'static str,
    },

    /// Failed to get info from HW info component. Contains a stringified FDomain error.
    UnableToGetInfo {
        error: &'a fidl::Error,
    },

    //////// FASTBOOT ERRORS ///////
    /// The target handle, for some reason, did not contain a fastboot handle.
    NonFastbootTargetHandle {
        handle: TargetHandle,
    },

    /// When querying the device for a serial number, we encountered an error.
    FastbootQueryingSerialNo {
        handle: TargetHandle,
    },
}

#[derive(Default)]
pub struct CustomEvent {
    pub category: &'static str,
    pub action: Option<String>,
    pub custom_dimensions: BTreeMap<&'static str, analytics::GA4Value>,
}

fn format_target_state(state: &TargetState) -> String {
    match state {
        TargetState::Product { .. } => "product",
        TargetState::Fastboot(_) => "fastboot",
        TargetState::Unknown => "unknown",
        TargetState::Zedboot => "zedboot",
    }
    .to_owned()
}

/// Convenience combinators for uploading analytics when using Result<_> types. Simpler than using
/// a newtype.
pub trait ResultExt {
    type Error;
    type Success;

    /// In the event of an error, takes `self` and returns the same value. If the value of `self`
    /// is an error, then sends the `pof` analytics failure before returning.
    fn or_analytics<I>(self, event: I) -> impl Future<Output = Result<Self::Success, Self::Error>>
    where
        I: Into<CustomEvent>;

    /// In the event of an error, takes `self` and returns the same value. If the value of `self`
    /// is an error, then sends the error to a closure to construct a CustomEvent struct to
    /// send to analytics.
    ///
    /// For convenience, and to avoid possible lifetime/borrow-checker issues, it is recommended
    /// to follow the pattern of implementing `Into<CustomEvent>` for a particular struct, so you
    /// would use this function like the following:
    ///
    /// ```rust
    /// possible_failure_func()
    ///     .or_else_analytics(|e| YouStruct(e).into())
    ///     .await?;
    /// ```
    fn or_else_analytics<F: FnOnce(&Self::Error) -> CustomEvent>(
        self,
        f: F,
    ) -> impl Future<Output = Result<Self::Success, Self::Error>>;

    /// See [or_else_analytics]. Behaves the same, but the closure can return None.
    fn or_else_maybe_analytics<F: FnOnce(&Self::Error) -> Option<CustomEvent>>(
        self,
        f: F,
    ) -> impl Future<Output = Result<Self::Success, Self::Error>>;
}

impl<S, E> ResultExt for Result<S, E> {
    type Error = E;
    type Success = S;

    async fn or_analytics<I>(self, event: I) -> Result<Self::Success, Self::Error>
    where
        I: Into<CustomEvent>,
    {
        match self {
            Ok(s) => Ok(s),
            Err(e) => {
                mark_point_of_failure(event).await;
                Err(e)
            }
        }
    }

    async fn or_else_maybe_analytics<F: FnOnce(&Self::Error) -> Option<CustomEvent>>(
        self,
        f: F,
    ) -> Result<Self::Success, Self::Error> {
        match self {
            Ok(s) => Ok(s),
            Err(e) => {
                match f(&e) {
                    Some(r) => mark_point_of_failure(r).await,
                    None => {}
                };
                Err(e)
            }
        }
    }

    async fn or_else_analytics<F: FnOnce(&Self::Error) -> CustomEvent>(
        self,
        f: F,
    ) -> Result<Self::Success, Self::Error> {
        match self {
            Ok(s) => Ok(s),
            Err(e) => {
                mark_point_of_failure(f(&e)).await;
                Err(e)
            }
        }
    }
}

impl From<PointOfFailure<'_>> for CustomEvent {
    fn from(pof: PointOfFailure<'_>) -> Self {
        use PointOfFailure::*;
        match pof {
            TargetHandleInBadState { state } => CustomEvent {
                category: "target_hdl_in_bad_state",
                custom_dimensions: [("target_state", format_target_state(&state).into())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            TargetDoesntSupportNetworking { state } => CustomEvent {
                category: "target_no_netwrk",
                custom_dimensions: [("target_state", format_target_state(&state).into())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            TargetAddressBadScope => {
                CustomEvent { category: "target_addr_bad_scope", ..Default::default() }
            }
            UnableToBuildFDomainCommand => {
                CustomEvent { category: "build_fdomain_cmd", ..Default::default() }
            }
            FailedToOpenHWInfoComponent { moniker, protocol } => CustomEvent {
                category: "open_hwinfo_comp",
                custom_dimensions: [("error", format!("{moniker}:{protocol}").into())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            UnableToGetInfo { error } => CustomEvent {
                category: "hwinfo_getinfo",
                custom_dimensions: [("error", format!("{error}").into())].into_iter().collect(),
                ..Default::default()
            },
            NonFastbootTargetHandle { handle } => CustomEvent {
                category: "non_fastboot_target_hdl",
                custom_dimensions: [("target_state", format_target_state(&handle.state).into())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            FastbootQueryingSerialNo { handle } => CustomEvent {
                category: "query_serialno",
                custom_dimensions: [("target_state", format_target_state(&handle.state).into())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
        }
    }
}

pub async fn is_analytics_enabled() -> bool {
    // For the time being only enable enhanced analytics for internal users.
    ffx_metrics::enhanced_analytics().await
}

fn set_or_panic(
    categories: &mut BTreeMap<&'static str, analytics::GA4Value>,
    category_name_key: &'static str,
    value: impl Into<analytics::GA4Value>,
) {
    if let Some(v) = categories.insert(category_name_key, value.into()) {
        panic!(
            "{category_name_key:?} somehow already set to {v:?}. This is a bug. Please report this to {}",
            errors::BUG_REPORT_URL
        );
    }
}

/// Takes an error, and a "point of failure," and uploads the analytics for this specific type of
/// failure for tracking.
pub async fn mark_point_of_failure(failure_point: impl Into<CustomEvent>) {
    let CustomEvent { category, action, mut custom_dimensions } = failure_point.into();
    if !is_analytics_enabled().await {
        return;
    }
    let mut client = match analytics::ga4_metrics().await {
        Ok(a) => a,
        _ => return,
    };
    set_or_panic(&mut custom_dimensions, "category_name", category);

    if let Some(subcmd_args) = ffx_diagnostics_analytics_state::get_command_line() {
        set_or_panic(&mut custom_dimensions, "command_root", subcmd_args.join(" "));
    }
    for (_k, v) in custom_dimensions.iter_mut() {
        if let analytics::GA4Value::Str(s) = v {
            *v = analytics::redact_host_and_user_from(s).into()
        }
    }
    match client
        .add_custom_event(
            None,
            action.as_deref(),
            None,
            custom_dimensions,
            Some("ffx_diagnostics_failure"),
        )
        .await
    {
        Ok(_) => {}
        Err(e) => {
            log::warn!("Unable to stage analytics: {e}");
            // No sense in sending analytics if we were unable to stage them.
            return;
        }
    }
    // If this occurs frequently enough, we may wish to inform the user via notifier that
    // analytics failed to send.
    let _ = client.send_events().await.map_err(|e| log::warn!("Unable to send analytics: {e}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_target_handle_in_bad_state_conversion() {
        let pof = PointOfFailure::TargetHandleInBadState { state: TargetState::Zedboot };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "target_hdl_in_bad_state");
        assert_eq!(event.action, None);
        assert_eq!(event.custom_dimensions.get("target_state"), Some(&"zedboot".to_owned().into()));
    }

    #[test]
    fn test_target_doesnt_support_networking_conversion() {
        let pof = PointOfFailure::TargetDoesntSupportNetworking { state: TargetState::Unknown };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "target_no_netwrk");
        assert_eq!(event.action, None);
        assert_eq!(event.custom_dimensions.get("target_state"), Some(&"unknown".to_owned().into()));
    }

    #[test]
    fn test_target_address_bad_scope_conversion() {
        let pof = PointOfFailure::TargetAddressBadScope;
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "target_addr_bad_scope");
        assert_eq!(event.action, None);
        assert!(event.custom_dimensions.is_empty());
    }

    #[test]
    fn test_unable_to_build_fdomain_command_conversion() {
        let pof = PointOfFailure::UnableToBuildFDomainCommand;
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "build_fdomain_cmd");
        assert_eq!(event.action, None);
        assert!(event.custom_dimensions.is_empty());
    }

    #[test]
    fn test_failed_to_open_hw_info_component_conversion() {
        let pof = PointOfFailure::FailedToOpenHWInfoComponent {
            moniker: "foo/bar",
            protocol: "foo.bar.Baz",
        };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "open_hwinfo_comp");
        assert_eq!(event.action, None);
        assert_eq!(event.custom_dimensions.get("error"), Some(&"foo/bar:foo.bar.Baz".into()));
    }

    #[test]
    fn test_unable_to_get_info_conversion() {
        let error = fidl::Error::ClientChannelClosed {
            epitaph: fidl::Epitaph::Explicit(Err(fidl::Status::err_from_raw(23))),
            protocol_name: "something-made-up",
            reason: None,
        };
        let pof = PointOfFailure::UnableToGetInfo { error: &error };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "hwinfo_getinfo");
        assert_eq!(event.action, None);
        assert_eq!(event.custom_dimensions.get("error"), Some(&format!("{error}").into()));
    }

    #[test]
    fn test_non_fastboot_target_handle_conversion() {
        let handle = TargetHandle { node_name: None, state: TargetState::Unknown, manual: false };
        let pof = PointOfFailure::NonFastbootTargetHandle { handle };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "non_fastboot_target_hdl");
        assert_eq!(event.action, None);
        assert_eq!(event.custom_dimensions.get("target_state"), Some(&"unknown".to_owned().into()));
    }

    #[test]
    fn test_fastboot_querying_serial_no_conversion() {
        let handle = TargetHandle { node_name: None, state: TargetState::Unknown, manual: false };
        let pof = PointOfFailure::FastbootQueryingSerialNo { handle };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "query_serialno");
        assert_eq!(event.action, None);
        assert_eq!(event.custom_dimensions.get("target_state"), Some(&"unknown".to_owned().into()));
    }
}
