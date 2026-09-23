// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::process::ExitStatus;
use traceable_error_derive::TraceableError;

/// Re-exported libraries for macros
#[doc(hidden)]
pub mod macro_deps {
    pub use anyhow;
}

pub const BUG_REPORT_URL: &str =
    "https://issues.fuchsia.dev/issues/new?component=1378294&template=1838957";

/// The ffx main function expects a anyhow::Result from ffx plugins. If the Result is an Err it be
/// downcast to FfxError, and if successful this error is presented as a user-readable error. All
/// other error types are printed with full context and a BUG prefix, guiding the user to file bugs
/// to improve the error condition that they have experienced, with a goal to maximize actionable
/// errors over time.
// TODO(https://fxbug.dev/42135455): consider extending this to allow custom types from plugins.
#[derive(thiserror::Error, Debug, TraceableError)]
pub enum FfxError {
    #[error("{}", .0)]
    Error(#[source] anyhow::Error, i32 /* Error status code */),
    #[error("Test Error")]
    TestingError, // this is here to be used in tests for verifying errors are translated properly.
    #[cfg(not(target_os = "fuchsia"))]
    #[error("{}", .err)]
    DaemonError { err: Box<dyn std::error::Error + Send + Sync>, target: Option<String> },
    #[cfg(not(target_os = "fuchsia"))]
    #[error("{}", .err)]
    #[trace(opaque)]
    OpenTargetError {
        #[source]
        err: Box<dyn std::error::Error + Send + Sync>,
        target: Option<String>,
        exit_code: i32,
    },
    #[cfg(not(target_os = "fuchsia"))]
    #[error("{}", .err)]
    #[trace(opaque)]
    TunnelError {
        #[source]
        err: Box<dyn std::error::Error + Send + Sync>,
        target: Option<String>,
    },
    #[cfg(not(target_os = "fuchsia"))]
    #[error("{}", .err)]
    #[trace(opaque)]
    TargetConnectionError {
        #[source]
        err: Box<dyn std::error::Error + Send + Sync>,
        target: Option<String>,
        logs: Option<String>,
    },
}

/// Constructs a user-facing `FfxError` containing an error string, with a default exit status code of 1.
///
/// This should be used when you need to return an error type that will be printed directly
/// to the user terminal without a generic BUG report stack trace prefix.
///
/// # Examples
///
/// ```no_run
/// let err = ffx_error!("Target device connection refused.");
/// let err_fmt = ffx_error!("Failed to open file: {}", path.display());
/// ```
#[macro_export]
macro_rules! ffx_error {
    ($error_message: expr) => {{
        $crate::FfxError::Error($crate::macro_deps::anyhow::anyhow!($error_message), 1)
    }};
    ($fmt:expr, $($arg:tt)*) => {
        $crate::ffx_error!(format!($fmt, $($arg)*))
    };
}

/// Constructs a user-facing `FfxError`, allowing the caller to explicitly specify a custom exit status code.
///
/// # Examples
///
/// ```no_run
/// let err = ffx_error_with_code!(2, "Configuration value not found.");
/// ```
#[macro_export]
macro_rules! ffx_error_with_code {
    ($error_code:expr, $error_message:expr $(,)?) => {{
        $crate::FfxError::Error($crate::macro_deps::anyhow::anyhow!($error_message), $error_code)
    }};
    ($error_code:expr, $fmt:expr, $($arg:tt)*) => {
        $crate::ffx_error_with_code!($error_code, format!($fmt, $($arg)*))
    };
}

/// A convenience macro that constructs a user-facing `FfxError` and immediately bails (returns early)
/// from the current function context, wrapping it in `Err(...)`.
///
/// # Examples
///
/// ```no_run
/// if !path.exists() {
///     ffx_bail!("Staged target file path does not exist.");
/// }
/// ```
#[macro_export]
macro_rules! ffx_bail {
    ($msg:literal $(,)?) => {
        return Err($crate::ffx_error!($msg).into())
    };
    ($fmt:expr, $($arg:tt)*) => {
        return Err($crate::ffx_error!($fmt, $($arg)*).into())
    };
}

/// A convenience macro that constructs a user-facing `FfxError` with a custom exit code and immediately
/// returns early from the function, wrapping it in `Err(...)`.
///
/// # Examples
///
/// ```no_run
/// if !key_exists {
///     ffx_bail_with_code!(2, "Target configuration key missing.");
/// }
/// ```
#[macro_export]
macro_rules! ffx_bail_with_code {
    ($code:literal, $msg:literal $(,)?) => {
        return Err($crate::ffx_error_with_code!($code, $msg).into())
    };
    ($code:expr, $fmt:expr, $($arg:tt)*) => {
        return Err($crate::ffx_error_with_code!($code, $fmt, $($arg)*).into())
    };
}

pub trait IntoExitCode {
    fn exit_code(&self) -> i32;
}

pub trait ResultExt: IntoExitCode {
    fn ffx_error<'a>(&'a self) -> Option<&'a FfxError>;
}

