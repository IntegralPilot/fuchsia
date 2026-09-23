// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use bstr::ByteSlice as _;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

// See bootable/recovery/bootloader_message for the canonical format.
#[repr(C)]
#[derive(Copy, Clone, KnownLayout, FromBytes, IntoBytes, Immutable)]
pub struct BootloaderMessageRaw {
    command: [u8; 32],
    status: [u8; 32],
    recovery: [u8; 768],
    _stage: [u8; 32],
    _reserved: [u8; 1184],
}

impl Default for BootloaderMessageRaw {
    fn default() -> Self {
        Self {
            command: [0; _],
            status: [0; _],
            recovery: [0; _],
            _stage: [0; _],
            _reserved: [0; _],
        }
    }
}

const REASON_PREFIX: &str = "--reason=";

/// Processed bootloader message.
#[derive(Debug, Clone, Default)]
pub struct BootloaderMessage {
    command: String,
    status: String,
    recovery: String,
}

impl BootloaderMessage {
    /// Creates a new bootloader message with the given recovery arguments.
    pub fn with_args(args: &str) -> Self {
        Self { recovery: args.into(), ..Default::default() }
    }

    /// Returns an iterator over all arguments specified in the bootloader message's recovery field.
    /// Arguments are assumed to use a newline character (`\n`) as a delimiter.
    fn recovery_args(&self) -> impl Iterator<Item = &str> {
        self.recovery.split('\n').map(|arg| arg.trim()).filter(|arg| !arg.is_empty())
    }

    fn reason(&self) -> Option<&str> {
        self.recovery_args().find_map(|arg| arg.strip_prefix(REASON_PREFIX))
    }

    pub fn handle_recovery_actions(&self, handler: &mut impl RecoveryActionHandler) {
        let reason = self.reason();
        for arg in self.recovery_args().filter(|arg| !arg.starts_with(REASON_PREFIX)) {
            match arg {
                "recovery" => {}
                "--wipe_data" => handler.wipe_data(),
                "--sideload" => handler.sideload(/*auto_reboot=*/ false),
                "--sideload_auto_reboot" => handler.sideload(/*auto_reboot=*/ true),
                "--prompt_and_wipe_data" => handler.prompt_and_wipe_data(reason),
                _ => handler.other(arg, reason),
            }
        }
    }
}

impl From<BootloaderMessageRaw> for BootloaderMessage {
    fn from(raw: BootloaderMessageRaw) -> Self {
        Self {
            command: bytes_to_string(&raw.command),
            status: bytes_to_string(&raw.status),
            recovery: bytes_to_string(&raw.recovery),
        }
    }
}

impl TryFrom<BootloaderMessage> for BootloaderMessageRaw {
    type Error = Error;

    fn try_from(message: BootloaderMessage) -> Result<Self, Error> {
        let mut raw = BootloaderMessageRaw::default();

        let BootloaderMessage { command, status, recovery } = message;

        // Ensure fields will fit before copying them.
        if command.len() > raw.command.len() {
            return Err(anyhow!("command field exceeds storage size"));
        }
        if status.len() > raw.status.len() {
            return Err(anyhow!("status field exceeds storage size"));
        }
        if recovery.len() > raw.recovery.len() {
            return Err(anyhow!("recovery arguments exceed storage size"));
        }

        raw.command[0..command.len()].copy_from_slice(command.as_bytes());
        raw.status[0..status.len()].copy_from_slice(status.as_bytes());
        raw.recovery[0..recovery.len()].copy_from_slice(recovery.as_bytes());

        Ok(raw)
    }
}

/// Converts a byte buffer to a Rust [`String`], where `buf` is *possibly* a null-terminated UTF-8
/// string. Invalid UTF-8 characters will be emitted as "�" (U+FFFD). Bytes after the first null
/// character, if present, will be ignored, otherwise all of `buf` is used.
fn bytes_to_string(buf: &[u8]) -> String {
    if let Some((contents, _)) = buf.split_once_str(&[0u8]) {
        contents.as_bstr().to_string()
    } else {
        buf.as_bstr().to_string()
    }
}

