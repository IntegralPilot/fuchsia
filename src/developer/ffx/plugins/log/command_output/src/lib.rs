// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! PLEASE READ: This is a crate for converting the [LogEntry] struct into a simpler
//! machine-readable output for the `ffx log` command.
//!
//! It is implemented via a wrapper around a [VerifiedMachineWriter]. This
//! [CommandOutputMachineWriter] takes a [LogEntry], when calling the
//! [CommandOutputMachineWriter::item] function, and converts it into a [CommandOutput] struct,
//! even though the machine writer is on the surface supporting a [LogEntry] format.
//!
//! This method of silent conversion is the simplest of three approaches. The other two being:
//!
//! 1. Implementing a [CommandOutput] and having it function with the logging formatter. This
//!    would require refactoring a rather large surface area of the logging framework, as most of
//!    it is built around using the concrete `LogEntry` struct. This would require several trait
//!    definitions and, as mentioned, lots of refactoring.
//! 2. Implementing [JsonSchema] for [LogEntry] directly. This requires piping implementations of
//!    the trait all the way down to `//third_party/rust_crates:flyweights` (a fast string
//!    representation), and due to the way [LogData] is represented as an alias of `Data<Logs>`,
//!    ends up inflating the schema definition unnecessarily (to the tune of a 1200 line long
//!    schema) as it requires covering a large set of definitions, 99% of which are not actually
//!    used when printing out the data the command invoker is actually going to use. This appears
//!    overly fragile, as it relies on every possible definition used by not just logging, but
//!    diagnostics in general.

use async_trait::async_trait;
use diagnostics_data::{DataSource, LogsHierarchy, LogsMetadata};
use ffx_writer::{Format, TestBuffers, ToolIO, VerifiedMachineWriter};
use fho::{FhoEnvironment, TryFromEnv};
use log_command_fdomain::LogEntry;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::io::Write;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct LogsData {
    pub data_source: DataSource,
    pub metadata: LogsMetadata,
    pub moniker: String,
    pub payload: Option<LogsHierarchy>,
    pub version: u64,
}

// This might seem a bit weird, but this is in the interest of wrapping the `log_command::LogData`
// structure, so the output is interchangeable. This allows for tests to use LogEntryBuilder to
// compare outputs, since they will serialize the same.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub enum LogData {
    TargetLog(LogsData),
}

/// The schema of `ffx --machine json log`.
///
/// This is the representation of a `diagnostics_data::Data<Logs>` struct.
///
/// To prevent having an over-inflated schema definition, output is not implementing for
/// `diagnostics_data::Data<T>` for all `T` as it would include many unused structures.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CommandOutput {
    data: LogData,
}

/// This isn't used. It is just necessary for the type to be passable to the logging formatter.
impl Display for CommandOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <Self as std::fmt::Debug>::fmt(self, f)
    }
}

impl From<&LogEntry> for CommandOutput {
    fn from(other: &LogEntry) -> Self {
        // This extracts the actual log data. For the time being this can ONLY be a
        // LogData::TargetLog, so this should never fail.
        //
        // If new enum values are added this will fail to compile, which will force changes.
        let log_command_fdomain::LogData::TargetLog(ref other) = other.data;
        let diagnostics_data::LogsData { data_source, metadata, moniker, payload, version } =
            other.clone();
        let data = LogData::TargetLog(LogsData {
            data_source,
            metadata,
            moniker: moniker.to_string(),
            payload,
            version,
        });
        Self { data }
    }
}

pub struct CommandOutputMachineWriter {
    inner: VerifiedMachineWriter<CommandOutput>,
}

impl CommandOutputMachineWriter {
    pub fn new_test(format: Option<Format>, test_buffers: &TestBuffers) -> Self {
        let inner = VerifiedMachineWriter::<CommandOutput>::new_test(format, test_buffers);
        Self { inner }
    }
}

impl Write for CommandOutputMachineWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[async_trait(?Send)]
impl TryFromEnv for CommandOutputMachineWriter {
    type Error = std::convert::Infallible;
    async fn try_from_env(env: &FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        Ok(Self { inner: VerifiedMachineWriter::try_from_env(env).await.unwrap() })
    }
}

impl ToolIO for CommandOutputMachineWriter {
    type OutputItem = LogEntry;

    fn supports_structured_output() -> bool {
        true
    }

    fn has_schema() -> bool {
        true
    }

    fn is_machine(&self) -> bool {
        self.inner.is_machine()
    }

    fn try_print_schema(&mut self) -> ffx_writer::Result<()> {
        self.inner.try_print_schema()
    }

    fn item(&mut self, value: &Self::OutputItem) -> ffx_writer::Result<()> {
        let v = value.into();
        self.inner.item(&v)
    }

    fn stderr(&mut self) -> &mut dyn Write {
        self.inner.stderr()
    }
}
