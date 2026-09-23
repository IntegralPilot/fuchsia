// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use errors::{FfxError, IntoExitCode};
use traceable_error_derive::TraceableError;

/// Represents a recoverable error. Intended to be embedded in `Error`.
#[derive(thiserror::Error, Debug)]
#[error("non-fatal error encountered")]
pub struct NonFatalError(#[source] pub anyhow::Error);

/// A top level error type for ffx tool results
#[derive(thiserror::Error, Debug, TraceableError)]
pub enum Error {
    /// An error that qualifies as a bugcheck
    Unexpected(#[source] anyhow::Error),
    /// A known kind of error that can be reported usefully to the user
    User(#[source] anyhow::Error),
    /// An early-exit that should result in outputting help to the user (like [`argh::EarlyExit`]),
    /// but is not itself an error in any meaningful sense.
    Help {
        /// The command name (argv[0..]) that should be used in supplemental help output
        command: Vec<String>,
        /// The text to output to the user
        output: String,
        /// The exit status
        code: i32,
    },
    /// An error from general I/O. Meant mostly to handle things like write!() and such, but also
    /// for potential issues with piping outputs of other commands into ffx. This isn't something
    /// that's exactly common, but is a possibility.
    #[trace(opaque)]
    IoError(#[from] std::io::Error),
    /// Something failed before ffx's configuration could be loaded (like an
    /// invalid argument, a failure to read an env config file, etc).
    ///
    /// Errors of this type should include any information the user might need
    /// to recover from the issue, because it will not advise the user to look
    /// in the log files or anything like that.
    Config(#[source] anyhow::Error),
    /// Exit with a specific error code but no output
    ExitWithCode(i32),
}

impl Error {
    /// Attempts to downcast this error into something non-fatal, returning `Ok(e)`
    /// if able to downcast to something non-fatal, else returning the original error.
    pub fn downcast_non_fatal(self) -> Result<anyhow::Error, Self> {
        fn try_downcast(err: anyhow::Error) -> Result<anyhow::Error, anyhow::Error> {
            match err.downcast::<NonFatalError>() {
                Ok(NonFatalError(e)) => Ok(e),
                Err(e) => Err(e),
            }
        }

        match self {
            Self::Help { .. } | Self::ExitWithCode(_) | Self::IoError(_) => Err(self),
            Self::User(e) => try_downcast(e).map_err(Self::User),
            Self::Unexpected(e) => try_downcast(e).map_err(Self::Unexpected),
            Self::Config(e) => try_downcast(e).map_err(Self::Config),
        }
    }

    /// Attempts to get the original `anyhow::Error` source (this is useful for chaining context
    /// errors). If successful, returns `Ok(e)` with the error source, but if there's no error
    /// source that can be returned, returns `self`.
    pub fn source(self) -> Result<anyhow::Error, Self> {
        match self {
            Self::User(e) | Self::Unexpected(e) | Self::Config(e) => Ok(e),
            Self::Help { .. } | Self::ExitWithCode(_) | Self::IoError(_) => Err(self),
        }
    }
}

/// Writes a detailed description of an anyhow error to the formatter
fn write_detailed(f: &mut std::fmt::Formatter<'_>, error: &anyhow::Error) -> std::fmt::Result {
    write!(f, "Error: {}", error)?;
    for (i, e) in error.chain().skip(1).enumerate() {
        write!(f, "\n  {: >3}.  {}", i + 1, e)?;
    }
    Ok(())
}

fn write_display(f: &mut std::fmt::Formatter<'_>, error: &anyhow::Error) -> std::fmt::Result {
    write!(f, "{error}")?;
    let mut previous_error = error.to_string();
    for e in error.chain().skip(1) {
        // This is a total hack. When errors are chained together through various thiserror
        // wrappers, what can happen is the error will use this display function to make itself
        // into a string, and the display function will show duplicates of the context chain.
        //
        // If, for example, we have something like `ffx_bail!` which returns an error, and it is
        // encapsulated into a `thiserror` enum, and then later wrapped into a
        // `ffx_command::Error::User`, we will have a context chain with the same error multiple
        // times in a row. For example, say we have something like:
        //
        // ```
        // let err = ffx_error!(anyhow!("this thing broke"));
        // let err2 = LogError::FfxError(err);
        // let err3 = ffx_command::Error::User(err2);
        // eprintln!("{err3}");
        // ```
        //
        // This will print: "this thing broke: this thing broke"
        //
        // This check will prevent that from happening without removing the context chain.
        // We check for containment using `.contains()` (rather than exact equality or
        // ends_with) to robustly handle cases where a wrapper's message includes its
        // source's message (which is common with `#[error("...: {0}")]`).
        let err_string = format!("{}", e);
        // There have been issues with empty strings in the past when formatting errors. Make
        // sure to explicitly show that an empty string is in one of the errors so that it can
        // be caught. This sort of thing used to happen with certain SSH errors.
        let err_string = if err_string.is_empty() { "\"\"".to_owned() } else { err_string };
        if previous_error.contains(&err_string) {
            continue;
        }
        write!(f, ": {}", err_string)?;
        previous_error = err_string;
    }
    Ok(())
}

// LINT.IfChange
const BUG_LINE: &str = "BUG: An internal command error occurred.";
// LINT.ThenChange(//src/testing/end_to_end/honeydew/honeydew/affordances/session/session_using_ffx.py)
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unexpected(error) => {
                writeln!(f, "{BUG_LINE}")?;
                write_detailed(f, error)
            }
            Self::User(error) | Self::Config(error) => write_display(f, error),
            Self::Help { output, .. } => write!(f, "{output}"),
            Self::ExitWithCode(code) => write!(f, "Exiting with code {code}"),
            Self::IoError(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl From<anyhow::Error> for Error {
    fn from(error: anyhow::Error) -> Self {
        // If it's already an Error, just return it
        match error.downcast::<Self>() {
            Ok(this) => this,
            // this is just a compatibility shim to extract information out of the way
            // we've traditionally divided user and unexpected errors.
            Err(error) => match error.downcast::<FfxError>() {
                Ok(err) => {
                    Self::User(anyhow::Error::from(traceable_error::TraceableBox::from(err)))
                }
                Err(err) => Self::Unexpected(err),
            },
        }
    }
}

impl From<FfxError> for Error {
    fn from(error: FfxError) -> Self {
        Error::User(anyhow::Error::from(traceable_error::TraceableBox::from(error)))
    }
}

impl Error {
    /// Map an argh early exit to our kind of error
    pub fn from_early_exit(command: &[impl AsRef<str>], early_exit: argh::EarlyExit) -> Self {
        let command = Vec::from_iter(command.iter().map(|s| s.as_ref().to_owned()));
        let output = early_exit.output;
        // if argh's early_exit status is Ok() that means it's printing help because
        // of a `--help` argument or `help` as a subcommand was passed. Otherwise
        // it's just an error parsing the arguments. So only map `status: Ok(())`
        // as help output.
        match early_exit.status {
            Ok(_) => Error::Help { command, output, code: 0 },
            Err(_) => Error::Config(anyhow::anyhow!("{}", output)),
        }
    }

    /// Get the exit code this error should correspond to if it bubbles up to `main()`
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::User(err) => err.exit_code(),
            Error::Help { code, .. } => *code,
            Error::ExitWithCode(code) => *code,
            _ => 1,
        }
    }
}

/// A convenience Result type
pub type Result<T, E = crate::Error> = core::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;
    use anyhow::anyhow;
    use assert_matches::assert_matches;
    use errors::{IntoExitCode, ffx_error, ffx_error_with_code};
    use std::io::{Cursor, Write};

    #[test]
    fn test_write_result_ffx_error() {
        let err = Error::from(ffx_error!(FFX_STR));
        let mut cursor = Cursor::new(Vec::new());

        assert_matches!(write!(&mut cursor, "{err}"), Ok(_));

        assert!(String::from_utf8(cursor.into_inner()).unwrap().contains(FFX_STR));
    }

    #[test]
    fn into_error_from_arbitrary_is_unexpected() {
        let err = anyhow!(ERR_STR);
        assert_matches!(
            Error::from(err),
            Error::Unexpected(_),
            "an arbitrary anyhow error should convert to an 'unexpected' bug check error"
        );
    }

    #[test]
    fn into_error_from_ffx_error_is_user_error() {
        let err = FfxError::Error(anyhow!(FFX_STR), 1);
        assert_matches!(
            Error::from(err),
            Error::User(_),
            "an arbitrary anyhow error should convert to a 'user' error"
        );
    }

    #[test]
    fn into_error_from_contextualized_ffx_error_prints_original_error() {
        let err = Error::from(anyhow::anyhow!(errors::ffx_error!(FFX_STR)).context("boom"));
        assert_eq!(
            &format!("{err}"),
            FFX_STR,
            "an anyhow error with context should print the original error, not the context, when stringified."
        );
    }

    #[test]
    fn test_write_result_arbitrary_error() {
        let err = Error::from(anyhow!(ERR_STR));
        let mut cursor = Cursor::new(Vec::new());

        assert_matches!(write!(&mut cursor, "{err}"), Ok(_));

        let err_str = String::from_utf8(cursor.into_inner()).unwrap();
        assert!(err_str.contains(BUG_LINE));
        assert!(err_str.contains(ERR_STR));
    }

    #[test]
    fn test_result_ext_exit_code_ffx_error() {
        let err = Result::<()>::Err(Error::from(ffx_error_with_code!(42, FFX_STR)));
        assert_eq!(err.exit_code(), 42);
    }

    #[test]
    fn test_from_ok_early_exit() {
        let command = ["testing", "--help"];
        let output = "stuff!".to_owned();
        let status = Ok(());
        let code = 0;

        let early_exit = argh::EarlyExit { output: output.clone(), status };
        let err = Error::from_early_exit(&command, early_exit);
        assert_eq!(err.exit_code(), code);
        assert_matches!(err, Error::Help { command: error_command, output: error_output, code: error_code } if error_command == command && error_output == output && error_code == code);
    }

    #[test]
    fn test_from_error_early_exit() {
        let command = ["testing", "bad", "command"];
        let output = "stuff!".to_owned();
        let status = Err(());
        let code = 1;

        let early_exit = argh::EarlyExit { output: output.clone(), status };
        let err = Error::from_early_exit(&command, early_exit);
        assert_eq!(err.exit_code(), code);
        assert_matches!(err, Error::Config(err) if format!("{err}") == output);
    }

    #[test]
    fn test_downcast_recasts_types() {
        let err = Error::User(anyhow!("boom"));
        assert_matches!(err.downcast_non_fatal(), Err(Error::User(_)));

        let err = Error::Unexpected(anyhow!("boom"));
        assert_matches!(err.downcast_non_fatal(), Err(Error::Unexpected(_)));

        let err = Error::Config(anyhow!("boom"));
        assert_matches!(err.downcast_non_fatal(), Err(Error::Config(_)));

        let err =
            Error::Help { command: vec!["foobar".to_owned()], output: "blorp".to_owned(), code: 1 };
        assert_matches!(err.downcast_non_fatal(), Err(Error::Help { .. }));

        let err = Error::ExitWithCode(2);
        assert_matches!(err.downcast_non_fatal(), Err(Error::ExitWithCode(2)));
    }

    #[test]
    fn test_downcast_non_fatal_recovers_non_fatal_error() {
        static ERR_STR: &'static str = "Oh look it's non fatal";
        let constructors = vec![Error::User, Error::Unexpected, Error::Config];
        for c in constructors.into_iter() {
            let err = c(NonFatalError(anyhow!(ERR_STR)).into());
            let res = err.downcast_non_fatal().expect("expected non-fatal downcast");
            assert_eq!(res.to_string(), ERR_STR.to_owned());
        }
    }

    #[test]
    fn test_error_source() {
        static ERR_STR: &'static str = "some nonsense";
        let constructors = vec![Error::User, Error::Unexpected, Error::Config];
        for cons in constructors.into_iter() {
            let err = cons(anyhow!(ERR_STR));
            let res = err.source();
            assert!(res.is_ok());
            assert_eq!(res.unwrap().to_string(), ERR_STR.to_owned());
        }
    }

    #[test]
    fn test_error_source_flatten_no_context() {
        assert_eq!("Some Operation", Error::User(anyhow!("Some Operation")).to_string());
    }

    // The order of context's is "in-side-out", the root-most error is
    // created first, and then the context() is attached on all of the
    // returned values, so they are created in the opposite order that they
    // are displayed.

    #[test]
    fn test_error_source_flatten_one_context() {
        let expected = "Some Other Operation: some failure";
        let error = anyhow!("some failure");
        let error = error.context("Some Other Operation");
        assert_eq!(expected, Error::User(error).to_string());
    }

    #[test]
    fn test_error_source_flatten_two_contexts() {
        let expected = "Some Operation: some context: some failure";
        let error = anyhow!("some failure");
        let error = error.context("some context");
        let error = error.context("Some Operation");
        assert_eq!(expected, Error::User(error).to_string());
    }

    #[test]
    fn test_error_source_flatten_three_contexts() {
        let expected = "Some Operation: some context: more context: some failure";
        let error = anyhow!("some failure")
            .context("more context")
            .context("some context")
            .context("Some Operation");
        assert_eq!(expected, Error::User(error).to_string());
    }

    #[test]
    fn test_error_doesnt_duplicate_when_rewrapped() {
        #[derive(thiserror::Error, Debug)]
        enum NonsenseErr {
            #[error(transparent)]
            Error(#[from] FfxError),
        }
        let expected = "This thing broke!";
        let error = ffx_error!(anyhow!(expected));
        let error: NonsenseErr = error.into();
        let error = Error::User(error.into());
        assert_eq!(
            error.to_string(),
            expected.to_owned(),
            "There should be no duplication from re-wrapping errors"
        );
    }

    #[test]
    fn test_non_fatal_error_formatting() {
        let inner = anyhow!("inner error");
        let non_fatal = NonFatalError(inner);
        let err = Error::User(anyhow!(non_fatal));
        assert_eq!(format!("{}", err), "non-fatal error encountered: inner error");
    }

    #[test]
    fn test_error_doesnt_duplicate_alternating() {
        #[derive(thiserror::Error, Debug)]
        #[error("Prefix: {0}")]
        struct OuterError(#[source] anyhow::Error);

        #[derive(thiserror::Error, Debug)]
        #[error("NonFatal")]
        struct NonFatal(#[source] anyhow::Error);

        #[derive(thiserror::Error, Debug)]
        #[error("TargetNotFound")]
        struct TargetNotFound;

        let leaf = TargetNotFound;
        let non_fatal = NonFatal(anyhow::Error::new(leaf));
        let fho_err = Error::User(anyhow::Error::new(non_fatal));
        let outer = OuterError(anyhow::Error::new(fho_err));
        let top = Error::User(anyhow::Error::new(outer));

        let formatted = top.to_string();
        assert_eq!(formatted, "Prefix: NonFatal: TargetNotFound");
    }
}
