// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::Arc;
use thiserror::Error;

use crate::interpreter::{Exception, FSError, IOError, MessageError};
use crate::value::ValueError;

/// Result type that uses our error type as the default.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A stack frame for errors which produce a backtrace.
#[derive(Clone, Default)]
struct Frame {
    function: Option<String>,
    file: Option<String>,
    span: Option<Span>,
}

/// A source code position for error messages.
#[derive(Copy, Clone, Default)]
struct Span {
    line: usize,
    col: Option<(usize, usize)>,
}

/// A single error, minus backtrace information, which occurred during execution.
#[derive(Error, Clone, Debug)]
pub enum RuntimeError {
    #[error(transparent)]
    Exception(#[from] Exception),
    #[error(transparent)]
    ValueError(#[from] ValueError),
    #[error(transparent)]
    FSError(#[from] FSError),
    #[error(transparent)]
    MessageError(#[from] MessageError),
    #[error(transparent)]
    IOError(#[from] IOError),

    #[error("The interpreter was destroyed before the operation completed")]
    InterpreterDied,

    #[error(transparent)]
    User(Arc<anyhow::Error>),

    // TODO: Make a real parse error enum and get rid of these stringy things.
    #[error("{0}")]
    ParseError(String),
}

/// Error type returned by failures when running playground commands.
#[derive(Clone, Error)]
pub struct Error {
    stack: Vec<Frame>,
    #[source]
    pub source: Box<RuntimeError>,
}

impl Error {
    /// Add a stack frame to this error.
    pub fn in_func(&mut self, func: &str) {
        if let Some(frame) = self.stack.last_mut().filter(|x| x.function.is_none()) {
            frame.function = Some(func.to_owned())
        } else {
            self.stack.push(Frame { function: Some(func.to_owned()), file: None, span: None })
        }
    }

    /// Annotate an error with the file, line, and column (start and end) where it occurred.
    pub fn at(&mut self, file: Option<String>, line: usize, col: Option<(usize, usize)>) {
        self.stack.push(Frame { function: None, file, span: Some(Span { line, col }) })
    }

    /// Turn an anyhow error into a user error. If the anyhow error already
    /// wraps a Playground error type, unwrap it.
    pub(crate) fn user_from_anyhow(err: anyhow::Error) -> Self {
        let err = match err.downcast::<Self>() {
            Ok(s) => return s,
            Err(err) => err,
        };

        let err = match err.downcast::<Exception>() {
            Ok(e) => return RuntimeError::from(e).into(),
            Err(err) => err,
        };

        let err = match err.downcast::<ValueError>() {
            Ok(e) => return RuntimeError::from(e).into(),
            Err(err) => err,
        };

        let err = match err.downcast::<FSError>() {
            Ok(e) => return RuntimeError::from(e).into(),
            Err(err) => err,
        };

        let err = match err.downcast::<MessageError>() {
            Ok(e) => return RuntimeError::from(e).into(),
            Err(err) => err,
        };

        let err = match err.downcast::<IOError>() {
            Ok(e) => return RuntimeError::from(e).into(),
            Err(err) => err,
        };

        Error { stack: Vec::new(), source: Box::new(RuntimeError::User(Arc::new(err))) }
    }
}

impl<T> From<T> for Error
where
    RuntimeError: From<T>,
{
    fn from(other: T) -> Error {
        Error { stack: Vec::new(), source: Box::new(RuntimeError::from(other)) }
    }
}

impl<'a> std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.source, f)?;

        if f.alternate() {
            for (i, frame) in self.stack.iter().enumerate() {
                write!(f, "\n")?;
                write!(f, " #{:3} ", i)?;
                if let Some(function) = &frame.function {
                    write!(f, "in {}", function)?;
                } else {
                    write!(f, "at toplevel")?;
                }

                if frame.file.is_some() || frame.span.is_some() {
                    write!(f, " (")?;
                }

                if let Some(file) = &frame.file {
                    write!(f, "{}", file)?;

                    if frame.span.is_some() {
                        write!(f, " ")?;
                    }
                }

                if let Some(span) = &frame.span {
                    write!(f, "line {}", span.line)?;

                    if let Some((start, end)) = &span.col {
                        if start == end {
                            write!(f, " column {}", start)?;
                        } else {
                            write!(f, " column {}-{}", start, end)?;
                        }
                    }
                }

                if frame.file.is_some() || frame.span.is_some() {
                    write!(f, ")")?;
                }
            }
        }
        Ok(())
    }
}

impl<'a> std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self, f)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn a_few_deep() {
        let error = RuntimeError::InterpreterDied;
        let mut error = Error::from(error);
        error.in_func("foo");
        error.in_func("bar");

        assert_eq!(
            "The interpreter was destroyed before the operation completed\n #  0 in foo\n #  1 in bar",
            format!("{error:#?}")
        );
    }

    #[test]
    fn located() {
        let error = RuntimeError::InterpreterDied;
        let mut error = Error::from(error);
        error.at(Some("my_script.script".to_owned()), 23, None);
        error.in_func("foo");
        error.at(None, 45, Some((5, 7)));
        error.in_func("bar");
        error.at(Some("your_script.script".to_owned()), 5, Some((1, 1)));
        error.in_func("baz");

        assert_eq!(
            "The interpreter was destroyed before the operation completed\n \
                    #  0 in foo (my_script.script line 23)\n \
                    #  1 in bar (line 45 column 5-7)\n \
                    #  2 in baz (your_script.script line 5 column 1)",
            format!("{error:#?}")
        );
    }

    #[test]
    fn exception_thru_anyhow() {
        fn test() -> anyhow::Result<()> {
            Err(Exception::BadAdditionOperands)?
        }

        let Error { stack, source } = test().map_err(Error::user_from_anyhow).unwrap_err();
        assert!(stack.is_empty());
        assert!(matches!(*source, RuntimeError::Exception(Exception::BadAdditionOperands)));
    }

    #[test]
    fn value_error_thru_anyhow() {
        fn test() -> anyhow::Result<()> {
            Err(ValueError::ChannelClosed)?
        }

        let Error { stack, source } = test().map_err(Error::user_from_anyhow).unwrap_err();
        assert!(stack.is_empty());
        assert!(matches!(*source, RuntimeError::ValueError(ValueError::ChannelClosed)));
    }

    #[test]
    fn message_error_thru_anyhow() {
        fn test() -> anyhow::Result<()> {
            Err(MessageError::NoMessage)?
        }

        let Error { stack, source } = test().map_err(Error::user_from_anyhow).unwrap_err();
        assert!(stack.is_empty());
        assert!(matches!(*source, RuntimeError::MessageError(MessageError::NoMessage)));
    }

    #[test]
    fn io_error_thru_anyhow() {
        fn test() -> anyhow::Result<()> {
            Err(IOError::ChannelRead(fdomain_client::Error::FDomain(
                fdomain_client::FDomainError::TargetError(
                    zx_status::Status::ACCESS_DENIED.into_raw(),
                ),
            )))?
        }

        let Error { stack, source } = test().map_err(Error::user_from_anyhow).unwrap_err();
        assert!(stack.is_empty());
        let RuntimeError::IOError(IOError::ChannelRead(fdomain_client::Error::FDomain(
            fdomain_client::FDomainError::TargetError(status),
        ))) = *source
        else {
            panic!()
        };
        assert_eq!(Err(zx_status::Status::ACCESS_DENIED), zx_status::Status::ok(status));
    }

    #[test]
    fn fs_error_thru_anyhow() {
        fn test() -> anyhow::Result<()> {
            Err(FSError::FSRootNotHandle)?
        }

        let Error { stack, source } = test().map_err(Error::user_from_anyhow).unwrap_err();
        assert!(stack.is_empty());
        assert!(matches!(*source, RuntimeError::FSError(FSError::FSRootNotHandle)));
    }

    #[test]
    fn full_error_thru_anyhow() {
        fn test() -> anyhow::Result<()> {
            let error = RuntimeError::InterpreterDied;
            let mut error = Error::from(error);
            error.at(Some("my_script.script".to_owned()), 23, None);
            error.in_func("foo");
            Err(error)?
        }

        let Error { mut stack, source } = test().map_err(Error::user_from_anyhow).unwrap_err();
        let Frame { function, file, span } = stack.pop().unwrap();
        let Span { line, col } = span.unwrap();
        assert_eq!("foo", function.unwrap().as_str());
        assert_eq!("my_script.script", file.unwrap().as_str());
        assert_eq!(23, line);
        assert!(col.is_none());
        assert!(stack.is_empty());
        assert!(matches!(*source, RuntimeError::InterpreterDied));
    }
}
