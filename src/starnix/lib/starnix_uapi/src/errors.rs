// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(dead_code)]

use crate::{SA_RESTART, sigaction_t};
use std::fmt::{Debug, Display, Formatter};

#[derive(Clone, Debug)]
pub struct Errno {
    pub code: ErrnoCode,
    location: &'static std::panic::Location<'static>,
    context: Option<String>,
}

impl Errno {
    /// Creates a new `Errno` with the given `ErrnoCode` and the location of the caller.
    ///
    /// Callers should use the `errno!` macro instead of calling this function directly.
    #[track_caller]
    pub fn new(code: ErrnoCode) -> Self {
        Errno { code, location: std::panic::Location::caller(), context: None }
    }

    #[track_caller]
    pub fn with_context(code: ErrnoCode, context: impl ToString) -> Self {
        Errno { code, location: std::panic::Location::caller(), context: Some(context.to_string()) }
    }

    pub fn return_value(&self) -> u64 {
        self.code.return_value()
    }

    /// Returns whether this `Errno` indicates that a restartable syscall was interrupted.
    pub fn is_restartable(&self) -> bool {
        self.code.is_restartable()
    }
}

impl PartialEq for Errno {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code
    }
}

impl PartialEq<ErrnoCode> for Errno {
    fn eq(&self, other: &ErrnoCode) -> bool {
        self.code == *other
    }
}

impl Eq for Errno {}

impl Display for Errno {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(context) = &self.context {
            write!(f, "{} from {}, context: {}", self.code, self.location, context)
        } else {
            write!(f, "{} from {}", self.code, self.location)
        }
    }
}

impl std::error::Error for Errno {}

impl From<Errno> for zx_status::Status {
    fn from(e: Errno) -> Self {
        match e.code.error_code() {
            crate::uapi::ENOENT => zx_status::Status::NOT_FOUND,
            crate::uapi::ENOMEM => zx_status::Status::NO_MEMORY,
            crate::uapi::EINVAL => zx_status::Status::INVALID_ARGS,
            crate::uapi::ETIMEDOUT => zx_status::Status::TIMED_OUT,
            crate::uapi::EBUSY => zx_status::Status::UNAVAILABLE,
            crate::uapi::EEXIST => zx_status::Status::ALREADY_EXISTS,
            crate::uapi::EPIPE => zx_status::Status::PEER_CLOSED,
            crate::uapi::ENAMETOOLONG => zx_status::Status::BAD_PATH,
            crate::uapi::EIO => zx_status::Status::IO,
            crate::uapi::EISDIR => zx_status::Status::NOT_FILE,
            crate::uapi::ENOTDIR => zx_status::Status::NOT_DIR,
            crate::uapi::EOPNOTSUPP => zx_status::Status::NOT_SUPPORTED,
            crate::uapi::EBADF => zx_status::Status::BAD_HANDLE,
            crate::uapi::EACCES => zx_status::Status::ACCESS_DENIED,
            crate::uapi::EAGAIN => zx_status::Status::SHOULD_WAIT,
            crate::uapi::EFBIG => zx_status::Status::FILE_BIG,
            crate::uapi::ENOSPC => zx_status::Status::NO_SPACE,
            crate::uapi::ENOTEMPTY => zx_status::Status::NOT_EMPTY,
            crate::uapi::EPROTONOSUPPORT => zx_status::Status::PROTOCOL_NOT_SUPPORTED,
            crate::uapi::ENETUNREACH => zx_status::Status::ADDRESS_UNREACHABLE,
            crate::uapi::EADDRINUSE => zx_status::Status::ADDRESS_IN_USE,
            crate::uapi::ENOTCONN => zx_status::Status::NOT_CONNECTED,
            crate::uapi::ECONNREFUSED => zx_status::Status::CONNECTION_REFUSED,
            crate::uapi::ECONNRESET => zx_status::Status::CONNECTION_RESET,
            crate::uapi::ECONNABORTED => zx_status::Status::CONNECTION_ABORTED,
            _ => zx_status::Status::NOT_SUPPORTED,
        }
    }
}

#[derive(Eq, PartialEq, Copy, Clone)]
pub struct ErrnoCode(u32);

impl ErrnoCode {
    pub const fn from_return_value(retval: u64) -> Self {
        let retval = retval as i64;
        if retval >= 0 {
            // Collapse all success codes to 0. This is the only value in the u32 range which
            // is guaranteed to not be an error code.
            return Self(0);
        }
        Self(-retval as u32)
    }

