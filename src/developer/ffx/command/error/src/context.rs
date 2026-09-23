// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Error;
use anyhow::Context;
use errors::{FfxError, IntoExitCode, ResultExt};
use std::fmt::Display;

/// Adds helpers to result types to produce useful error messages to the user from
/// the ffx frontend (through [`crate::Error`])
pub trait FfxContext<T, E> {
    /// Make this error into a BUG check that will display to the user as an error that
    /// shouldn't happen.
    fn bug(self) -> Result<T, Error>;

    /// Make this error into a BUG check that will display to the user as an error that
    /// shouldn't happen, with the added context.
    fn bug_context<C: Display + Send + Sync + 'static>(self, context: C) -> Result<T, Error>;

    /// Make this error into a BUG check that will display to the user as an error that
    /// shouldn't happen, with the added context returned by the closure `f`.
    fn with_bug_context<C: Display + Send + Sync + 'static>(
        self,
        f: impl FnOnce() -> C,
    ) -> Result<T, Error>;

    /// Make this error into a displayed user error, with the added context for display to the user.
    /// Use this for errors that happen in the normal course of execution, like files not being found.
    fn user_message<C: Display + Send + Sync + 'static>(self, context: C) -> Result<T, Error>;

    /// Make this error into a displayed user error, with the added context for display to the user.
    /// Use this for errors that happen in the normal course of execution, like files not being found.
    fn with_user_message<C: Display + Send + Sync + 'static>(
        self,
        f: impl FnOnce() -> C,
    ) -> Result<T, Error>;
}

/// Helper function for preventing duplicate error messages. Takes an error and, if it is of type
/// `crate::Error` extracts its source. This prevents duplicating error messages by re-wrapping the
/// error types within `FfxContext` multiple times, as the context chain messages get copied in
/// each subsequent error wrapping.
fn unwrap_source(err: anyhow::Error) -> anyhow::Error {
    match err.downcast::<Error>() {
        Ok(e) => match e.source() {
            Ok(source) => source,
            Err(e) => e.into(),
        },
        Err(e) => e,
    }
}

impl<T, E> FfxContext<T, E> for Result<T, E>
where
    Self: anyhow::Context<T, E>,
    E: Into<anyhow::Error>,
{
    fn bug(self) -> Result<T, Error> {
        self.map_err(|e| Error::Unexpected(e.into()))
    }

    fn bug_context<C: Display + Send + Sync + 'static>(self, context: C) -> Result<T, Error> {
        self.map_err(|e| Error::Unexpected(unwrap_source(e.into()).context(context)))
    }

    fn with_bug_context<C: Display + Send + Sync + 'static>(
        self,
        f: impl FnOnce() -> C,
    ) -> Result<T, Error> {
        self.bug_context((f)())
    }

    fn user_message<C: Display + Send + Sync + 'static>(self, context: C) -> Result<T, Error> {
        self.map_err(|e| Error::User(unwrap_source(e.into()).context(context)))
    }

    fn with_user_message<C: Display + Send + Sync + 'static>(
        self,
        f: impl FnOnce() -> C,
    ) -> Result<T, Error> {
        self.user_message((f)())
    }
}

impl<T> FfxContext<T, core::convert::Infallible> for Option<T>
where
    Self: anyhow::Context<T, core::convert::Infallible>,
{
    fn bug(self) -> Result<T, Error> {
        self.ok_or_else(|| Error::Unexpected(anyhow::anyhow!("Option is None")))
    }

    fn bug_context<C: Display + Send + Sync + 'static>(self, context: C) -> Result<T, Error> {
        self.context(context).map_err(Error::Unexpected)
    }

    fn with_bug_context<C: Display + Send + Sync + 'static>(
        self,
        f: impl FnOnce() -> C,
    ) -> Result<T, Error> {
        self.with_context(f).map_err(Error::Unexpected)
    }

    fn user_message<C: Display + Send + Sync + 'static>(self, context: C) -> Result<T, Error> {
        self.context(context).map_err(Error::User)
    }

    fn with_user_message<C: Display + Send + Sync + 'static>(
        self,
        f: impl FnOnce() -> C,
    ) -> Result<T, Error> {
        self.with_context(f).map_err(Error::User)
    }
}

