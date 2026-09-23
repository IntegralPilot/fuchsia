// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::ConnectionError;
use crate::target_connector::TargetConnectionError;

use discovery::{DiscoverySources, TargetState};

use ffx_diagnostics_analytics::CustomEvent;

/// An enum to make easy-use of the ffx_diagnostics_analytics library code. Implements
/// `Into<CustomEvent>`.
pub enum PointOfFailure<'a> {
    /// We've failed trying to get the target specifier.
    GetTargetSpecifier,

    /// SSH into the target failed.
    SshConnectionFailed { state: TargetState, reason: &'a ConnectionError },

    /// General connectivity error for the TargetConnector trait.
    TargetConnectorFailure { connection_type: &'static str, error: &'a TargetConnectionError },

    /// Failed to connect to RCS. Contains a stringified FDomain error.
    FailedToConnectRCS { error: &'a ConnectionError },

    /// This denotes a failure when we run `DiagnosticsResolver::discovered_targets`, specifically.
    DiscoveryFailure { query: Option<String>, discovery_sources: DiscoverySources },

    /// We were not able to find any matching targets.
    NoMatchingTargets { query: Option<String>, discovery_sources: DiscoverySources },

    /// We were not able to find any matching targets.
    TooManyMatchingTargets { query: Option<String>, discovery_sources: DiscoverySources },
}

fn query_or_unspecified(query: &Option<String>) -> Option<String> {
    match query {
        Some(s) => Some(s.clone()),
        None => Some("unspecified".to_owned()),
    }
}

fn sanitize_connection_error(error: &ConnectionError) -> String {
    use ConnectionError::*;
    // Depending on the frequency with which these errors are hit, it may be necessary to dive
    // deeper into the actual reason, but we want to avoid leaking PII by accident. A lot of
    // connectivity error code contains hostnames, addresses, etc.
    match error {
        ConnectionStartError(_dbg, reason) => {
            format!("ConnectionStartError: {reason}")
        }
        InternalError(_) => "InternalError".to_owned(),
        KnockError(_) => "KnockError".to_owned(),
        OvernetUnsupported => "OvernetUnsupported".to_owned(),
    }
}

fn format_target_connection_error(error: &TargetConnectionError) -> String {
    use TargetConnectionError::*;
    match error {
        // TODO(468435084): Depending on the outputs here these might need to be
        // cleaned-up/redacted. There's a rather large surface error, unfortunately, so it's
        // difficult to be exhaustive. For the time being since this is going to be handled by
        // internal-only sources it isn't that big of an issue, but it would be good to have some
        // more exhaustive audit.
        Fatal(e) => format!("fatal: {e}"),
        // In all likelihood this will provide a lot of noise in analytics, but it wouldn't make
        // sense to omit it entirely, as it could point to important trends. So if they prove
        // unimportant for the majority case these can be filtered out in a query downstream.
        //
        // It turns out there may still be unhandled ssh errors, so, since this includes the
        // `ffx_ssh::ssh::SshError::Unknown(_)` case that is not omitted (it was previously).
        NonFatal(e) => format!("non-fatal: {e}"),
    }
}

impl Into<CustomEvent> for PointOfFailure<'_> {
    fn into(self) -> CustomEvent {
        match self {
            Self::GetTargetSpecifier => {
                CustomEvent { category: "get_target_specifier", ..Default::default() }
            }
            Self::SshConnectionFailed { state, reason } => CustomEvent {
                category: "ssh_connection",
                custom_dimensions: [
                    (
                        "target_state",
                        ffx_diagnostics_formatting::format_target_state(&state).into(),
                    ),
                    ("error", sanitize_connection_error(reason).into()),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
            Self::FailedToConnectRCS { error } => CustomEvent {
                category: "connect_rcs",
                custom_dimensions: [("error", sanitize_connection_error(error).into())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            Self::DiscoveryFailure { query, discovery_sources } => CustomEvent {
                category: "discovery_failed",
                action: query_or_unspecified(&query),

                custom_dimensions: [(
                    "discovery_sources",
                    (discovery_sources.bits() as u64).into(),
                )]
                .into_iter()
                .collect(),
            },
            Self::TargetConnectorFailure { connection_type, error } => CustomEvent {
                category: "target_connector_failure",
                custom_dimensions: [
                    ("error", format_target_connection_error(&error).into()),
                    ("connection_type", connection_type.into()),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
            Self::NoMatchingTargets { query, discovery_sources } => CustomEvent {
                category: "no_matching_targets",
                action: query_or_unspecified(&query),

                custom_dimensions: [(
                    "discovery_sources",
                    (discovery_sources.bits() as u64).into(),
                )]
                .into_iter()
                .collect(),
            },
            Self::TooManyMatchingTargets { query, discovery_sources } => CustomEvent {
                category: "too_many_matching_targets",
                action: query_or_unspecified(&query),

                custom_dimensions: [(
                    "discovery_sources",
                    (discovery_sources.bits() as u64).into(),
                )]
                .into_iter()
                .collect(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_too_many_matching_targets_conversion() {
        let pof = PointOfFailure::TooManyMatchingTargets {
            query: Some("nodename or serial".to_string()),
            discovery_sources: DiscoverySources::all(),
        };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "too_many_matching_targets");
        assert_eq!(event.action, Some("nodename or serial".to_owned()));

        assert_eq!(
            event.custom_dimensions.get("discovery_sources"),
            Some(&(DiscoverySources::all().bits() as u64).into())
        );
    }

    #[test]
    fn test_no_matching_targets_conversion() {
        let pof = PointOfFailure::NoMatchingTargets {
            query: Some("nodename or serial".to_string()),
            discovery_sources: DiscoverySources::all(),
        };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "no_matching_targets");
        assert_eq!(event.action, Some("nodename or serial".to_owned()));

        assert_eq!(
            event.custom_dimensions.get("discovery_sources"),
            Some(&(DiscoverySources::all().bits() as u64).into())
        );
    }

    #[test]
    fn test_discovery_failure_conversion() {
        let pof = PointOfFailure::DiscoveryFailure {
            query: Some("nodename or serial".to_string()),
            discovery_sources: DiscoverySources::all(),
        };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "discovery_failed");
        assert_eq!(event.action, Some("nodename or serial".to_owned()));

        assert_eq!(
            event.custom_dimensions.get("discovery_sources"),
            Some(&(DiscoverySources::all().bits() as u64).into())
        );
    }

    #[test]
    fn test_invalid_query_conversion() {
        let pof = PointOfFailure::NoMatchingTargets {
            query: Some("invalid".to_string()),
            discovery_sources: DiscoverySources::all(),
        };

        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "no_matching_targets");
        assert_eq!(event.action, Some("invalid".to_owned()));
    }

    #[test]
    fn test_none_query_conversion() {
        let pof = PointOfFailure::NoMatchingTargets {
            query: None,
            discovery_sources: DiscoverySources::all(),
        };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "no_matching_targets");
        assert_eq!(event.action, Some("unspecified".to_owned()));
    }

    #[test]
    fn test_failed_to_connect_rcs_conversion() {
        let error = ConnectionError::ConnectionStartError("foo".to_owned(), "bar".to_owned());
        let pof = PointOfFailure::FailedToConnectRCS { error: &error };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "connect_rcs");
        assert_eq!(event.action, None);
        assert_eq!(
            event.custom_dimensions.get("error"),
            Some(&sanitize_connection_error(&error).into())
        );
    }

    #[test]
    fn test_ssh_connection_failed_conversion() {
        let reason = ConnectionError::ConnectionStartError("foo".to_owned(), "bar".to_owned());
        let pof =
            PointOfFailure::SshConnectionFailed { state: TargetState::Zedboot, reason: &reason };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "ssh_connection");
        assert_eq!(event.action, None);
        assert_eq!(
            event.custom_dimensions.get("target_state"),
            Some(&"in zedboot".to_owned().into())
        );
        assert_eq!(
            event.custom_dimensions.get("error"),
            Some(&sanitize_connection_error(&reason).into())
        );
    }

    #[test]
    fn test_get_target_specifier_conversion() {
        let pof = PointOfFailure::GetTargetSpecifier;
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "get_target_specifier");
        assert_eq!(event.action, None);
        assert!(event.custom_dimensions.is_empty());
    }

    #[test]
    fn test_target_connection_error_fatal() {
        let pof = PointOfFailure::TargetConnectorFailure {
            connection_type: "ssh",
            error: &TargetConnectionError::Fatal(anyhow::anyhow!("kaboom!")),
        };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "target_connector_failure");
        assert_eq!(event.action, None);
        assert_eq!(event.custom_dimensions.get("error"), Some(&"fatal: kaboom!".to_owned().into()));
        assert_eq!(event.custom_dimensions.get("connection_type"), Some(&"ssh".to_owned().into()));
    }

    #[test]
    fn test_target_connection_error_non_fatal() {
        let pof = PointOfFailure::TargetConnectorFailure {
            connection_type: "ssh",
            error: &TargetConnectionError::NonFatal(anyhow::anyhow!("kaboom?")),
        };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "target_connector_failure");
        assert_eq!(event.action, None);
        assert_eq!(
            event.custom_dimensions.get("error"),
            Some(&"non-fatal: kaboom?".to_owned().into())
        );
        assert_eq!(event.custom_dimensions.get("connection_type"), Some(&"ssh".to_owned().into()));
    }

    #[test]
    fn test_target_connection_error_ssh_stringifier() {
        let pof = PointOfFailure::TargetConnectorFailure {
            connection_type: "ssh",
            error: &TargetConnectionError::NonFatal(
                ffx_ssh::ssh::SshError::Unknown("foobar".into()).into(),
            ),
        };
        let event: CustomEvent = pof.into();
        assert_eq!(event.category, "target_connector_failure");
        assert_eq!(event.action, None);
        assert_eq!(
            event.custom_dimensions.get("error"),
            Some(&"non-fatal: unknown ssh error: foobar".to_owned().into())
        );
        assert_eq!(event.custom_dimensions.get("connection_type"), Some(&"ssh".to_owned().into()));
    }
}