    pub const fn from_error_code(code: i16) -> Self {
        Self(code as u32)
    }

    pub const fn return_value(&self) -> u64 {
        -(self.0 as i32) as u64
    }

    pub const fn error_code(&self) -> u32 {
        self.0
    }

    /// Returns whether this `ErrnoCode` indicates that a restartable syscall was interrupted.
    pub fn is_restartable(&self) -> bool {
        matches!(*self, ERESTARTSYS | ERESTARTNOINTR | ERESTARTNOHAND | ERESTART_RESTARTBLOCK)
    }

    /// Returns whether a combination of this `ErrnoCode` and a given `sigaction_t` mean that an
    /// interrupted syscall should be restarted.
    ///
    /// Conditions for restarting syscalls:
    ///
    /// * the error code is `ERESTARTSYS`, `ERESTARTNOINTR`, `ERESTARTNOHAND`, or
    ///   `ERESTART_RESTARTBLOCK` and
    /// * `ERESTARTNOINTR` is always restarted
    /// * `ERESTARTSYS` is only restarted if the `SA_RESTART` flag is set in the `sigaction_t`
    /// * all four error codes are restarted if no signal handler was present in the `sigaction_t`
    pub fn should_restart(&self, sigaction: Option<sigaction_t>) -> bool {
        // If sigaction is None, the syscall must be restarted if it is restartable. The default
        // sigaction will not have a sighandler, which will guarantee a restart.
        let sigaction = sigaction.unwrap_or_default();

        let should_restart_even_if_sigaction_handler_not_null = match *self {
            ERESTARTSYS => sigaction.sa_flags & SA_RESTART as u64 != 0,
            ERESTARTNOINTR => true,
            ERESTARTNOHAND | ERESTART_RESTARTBLOCK => false,

            // Never restart a syscall unless it's one of the above errno codes.
            _ => return false,
        };

        // Always restart if the signal did not call a handler (i.e. SIGSTOP).
        should_restart_even_if_sigaction_handler_not_null || sigaction.sa_handler.addr == 0
    }
}

impl Debug for ErrnoCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

impl Display for ErrnoCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name(), self.0)
    }
}

// Special errors indicating a blocking syscall was interrupted, but it can be restarted.
//
// They are not defined in uapi, but can be observed by ptrace on Linux.
//
// If the syscall is restartable, it might not be restarted, depending on the value of SA_RESTART
// for the signal handler and the specific restartable error code.
// But it will always be restarted if the signal did not call a userspace signal handler.
// If not restarted, this error code is converted into EINTR.
//
// More information can be found at
// https://cs.opensource.google/gvisor/gvisor/+/master:pkg/errors/linuxerr/internal.go;l=71;drc=2bb73c7bd7dcf0b36e774d8e82e464d04bc81f4b.

/// Convert to EINTR if interrupted by a signal handler without SA_RESTART enabled, otherwise
/// restart.
pub const ERESTARTSYS: ErrnoCode = ErrnoCode(512);

/// Always restart, regardless of the signal handler.
pub const ERESTARTNOINTR: ErrnoCode = ErrnoCode(513);

/// Convert to EINTR if interrupted by a signal handler. SA_RESTART is ignored. Otherwise restart.
pub const ERESTARTNOHAND: ErrnoCode = ErrnoCode(514);

/// Like `ERESTARTNOHAND`, but restart by invoking a closure instead of calling the syscall
/// implementation again.
pub const ERESTART_RESTARTBLOCK: ErrnoCode = ErrnoCode(516);

/// An extension trait for `Result<T, Errno>`.
pub trait ErrnoResultExt<T> {
    /// Maps `Err(EINTR)` to the specified errno.
    fn map_eintr(self, make_errno: impl Fn() -> Errno) -> Result<T, Errno>;
}

impl<T> ErrnoResultExt<T> for Result<T, Errno> {
    fn map_eintr(self, make_errno: impl Fn() -> Errno) -> Result<T, Errno> {
        self.map_err(|err| if err == EINTR { make_errno() } else { err })
    }
}