/// Handler for actions which are specified in the bootloader recovery message.
pub trait RecoveryActionHandler {
    /// Invoked when the "wipe_data" recovery action is encountered.
    fn wipe_data(&mut self);

    /// Invoked when the "sideload" or "sideload_auto_reboot" recovery action is encountered.
    fn sideload(&mut self, auto_reboot: bool);

    /// Invoked when the "prompt_and_wipe_data" recovery action is encountered.
    fn prompt_and_wipe_data(&mut self, reason: Option<&str>);

    /// Invoked when an unknown recovery action is encountered.
    fn other(&mut self, arg: &str, reason: Option<&str>);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bytes_to_string_with_null() {
        let buf = b"hello\0world";
        assert_eq!(bytes_to_string(buf), "hello");
    }

    #[test]
    fn test_bytes_to_string_without_null() {
        let buf = b"hello";
        assert_eq!(bytes_to_string(buf), "hello");
    }

    #[test]
    fn test_bytes_to_string_invalid_utf8() {
        let buf = b"hello\xffworld";
        // invalid utf8 is replaced by replacement char
        assert_eq!(bytes_to_string(buf), "hello\u{FFFD}world");
    }

    #[test]
    fn test_with_args() {
        let msg = BootloaderMessage::with_args("test\nargs");
        assert_eq!(msg.recovery, "test\nargs");
        let args: Vec<_> = msg.recovery_args().collect();
        assert_eq!(args, ["test", "args"]);
    }

    #[test]
    fn test_raw_roundtrip() {
        let original = BootloaderMessage {
            command: "cmd".to_string(),
            status: "stat".to_string(),
            recovery: "rec".to_string(),
        };

        let raw: BootloaderMessageRaw = original.clone().try_into().expect("convert to raw");
        let converted: BootloaderMessage = raw.into();

        assert_eq!(converted.command, original.command);
        assert_eq!(converted.status, original.status);
        assert_eq!(converted.recovery, original.recovery);
    }

    #[test]
    fn test_raw_overflow() {
        let long_string = "a".repeat(1000);
        let msg = BootloaderMessage { command: long_string, ..Default::default() };
        let result: Result<BootloaderMessageRaw, _> = msg.try_into();
        assert!(result.is_err());
    }

    #[derive(Default)]
    struct MockHandler {
        wipe_data_called: bool,
        sideload_called: Vec<bool>,
        prompt_and_wipe_data_reason: Option<String>,
        other_calls: Vec<(String, Option<String>)>,
    }

    impl RecoveryActionHandler for MockHandler {
        fn wipe_data(&mut self) {
            self.wipe_data_called = true;
        }
        fn sideload(&mut self, auto_reboot: bool) {
            self.sideload_called.push(auto_reboot);
        }
        fn prompt_and_wipe_data(&mut self, reason: Option<&str>) {
            self.prompt_and_wipe_data_reason = reason.map(String::from);
        }
        fn other(&mut self, arg: &str, reason: Option<&str>) {
            self.other_calls.push((arg.to_string(), reason.map(String::from)));
        }
    }

    #[test]
    fn test_handle_recovery_actions() {
        let msg = BootloaderMessage::with_args(
            "recovery\n  \n--wipe_data\n--sideload\n--reason=test_reason\nunknown_arg",
        );
        let mut handler = MockHandler::default();
        msg.handle_recovery_actions(&mut handler);

        assert!(handler.wipe_data_called);
        assert_eq!(handler.sideload_called, [false]);
        assert_eq!(
            handler.other_calls,
            [("unknown_arg".to_string(), Some("test_reason".to_string()))]
        );
    }

    #[test]
    fn test_empty_recovery_string() {
        let msg = BootloaderMessage::with_args("");
        assert_eq!(msg.recovery_args().count(), 0);

        let mut handler = MockHandler::default();
        msg.handle_recovery_actions(&mut handler);

        assert!(!handler.wipe_data_called);
        assert!(handler.sideload_called.is_empty());
        assert!(handler.prompt_and_wipe_data_reason.is_none());
        assert!(handler.other_calls.is_empty());
    }
}
