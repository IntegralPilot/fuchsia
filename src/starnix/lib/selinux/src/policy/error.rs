// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::new_policy::{ClassDefault, ClassDefaultRange, FsUseType};

use bstr::BString;
use thiserror::Error;

/// Structured errors that may be encountered parsing a binary policy.
#[derive(Clone, Debug, Error, PartialEq)]
pub enum ParseError {
    #[error("expected end of policy, but found {num_bytes} additional bytes")]
    TrailingBytes { num_bytes: usize },
    #[error("expected data item of type {type_name} ({type_size} bytes), but found {num_bytes}")]
    MissingData { type_name: &'static str, type_size: usize, num_bytes: usize },
    #[error(
        "policy is of {observed} bytes, but this implementation only supports policies of up to {limit} bytes"
    )]
    UnsupportedlyLarge { observed: usize, limit: usize },
    #[error("invalid ID value: {value}")]
    InvalidId { value: u32 },
}

/// Structured errors that may be encountered validating a binary policy.
#[derive(Debug, Error, PartialEq)]
pub enum ValidateError {
    #[error(
        "expected class default binary value to be one of {}, {}, or {}, but found {value}",
        ClassDefault::Unspecified as u32,
        ClassDefault::Source as u32,
        ClassDefault::Target as u32
    )]
    InvalidClassDefault { value: u32 },
    #[error(
        "expected class default binary value to be one of {:?}, but found {value}",
        [ClassDefaultRange::Unspecified as u32,
        ClassDefaultRange::SourceLow as u32,
        ClassDefaultRange::SourceHigh as u32,
        ClassDefaultRange::SourceLowHigh as u32,
        ClassDefaultRange::TargetLow as u32,
        ClassDefaultRange::TargetHigh as u32,
        ClassDefaultRange::TargetLowHigh as u32,
        ClassDefaultRange::UnknownUsedValue as u32]
    )]
    InvalidClassDefaultRange { value: u32 },
    #[error("paths not ordered lexicographicaly")]
    InvalidGenFsPathOrdering,
    #[error("missing initial SID {initial_sid:?}")]
    MissingInitialSid { initial_sid: crate::InitialSid },
    #[error(
        "invalid SELinux fs_use type; expected one of {:?}, but found {value}",
        [FsUseType::Xattr as u32,
        FsUseType::Trans as u32,
        FsUseType::Task as u32]
    )]
    InvalidFsUseType { value: u32 },
    #[error("non-optional Id field is zero")]
    NonOptionalIdIsZero,
    #[error("undefined {kind} Id value {id}")]
    UnknownId { kind: &'static str, id: String },
    #[error("invalid MLS range: {low}-{high}")]
    InvalidMlsRange { low: BString, high: BString },
}