macro_rules! errno_codes {
    ($($name:ident),+) => {
        $(pub const $name: ErrnoCode = ErrnoCode(crate::uapi::$name);)+

        impl ErrnoCode {
            fn name(&self) -> &'static str {
                match self.0 {
                    $(
                        crate::uapi::$name => stringify!($name),
                    )+
                    _ => "unknown error code",
                }
            }
        }

        #[cfg(test)]
        #[test]
        fn expected_errno_code_strings() {
            $(
                assert_eq!(
                    $name.to_string(),
                    format!("{}({})", stringify!($name), crate::uapi::$name),
                );
            )+
        }
    };
}

errno_codes![
    EPERM,
    ENOENT,
    ESRCH,
    EINTR,
    EIO,
    ENXIO,
    E2BIG,
    ENOEXEC,
    EBADF,
    ECHILD,
    EAGAIN,
    ENOMEM,
    EACCES,
    EFAULT,
    ENOTBLK,
    EBUSY,
    EEXIST,
    EXDEV,
    ENODEV,
    ENOTDIR,
    EISDIR,
    EINVAL,
    ENFILE,
    EMFILE,
    ENOTTY,
    ETXTBSY,
    EFBIG,
    ENOSPC,
    ESPIPE,
    EROFS,
    EMLINK,
    EPIPE,
    EDOM,
    ERANGE,
    ENAMETOOLONG,
    ENOLCK,
    ENOSYS,
    ENOTEMPTY,
    ELOOP,
    ENOMSG,
    EIDRM,
    ECHRNG,
    EL2NSYNC,
    EL3HLT,
    EL3RST,
    ELNRNG,
    EUNATCH,
    ENOCSI,
    EL2HLT,
    EBADE,
    EBADR,
    EXFULL,
    ENOANO,
    EBADRQC,
    EBADSLT,
    EDEADLOCK,
    EBFONT,
    ENOSTR,
    ENODATA,
    ETIME,
    ENOSR,
    ENONET,
    ENOPKG,
    EREMOTE,
    ENOLINK,
    EADV,
    ESRMNT,
    ECOMM,
    EPROTO,
    EMULTIHOP,
    EDOTDOT,
    EBADMSG,
    EOVERFLOW,
    ENOTUNIQ,
    EBADFD,
    EREMCHG,
    ELIBACC,
    ELIBBAD,
    ELIBSCN,
    ELIBMAX,
    ELIBEXEC,
    EILSEQ,
    ERESTART,
    ESTRPIPE,
    EUSERS,
    ENOTSOCK,
    EDESTADDRREQ,
    EMSGSIZE,
    EPROTOTYPE,
    ENOPROTOOPT,
    EPROTONOSUPPORT,
    ESOCKTNOSUPPORT,
    EOPNOTSUPP,
    EPFNOSUPPORT,
    EAFNOSUPPORT,
    EADDRINUSE,
    EADDRNOTAVAIL,
    ENETDOWN,
    ENETUNREACH,
    ENETRESET,
    ECONNABORTED,
    ECONNRESET,
    ENOBUFS,
    EISCONN,
    ENOTCONN,
    ESHUTDOWN,
    ETOOMANYREFS,
    ETIMEDOUT,
    ECONNREFUSED,
    EHOSTDOWN,
    EHOSTUNREACH,
    EALREADY,
    EINPROGRESS,
    ESTALE,
    EUCLEAN,
    ENOTNAM,
    ENAVAIL,
    EISNAM,
    EREMOTEIO,
    EDQUOT,
    ENOMEDIUM,
    EMEDIUMTYPE,
    ECANCELED,
    ENOKEY,
    EKEYEXPIRED,
    EKEYREVOKED,
    EKEYREJECTED,
    EOWNERDEAD,
    ENOTRECOVERABLE,
    ERFKILL,
    EHWPOISON
];

// ENOTSUP is a different error in posix, but has the same value as EOPNOTSUPP in linux.
pub const ENOTSUP: ErrnoCode = EOPNOTSUPP;

/// `errno` returns an `Errno` struct tagged with the current file name and line number.
///
/// Use `error!` instead if you want the `Errno` to be wrapped in an `Err`.
#[macro_export]
macro_rules! errno {
    ($err:ident) => {
        $crate::errors::Errno::new($crate::errors::$err)
    };
    ($err:ident, $context:expr) => {
        $crate::errors::Errno::with_context($crate::errors::$err, $context.to_string())
    };
}

/// `error` returns a `Err` containing an `Errno` struct tagged with the current file name and line
/// number.
///
/// Use `errno!` instead if you want an unwrapped, but still tagged, `Errno`.
#[macro_export]
macro_rules! error {
    ($($args:tt)*) => { Err($crate::errno!($($args)*)) };
}