impl ResultExt for Error {
    fn ffx_error<'a>(&'a self) -> Option<&'a FfxError> {
        match self {
            Error::User(err) | Error::Unexpected(err) | Error::Config(err) => err.ffx_error(),
            _ => None,
        }
    }
}
impl IntoExitCode for Error {
    fn exit_code(&self) -> i32 {
        use Error::*;
        match self {
            Help { code, .. } | ExitWithCode(code) => *code,
            IoError(_) => 1,
            Unexpected(err) | User(err) | Config(err) => err.exit_code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;
    use anyhow::anyhow;
    use assert_matches::assert_matches;

    #[test]
    fn error_context_helpers() {
        assert_matches!(
            anyhow::Result::<()>::Err(anyhow!(ERR_STR)).bug(),
            Err(Error::Unexpected(_)),
            "anyhow.bug() should be a bugcheck error"
        );
        assert_matches!(
            anyhow::Result::<()>::Err(anyhow!(ERR_STR)).bug_context("boom"),
            Err(Error::Unexpected(_)),
            "anyhow.bug_context() should be a bugcheck error"
        );
        assert_matches!(
            anyhow::Result::<()>::Err(anyhow!(ERR_STR)).with_bug_context(|| "boom"),
            Err(Error::Unexpected(_)),
            "anyhow.bug_context() should be a bugcheck error"
        );
        assert_matches!(
            anyhow::Result::<()>::Err(anyhow!(ERR_STR)).bug_context(FfxError::TestingError),
            Err(Error::Unexpected(_)),
            "anyhow.bug_context() should create a bugcheck error even if given an ffx error (magic reduction)"
        );
        assert_matches!(
            anyhow::Result::<()>::Err(anyhow!(ERR_STR)).user_message("boom"),
            Err(Error::User(_)),
            "anyhow.user_message() should be a user error"
        );
        assert_matches!(
            anyhow::Result::<()>::Err(anyhow!(ERR_STR)).with_user_message(|| "boom"),
            Err(Error::User(_)),
            "anyhow.with_user_message() should be a user error"
        );
        assert_matches!(
            anyhow::Result::<()>::Err(anyhow!(ERR_STR))
                .with_user_message(|| FfxError::TestingError)
                .ffx_error(),
            Some(FfxError::TestingError),
            "anyhow.with_user_message should be a user error that properly extracts to the ffx error."
        );
    }

    #[test]
    fn test_user_error_formats_through_multiple_levels() {
        let user_err =
            anyhow::Result::<()>::Err(anyhow!("the wubbler broke")).user_message("broken wubbler");
        let user_err2 = user_err.user_message("getting wubbler");
        let err_string = format!("{}", user_err2.unwrap_err());
        assert_eq!(err_string, "getting wubbler: broken wubbler: the wubbler broke");
    }

    #[test]
    fn test_bug_and_user_error_override_each_other_but_add_context() {
        let user_err =
            anyhow::Result::<()>::Err(anyhow!("the wubbler broke")).user_message("broken wubbler");
        let user_err2 = user_err.bug_context("getting wubbler");
        let user_err3 = user_err2.user_message("delegating wubbler");
        let err_string = format!("{}", user_err3.unwrap_err());
        assert_eq!(
            err_string,
            "delegating wubbler: getting wubbler: broken wubbler: the wubbler broke"
        );
    }

    #[test]
    fn test_bug_and_user_error_override_each_other_but_add_context_part_two() {
        let user_err =
            anyhow::Result::<()>::Err(anyhow!("the wubbler broke")).user_message("broken wubbler");
        let user_err2 = user_err.bug_context("getting wubbler");
        let user_err3 = user_err2.bug_context("delegating wubbler");
        let err_string = format!("{}", user_err3.unwrap_err());
        assert_eq!(
            err_string,
            "BUG: An internal command error occurred.\nError: delegating wubbler\n    1.  getting wubbler\n    2.  broken wubbler\n    3.  the wubbler broke"
        );
    }
}
