// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_log_encoding::{Argument, Record, Value};
use {fidl_fuchsia_diagnostics_types as fdiagnostics, fidl_fuchsia_validate_logs as fvalidate};

pub fn record_to_fidl(record: Record<'_>) -> fvalidate::Record {
    fvalidate::Record {
        timestamp: record.timestamp,
        severity: fdiagnostics::Severity::from_primitive(record.severity).unwrap(),
        arguments: record.arguments.into_iter().map(argument_to_fidl).collect(),
    }
}

fn argument_to_fidl(argument: Argument<'_>) -> fvalidate::Argument {
    let name = argument.name().to_string();
    let value = match argument {
        Argument::Pid(pid) => fvalidate::Value::UnsignedInt(pid.raw_koid()),
        Argument::Tid(tid) => fvalidate::Value::UnsignedInt(tid.raw_koid()),
        Argument::Tag(tag) => fvalidate::Value::Text(tag.to_string()),
        Argument::Dropped(dropped) => fvalidate::Value::UnsignedInt(dropped),
        Argument::File(file) => fvalidate::Value::Text(file.to_string()),
        Argument::Line(line) => fvalidate::Value::UnsignedInt(line),
        Argument::Message(msg) => fvalidate::Value::Text(msg.to_string()),
        Argument::Other { value, .. } => value_to_fidl(value),
    };
    fvalidate::Argument { name, value }
}

pub fn value_to_fidl(value: Value<'_>) -> fvalidate::Value {
    match value {
        Value::Text(s) => fvalidate::Value::Text(s.to_string()),
        Value::SignedInt(n) => fvalidate::Value::SignedInt(n),
        Value::UnsignedInt(n) => fvalidate::Value::UnsignedInt(n),
        Value::Floating(n) => fvalidate::Value::Floating(n),
        Value::Boolean(n) => fvalidate::Value::Boolean(n),
    }
}

pub fn fidl_to_record(record: fvalidate::Record) -> Record<'static> {
    Record {
        timestamp: record.timestamp,
        severity: record.severity.into_primitive(),
        arguments: record.arguments.into_iter().map(fidl_to_argument).collect(),
    }
}

fn fidl_to_argument(argument: fvalidate::Argument) -> Argument<'static> {
    let fvalidate::Argument { name, value } = argument;
    match value {
        fvalidate::Value::Text(s) => Argument::new(name, s),
        fvalidate::Value::SignedInt(n) => Argument::new(name, n),
        fvalidate::Value::UnsignedInt(n) => Argument::new(name, n),
        fvalidate::Value::Floating(n) => Argument::new(name, n),
        fvalidate::Value::Boolean(n) => Argument::new(name, n),
        fvalidate::ValueUnknown!() => {
            unreachable!("got unknown value which we never set in our tests")
        }
    }
}