/// `errno_from_code` returns a `Err` containing an `Errno` struct with the given error code and is
/// tagged with the current file name and line number.
#[macro_export]
macro_rules! errno_from_code {
    ($err:expr) => {{ $crate::errors::Errno::new($crate::errors::ErrnoCode::from_error_code($err)) }};
}

/// `errno_from_zxio_code` returns an `Errno` struct with the given error code and is
/// tagged with the current file name and line number.
#[macro_export]
macro_rules! errno_from_zxio_code {
    ($err:expr) => {{ $crate::errno_from_code!($err.raw()) }};
}

// There isn't really a mapping from zx_status::Status to Errno. The correct mapping is
// context-specific but this converter is a reasonable first-approximation. The translation matches
// fdio_status_to_errno. See https://fxbug.dev/42105838 for more context.
// TODO: Replace clients with more context-specific mappings.
#[macro_export]
macro_rules! from_status_like_fdio {
    ($status:expr) => {{ $crate::from_status_like_fdio!($status, "") }};
    ($status:expr, $context:expr) => {{
        match $status {
            $crate::__zx_status::Status::NOT_FOUND => $crate::errno!(ENOENT, $context),
            $crate::__zx_status::Status::NO_MEMORY => $crate::errno!(ENOMEM, $context),
            $crate::__zx_status::Status::INVALID_ARGS => $crate::errno!(EINVAL, $context),
            $crate::__zx_status::Status::BUFFER_TOO_SMALL => $crate::errno!(EINVAL, $context),
            $crate::__zx_status::Status::TIMED_OUT => $crate::errno!(ETIMEDOUT, $context),
            $crate::__zx_status::Status::UNAVAILABLE => $crate::errno!(EBUSY, $context),
            $crate::__zx_status::Status::ALREADY_EXISTS => $crate::errno!(EEXIST, $context),
            $crate::__zx_status::Status::PEER_CLOSED => $crate::errno!(EPIPE, $context),
            $crate::__zx_status::Status::BAD_STATE => $crate::errno!(EPIPE, $context),
            $crate::__zx_status::Status::BAD_PATH => $crate::errno!(ENAMETOOLONG, $context),
            $crate::__zx_status::Status::IO => $crate::errno!(EIO, $context),
            $crate::__zx_status::Status::NOT_FILE => $crate::errno!(EISDIR, $context),
            $crate::__zx_status::Status::NOT_DIR => $crate::errno!(ENOTDIR, $context),
            $crate::__zx_status::Status::NOT_SUPPORTED => $crate::errno!(EOPNOTSUPP, $context),
            $crate::__zx_status::Status::WRONG_TYPE => $crate::errno!(EOPNOTSUPP, $context),
            $crate::__zx_status::Status::OUT_OF_RANGE => $crate::errno!(EINVAL, $context),
            $crate::__zx_status::Status::NO_RESOURCES => $crate::errno!(ENOMEM, $context),
            $crate::__zx_status::Status::BAD_HANDLE => $crate::errno!(EBADF, $context),
            $crate::__zx_status::Status::ACCESS_DENIED => $crate::errno!(EACCES, $context),
            $crate::__zx_status::Status::SHOULD_WAIT => $crate::errno!(EAGAIN, $context),
            $crate::__zx_status::Status::FILE_BIG => $crate::errno!(EFBIG, $context),
            $crate::__zx_status::Status::NO_SPACE => $crate::errno!(ENOSPC, $context),
            $crate::__zx_status::Status::NOT_EMPTY => $crate::errno!(ENOTEMPTY, $context),
            $crate::__zx_status::Status::IO_REFUSED => $crate::errno!(ECONNREFUSED, $context),
            $crate::__zx_status::Status::IO_INVALID => $crate::errno!(EIO, $context),
            $crate::__zx_status::Status::CANCELED => $crate::errno!(EBADF, $context),
            $crate::__zx_status::Status::PROTOCOL_NOT_SUPPORTED => {
                $crate::errno!(EPROTONOSUPPORT, $context)
            }
            $crate::__zx_status::Status::ADDRESS_UNREACHABLE => {
                $crate::errno!(ENETUNREACH, $context)
            }
            $crate::__zx_status::Status::ADDRESS_IN_USE => $crate::errno!(EADDRINUSE, $context),
            $crate::__zx_status::Status::NOT_CONNECTED => $crate::errno!(ENOTCONN, $context),
            $crate::__zx_status::Status::CONNECTION_REFUSED => {
                $crate::errno!(ECONNREFUSED, $context)
            }
            $crate::__zx_status::Status::CONNECTION_RESET => $crate::errno!(ECONNRESET, $context),
            $crate::__zx_status::Status::CONNECTION_ABORTED => {
                $crate::errno!(ECONNABORTED, $context)
            }
            _ => $crate::errno!(EIO, $context),
        }
    }};
}

