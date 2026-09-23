// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use errors::{FfxError, IntoExitCode};
use ffx_config::{ConfigLevel, ConfigSource};
use fidl_fuchsia_developer_ffx::{
    DaemonError, OpenTargetError, TargetConnectionError, TunnelError,
};
use traceable_error::TraceableError;

/// Describes the source of a target specifier.
///
/// Note: The variants here do not directly map 1:1 to `ffx_config::ConfigLevel`
/// because default target resolution is intentionally stateless (see https://fxbug.dev/394619603).
/// Persistent stateful configuration levels (`User`, `Build`, `Global`) for `target.default`
/// are ignored by `ffx` to prevent configuration drift and cross-build/cross-device conflicts.
/// Instead, targets can only originate from:
/// - Command line flags (`-t` / `--target`, corresponding to `ConfigLevel::Runtime`)
/// - Programmatic overrides set directly in the tool (`context.override_target_specifier()`)
/// - Environment variables (e.g. `$FUCHSIA_NODENAME` set by `fx set-device`, or `$FUCHSIA_DEVICE_ADDR`,
///   evaluated via `ConfigLevel::Default`)
/// - Default configuration fallbacks from `config.json`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetSource {
    /// The target was specified on the command line via `-t` or `--target`.
    CommandLine,
    /// The target was overridden programmatically in the tool.
    Overridden,
    /// The target was configured via an environment variable (e.g. `$FUCHSIA_NODENAME` or `$FUCHSIA_DEVICE_ADDR`).
    Environment(String),
    /// The target was configured via default configuration.
    Default,
}

impl TargetSource {
    pub fn is_explicit(&self) -> bool {
        matches!(self, Self::CommandLine | Self::Overridden)
    }

    pub fn source_description(&self) -> String {
        match self {
            Self::CommandLine => "specified on the command line".to_string(),
            Self::Overridden => "specified in the tool".to_string(),
            Self::Environment(var) => format!("target configured by ${var}"),
            Self::Default => "target configured in default config".to_string(),
        }
    }

    pub fn remediation_hint(&self) -> String {
        match self {
            Self::CommandLine | Self::Overridden => {
                "Use `ffx target list` to list known targets, and use a different target query."
                    .to_string()
            }
            Self::Environment(var) => {
                if var == "FUCHSIA_NODENAME" {
                    "This is set by `fx set-device <name>`. Change it with `fx set-device <name>`, or remove it with `fx unset-device` (or by unsetting $FUCHSIA_NODENAME).".to_string()
                } else if var == "FUCHSIA_DEVICE_ADDR" {
                    "Change or remove it by setting/unsetting $FUCHSIA_DEVICE_ADDR.".to_string()
                } else {
                    format!("Change or remove it by setting/unsetting ${var}.")
                }
            }
            Self::Default => {
                "Set a default target with `ffx config set target.default <name>` or specify one with `-t`.".to_string()
            }
        }
    }
}

impl From<ConfigSource> for TargetSource {
    fn from(src: ConfigSource) -> Self {
        if let Some(var) = src.expanded_var {
            TargetSource::Environment(var)
        } else {
            match src.level {
                ConfigLevel::Runtime => TargetSource::CommandLine,
                _ => TargetSource::Default,
            }
        }
    }
}

