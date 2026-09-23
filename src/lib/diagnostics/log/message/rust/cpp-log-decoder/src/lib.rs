// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

use bumpalo::Bump;
use diagnostics_log_encoding;
use diagnostics_log_encoding::parse::ParseError;
use diagnostics_message::error::MessageError;
use diagnostics_message::ffi::{CPPMessageFormatter, CppArray, LogMessage};
use diagnostics_message::{self as message, MonikerWithUrl};
use std::ffi::CString;
use std::ops::{Deref, DerefMut};
use std::os::raw::c_char;
use std::ptr::NonNull;
use thiserror::Error;

/// # Safety
///
/// Same as for `std::slice::from_raw_parts`. Summarizing in terms of this API:
///
/// - `msg` must be valid for reads for `size`, and it must be properly aligned.
/// - `msg` must point to `size` consecutive u8 values.
/// - The `size` of the slice must be no larger than `isize::MAX`, and adding
///   that size to data must not "wrap around" the address space. See the safety
///   documentation of pointer::offset.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuchsia_decode_log_message_to_json(
    msg: *const u8,
    size: usize,
) -> *mut c_char {
    if msg.is_null() || size == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: caller guarantees msg is valid for reads of size bytes.
    let managed_ptr = unsafe { std::slice::from_raw_parts(msg, size) };
    let Ok(data) = message::from_structured(
        MonikerWithUrl { moniker: "test_moniker".try_into().unwrap(), url: "".into() },
        managed_ptr,
    ) else {
        return std::ptr::null_mut();
    };
    let Ok(item) = serde_json::to_string(&data) else {
        return std::ptr::null_mut();
    };
    let Ok(c_string) = CString::new(format!("[{}]", item)) else {
        return std::ptr::null_mut();
    };
    c_string.into_raw()
}

/// LogMessages struct containing log messages
/// It is created by calling fuchsia_decode_log_messages_to_struct,
/// and freed by calling fuchsia_free_log_messages.
/// Log messages contain embedded pointers to the bytes from
/// which they were created, so the memory referred to
/// by the LogMessages must not be modified or free'd until
/// the LogMessages are free'd.
#[repr(C)]
pub struct LogMessages<'a> {
    messages: CppArray<'a, &'a LogMessage<'a>>,
    error_str: *const c_char,
    allocator: AliasableBox<Bump>,
}

#[derive(Error, Debug)]
pub enum DecodeError {
    #[error(transparent)]
    Message(#[from] MessageError),
    #[error(transparent)]
    ParserError(#[from] ParseError),
}

pub type MessageParser = message::MessageParser;

#[unsafe(no_mangle)]
pub extern "C" fn fuchsia_new_message_parser() -> *mut MessageParser {
    Box::into_raw(Box::new(MessageParser::default()))
}

/// # Safety
///
/// This should only be called with a pointer obtained through
/// `fuchsia_new_message_parser`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuchsia_free_message_parser(parser: *mut MessageParser) {
    if !parser.is_null() {
        // SAFETY: parser must be a valid MessageParser constructed from
        // fuchsia_new_message_parser.
        unsafe { drop(Box::from_raw(parser)) };
    }
}

/// # Safety
///
/// - This function is NOT thread-safe. The caller must ensure that it is not called
///   concurrently with the same `parser` pointer.
///
/// Same as for `std::slice::from_raw_parts`. Summarizing in terms of this API:
///
/// - `msg` must be valid for reads for `size`, and it must be properly aligned.
/// - `msg` must point to `size` consecutive u8 values.
/// - The `size` of the slice must be no larger than `isize::MAX`, and adding
///   that size to data must not "wrap around" the address space. See the safety
///   documentation of pointer::offset.
/// If identity is provided, it must contain a valid moniker and URL.
///
/// The returned LogMessages must be free'd with fuchsia_free_log_messages(log_messages).  Free'ing
/// the LogMessages struct frees the bump allocator itself (and everything allocated from it).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuchsia_decode_log_messages_to_struct<'a>(
    msg: *const u8,
    size: usize,
    expect_extended_attribution: bool,
    parser: *mut MessageParser,
) -> LogMessages<'a> {
    let allocator = AliasableBox::new(Bump::new());

    // SAFETY: The C++ side is responsible for managing the lifetime.  We want to return
    // `LogMessages<'a>`, so we create a reference to the allocator here with a 'a lifetime.  We are
    // using `AliasableBox` which allows us to move `allocator` without invalidating any of the
    // data.
    let allocator_ref: &'a Bump = unsafe { allocator.get_ref() };

    // SAFETY: If `parser` is non-null, it must be valid and the caller guarantees exclusive access
    // to it.
    let maybe_parser = unsafe { parser.as_mut() };

    // SAFETY: The caller guarantees that `msg` is valid for reads for `size` bytes.
    // The returned `LogMessages<'a>` borrows from `msg` for lifetime `'a`.
    let buf: &'a [u8] = unsafe { std::slice::from_raw_parts(msg, size) };

    let messages = if let Some(parser) = maybe_parser {
        fuchsia_decode_log_messages_to_struct_internal(buf, parser, allocator_ref)
    } else {
        fuchsia_decode_log_messages_to_struct_internal_legacy(
            buf,
            expect_extended_attribution,
            allocator_ref,
        )
    };

    match messages {
        Ok(messages) => {
            let messages: &[_] =
                allocator_ref.alloc_slice_fill_iter(messages.into_iter().map(|m| &*m));
            LogMessages { messages: messages.into(), error_str: std::ptr::null(), allocator }
        }
        Err(err) => LogMessages {
            messages: CppArray::default(),
            error_str: allocator_ref
                .alloc_slice_copy(CString::new(err.to_string()).unwrap().as_bytes_with_nul())
                .as_ptr() as *const c_char,
            allocator,
        },
    }
}