// Public re-export of macros allows them to be used like regular rust items.
pub use errno;
pub use errno_from_code;
pub use errno_from_zxio_code;
pub use error;
pub use from_status_like_fdio;

pub trait SourceContext<T, E> {
    /// Similar to `with_context` in [`anyhow::Context`], but adds the source location of the
    /// caller. This is especially useful when generating a message to attach to an error object as
    /// the context.
    ///
    /// Example:
    ///
    ///     let mount_point = system_task
    ///         .lookup_path_from_root(mount_point)
    ///         .with_source_context(|| {
    ///             format!("lookup path from root: {}", String::from_utf8_lossy(mount_point))
    ///         })?;
    ///
    /// Results in a context message like:
    ///
    ///     Caused by:
    ///         1: lookup path from root: ..., some_file.rs:12:34
    ///
    #[track_caller]
    fn with_source_context(self, context: impl FnOnce() -> String) -> anyhow::Result<T>;

    /// Similar to `context` in [`anyhow::Context`], but adds the source location of the caller.
    /// This is especially useful when generating a message to attach to an error object as the
    /// context.
    #[track_caller]
    fn source_context<C>(self, context: C) -> anyhow::Result<T>
    where
        C: Display + Send + Sync + 'static;
}

/// All result objects that support `with_context` will also support `with_source_context`.
impl<T, E> SourceContext<T, E> for Result<T, E>
where
    Result<T, E>: anyhow::Context<T, E>,
{
    fn with_source_context(self, context: impl FnOnce() -> String) -> anyhow::Result<T> {
        use anyhow::Context;
        let caller = std::panic::Location::caller();
        self.with_context(move || format!("{}, {}", context(), caller))
    }

    fn source_context<C>(self, context: C) -> anyhow::Result<T>
    where
        C: Display + Send + Sync + 'static,
    {
        use anyhow::Context;
        let caller = std::panic::Location::caller();
        self.context(format!("{}, {}", context, caller))
    }
}

const MAX_ERRNO: i32 = 4095;

pub fn is_error_return_value(value: i32) -> bool {
    value < 0 && value >= -MAX_ERRNO
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::Location;

    #[test]
    fn basic_errno_formatting() {
        let location = std::panic::Location::caller();
        let errno = Errno { code: ENOENT, location, context: None };
        assert_eq!(errno.to_string(), format!("ENOENT(2) from {}", location));
    }

    #[test]
    fn context_errno_formatting() {
        let location = std::panic::Location::caller();
        let errno = Errno { code: ENOENT, location, context: Some("TEST CONTEXT".to_string()) };
        assert_eq!(
            errno.to_string(),
            format!("ENOENT(2) from {}, context: TEST CONTEXT", location)
        );
    }

    #[test]
    fn with_source_context() {
        let errno = Errno { code: ENOENT, location: std::panic::Location::caller(), context: None };
        let result: Result<(), Errno> = Err(errno);
        let error = result.with_source_context(|| format!("42")).unwrap_err();
        let line_after_error = Location::caller();
        let expected_prefix =
            format!("42, {}:{}:", line_after_error.file(), line_after_error.line() - 1,);
        assert!(
            error.to_string().starts_with(&expected_prefix),
            "{} must start with {}",
            error,
            expected_prefix
        );
    }

    #[test]
    fn source_context() {
        let errno = Errno { code: ENOENT, location: std::panic::Location::caller(), context: None };
        let result: Result<(), Errno> = Err(errno);
        let error = result.source_context("42").unwrap_err();
        let line_after_error = Location::caller();
        let expected_prefix =
            format!("42, {}:{}:", line_after_error.file(), line_after_error.line() - 1,);
        assert!(
            error.to_string().starts_with(&expected_prefix),
            "{} must start with {}",
            error,
            expected_prefix
        );
    }
}