impl ResultExt for anyhow::Error {
    fn ffx_error<'a>(&'a self) -> Option<&'a FfxError> {
        if let Some(ffx_err) = self.downcast_ref::<FfxError>() {
            Some(ffx_err)
        } else if let Some(boxed) = self.downcast_ref::<traceable_error::TraceableBox>() {
            boxed.as_any().downcast_ref::<FfxError>()
        } else {
            None
        }
    }
}

impl IntoExitCode for anyhow::Error {
    fn exit_code(&self) -> i32 {
        self.ffx_error().map(IntoExitCode::exit_code).unwrap_or(1)
    }
}

impl IntoExitCode for FfxError {
    fn exit_code(&self) -> i32 {
        match self {
            FfxError::Error(_, code) => *code,
            FfxError::TestingError => 254,
            #[cfg(not(target_os = "fuchsia"))]
            FfxError::DaemonError { .. } => 1,
            #[cfg(not(target_os = "fuchsia"))]
            FfxError::OpenTargetError { exit_code, .. } => *exit_code,
            #[cfg(not(target_os = "fuchsia"))]
            FfxError::TunnelError { .. } => 1,
            #[cfg(not(target_os = "fuchsia"))]
            FfxError::TargetConnectionError { .. } => 1,
        }
    }
}

// so that Result<(), E>::Ok is treated as exit code 0.
impl IntoExitCode for () {
    fn exit_code(&self) -> i32 {
        0
    }
}

impl IntoExitCode for ExitStatus {
    fn exit_code(&self) -> i32 {
        self.code().unwrap_or(0)
    }
}

impl<T, E> ResultExt for Result<T, E>
where
    T: IntoExitCode,
    E: ResultExt,
{
    fn ffx_error<'a>(&'a self) -> Option<&'a FfxError> {
        match self {
            Ok(_) => None,
            Err(err) => err.ffx_error(),
        }
    }
}

impl<T, E> IntoExitCode for Result<T, E>
where
    T: IntoExitCode,
    E: ResultExt,
{
    fn exit_code(&self) -> i32 {
        match self {
            Ok(code) => code.exit_code(),
            Err(err) => err.exit_code(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use anyhow::anyhow;
    use assert_matches::assert_matches;

    const FFX_STR: &str = "I am an ffx error";
    const ERR_STR: &str = "I am not an ffx error";

    #[test]
    fn test_ffx_result_extension() {
        let err = anyhow::Result::<()>::Err(anyhow!(ERR_STR));
        assert!(err.ffx_error().is_none());

        let err = anyhow::Result::<()>::Err(anyhow::Error::new(ffx_error!(FFX_STR)));
        assert_matches!(err.ffx_error(), Some(FfxError::Error(_, _)));
    }

    #[test]
    fn test_result_ext_exit_code_arbitrary_error() {
        let err = Result::<(), _>::Err(anyhow!(ERR_STR));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn test_ffx_result_extension_with_traceable_box() {
        let ffx_err = ffx_error_with_code!(42, FFX_STR);
        let boxed = traceable_error::TraceableBox::from(ffx_err);
        let err = anyhow::Error::from(boxed);
        assert_matches!(err.ffx_error(), Some(FfxError::Error(_, 42)));
        assert_eq!(err.exit_code(), 42);
    }
}