fn format_open_target_error(
    err: &OpenTargetError,
    target: &Option<String>,
    targets: &[String],
    target_source: &Option<TargetSource>,
) -> String {
    let target_str = target_string(target);
    match err {
        OpenTargetError::FailedDiscovery => match target_source {
            Some(src) if !src.is_explicit() && target_str != UNSPECIFIED_TARGET_NAME => {
                format!(
                    "Could not resolve default target {target_str} ({}) due to discovery failure",
                    src.source_description()
                )
            }
            _ => format!("Could not resolve specification {target_str} due to discovery failure"),
        },
        OpenTargetError::QueryAmbiguous => {
            if target_str == UNSPECIFIED_TARGET_NAME {
                format!(
                    "More than one device/emulator found. Use `ffx target list` to list known targets and specify one with the `-t` or `--target` flag.\nCurrently found: \n\t{}",
                    targets.join("\n\t")
                )
            } else {
                match target_source {
                    Some(src) if !src.is_explicit() => {
                        format!(
                            "Default target {target_str} matched multiple targets ({}). {}\nCurrently found: \n\t{}",
                            src.source_description(),
                            src.remediation_hint(),
                            targets.join("\n\t")
                        )
                    }
                    _ => {
                        format!(
                            "Target specification {target_str} matched multiple targets. Use `ffx target list` to list known targets, and use a more specific target query.\nCurrently found: \n\t{}",
                            targets.join("\n\t")
                        )
                    }
                }
            }
        }
        OpenTargetError::TargetNotFound => {
            if target_str == UNSPECIFIED_TARGET_NAME {
                "No devices/emulators found. Please ensure the device you want to use is connected and reachable, or an emulator is started.".to_string()
            } else {
                match target_source {
                    Some(src) if !src.is_explicit() => {
                        format!(
                            "Default target {target_str} was not found ({}). {}",
                            src.source_description(),
                            src.remediation_hint()
                        )
                    }
                    _ => {
                        format!(
                            "Target specification {target_str} was not found. Use `ffx target list` to list known targets, and use a different target query."
                        )
                    }
                }
            }
        }
    }
}

/// The default target name if no target spec is given (for debugging, reporting to the user, etc).
pub const UNSPECIFIED_TARGET_NAME: &str = "[unspecified]";
/// The default target name if we fail to query the target's name.
pub const UNKNOWN_TARGET_NAME: &str = "<unknown>";
pub const BUG_REPORT_URL: &str =
    "https://issues.fuchsia.dev/issues/new?component=1378294&template=1838957";