/// Decodes log messages from a FXT stream.
fn fuchsia_decode_log_messages_to_struct_internal<'a>(
    buf: &'a [u8],
    parser: &mut MessageParser,
    allocator: &'a Bump,
) -> Result<Vec<&'a mut LogMessage<'a>>, DecodeError> {
    let mut messages = vec![];
    let mut current_slice = buf.as_ref();
    let formatter = CPPMessageFormatter(allocator);
    loop {
        let (data, remaining) = parser.parse_next(current_slice, &formatter)?;

        if let Some(data) = data {
            messages.push(data);
        }
        if remaining.is_empty() {
            break;
        }
        current_slice = remaining;
    }

    Ok(messages)
}

/// Decodes log messages from a legacy FXT stream.
fn fuchsia_decode_log_messages_to_struct_internal_legacy<'a>(
    buf: &'a [u8],
    expect_extended_attribution: bool,
    allocator: &'a Bump,
) -> Result<Vec<&'a mut LogMessage<'a>>, DecodeError> {
    let mut messages = vec![];
    let mut current_slice = buf.as_ref();
    loop {
        let (data, remaining) = if expect_extended_attribution {
            message::ffi::ffi_from_extended_record(current_slice, allocator)?
        } else {
            let (_, remaining_after_parse) =
                diagnostics_log_encoding::parse::parse_record(current_slice)?;
            let record_len = current_slice.len() - remaining_after_parse.len();
            let record_slice = &current_slice[..record_len];
            let (data, _) = message::ffi::ffi_from_extended_record(record_slice, allocator)?;
            (data, remaining_after_parse)
        };
        messages.push(data);
        if remaining.is_empty() {
            break;
        }
        current_slice = remaining;
    }

    Ok(messages)
}

/// # Safety
///
/// This should only be called with a pointer obtained through
/// `fuchsia_decode_log_message_to_json`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuchsia_free_decoded_log_message(msg: *mut c_char) {
    let str_to_free = unsafe { CString::from_raw(msg) };
    let _freer = str_to_free;
}

/// # Safety
///
/// This should only be called with `input` obtained through
/// `fuchsia_decode_log_messages_to_struct`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuchsia_free_log_messages(input: LogMessages<'_>) {
    drop(input);
}

/// Like `Box` except that it can be moved when there are live pointers.
#[repr(C)]
struct AliasableBox<T>(NonNull<T>);

impl<T> AliasableBox<T> {
    /// Returns a reference with an arbitrary lifetime.
    unsafe fn get_ref<'a>(&self) -> &'a T {
        // SAFETY: The caller must make this safe.
        unsafe { self.0.as_ref() }
    }
}

impl<T> AliasableBox<T> {
    fn new(value: T) -> Self {
        // SAFETY: `Box::into_raw` won't return null.
        Self(unsafe { NonNull::new_unchecked(Box::into_raw(Box::new(value))) })
    }
}

impl<T> Drop for AliasableBox<T> {
    fn drop(&mut self) {
        // SAFETY: We own the pointer.
        unsafe {
            let _ = Box::from_raw(self.0.as_ptr());
        }
    }
}

impl<T> Deref for AliasableBox<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        // SAFETY: We own the pointer.
        unsafe { self.0.as_ref() }
    }
}

impl<T> DerefMut for AliasableBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: We own the pointer.
        unsafe { self.0.as_mut() }
    }
}
