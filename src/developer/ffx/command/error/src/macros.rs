// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Utility macro for constructing an [`Error`] that represents an unexpected failure
/// condition (a bug).
#[macro_export]
macro_rules! bug {
    ($error_message: expr) => {{
        $crate::Error::Unexpected($crate::macro_deps::anyhow::anyhow!($error_message))
    }};
    ($fmt:expr, $($arg:tt)*) => {
        $crate::bug!(format!($fmt, $($arg)*))
    };
}

/// Utility macro for an early exit from a function returnng an [`Error`] that
/// represents an unexpected failure condition (a bug).
#[macro_export]
macro_rules! return_bug {
    ($error_message: expr) => {{
        return Err($crate::bug!($error_message))
    }};
    ($fmt:expr, $($arg:tt)*) => {
        return Err($crate::bug!($fmt, $($arg)*))
    };
}

/// Utility macro for constructing an [`Error`] that represents a user-actionable
/// failure condition.
#[macro_export]
macro_rules! user_error {
    ($error_message: expr) => {{
        $crate::Error::User($crate::macro_deps::anyhow::anyhow!($error_message))
    }};
    ($fmt:expr, $($arg:tt)*) => {
        $crate::user_error!(format!($fmt, $($arg)*))
    };
}

/// Utility macro for an early exit from a function returnng an [`Error`] that
/// represents a user-actionable failure condition.
#[macro_export]
macro_rules! return_user_error {
    ($error_message: expr) => {{
        return Err($crate::user_error!($error_message))
    }};
    ($fmt:expr, $($arg:tt)*) => {
        return Err($crate::user_error!($fmt, $($arg)*))
    };
}

/// Utility macro for an early exit from a function returning an [`Error`] that
/// represents an exit with a specific error code but no output.
#[macro_export]
macro_rules! exit_with_code {
    ($code: expr) => {{
        return Err($crate::Error::ExitWithCode($code));
    }};
}

#[cfg(test)]
mod tests {
    use crate::Error;
    use assert_matches::assert_matches;

    /// to ease type inference around blocks used to test the `return_` variants
    fn run(f: impl Fn() -> Result<(), Error>) -> Result<(), Error> {
        f()
    }

    #[test]
    fn test_bug_macro() {
        assert_matches!(bug!("A bug!"), Error::Unexpected(err) if format!("{err}") == "A bug!");
        assert_matches!(bug!("A bug with a substitution {}!", 1), Error::Unexpected(err) if format!("{err}") == "A bug with a substitution 1!");
    }

    #[test]
    fn test_return_bug_macro() {
        assert_matches!(run(|| { return_bug!("A bug!") }), Err(Error::Unexpected(err)) if format!("{err}") == "A bug!");
        assert_matches!(run(|| { return_bug!("A bug with a substitution {}!", 1) }), Err(Error::Unexpected(err)) if format!("{err}") == "A bug with a substitution 1!");
    }

    #[test]
    fn test_user_error_macro() {
        assert_matches!(user_error!("A user error!"), Error::User(err) if format!("{err}") == "A user error!");
        assert_matches!(user_error!("A user error with a substitution {}!", 1), Error::User(err) if format!("{err}") == "A user error with a substitution 1!");
    }

    #[test]
    fn test_return_user_error_macro() {
        assert_matches!(run(|| { return_user_error!("A user error!") }), Err(Error::User(err)) if format!("{err}") == "A user error!");
        assert_matches!(run(|| { return_user_error!("A user error with a substitution {}!", 1) }), Err(Error::User(err)) if format!("{err}") == "A user error with a substitution 1!");
    }
}