/// FfxTargetError is an error type that maps FIDL errors onto to an error type
/// that can derive |thiserror::Error| for better error messages. These errors should
/// be use for target based (include daemon) libraries.
/// To expose these errors at a higher level with ffx subtools that are not interested
/// in accessing the FIDL error, FfxTargetError should be transformed into FfxError using the
/// Into trait.
#[derive(thiserror::Error, Clone, Debug)]
pub enum FfxTargetError {
    //#[error("{}", .0)]
    // Error(#[source] anyhow::Error, i32 /* Error status code */),
    #[cfg(not(target_os = "fuchsia"))]
    #[error("{}", match .err {
            DaemonError::Timeout => format!("Timeout attempting to reach target {}", target_string(.target)),
            DaemonError::ShutdownTimeout => match .target {
                Some(spec) if !spec.is_empty() => format!("Timeout waiting for device to shut down. Device \"{spec}\" is still responsive."),
                _ => "Timeout waiting for device to shut down. The device is still responsive.".to_string(),
            },
            DaemonError::TargetCacheEmpty => format!("No devices found."),
            DaemonError::TargetAmbiguous => format!("Target specification {} matched multiple targets. Use `ffx target list` to list known targets, and use a more specific target query.", target_string(.target)),
            DaemonError::TargetNotFound => format!("Target {} was not found.", target_string(.target)),
            DaemonError::ProtocolNotFound => "The requested ffx service was not found. Run `ffx doctor --restart-daemon`.".to_string(),
            DaemonError::ProtocolOpenError => "The requested ffx service failed to open. Run `ffx doctor --restart-daemon`.".to_string(),
            DaemonError::BadProtocolRegisterState => "The requested service could not be registered. Run `ffx doctor --restart-daemon`.".to_string(),
        })]
    DaemonError { err: DaemonError, target: Option<String> },

    #[cfg(not(target_os = "fuchsia"))]
    #[error("{}", format_open_target_error(.err, .target, .targets, .target_source))]
    OpenTargetError {
        err: OpenTargetError,
        target: Option<String>,
        targets: Vec<String>,
        target_source: Option<TargetSource>,
    },

    #[cfg(not(target_os = "fuchsia"))]
    #[error("{}", match .err {
            TunnelError::CouldNotListen => "Could not establish a host-side TCP listen socket".to_string(),
            TunnelError::TargetConnectFailed => "Couldn not connect to target to establish a tunnel".to_string(),
        })]
    TunnelError { err: TunnelError, target: Option<String> },

    #[cfg(not(target_os = "fuchsia"))]
    #[error("{}", match .err {
            TargetConnectionError::PermissionDenied => format!("Could not establish SSH connection to the target {}: Permission denied.", target_string(.target)),
            TargetConnectionError::ConnectionRefused => format!("Could not establish SSH connection to the target {}: Connection refused.", target_string(.target)),
            TargetConnectionError::ConnectionClosedByRemoteHost => format!("Could not establish SSH connection to the target {}: Connection closed by remote host.", target_string(.target)),
            TargetConnectionError::UnknownNameOrService => format!("Could not establish SSH connection to the target {}: Unknown name or service.", target_string(.target)),
            TargetConnectionError::Timeout => format!("Could not establish SSH connection to the target {}: Timed out awaiting connection.", target_string(.target)),
            TargetConnectionError::KeyVerificationFailure => format!("Could not establish SSH connection to the target {}: Key verification failed.", target_string(.target)),
            TargetConnectionError::NoRouteToHost => format!("Could not establish SSH connection to the target {}: No route to host.", target_string(.target)),
            TargetConnectionError::NetworkUnreachable => format!("Could not establish SSH connection to the target {}: Network unreachable.", target_string(.target)),
            TargetConnectionError::InvalidArgument => format!("Could not establish SSH connection to the target {}: Invalid argument. Please check the address of the target you are attempting to add.", target_string(.target)),
            TargetConnectionError::UnknownError => format!("Could not establish SSH connection to the target {}. {}. Report the error to the FFX team at {BUG_REPORT_URL}", target_string(.target), .logs.as_ref().map(|s| s.as_str()).unwrap_or("As-yet unknown error. Please refer to the logs at `ffx config get log.dir` and look for 'Unknown host-pipe error received'")),
            TargetConnectionError::FidlCommunicationError => format!("Connection was established to {}, but FIDL communication to the Remote Control Service failed. It may help to try running the command again. If this problem persists, please open a bug at {BUG_REPORT_URL}", target_string(.target)),
            TargetConnectionError::RcsConnectionError => format!("Connection was established to {}, but the Remote Control Service failed initiating a test connection. It may help to try running the command again. If this problem persists, please open a bug at {BUG_REPORT_URL}", target_string(.target)),
            TargetConnectionError::FailedToKnockService => format!("Connection was established to {}, but the Remote Control Service test connection was dropped prematurely. It may help to try running the command again. If this problem persists, please open a bug at {BUG_REPORT_URL}", target_string(.target)),
            TargetConnectionError::TargetIncompatible => {
                match .logs.as_ref() {
                    Some(l) => format!("{l}."),
                    None => format!(
                        "ffx revision {:#X} is not compatible with the target. Unable to determine target ABI revision.",
                        version_history_data::HISTORY.get_misleading_version_for_ffx().abi_revision.as_u64(),
                    ),
                }
            },
        })]
    TargetConnectionError {
        err: TargetConnectionError,
        target: Option<String>,
        logs: Option<String>,
    },

    #[cfg(not(target_os = "fuchsia"))]
    #[error("Communication with the daemon failed: {error}. Target: {}", target_string(.target))]
    DaemonCommunicationError { target: Option<String>, error: std::sync::Arc<fidl::Error> },
}

pub fn target_string(matcher: &Option<String>) -> String {
    match matcher.as_ref().map(|s| s.as_str()) {
        None | Some("") => UNSPECIFIED_TARGET_NAME.to_string(),
        Some(spec) => format!("\"{spec}\""),
    }
}

#[derive(thiserror::Error, Debug, Clone)]
pub enum DaemonProtocolError {
    #[error(
        "The daemon protocol '{svc_name}' did not match any protocols on the daemon\nIf you are not developing this plugin or the protocol it connects to, then this is a bug\nPlease report it at https://fxbug.dev/new/ffx+User+Bug."
    )]
    ProtocolNotFound { svc_name: String },

    #[error(
        "The daemon protocol '{svc_name}' failed to open on the daemon.\nIf you are developing the protocol, there may be an internal failure when invoking the start\nfunction. See the ffx.daemon.log for details at `ffx config get log.dir -p sub`.\nIf you are NOT developing this plugin or the protocol it connects to, then this is a bug.\nPlease report it at https://fxbug.dev/new/ffx+User+Bug."
    )]
    ProtocolOpenError { svc_name: String },

    #[error(
        "While attempting to open the daemon protocol '{svc_name}', received an unexpected error:\n{unexpected:?}\nThis is not intended behavior and is a bug.\nPlease report it at https://fxbug.dev/new/ffx+User+Bug."
    )]
    Unexpected { svc_name: String, unexpected: DaemonError },
}

/// Convenience function for converting protocol connection requests into more
/// diagnosable/actionable errors for the user.
pub fn map_daemon_error(svc_name: &str, err: DaemonError) -> DaemonProtocolError {
    match err {
        DaemonError::ProtocolNotFound => {
            DaemonProtocolError::ProtocolNotFound { svc_name: svc_name.to_string() }
        }
        DaemonError::ProtocolOpenError => {
            DaemonProtocolError::ProtocolOpenError { svc_name: svc_name.to_string() }
        }
        unexpected => {
            DaemonProtocolError::Unexpected { svc_name: svc_name.to_string(), unexpected }
        }
    }
}

impl IntoExitCode for FfxTargetError {
    fn exit_code(&self) -> i32 {
        match self {
            FfxTargetError::DaemonError { err, .. } => {
                i32::try_from(err.into_primitive()).unwrap_or(1)
            }
            FfxTargetError::OpenTargetError { err, .. } => {
                i32::try_from(err.into_primitive()).unwrap_or(1)
            }
            FfxTargetError::TunnelError { err, .. } => {
                i32::try_from(err.into_primitive()).unwrap_or(1)
            }
            FfxTargetError::TargetConnectionError { err, .. } => {
                i32::try_from(err.into_primitive()).unwrap_or(1)
            }
            FfxTargetError::DaemonCommunicationError { .. } => 1,
        }
    }
}
impl Into<FfxError> for FfxTargetError {
    fn into(self) -> FfxError {
        match self {
            FfxTargetError::DaemonError { ref target, .. } => {
                FfxError::DaemonError { err: Box::new(self.clone()), target: target.clone() }
            }
            FfxTargetError::OpenTargetError { ref target, .. } => FfxError::OpenTargetError {
                err: Box::new(self.clone()),
                target: target.clone(),
                exit_code: self.exit_code(),
            },
            FfxTargetError::TunnelError { ref target, .. } => {
                FfxError::TunnelError { err: Box::new(self.clone()), target: target.clone() }
            }
            FfxTargetError::TargetConnectionError { ref logs, ref target, .. } => {
                FfxError::TargetConnectionError {
                    err: Box::new(self.clone()),
                    target: target.clone(),
                    logs: logs.clone(),
                }
            }
            FfxTargetError::DaemonCommunicationError { ref target, .. } => {
                FfxError::DaemonError { err: Box::new(self.clone()), target: target.clone() }
            }
        }
    }
}

impl TraceableError for FfxTargetError {
    fn as_any(&self) -> &dyn ::std::any::Any {
        self
    }

    fn layer_code(&self) -> String {
        let variant_str = match self {
            Self::DaemonError { err, .. } => format!("DaemonError({:?})", err),
            Self::OpenTargetError { err, .. } => format!("OpenTargetError({:?})", err),
            Self::TunnelError { err, .. } => format!("TunnelError({:?})", err),
            Self::TargetConnectionError { err, .. } => format!("TargetConnectionError({:?})", err),
            Self::DaemonCommunicationError { .. } => "DaemonCommunicationError".to_string(),
        };
        format!("target_errors::FfxTargetError::{}", variant_str)
    }

    fn chain_codes(&self) -> Vec<String> {
        vec![self.layer_code()]
    }
}

#[cfg(cw)]
mod cw {
    #[cfg(not(target_os = "fuchsia"))]
    impl IntoExitCode for DaemonError {
        fn exit_code(&self) -> i32 {
            match self {
                DaemonError::Timeout => 14,
                DaemonError::TargetCacheEmpty => 15,
                DaemonError::TargetAmbiguous => 16,
                DaemonError::TargetNotFound => 17,
                DaemonError::ProtocolNotFound => 20,
                DaemonError::ProtocolOpenError => 21,
                DaemonError::BadProtocolRegisterState => 22,
            }
        }
    }

    #[cfg(not(target_os = "fuchsia"))]
    impl IntoExitCode for OpenTargetError {
        fn exit_code(&self) -> i32 {
            match self {
                OpenTargetError::TargetNotFound => 26,
                OpenTargetError::QueryAmbiguous => 27,
            }
        }
    }

    #[cfg(not(target_os = "fuchsia"))]
    impl IntoExitCode for TunnelError {
        fn exit_code(&self) -> i32 {
            match self {
                TunnelError::CouldNotListen => 31,
                TunnelError::TargetConnectFailed => 32,
            }
        }
    }

    #[cfg(not(target_os = "fuchsia"))]
    impl IntoExitCode for TargetConnectionError {
        fn exit_code(&self) -> i32 {
            match self {
                TargetConnectionError::PermissionDenied => 41,
                TargetConnectionError::ConnectionRefused => 42,
                TargetConnectionError::UnknownNameOrService => 43,
                TargetConnectionError::Timeout => 44,
                TargetConnectionError::KeyVerificationFailure => 45,
                TargetConnectionError::NoRouteToHost => 46,
                TargetConnectionError::NetworkUnreachable => 47,
                TargetConnectionError::InvalidArgument => 48,
                TargetConnectionError::UnknownError => 49,
                TargetConnectionError::FidlCommunicationError => 50,
                TargetConnectionError::RcsConnectionError => 51,
                TargetConnectionError::FailedToKnockService => 52,
                TargetConnectionError::TargetIncompatible => 53,
                TargetConnectionError::ConnectionClosedByRemoteHost => 54,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    #[test]
    fn test_daemon_error_strings_containing_target_name() {
        fn assert_contains_target_name(err: DaemonError) {
            let name: Option<String> = Some("fuchsia-f00d".to_string());
            assert!(
                format!("{}", FfxTargetError::DaemonError { err, target: name.clone() })
                    .contains(name.as_ref().unwrap())
            );
        }

        assert_contains_target_name(DaemonError::Timeout);
        assert_contains_target_name(DaemonError::TargetAmbiguous);
        assert_contains_target_name(DaemonError::TargetNotFound);
    }

    #[test]
    fn test_open_target_error_string_display() {
        fn error_message(
            err: OpenTargetError,
            target: Option<&str>,
            source: Option<TargetSource>,
        ) -> String {
            format!(
                "{}",
                FfxTargetError::OpenTargetError {
                    err,
                    target: target.map(|s| s.to_owned()),
                    targets: vec!["foo".to_string(), "bar".to_string()],
                    target_source: source,
                }
            )
        }

        // Test without source (legacy/unspecified source behavior)
        assert!(
            error_message(OpenTargetError::QueryAmbiguous, Some("ambigious-query"), None)
                .contains("Target specification \"ambigious-query\" matched multiple targets")
        );
        assert!(
            !Regex::new(r"Target specification .* matched multiple targets")
                .unwrap()
                .is_match(error_message(OpenTargetError::QueryAmbiguous, None, None).as_str())
        );

        assert!(
            error_message(OpenTargetError::TargetNotFound, Some("nonexistent-target"), None)
                .contains("Target specification \"nonexistent-target\" was not found")
        );
        assert!(
            !Regex::new(r"Target specification .* was not found")
                .unwrap()
                .is_match(error_message(OpenTargetError::TargetNotFound, None, None).as_str())
        );

        // Test with CommandLine source
        assert_eq!(
            error_message(
                OpenTargetError::TargetNotFound,
                Some("nonexistent-target"),
                Some(TargetSource::CommandLine),
            ),
            "Target specification \"nonexistent-target\" was not found. Use `ffx target list` to list known targets, and use a different target query."
        );

        // Test with Overridden source
        let overridden_err = error_message(
            OpenTargetError::TargetNotFound,
            Some("foo"),
            Some(TargetSource::Overridden),
        );
        assert_eq!(
            overridden_err,
            "Target specification \"foo\" was not found. Use `ffx target list` to list known targets, and use a different target query."
        );

        // Test with FUCHSIA_NODENAME environment source
        let nodename_err = error_message(
            OpenTargetError::TargetNotFound,
            Some("foo"),
            Some(TargetSource::Environment("FUCHSIA_NODENAME".to_string())),
        );
        assert!(nodename_err.contains(
            "Default target \"foo\" was not found (target configured by $FUCHSIA_NODENAME)."
        ));
        assert!(nodename_err.contains("fx set-device"));
        assert!(nodename_err.contains("fx unset-device"));

        // Test with FUCHSIA_DEVICE_ADDR environment source
        let addr_err = error_message(
            OpenTargetError::TargetNotFound,
            Some("192.168.1.1"),
            Some(TargetSource::Environment("FUCHSIA_DEVICE_ADDR".to_string())),
        );
        assert!(addr_err.contains("Default target \"192.168.1.1\" was not found (target configured by $FUCHSIA_DEVICE_ADDR)."));
        assert!(addr_err.contains("setting/unsetting $FUCHSIA_DEVICE_ADDR"));

        // Test with Default config source
        let default_config_err = error_message(
            OpenTargetError::TargetNotFound,
            Some("foo"),
            Some(TargetSource::Default),
        );
        assert!(default_config_err.contains(
            "Default target \"foo\" was not found (target configured in default config)."
        ));

        // Test QueryAmbiguous with source
        let ambig_err = error_message(
            OpenTargetError::QueryAmbiguous,
            Some("foo"),
            Some(TargetSource::Environment("FUCHSIA_NODENAME".to_string())),
        );
        assert!(ambig_err.contains("Default target \"foo\" matched multiple targets (target configured by $FUCHSIA_NODENAME)."));

        // Test FailedDiscovery with source
        let failed_disc_err = error_message(
            OpenTargetError::FailedDiscovery,
            Some("foo"),
            Some(TargetSource::Environment("FUCHSIA_NODENAME".to_string())),
        );
        assert_eq!(
            failed_disc_err,
            "Could not resolve default target \"foo\" (target configured by $FUCHSIA_NODENAME) due to discovery failure"
        );
    }

    #[test]
    fn test_target_string() {
        assert_eq!(target_string(&None), UNSPECIFIED_TARGET_NAME);
        assert_eq!(target_string(&Some("".to_string())), UNSPECIFIED_TARGET_NAME);
        assert_eq!(target_string(&Some("kittens".to_string())), "\"kittens\"");
    }

    #[test]
    fn test_traceable_error() {
        let err = FfxTargetError::DaemonError {
            err: DaemonError::Timeout,
            target: Some("test".to_string()),
        };
        assert_eq!(err.chain_codes().len(), 1);
        assert_eq!(err.layer_code(), "target_errors::FfxTargetError::DaemonError(Timeout)");

        let open_err = FfxTargetError::OpenTargetError {
            err: OpenTargetError::TargetNotFound,
            target: None,
            targets: vec![],
            target_source: None,
        };
        assert_eq!(
            open_err.layer_code(),
            "target_errors::FfxTargetError::OpenTargetError(TargetNotFound)"
        );

        let comm_err = FfxTargetError::DaemonCommunicationError {
            error: std::sync::Arc::new(fidl::Error::ExtraBytes),
            target: None,
        };
        assert_eq!(
            comm_err.layer_code(),
            "target_errors::FfxTargetError::DaemonCommunicationError"
        );
    }
}
