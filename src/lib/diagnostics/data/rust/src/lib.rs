// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! # Diagnostics data
//!
//! This library contains the Diagnostics data schema used for inspect and logs . This is
//! the data that the Archive returns on `fuchsia.diagnostics.ArchiveAccessor` reads.

use chrono::{Local, TimeZone, Utc};
use diagnostics_hierarchy::HierarchyMatcher;
use fidl_fuchsia_diagnostics_common::{DataType, Selector};
use fidl_fuchsia_inspect_common as finspect;
use flyweights::FlyStr;
use itertools::Itertools;
use moniker::EXTENDED_MONIKER_COMPONENT_MANAGER_STR;
use selectors::SelectorExt;
use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use std::borrow::{Borrow, Cow};
use std::cmp::Ordering;
use std::fmt;
use std::hash::Hash;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::LazyLock;
use std::time::Duration;
use termion::{color, style};
use thiserror::Error;

pub use diagnostics_hierarchy::{DiagnosticsHierarchy, Property, hierarchy};
pub use diagnostics_log_types_serde::Severity;
pub use moniker::ExtendedMoniker;

#[cfg(target_os = "fuchsia")]
#[doc(hidden)]
pub mod logs_legacy;

#[cfg(feature = "json_schema")]
use schemars::JsonSchema;

const SCHEMA_VERSION: u64 = 1;
const MICROS_IN_SEC: u128 = 1000000;
const ROOT_MONIKER_REPR: &str = "<root>";

static DEFAULT_TREE_NAME: LazyLock<FlyStr> =
    LazyLock::new(|| FlyStr::new(finspect::DEFAULT_TREE_NAME));

/// The possible name for a handle to inspect data. It could be a filename (being deprecated) or a
/// name published using `fuchsia.inspect.InspectSink`.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Hash, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InspectHandleName {
    /// The name of an `InspectHandle`. This comes from the `name` argument
    /// in `InspectSink`.
    Name(FlyStr),

    /// The name of the file source when reading a file source of Inspect
    /// (eg an inspect VMO file or fuchsia.inspect.Tree in out/diagnostics)
    Filename(FlyStr),
}

impl std::fmt::Display for InspectHandleName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_ref())
    }
}

impl InspectHandleName {
    /// Construct an InspectHandleName::Name
    pub fn name(n: impl Into<FlyStr>) -> Self {
        Self::Name(n.into())
    }

    /// Construct an InspectHandleName::Filename
    pub fn filename(n: impl Into<FlyStr>) -> Self {
        Self::Filename(n.into())
    }

    /// If variant is Name, get the underlying value.
    pub fn as_name(&self) -> Option<&str> {
        if let Self::Name(n) = self { Some(n.as_str()) } else { None }
    }

    /// If variant is Filename, get the underlying value
    pub fn as_filename(&self) -> Option<&str> {
        if let Self::Filename(f) = self { Some(f.as_str()) } else { None }
    }
}

impl AsRef<str> for InspectHandleName {
    fn as_ref(&self) -> &str {
        match self {
            Self::Filename(f) => f.as_str(),
            Self::Name(n) => n.as_str(),
        }
    }
}

/// The source of diagnostics data
#[cfg_attr(feature = "json_schema", derive(JsonSchema))]
#[derive(Default, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum DataSource {
    #[default]
    Unknown,
    Inspect,
    Logs,
}

pub trait MetadataError {
    fn dropped_payload() -> Self;
    fn message(&self) -> Option<&str>;
}

pub trait Metadata: DeserializeOwned + Serialize + Clone + Send {
    /// The type of error returned in this metadata.
    type Error: Clone + MetadataError;

    /// Returns the timestamp at which this value was recorded.
    fn timestamp(&self) -> Timestamp;

    /// Overrides the timestamp at which this value was recorded.
    fn set_timestamp(&mut self, timestamp: Timestamp);

    /// Returns the errors recorded with this value, if any.
    fn errors(&self) -> Option<&[Self::Error]>;

    /// Overrides the errors associated with this value.
    fn set_errors(&mut self, errors: Vec<Self::Error>);

    /// Returns whether any errors are recorded on this value.
    fn has_errors(&self) -> bool {
        self.errors().map(|e| !e.is_empty()).unwrap_or_default()
    }

    /// Merge with another Metadata, taking latest timestamps and combining
    /// errors.
    fn merge(&mut self, other: Self) {
        if self.timestamp() < other.timestamp() {
            self.set_timestamp(other.timestamp());
        }

        if let Some(more) = other.errors() {
            let mut errs = Vec::from(self.errors().unwrap_or_default());
            errs.extend_from_slice(more);
            self.set_errors(errs);
        }
    }
}

/// A trait implemented by marker types which denote "kinds" of diagnostics data.
pub trait DiagnosticsData {
    /// The type of metadata included in results of this type.
    type Metadata: Metadata;

    /// The type of key used for indexing node hierarchies in the payload.
    type Key: AsRef<str> + Clone + DeserializeOwned + Eq + FromStr + Hash + Send + 'static;

    /// Used to query for this kind of metadata in the ArchiveAccessor.
    const DATA_TYPE: DataType;
}

/// Inspect carries snapshots of data trees hosted by components.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct Inspect;

impl DiagnosticsData for Inspect {
    type Metadata = InspectMetadata;
    type Key = String;
    const DATA_TYPE: DataType = DataType::Inspect;
}

impl Metadata for InspectMetadata {
    type Error = InspectError;

    fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    fn set_timestamp(&mut self, timestamp: Timestamp) {
        self.timestamp = timestamp;
    }

    fn errors(&self) -> Option<&[Self::Error]> {
        self.errors.as_deref()
    }

    fn set_errors(&mut self, errors: Vec<Self::Error>) {
        self.errors = Some(errors);
    }
}

/// Logs carry streams of structured events from components.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct Logs;

impl DiagnosticsData for Logs {
    type Metadata = LogsMetadata;
    type Key = LogsField;
    const DATA_TYPE: DataType = DataType::Logs;
}

impl Metadata for LogsMetadata {
    type Error = LogError;

    fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    fn set_timestamp(&mut self, timestamp: Timestamp) {
        self.timestamp = timestamp;
    }

    fn errors(&self) -> Option<&[Self::Error]> {
        self.errors.as_deref()
    }

    fn set_errors(&mut self, errors: Vec<Self::Error>) {
        self.errors = Some(errors);
    }
}

pub fn serialize_timestamp<S>(timestamp: &Timestamp, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_i64(timestamp.into_nanos())
}

pub fn deserialize_timestamp<'de, D>(deserializer: D) -> Result<Timestamp, D::Error>
where
    D: Deserializer<'de>,
{
    let nanos = i64::deserialize(deserializer)?;
    Ok(Timestamp::from_nanos(nanos))
}

#[cfg(target_os = "fuchsia")]
mod zircon {
    pub type Timestamp = zx::BootInstant;

    /// De-applies the mono-to-boot offset on this timestamp.
    ///
    /// This works only if called soon after `self` is produced, otherwise
    /// the timestamp will be placed further back in time.
    pub fn unapply_mono_to_boot_offset(timestamp: Timestamp) -> zx::MonotonicInstant {
        let mono_now = zx::MonotonicInstant::get();
        let boot_now = zx::BootInstant::get();

        let mono_to_boot_offset_nanos = boot_now.into_nanos() - mono_now.into_nanos();
        zx::MonotonicInstant::from_nanos(timestamp.into_nanos() - mono_to_boot_offset_nanos)
    }
}

#[cfg(target_os = "fuchsia")]
pub use zircon::Timestamp;
#[cfg(target_os = "fuchsia")]
pub use zircon::unapply_mono_to_boot_offset;

#[cfg(not(target_os = "fuchsia"))]
mod host {
    use serde::{Deserialize, Serialize};
    use std::fmt;
    use std::ops::Add;
    use std::time::Duration;

    #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
    pub struct Timestamp(i64);

    impl Timestamp {
        /// Returns the number of nanoseconds associated with this timestamp.
        pub fn into_nanos(self) -> i64 {
            self.0
        }

        /// Constructs a timestamp from the given nanoseconds.
        pub fn from_nanos(nanos: i64) -> Self {
            Self(nanos)
        }
    }

    impl Add<Duration> for Timestamp {
        type Output = Timestamp;
        fn add(self, rhs: Duration) -> Self::Output {
            Timestamp(self.0 + rhs.as_nanos() as i64)
        }
    }

    impl fmt::Display for Timestamp {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }
}

#[cfg(not(target_os = "fuchsia"))]
pub use host::Timestamp;

#[cfg(feature = "json_schema")]
impl JsonSchema for Timestamp {
    fn schema_name() -> String {
        "integer".to_owned()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        i64::json_schema(generator)
    }
}

/// The metadata contained in a `DiagnosticsData` object where the data source is
/// `DataSource::Inspect`.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct InspectMetadata {
    /// Optional vector of errors encountered by platform.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<InspectError>>,

    /// Name of diagnostics source producing data.
    #[serde(flatten)]
    pub name: InspectHandleName,

    /// The url with which the component was launched.
    pub component_url: FlyStr,

    /// Boot time in nanos.
    #[serde(serialize_with = "serialize_timestamp", deserialize_with = "deserialize_timestamp")]
    pub timestamp: Timestamp,

    /// When set to true, the data was escrowed. Otherwise, the data was fetched live from the
    /// source component at runtime. When absent, it means the value is false.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    #[serde(default)]
    pub escrowed: bool,
}

impl InspectMetadata {
    /// Returns the component URL with which the component that emitted the associated Inspect data
    /// was launched.
    pub fn component_url(&self) -> &str {
        self.component_url.as_str()
    }
}

/// The metadata contained in a `DiagnosticsData` object where the data source is
/// `DataSource::Logs`.
#[cfg_attr(feature = "json_schema", derive(JsonSchema))]
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct LogsMetadata {
    // TODO(https://fxbug.dev/42136318) figure out exact spelling of pid/tid context and severity
    /// Optional vector of errors encountered by platform.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<LogError>>,

    /// The url with which the component was launched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_url: Option<FlyStr>,

    /// Boot time in nanos.
    #[serde(serialize_with = "serialize_timestamp", deserialize_with = "deserialize_timestamp")]
    pub timestamp: Timestamp,

    /// Severity of the message.
    // For some reason using the `with` field was causing clippy errors, so this manually uses
    // `serialize_with` and `deserialize_with`
    #[serde(
        serialize_with = "diagnostics_log_types_serde::severity::serialize",
        deserialize_with = "diagnostics_log_types_serde::severity::deserialize"
    )]
    pub severity: Severity,

    /// Raw severity if any. This will typically be unset unless the log message carries a severity
    /// that differs from the standard values of each severity.
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_severity: Option<u8>,

    /// Tags to add at the beginning of the message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,

    /// The process ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u64>,

    /// The thread ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tid: Option<u64>,

    /// The file name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,

    /// The line number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,

    /// Number of dropped messages
    /// DEPRECATED: do not set. Left for backwards compatibility with older serialized metadatas
    /// that contain this field.
    #[serde(skip)]
    dropped: Option<u64>,

    /// Size of the original message on the wire, in bytes.
    /// DEPRECATED: do not set. Left for backwards compatibility with older serialized metadatas
    /// that contain this field.
    #[serde(skip)]
    size_bytes: Option<usize>,
}

impl LogsMetadata {
    /// Returns the component URL which generated this value.
    pub fn component_url(&self) -> Option<&str> {
        self.component_url.as_ref().map(|s| s.as_str())
    }

    /// Returns the raw severity of this log.
    pub fn raw_severity(&self) -> u8 {
        match self.raw_severity {
            Some(s) => s,
            None => self.severity as u8,
        }
    }
}

/// An instance of diagnostics data with typed metadata and an optional nested payload.
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct Data<D: DiagnosticsData> {
    /// The source of the data.
    #[serde(default)]
    // TODO(https://fxbug.dev/42135946) remove this once the Metadata enum is gone everywhere
    pub data_source: DataSource,

    /// The metadata for the diagnostics payload.
    #[serde(bound(
        deserialize = "D::Metadata: DeserializeOwned",
        serialize = "D::Metadata: Serialize"
    ))]
    pub metadata: D::Metadata,

    /// Moniker of the component that generated the payload.
    #[serde(deserialize_with = "moniker_deserialize", serialize_with = "moniker_serialize")]
    pub moniker: ExtendedMoniker,

    /// Payload containing diagnostics data, if the payload exists, else None.
    pub payload: Option<DiagnosticsHierarchy<D::Key>>,

    /// Schema version.
    #[serde(default)]
    pub version: u64,
}

struct MonikerWrapper<'a>(&'a ExtendedMoniker);

impl Serialize for MonikerWrapper<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self.0)
    }
}

struct RootHierarchyWrapper<'a, Key> {
    hierarchy: &'a DiagnosticsHierarchy<Key>,
    moniker: Option<&'a str>,
}

impl<Key> Serialize for RootHierarchyWrapper<'_, Key>
where
    Key: AsRef<str>,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        let mut s = serializer.serialize_map(Some(1))?;
        s.serialize_entry(
            self.hierarchy.name.as_str(),
            &diagnostics_hierarchy::serialization::SerializableHierarchyFields {
                hierarchy: self.hierarchy,
                moniker: self.moniker,
            },
        )?;
        s.end()
    }
}

impl<D: DiagnosticsData> Serialize for Data<D> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("Data", 5)?;
        s.serialize_field("data_source", &self.data_source)?;
        s.serialize_field("metadata", &self.metadata)?;
        s.serialize_field("moniker", &MonikerWrapper(&self.moniker))?;
        s.serialize_field("version", &self.version)?;

        let payload_wrapper = self
            .payload
            .as_ref()
            .map(|h| RootHierarchyWrapper { hierarchy: h, moniker: Some(self.moniker.as_ref()) });
        s.serialize_field("payload", &payload_wrapper)?;
        s.end()
    }
}

fn moniker_deserialize<'de, D>(deserializer: D) -> Result<ExtendedMoniker, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let moniker_str = String::deserialize(deserializer)?;
    ExtendedMoniker::parse_str(&moniker_str).map_err(serde::de::Error::custom)
}

impl<D> Data<D>
where
    D: DiagnosticsData,
{
    /// Returns a [`Data`] with an error indicating that the payload was dropped.
    pub fn drop_payload(&mut self) {
        self.metadata.set_errors(vec![
            <<D as DiagnosticsData>::Metadata as Metadata>::Error::dropped_payload(),
        ]);
        self.payload = None;
    }

    /// Sorts this [`Data`]'s payload if one is present.
    pub fn sort_payload(&mut self) {
        if let Some(payload) = &mut self.payload {
            payload.sort();
        }
    }

    /// Merge from another Data, combining data.
    pub fn merge(&mut self, other: Self) {
        let Data { data_source, metadata, moniker, payload, version } = other;

        if self.data_source != data_source || self.moniker != moniker || self.version != version {
            // other does not represent the same data.
            return;
        }

        self.metadata.merge(metadata);

        match (&mut self.payload, payload) {
            (Some(existing), Some(more)) => {
                existing.merge(more);
            }
            (None, Some(payload)) => {
                self.payload = Some(payload);
            }
            _ => {}
        }
    }

    /// Uses a set of Selectors to filter self's payload and returns the resulting
    /// Data. If the resulting payload is empty, it returns Ok(None).
    pub fn filter<'a>(
        mut self,
        selectors: impl IntoIterator<Item = &'a Selector>,
    ) -> Result<Option<Self>, Error> {
        let Some(hierarchy) = self.payload else {
            return Ok(None);
        };
        let matching_selectors =
            match self.moniker.match_against_selectors(selectors).collect::<Result<Vec<_>, _>>() {
                Ok(selectors) if selectors.is_empty() => return Ok(None),
                Ok(selectors) => selectors,
                Err(e) => {
                    return Err(Error::Internal(e));
                }
            };

        // TODO(https://fxbug.dev/300319116): Cache the `HierarchyMatcher`s
        let matcher: HierarchyMatcher =
            matching_selectors.try_into().map_err(|e| Error::Internal(anyhow::Error::from(e)))?;

        self.payload = match diagnostics_hierarchy::filter_hierarchy(hierarchy, &matcher) {
            Some(hierarchy) => Some(hierarchy),
            None => return Ok(None),
        };
        Ok(Some(self))
    }
}

/// Errors that can happen in this library.
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

/// A diagnostics data object containing inspect data.
pub type InspectData = Data<Inspect>;

/// A diagnostics data object containing logs data.
pub type LogsData = Data<Logs>;

/// A diagnostics data payload containing logs data.
pub type LogsHierarchy = DiagnosticsHierarchy<LogsField>;

/// A diagnostics hierarchy property keyed by `LogsField`.
pub type LogsProperty = Property<LogsField>;

impl Data<Inspect> {
    /// Access the name or filename within `self.metadata`.
    pub fn name(&self) -> &str {
        self.metadata.name.as_ref()
    }
}

pub struct InspectDataBuilder {
    data: Data<Inspect>,
}

impl InspectDataBuilder {
    pub fn new(
        moniker: ExtendedMoniker,
        component_url: impl Into<FlyStr>,
        timestamp: impl Into<Timestamp>,
    ) -> Self {
        Self {
            data: Data {
                data_source: DataSource::Inspect,
                moniker,
                payload: None,
                version: 1,
                metadata: InspectMetadata {
                    errors: None,
                    name: InspectHandleName::name(DEFAULT_TREE_NAME.clone()),
                    component_url: component_url.into(),
                    timestamp: timestamp.into(),
                    escrowed: false,
                },
            },
        }
    }

    pub fn escrowed(mut self, escrowed: bool) -> Self {
        self.data.metadata.escrowed = escrowed;
        self
    }

    pub fn with_hierarchy(
        mut self,
        hierarchy: DiagnosticsHierarchy<<Inspect as DiagnosticsData>::Key>,
    ) -> Self {
        self.data.payload = Some(hierarchy);
        self
    }

    pub fn with_errors(mut self, errors: Vec<InspectError>) -> Self {
        self.data.metadata.errors = Some(errors);
        self
    }

    pub fn with_name(mut self, name: InspectHandleName) -> Self {
        self.data.metadata.name = name;
        self
    }

    pub fn build(self) -> Data<Inspect> {
        self.data
    }
}

/// Internal state of the LogsDataBuilder impl
/// External customers should not directly access these fields.
pub struct LogsDataBuilder {
    /// List of errors
    errors: Vec<LogError>,
    /// Message in log
    msg: Option<String>,
    /// List of tags
    tags: Vec<String>,
    /// Process ID
    pid: Option<u64>,
    /// Thread ID
    tid: Option<u64>,
    /// File name
    file: Option<String>,
    /// Line number
    line: Option<u64>,
    /// BuilderArgs that was passed in at construction time
    args: BuilderArgs,
    /// List of KVPs from the user
    keys: Vec<Property<LogsField>>,
    /// Raw severity.
    raw_severity: Option<u8>,
}

/// Arguments used to create a new [`LogsDataBuilder`].
pub struct BuilderArgs {
    /// The moniker for the component
    pub moniker: ExtendedMoniker,
    /// The timestamp of the message in nanoseconds
    pub timestamp: Timestamp,
    /// The component URL
    pub component_url: Option<FlyStr>,
    /// The message severity
    pub severity: Severity,
}

impl LogsDataBuilder {
    /// Constructs a new LogsDataBuilder
    pub fn new(args: BuilderArgs) -> Self {
        LogsDataBuilder {
            args,
            errors: vec![],
            msg: None,
            file: None,
            line: None,
            pid: None,
            tags: vec![],
            tid: None,
            keys: vec![],
            raw_severity: None,
        }
    }

    /// Sets the moniker of the message.
    #[must_use = "You must call build on your builder to consume its result"]
    pub fn set_moniker(mut self, value: ExtendedMoniker) -> Self {
        self.args.moniker = value;
        self
    }

    /// Sets the URL of the message.
    #[must_use = "You must call build on your builder to consume its result"]
    pub fn set_url(mut self, value: Option<FlyStr>) -> Self {
        self.args.component_url = value;
        self
    }

    /// Sets the number of dropped messages.
    /// If value is greater than zero, a DroppedLogs error
    /// will also be added to the list of errors or updated if
    /// already present.
    #[must_use = "You must call build on your builder to consume its result"]
    pub fn set_dropped(mut self, value: u64) -> Self {
        if value == 0 {
            return self;
        }
        let val = self.errors.iter_mut().find_map(|error| {
            if let LogError::DroppedLogs { count } = error { Some(count) } else { None }
        });
        if let Some(v) = val {
            *v = value;
        } else {
            self.errors.push(LogError::DroppedLogs { count: value });
        }
        self
    }

    /// Overrides the severity set through the args with a raw severity.
    pub fn set_raw_severity(mut self, severity: u8) -> Self {
        self.raw_severity = Some(severity);
        self
    }

    /// Sets the number of rolled out messages.
    /// If value is greater than zero, a RolledOutLogs error
    /// will also be added to the list of errors or updated if
    /// already present.
    #[must_use = "You must call build on your builder to consume its result"]
    pub fn set_rolled_out(mut self, value: u64) -> Self {
        if value == 0 {
            return self;
        }
        let val = self.errors.iter_mut().find_map(|error| {
            if let LogError::RolledOutLogs { count } = error { Some(count) } else { None }
        });
        if let Some(v) = val {
            *v = value;
        } else {
            self.errors.push(LogError::RolledOutLogs { count: value });
        }
        self
    }

    /// Sets the severity of the log. This will unset the raw severity.
    pub fn set_severity(mut self, severity: Severity) -> Self {
        self.args.severity = severity;
        self.raw_severity = None;
        self
    }

    /// Sets the process ID that logged the message
    #[must_use = "You must call build on your builder to consume its result"]
    pub fn set_pid(mut self, value: u64) -> Self {
        self.pid = Some(value);
        self
    }

    /// Sets the thread ID that logged the message
    #[must_use = "You must call build on your builder to consume its result"]
    pub fn set_tid(mut self, value: u64) -> Self {
        self.tid = Some(value);
        self
    }

    /// Constructs a LogsData from this builder
    pub fn build(self) -> LogsData {
        let mut args = vec![];
        if let Some(msg) = self.msg {
            args.push(LogsProperty::String(LogsField::MsgStructured, msg));
        }
        let mut payload_fields = vec![DiagnosticsHierarchy::new("message", args, vec![])];
        if !self.keys.is_empty() {
            let val = DiagnosticsHierarchy::new("keys", self.keys, vec![]);
            payload_fields.push(val);
        }
        let mut payload = LogsHierarchy::new("root", vec![], payload_fields);
        payload.sort();
        let (raw_severity, severity) =
            self.raw_severity.map(Severity::parse_exact).unwrap_or((None, self.args.severity));
        let mut ret = LogsData::for_logs(
            self.args.moniker,
            Some(payload),
            self.args.timestamp,
            self.args.component_url,
            severity,
            self.errors,
        );
        ret.metadata.raw_severity = raw_severity;
        ret.metadata.file = self.file;
        ret.metadata.line = self.line;
        ret.metadata.pid = self.pid;
        ret.metadata.tid = self.tid;
        ret.metadata.tags = Some(self.tags);
        ret
    }

    /// Adds an error
    #[must_use = "You must call build on your builder to consume its result"]
    pub fn add_error(mut self, error: LogError) -> Self {
        self.errors.push(error);
        self
    }

    /// Sets the message to be printed in the log message
    #[must_use = "You must call build on your builder to consume its result"]
    pub fn set_message(mut self, msg: impl Into<String>) -> Self {
        self.msg = Some(msg.into());
        self
    }

    /// Sets the file name that printed this message.
    #[must_use = "You must call build on your builder to consume its result"]
    pub fn set_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// Sets the line number that printed this message.
    #[must_use = "You must call build on your builder to consume its result"]
    pub fn set_line(mut self, line: u64) -> Self {
        self.line = Some(line);
        self
    }

    /// Adds a property to the list of key value pairs that are a part of this log message.
    #[must_use = "You must call build on your builder to consume its result"]
    pub fn add_key(mut self, kvp: Property<LogsField>) -> Self {
        self.keys.push(kvp);
        self
    }

    /// Adds a tag to the list of tags that precede this log message.
    #[must_use = "You must call build on your builder to consume its result"]
    pub fn add_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }
}

impl Data<Logs> {
    /// Creates a new data instance for logs.
    pub fn for_logs(
        moniker: ExtendedMoniker,
        payload: Option<LogsHierarchy>,
        timestamp: impl Into<Timestamp>,
        component_url: Option<FlyStr>,
        severity: impl Into<Severity>,
        errors: Vec<LogError>,
    ) -> Self {
        let errors = if errors.is_empty() { None } else { Some(errors) };

        Data {
            moniker,
            version: SCHEMA_VERSION,
            data_source: DataSource::Logs,
            payload,
            metadata: LogsMetadata {
                timestamp: timestamp.into(),
                component_url,
                severity: severity.into(),
                raw_severity: None,
                errors,
                file: None,
                line: None,
                pid: None,
                tags: None,
                tid: None,
                dropped: None,
                size_bytes: None,
            },
        }
    }

    /// Sets the severity from a raw severity number. Overrides the severity to match the raw
    /// severity.
    pub fn set_raw_severity(&mut self, raw_severity: u8) {
        self.metadata.raw_severity = Some(raw_severity);
        self.metadata.severity = Severity::from(raw_severity);
    }

    /// Sets the severity of the log. This will unset the raw severity.
    pub fn set_severity(&mut self, severity: Severity) {
        self.metadata.severity = severity;
        self.metadata.raw_severity = None;
    }

    /// Returns the string log associated with the message, if one exists.
    pub fn msg(&self) -> Option<&str> {
        self.payload_message().as_ref().and_then(|p| {
            p.properties.iter().find_map(|property| match property {
                LogsProperty::String(LogsField::MsgStructured, msg) => Some(msg.as_str()),
                _ => None,
            })
        })
    }

    /// If the log has a message, returns a shared reference to the message contents.
    pub fn msg_mut(&mut self) -> Option<&mut String> {
        self.payload_message_mut().and_then(|p| {
            p.properties.iter_mut().find_map(|property| match property {
                LogsProperty::String(LogsField::MsgStructured, msg) => Some(msg),
                _ => None,
            })
        })
    }

    /// If the log has message, returns an exclusive reference to it.
    pub fn payload_message(&self) -> Option<&DiagnosticsHierarchy<LogsField>> {
        self.payload
            .as_ref()
            .and_then(|p| p.children.iter().find(|property| property.name.as_str() == "message"))
    }

    /// If the log has structured keys, returns an exclusive reference to them.
    pub fn payload_keys(&self) -> Option<&DiagnosticsHierarchy<LogsField>> {
        self.payload
            .as_ref()
            .and_then(|p| p.children.iter().find(|property| property.name.as_str() == "keys"))
    }

    pub fn metadata(&self) -> &LogsMetadata {
        &self.metadata
    }

    /// Returns an iterator over the payload keys as strings with the format "key=value".
    pub fn payload_keys_strings(&self) -> Box<dyn Iterator<Item = String> + Send + '_> {
        let maybe_iter = self.payload_keys().map(|p| {
            Box::new(p.properties.iter().filter_map(|property| match property {
                LogsProperty::String(LogsField::Tag, _tag) => None,
                LogsProperty::String(LogsField::ProcessId, _tag) => None,
                LogsProperty::String(LogsField::ThreadId, _tag) => None,
                LogsProperty::String(LogsField::Dropped, _tag) => None,
                LogsProperty::String(LogsField::Msg, _tag) => None,
                LogsProperty::String(LogsField::FilePath, _tag) => None,
                LogsProperty::String(LogsField::LineNumber, _tag) => None,
                LogsProperty::String(
                    key @ (LogsField::Other(_) | LogsField::MsgStructured),
                    value,
                ) => Some(format!("{key}={value}")),
                LogsProperty::Bytes(key @ (LogsField::Other(_) | LogsField::MsgStructured), _) => {
                    Some(format!("{key} = <bytes>"))
                }
                LogsProperty::Int(
                    key @ (LogsField::Other(_) | LogsField::MsgStructured),
                    value,
                ) => Some(format!("{key}={value}")),
                LogsProperty::Uint(
                    key @ (LogsField::Other(_) | LogsField::MsgStructured),
                    value,
                ) => Some(format!("{key}={value}")),
                LogsProperty::Double(
                    key @ (LogsField::Other(_) | LogsField::MsgStructured),
                    value,
                ) => Some(format!("{key}={value}")),
                LogsProperty::Bool(
                    key @ (LogsField::Other(_) | LogsField::MsgStructured),
                    value,
                ) => Some(format!("{key}={value}")),
                LogsProperty::DoubleArray(
                    key @ (LogsField::Other(_) | LogsField::MsgStructured),
                    value,
                ) => Some(format!("{key}={value:?}")),
                LogsProperty::IntArray(
                    key @ (LogsField::Other(_) | LogsField::MsgStructured),
                    value,
                ) => Some(format!("{key}={value:?}")),
                LogsProperty::UintArray(
                    key @ (LogsField::Other(_) | LogsField::MsgStructured),
                    value,
                ) => Some(format!("{key}={value:?}")),
                LogsProperty::StringList(
                    key @ (LogsField::Other(_) | LogsField::MsgStructured),
                    value,
                ) => Some(format!("{key}={value:?}")),
                _ => None,
            }))
        });
        match maybe_iter {
            Some(i) => Box::new(i),
            None => Box::new(std::iter::empty()),
        }
    }

    /// If the log has a message, returns a mutable reference to it.
    pub fn payload_message_mut(&mut self) -> Option<&mut DiagnosticsHierarchy<LogsField>> {
        self.payload.as_mut().and_then(|p| {
            p.children.iter_mut().find(|property| property.name.as_str() == "message")
        })
    }

    /// Returns the file path associated with the message, if one exists.
    pub fn file_path(&self) -> Option<&str> {
        self.metadata.file.as_deref()
    }

    /// Returns the line number associated with the message, if one exists.
    pub fn line_number(&self) -> Option<&u64> {
        self.metadata.line.as_ref()
    }

    /// Returns the pid associated with the message, if one exists.
    pub fn pid(&self) -> Option<u64> {
        self.metadata.pid
    }

    /// Returns the tid associated with the message, if one exists.
    pub fn tid(&self) -> Option<u64> {
        self.metadata.tid
    }

    /// Returns the tags associated with the message, if any exist.
    pub fn tags(&self) -> Option<&Vec<String>> {
        self.metadata.tags.as_ref()
    }

    /// Returns the severity level of this log.
    pub fn severity(&self) -> Severity {
        self.metadata.severity
    }

    /// Returns number of dropped logs if reported in the message.
    pub fn dropped_logs(&self) -> Option<u64> {
        self.metadata.errors.as_ref().and_then(|errors| {
            errors.iter().find_map(|e| match e {
                LogError::DroppedLogs { count } => Some(*count),
                _ => None,
            })
        })
    }

    /// Returns number of rolled out logs if reported in the message.
    pub fn rolled_out_logs(&self) -> Option<u64> {
        self.metadata.errors.as_ref().and_then(|errors| {
            errors.iter().find_map(|e| match e {
                LogError::RolledOutLogs { count } => Some(*count),
                _ => None,
            })
        })
    }

    /// Returns a component name derived from the component URL if available and non-empty.
    /// Otherwise, it falls back to the component's moniker. This name is intended for display
    /// purposes in logs, where showing the full URL or moniker might be impractical.
    pub fn component_name_by_url(&self) -> Cow<'_, str> {
        if let Some(url_str) = &self.metadata.component_url
            && !url_str.is_empty()
        {
            // Remove the .cm suffix if present
            let last_part = url_str.rsplit('/').next().unwrap_or(url_str);
            if let Some(stripped) = last_part.strip_suffix(".cm") {
                return Cow::Borrowed(stripped);
            }
            return Cow::Borrowed(last_part);
        }
        // No URL available, fallback to moniker
        self.component_name()
    }

    /// Returns the component name. This only makes sense for v1 components.
    pub fn component_name(&self) -> Cow<'_, str> {
        match &self.moniker {
            ExtendedMoniker::ComponentManager => {
                Cow::Borrowed(EXTENDED_MONIKER_COMPONENT_MANAGER_STR)
            }
            ExtendedMoniker::ComponentInstance(moniker) => {
                if moniker.is_root() {
                    Cow::Borrowed(ROOT_MONIKER_REPR)
                } else {
                    Cow::Owned(moniker.leaf().unwrap().to_string())
                }
            }
        }
    }
}

/// Display options for unstructured logs.
#[derive(Clone, Copy, Debug)]
pub struct LogTextDisplayOptions {
    /// Whether or not to display the moniker.
    pub show_moniker: bool,

    /// Whether or not to display the full moniker.
    pub show_full_moniker: bool,

    /// Whether or not to prefer the component URL over the moniker for the component name.
    pub prefer_url_component_name: bool,

    /// Whether or not to display metadata like PID & TID.
    pub show_metadata: bool,

    /// Whether or not to display tags provided by the log producer.
    pub show_tags: bool,

    /// Whether or not to display the source location which produced the log.
    pub show_file: bool,

    /// Whether to include ANSI color codes in the output.
    pub color: LogTextColor,

    /// How to print timestamps for this log message.
    pub time_format: LogTimeDisplayFormat,
}

impl Default for LogTextDisplayOptions {
    fn default() -> Self {
        Self {
            show_moniker: true,
            show_full_moniker: true,
            prefer_url_component_name: false,
            show_metadata: true,
            show_tags: true,
            show_file: true,
            color: Default::default(),
            time_format: Default::default(),
        }
    }
}

/// Configuration for the color of a log line that is displayed in tools using [`LogTextPresenter`].
#[derive(Clone, Copy, Debug, Default)]
pub enum LogTextColor {
    /// Do not print this log with ANSI colors.
    #[default]
    None,

    /// Display color codes according to log severity and presence of dropped or rolled out logs.
    BySeverity,

    /// Highlight this message as noteworthy regardless of severity, e.g. for known spam messages.
    Highlight,
}

impl LogTextColor {
    fn begin_record(&self, f: &mut fmt::Formatter<'_>, severity: Severity) -> fmt::Result {
        match self {
            LogTextColor::BySeverity => match severity {
                Severity::Fatal => {
                    write!(f, "{}{}", color::Bg(color::Red), color::Fg(color::White))?
                }
                Severity::Error => write!(f, "{}", color::Fg(color::Red))?,
                Severity::Warn => write!(f, "{}", color::Fg(color::Yellow))?,
                Severity::Info => (),
                Severity::Debug => write!(f, "{}", color::Fg(color::LightBlue))?,
                Severity::Trace => write!(f, "{}", color::Fg(color::LightMagenta))?,
            },
            LogTextColor::Highlight => write!(f, "{}", color::Fg(color::LightYellow))?,
            LogTextColor::None => {}
        }
        Ok(())
    }

    fn begin_lost_message_counts(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let LogTextColor::BySeverity = self {
            // This will be reset below before the next line.
            write!(f, "{}", color::Fg(color::Yellow))?;
        }
        Ok(())
    }

    fn end_record(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogTextColor::BySeverity | LogTextColor::Highlight => write!(f, "{}", style::Reset)?,
            LogTextColor::None => {}
        };
        Ok(())
    }
}

/// Options for the timezone associated to the timestamp of a log line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Timezone {
    /// Display a timestamp in terms of the local timezone as reported by the operating system.
    Local,

    /// Display a timestamp in terms of UTC.
    Utc,
}

impl Timezone {
    fn format(&self, seconds: i64, rem_nanos: u32) -> impl std::fmt::Display {
        const TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M:%S.%3f";
        match self {
            Timezone::Local => {
                Local.timestamp_opt(seconds, rem_nanos).unwrap().format(TIMESTAMP_FORMAT)
            }
            Timezone::Utc => {
                Utc.timestamp_opt(seconds, rem_nanos).unwrap().format(TIMESTAMP_FORMAT)
            }
        }
    }
}

/// Configuration for how to display the timestamp associated to a log line.
#[derive(Clone, Copy, Debug, Default)]
pub enum LogTimeDisplayFormat {
    /// Display the log message's timestamp as monotonic nanoseconds since boot.
    #[default]
    Original,

    /// Display the log's timestamp as a human-readable string in ISO 8601 format.
    WallTime {
        /// The format for displaying a timestamp as a string.
        tz: Timezone,

        /// The offset to apply to the original device-monotonic time before printing it as a
        /// human-readable timestamp.
        offset: i64,
    },
}

impl LogTimeDisplayFormat {
    fn write_timestamp(&self, f: &mut fmt::Formatter<'_>, time: Timestamp) -> fmt::Result {
        const NANOS_IN_SECOND: i64 = 1_000_000_000;

        match self {
            // Don't try to print a human readable string if it's going to be in 1970, fall back
            // to monotonic.
            Self::Original | Self::WallTime { offset: 0, .. } => {
                let time: Duration =
                    Duration::from_nanos(time.into_nanos().try_into().unwrap_or(0));
                write!(f, "[{:05}.{:06}]", time.as_secs(), time.as_micros() % MICROS_IN_SEC)?;
            }
            Self::WallTime { tz, offset } => {
                let adjusted = time.into_nanos() + offset;
                let seconds = adjusted / NANOS_IN_SECOND;
                let rem_nanos = (adjusted % NANOS_IN_SECOND) as u32;
                let formatted = tz.format(seconds, rem_nanos);
                write!(f, "[{formatted}]")?;
            }
        }
        Ok(())
    }
}

/// Used to control stringification options of Data<Logs>
pub struct LogTextPresenter<'a> {
    /// The log to parameterize
    log: &'a Data<Logs>,

    /// Options for stringifying the log
    options: LogTextDisplayOptions,
}

impl<'a> LogTextPresenter<'a> {
    /// Creates a new LogTextPresenter with the specified options and
    /// log message. This presenter is bound to the lifetime of the
    /// underlying log message.
    pub fn new(log: &'a Data<Logs>, options: LogTextDisplayOptions) -> Self {
        Self { log, options }
    }
}

impl fmt::Display for Data<Logs> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        LogTextPresenter::new(self, Default::default()).fmt(f)
    }
}

impl Deref for LogTextPresenter<'_> {
    type Target = Data<Logs>;
    fn deref(&self) -> &Self::Target {
        self.log
    }
}

impl fmt::Display for LogTextPresenter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.options.color.begin_record(f, self.log.severity())?;
        self.options.time_format.write_timestamp(f, self.metadata.timestamp)?;

        if self.options.show_metadata {
            match self.pid() {
                Some(pid) => write!(f, "[{pid}]")?,
                None => write!(f, "[]")?,
            }
            match self.tid() {
                Some(tid) => write!(f, "[{tid}]")?,
                None => write!(f, "[]")?,
            }
        }

        let moniker = if self.options.show_full_moniker {
            match &self.moniker {
                ExtendedMoniker::ComponentManager => {
                    Cow::Borrowed(EXTENDED_MONIKER_COMPONENT_MANAGER_STR)
                }
                ExtendedMoniker::ComponentInstance(instance) => {
                    if instance.is_root() {
                        Cow::Borrowed(ROOT_MONIKER_REPR)
                    } else {
                        Cow::Owned(instance.to_string())
                    }
                }
            }
        } else {
            if self.options.prefer_url_component_name {
                self.component_name_by_url()
            } else {
                self.component_name()
            }
        };
        if self.options.show_moniker {
            write!(f, "[{moniker}]")?;
        }

        if self.options.show_tags {
            match &self.metadata.tags {
                Some(tags) if !tags.is_empty() => {
                    let mut filtered =
                        tags.iter().filter(|tag| *tag != moniker.as_ref()).peekable();
                    if filtered.peek().is_some() {
                        write!(f, "[{}]", filtered.join(","))?;
                    }
                }
                _ => {}
            }
        }

        write!(f, " {}:", self.metadata.severity)?;

        if self.options.show_file {
            match (&self.metadata.file, &self.metadata.line) {
                (Some(file), Some(line)) => write!(f, " [{file}({line})]")?,
                (Some(file), None) => write!(f, " [{file}]")?,
                _ => (),
            }
        }

        if let Some(mut msg) = self.msg() {
            if let Some(nul) = msg.find("\0") {
                msg = &msg[0..nul];
            }
            write!(f, " {msg}")?;
        } else {
            write!(f, " <missing message>")?;
        }
        for kvp in self.payload_keys_strings() {
            write!(f, " {kvp}")?;
        }

        let dropped = self.log.dropped_logs().unwrap_or_default();
        let rolled = self.log.rolled_out_logs().unwrap_or_default();
        if dropped != 0 || rolled != 0 {
            self.options.color.begin_lost_message_counts(f)?;
            if dropped != 0 {
                write!(f, " [dropped={dropped}]")?;
            }
            if rolled != 0 {
                write!(f, " [rolled={rolled}]")?;
            }
        }

        self.options.color.end_record(f)?;

        Ok(())
    }
}

impl Eq for Data<Logs> {}

impl PartialOrd for Data<Logs> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Data<Logs> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.metadata.timestamp.cmp(&other.metadata.timestamp)
    }
}

/// An enum containing well known argument names passed through logs, as well
/// as an `Other` variant for any other argument names.
///
/// This contains the fields of logs sent as a [`LogMessage`].
///
/// [`LogMessage`]: https://fuchsia.dev/reference/fidl/fuchsia.logger#LogMessage
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, PartialOrd, Ord, Serialize)]
pub enum LogsField {
    ProcessId,
    ThreadId,
    Dropped,
    Tag,
    Msg,
    MsgStructured,
    FilePath,
    LineNumber,
    Other(String),
}

impl fmt::Display for LogsField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogsField::ProcessId => write!(f, "pid"),
            LogsField::ThreadId => write!(f, "tid"),
            LogsField::Dropped => write!(f, "num_dropped"),
            LogsField::Tag => write!(f, "tag"),
            LogsField::Msg => write!(f, "message"),
            LogsField::MsgStructured => write!(f, "value"),
            LogsField::FilePath => write!(f, "file_path"),
            LogsField::LineNumber => write!(f, "line_number"),
            LogsField::Other(name) => write!(f, "{name}"),
        }
    }
}

// TODO(https://fxbug.dev/42127608) - ensure that strings reported here align with naming
// decisions made for the structured log format sent by other components.
/// The label for the process koid in the log metadata.
pub const PID_LABEL: &str = "pid";
/// The label for the thread koid in the log metadata.
pub const TID_LABEL: &str = "tid";
/// The label for the number of dropped logs in the log metadata.
pub const DROPPED_LABEL: &str = "num_dropped";
/// The label for a tag in the log metadata.
pub const TAG_LABEL: &str = "tag";
/// The label for the contents of a message in the log payload.
pub const MESSAGE_LABEL_STRUCTURED: &str = "value";
/// The label for the message in the log payload.
pub const MESSAGE_LABEL: &str = "message";
/// The label for the file associated with a log line.
pub const FILE_PATH_LABEL: &str = "file";
/// The label for the line number in the file associated with a log line.
pub const LINE_NUMBER_LABEL: &str = "line";

impl AsRef<str> for LogsField {
    fn as_ref(&self) -> &str {
        match self {
            Self::ProcessId => PID_LABEL,
            Self::ThreadId => TID_LABEL,
            Self::Dropped => DROPPED_LABEL,
            Self::Tag => TAG_LABEL,
            Self::Msg => MESSAGE_LABEL,
            Self::FilePath => FILE_PATH_LABEL,
            Self::LineNumber => LINE_NUMBER_LABEL,
            Self::MsgStructured => MESSAGE_LABEL_STRUCTURED,
            Self::Other(str) => str.as_str(),
        }
    }
}

impl<T> From<T> for LogsField
where
    // Deref instead of AsRef b/c LogsField: AsRef<str> so this conflicts with concrete From<Self>
    T: Deref<Target = str>,
{
    fn from(s: T) -> Self {
        match s.as_ref() {
            PID_LABEL => Self::ProcessId,
            TID_LABEL => Self::ThreadId,
            DROPPED_LABEL => Self::Dropped,
            TAG_LABEL => Self::Tag,
            MESSAGE_LABEL => Self::Msg,
            FILE_PATH_LABEL => Self::FilePath,
            LINE_NUMBER_LABEL => Self::LineNumber,
            MESSAGE_LABEL_STRUCTURED => Self::MsgStructured,
            _ => Self::Other(s.to_string()),
        }
    }
}

impl FromStr for LogsField {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(s))
    }
}

/// Possible errors that can come in a `DiagnosticsData` object where the data source is
/// `DataSource::Logs`.
#[cfg_attr(feature = "json_schema", derive(JsonSchema))]
#[derive(Clone, Deserialize, Debug, Eq, PartialEq, Serialize)]
pub enum LogError {
    /// Represents the number of logs that were dropped by the component writing the logs due to an
    /// error writing to the socket before succeeding to write a log.
    #[serde(rename = "dropped_logs")]
    DroppedLogs { count: u64 },
    /// Represents the number of logs that were dropped for a component by the archivist due to the
    /// log buffer execeeding its maximum capacity before the current message.
    #[serde(rename = "rolled_out_logs")]
    RolledOutLogs { count: u64 },
    #[serde(rename = "parse_record")]
    FailedToParseRecord(String),
    #[serde(rename = "other")]
    Other { message: String },
}

const DROPPED_PAYLOAD_MSG: &str = "Schema failed to fit component budget.";

impl MetadataError for LogError {
    fn dropped_payload() -> Self {
        Self::Other { message: DROPPED_PAYLOAD_MSG.into() }
    }

    fn message(&self) -> Option<&str> {
        match self {
            Self::FailedToParseRecord(msg) => Some(msg.as_str()),
            Self::Other { message } => Some(message.as_str()),
            _ => None,
        }
    }
}

/// Possible error that can come in a `DiagnosticsData` object where the data source is
/// `DataSource::Inspect`..
#[derive(Debug, PartialEq, Clone, Eq)]
pub struct InspectError {
    pub message: String,
}

impl MetadataError for InspectError {
    fn dropped_payload() -> Self {
        Self { message: "Schema failed to fit component budget.".into() }
    }

    fn message(&self) -> Option<&str> {
        Some(self.message.as_str())
    }
}

impl fmt::Display for InspectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Borrow<str> for InspectError {
    fn borrow(&self) -> &str {
        &self.message
    }
}

impl Serialize for InspectError {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        self.message.serialize(ser)
    }
}

impl<'de> Deserialize<'de> for InspectError {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let message = String::deserialize(de)?;
        Ok(Self { message })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_hierarchy::hierarchy;
    use selectors::FastError;
    use serde_json::json;
    use test_case::test_case;

    const TEST_URL: &str = "fuchsia-pkg://test";

    #[fuchsia::test]
    fn test_canonical_json_inspect_formatting() {
        let mut hierarchy = hierarchy! {
            root: {
                x: "foo",
            }
        };

        hierarchy.sort();
        let json_schema = InspectDataBuilder::new(
            "a/b/c/d".try_into().unwrap(),
            TEST_URL,
            Timestamp::from_nanos(123456i64),
        )
        .with_hierarchy(hierarchy)
        .with_name(InspectHandleName::filename("test_file_plz_ignore.inspect"))
        .build();

        let result_json =
            serde_json::to_value(&json_schema).expect("serialization should succeed.");

        let expected_json = json!({
          "moniker": "a/b/c/d",
          "version": 1,
          "data_source": "Inspect",
          "payload": {
            "root": {
              "x": "foo"
            }
          },
          "metadata": {
            "component_url": TEST_URL,
            "filename": "test_file_plz_ignore.inspect",
            "timestamp": 123456,
          }
        });

        pretty_assertions::assert_eq!(result_json, expected_json, "golden diff failed.");
    }

    #[fuchsia::test]
    fn test_errorful_json_inspect_formatting() {
        let json_schema = InspectDataBuilder::new(
            "a/b/c/d".try_into().unwrap(),
            TEST_URL,
            Timestamp::from_nanos(123456i64),
        )
        .with_name(InspectHandleName::filename("test_file_plz_ignore.inspect"))
        .with_errors(vec![InspectError { message: "too much fun being had.".to_string() }])
        .build();

        let result_json =
            serde_json::to_value(&json_schema).expect("serialization should succeed.");

        let expected_json = json!({
          "moniker": "a/b/c/d",
          "version": 1,
          "data_source": "Inspect",
          "payload": null,
          "metadata": {
            "component_url": TEST_URL,
            "errors": ["too much fun being had."],
            "filename": "test_file_plz_ignore.inspect",
            "timestamp": 123456,
          }
        });

        pretty_assertions::assert_eq!(result_json, expected_json, "golden diff failed.");
    }

    fn parse_selectors(strings: Vec<&str>) -> Vec<Selector> {
        strings
            .iter()
            .map(|s| match selectors::parse_selector::<FastError>(s) {
                Ok(selector) => selector,
                Err(e) => panic!("Couldn't parse selector {s}: {e}"),
            })
            .collect::<Vec<_>>()
    }

    #[fuchsia::test]
    fn test_filter_returns_none_on_empty_hierarchy() {
        let data = InspectDataBuilder::new(
            "a/b/c/d".try_into().unwrap(),
            TEST_URL,
            Timestamp::from_nanos(123456i64),
        )
        .build();
        let selectors = parse_selectors(vec!["a/b/c/d:foo"]);
        assert_eq!(data.filter(&selectors).expect("Filter OK"), None);
    }

    #[fuchsia::test]
    fn test_filter_returns_none_on_selector_mismatch() {
        let mut hierarchy = hierarchy! {
            root: {
                x: "foo",
            }
        };
        hierarchy.sort();
        let data = InspectDataBuilder::new(
            "b/c/d/e".try_into().unwrap(),
            TEST_URL,
            Timestamp::from_nanos(123456i64),
        )
        .with_hierarchy(hierarchy)
        .build();
        let selectors = parse_selectors(vec!["a/b/c/d:foo"]);
        assert_eq!(data.filter(&selectors).expect("Filter OK"), None);
    }

    #[fuchsia::test]
    fn test_filter_returns_none_on_data_mismatch() {
        let mut hierarchy = hierarchy! {
            root: {
                x: "foo",
            }
        };
        hierarchy.sort();
        let data = InspectDataBuilder::new(
            "a/b/c/d".try_into().unwrap(),
            TEST_URL,
            Timestamp::from_nanos(123456i64),
        )
        .with_hierarchy(hierarchy)
        .build();
        let selectors = parse_selectors(vec!["a/b/c/d:foo"]);

        assert_eq!(data.filter(&selectors).expect("FIlter OK"), None);
    }

    #[fuchsia::test]
    fn test_filter_returns_matching_data() {
        let mut hierarchy = hierarchy! {
            root: {
                x: "foo",
                y: "bar",
            }
        };
        hierarchy.sort();
        let data = InspectDataBuilder::new(
            "a/b/c/d".try_into().unwrap(),
            TEST_URL,
            Timestamp::from_nanos(123456i64),
        )
        .with_name(InspectHandleName::filename("test_file_plz_ignore.inspect"))
        .with_hierarchy(hierarchy)
        .build();
        let selectors = parse_selectors(vec!["a/b/c/d:root:x"]);

        let expected_json = json!({
          "moniker": "a/b/c/d",
          "version": 1,
          "data_source": "Inspect",
          "payload": {
            "root": {
              "x": "foo"
            }
          },
          "metadata": {
            "component_url": TEST_URL,
            "filename": "test_file_plz_ignore.inspect",
            "timestamp": 123456,
          }
        });

        let result_json = serde_json::to_value(data.filter(&selectors).expect("Filter Ok"))
            .expect("serialization should succeed.");

        pretty_assertions::assert_eq!(result_json, expected_json, "golden diff failed.");
    }

    #[fuchsia::test]
    fn default_builder_test() {
        let builder = LogsDataBuilder::new(BuilderArgs {
            component_url: Some("url".into()),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
            timestamp: Timestamp::from_nanos(0),
        });
        //let tree = builder.build();
        let expected_json = json!({
          "moniker": "moniker",
          "version": 1,
          "data_source": "Logs",
          "payload": {
              "root":
              {
                  "message":{}
              }
          },
          "metadata": {
            "component_url": "url",
              "severity": "INFO",
              "tags": [],

            "timestamp": 0,
          }
        });
        let result_json =
            serde_json::to_value(builder.build()).expect("serialization should succeed.");
        pretty_assertions::assert_eq!(result_json, expected_json, "golden diff failed.");
    }

    #[fuchsia::test]
    fn regular_message_test() {
        let builder = LogsDataBuilder::new(BuilderArgs {
            component_url: Some("url".into()),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
            timestamp: Timestamp::from_nanos(0),
        })
        .set_message("app")
        .set_file("test file.cc")
        .set_line(420)
        .set_pid(1001)
        .set_tid(200)
        .set_dropped(2)
        .add_tag("You're")
        .add_tag("IT!")
        .add_key(LogsProperty::String(LogsField::Other("key".to_string()), "value".to_string()));
        // TODO(https://fxbug.dev/42157027): Convert to our custom DSL when possible.
        let expected_json = json!({
          "moniker": "moniker",
          "version": 1,
          "data_source": "Logs",
          "payload": {
              "root":
              {
                  "keys":{
                      "key":"value"
                  },
                  "message":{
                      "value":"app"
                  }
              }
          },
          "metadata": {
            "errors": [],
            "component_url": "url",
              "errors": [{"dropped_logs":{"count":2}}],
              "file": "test file.cc",
              "line": 420,
              "pid": 1001,
              "severity": "INFO",
              "tags": ["You're", "IT!"],
              "tid": 200,

            "timestamp": 0,
          }
        });
        let result_json =
            serde_json::to_value(builder.build()).expect("serialization should succeed.");
        pretty_assertions::assert_eq!(result_json, expected_json, "golden diff failed.");
    }

    #[fuchsia::test]
    fn display_for_logs() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("foo")
        .add_tag("bar")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            "[00012.345678][123][456][moniker][foo,bar] INFO: [some_file.cc(420)] some message test=property value=test",
            format!("{data}")
        )
    }

    #[fuchsia::test]
    fn display_for_logs_with_duplicate_moniker() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("moniker")
        .add_tag("bar")
        .add_tag("moniker")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            "[00012.345678][123][456][moniker][bar] INFO: [some_file.cc(420)] some message test=property value=test",
            format!("{data}")
        )
    }

    #[fuchsia::test]
    fn display_for_logs_with_duplicate_moniker_and_no_other_tags() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("moniker")
        .add_tag("moniker")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            "[00012.345678][123][456][moniker] INFO: [some_file.cc(420)] some message test=property value=test",
            format!("{data}")
        )
    }

    #[fuchsia::test]
    fn test_component_name_by_url() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(0),
            component_url: Some(FlyStr::from(
                "fuchsia-pkg://fuchsia.com/my-pkg#meta/my-component.cm",
            )),
            moniker: ExtendedMoniker::parse_str("test/moniker").unwrap(),
            severity: Severity::Info,
        })
        .build();
        assert_eq!(data.component_name_by_url(), "my-component");

        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(0),
            component_url: Some(FlyStr::from("fuchsia-pkg://fuchsia.com/my-pkg#meta/my-component")),
            moniker: ExtendedMoniker::parse_str("test/moniker").unwrap(),
            severity: Severity::Info,
        })
        .build();
        assert_eq!(data.component_name_by_url(), "my-component");

        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(0),
            component_url: Some(FlyStr::from("")),
            moniker: ExtendedMoniker::parse_str("test/moniker").unwrap(),
            severity: Severity::Info,
        })
        .build();
        assert_eq!(data.component_name_by_url(), "moniker");

        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(0),
            component_url: None,
            moniker: ExtendedMoniker::parse_str("test/moniker").unwrap(),
            severity: Severity::Info,
        })
        .build();
        assert_eq!(data.component_name_by_url(), "moniker");
    }

    #[fuchsia::test]
    fn display_for_logs_partial_moniker() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("test/moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("foo")
        .add_tag("bar")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            "[00012.345678][123][456][fake-url][foo,bar] INFO: [some_file.cc(420)] some message test=property value=test",
            format!(
                "{}",
                LogTextPresenter::new(
                    &data,
                    LogTextDisplayOptions {
                        show_full_moniker: false,
                        prefer_url_component_name: true,
                        ..Default::default()
                    }
                )
            )
        )
    }

    #[fuchsia::test]
    fn display_for_logs_exclude_metadata() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("foo")
        .add_tag("bar")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            "[00012.345678][moniker][foo,bar] INFO: [some_file.cc(420)] some message test=property value=test",
            format!(
                "{}",
                LogTextPresenter::new(
                    &data,
                    LogTextDisplayOptions { show_metadata: false, ..Default::default() }
                )
            )
        )
    }

    #[fuchsia::test]
    fn display_for_logs_exclude_tags() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("foo")
        .add_tag("bar")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            "[00012.345678][123][456][moniker] INFO: [some_file.cc(420)] some message test=property value=test",
            format!(
                "{}",
                LogTextPresenter::new(
                    &data,
                    LogTextDisplayOptions { show_tags: false, ..Default::default() }
                )
            )
        )
    }

    #[fuchsia::test]
    fn display_for_logs_exclude_file() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("foo")
        .add_tag("bar")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            "[00012.345678][123][456][moniker][foo,bar] INFO: some message test=property value=test",
            format!(
                "{}",
                LogTextPresenter::new(
                    &data,
                    LogTextDisplayOptions { show_file: false, ..Default::default() }
                )
            )
        )
    }

    #[fuchsia::test]
    fn display_for_logs_include_color_by_severity() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Error,
        })
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("foo")
        .add_tag("bar")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            format!(
                "{}[00012.345678][123][456][moniker][foo,bar] ERROR: [some_file.cc(420)] some message test=property value=test{}",
                color::Fg(color::Red),
                style::Reset
            ),
            format!(
                "{}",
                LogTextPresenter::new(
                    &data,
                    LogTextDisplayOptions { color: LogTextColor::BySeverity, ..Default::default() }
                )
            )
        )
    }

    #[fuchsia::test]
    fn display_for_logs_highlight_line() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("foo")
        .add_tag("bar")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            format!(
                "{}[00012.345678][123][456][moniker][foo,bar] INFO: [some_file.cc(420)] some message test=property value=test{}",
                color::Fg(color::LightYellow),
                style::Reset
            ),
            LogTextPresenter::new(
                &data,
                LogTextDisplayOptions { color: LogTextColor::Highlight, ..Default::default() }
            )
            .to_string()
        )
    }

    #[fuchsia::test]
    fn display_for_logs_with_wall_time() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("foo")
        .add_tag("bar")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            "[1970-01-01 00:00:12.345][123][456][moniker][foo,bar] INFO: [some_file.cc(420)] some message test=property value=test",
            LogTextPresenter::new(
                &data,
                LogTextDisplayOptions {
                    time_format: LogTimeDisplayFormat::WallTime { tz: Timezone::Utc, offset: 1 },
                    ..Default::default()
                }
            )
            .to_string()
        );

        assert_eq!(
            "[00012.345678][123][456][moniker][foo,bar] INFO: [some_file.cc(420)] some message test=property value=test",
            LogTextPresenter::new(
                &data,
                LogTextDisplayOptions {
                    time_format: LogTimeDisplayFormat::WallTime { tz: Timezone::Utc, offset: 0 },
                    ..Default::default()
                }
            )
            .to_string(),
            "should fall back to monotonic if offset is 0"
        );
    }

    #[fuchsia::test]
    fn display_for_logs_with_dropped_count() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_dropped(5)
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("foo")
        .add_tag("bar")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            "[00012.345678][123][456][moniker][foo,bar] INFO: [some_file.cc(420)] some message test=property value=test [dropped=5]",
            format!("{}", LogTextPresenter::new(&data, LogTextDisplayOptions::default())),
        );

        assert_eq!(
            format!(
                "[00012.345678][123][456][moniker][foo,bar] INFO: [some_file.cc(420)] some message test=property value=test{} [dropped=5]{}",
                color::Fg(color::Yellow),
                style::Reset
            ),
            LogTextPresenter::new(
                &data,
                LogTextDisplayOptions { color: LogTextColor::BySeverity, ..Default::default() }
            )
            .to_string()
        );
    }

    #[fuchsia::test]
    fn display_for_logs_with_rolled_count() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_rolled_out(10)
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("foo")
        .add_tag("bar")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            "[00012.345678][123][456][moniker][foo,bar] INFO: [some_file.cc(420)] some message test=property value=test [rolled=10]",
            format!("{}", LogTextPresenter::new(&data, LogTextDisplayOptions::default())),
        );

        assert_eq!(
            format!(
                "[00012.345678][123][456][moniker][foo,bar] INFO: [some_file.cc(420)] some message test=property value=test{} [rolled=10]{}",
                color::Fg(color::Yellow),
                style::Reset
            ),
            LogTextPresenter::new(
                &data,
                LogTextDisplayOptions { color: LogTextColor::BySeverity, ..Default::default() }
            )
            .to_string()
        );
    }

    #[fuchsia::test]
    fn display_for_logs_with_dropped_and_rolled_counts() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_dropped(5)
        .set_rolled_out(10)
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("foo")
        .add_tag("bar")
        .add_key(LogsProperty::String(LogsField::Other("test".to_string()), "property".to_string()))
        .add_key(LogsProperty::String(LogsField::MsgStructured, "test".to_string()))
        .build();

        assert_eq!(
            "[00012.345678][123][456][moniker][foo,bar] INFO: [some_file.cc(420)] some message test=property value=test [dropped=5] [rolled=10]",
            format!("{}", LogTextPresenter::new(&data, LogTextDisplayOptions::default())),
        );

        assert_eq!(
            format!(
                "[00012.345678][123][456][moniker][foo,bar] INFO: [some_file.cc(420)] some message test=property value=test{} [dropped=5] [rolled=10]{}",
                color::Fg(color::Yellow),
                style::Reset
            ),
            LogTextPresenter::new(
                &data,
                LogTextDisplayOptions { color: LogTextColor::BySeverity, ..Default::default() }
            )
            .to_string()
        );
    }

    #[fuchsia::test]
    fn display_for_logs_no_tags() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_pid(123)
        .set_tid(456)
        .set_message("some message".to_string())
        .build();

        assert_eq!("[00012.345678][123][456][moniker] INFO: some message", format!("{data}"))
    }

    #[fuchsia::test]
    fn size_bytes_deserialize_backwards_compatibility() {
        let original_json = json!({
          "moniker": "a/b",
          "version": 1,
          "data_source": "Logs",
          "payload": {
            "root": {
              "message":{}
            }
          },
          "metadata": {
            "component_url": "url",
              "severity": "INFO",
              "tags": [],

            "timestamp": 123,
          }
        });
        let expected_data = LogsDataBuilder::new(BuilderArgs {
            component_url: Some("url".into()),
            moniker: ExtendedMoniker::parse_str("a/b").unwrap(),
            severity: Severity::Info,
            timestamp: Timestamp::from_nanos(123),
        })
        .build();
        let original_data: LogsData = serde_json::from_value(original_json).unwrap();
        assert_eq!(original_data, expected_data);
        // We skip deserializing the size_bytes
        assert_eq!(original_data.metadata.size_bytes, None);
    }

    #[fuchsia::test]
    fn display_for_logs_with_null_terminator() {
        let data = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(12345678000i64),
            component_url: Some(FlyStr::from("fake-url")),
            moniker: ExtendedMoniker::parse_str("moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_pid(123)
        .set_tid(456)
        .set_message("some message\0garbage".to_string())
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .add_tag("foo")
        .add_tag("bar")
        .build();

        assert_eq!(
            "[00012.345678][123][456][moniker][foo,bar] INFO: [some_file.cc(420)] some message",
            format!("{data}")
        )
    }

    #[fuchsia::test]
    fn dropped_deserialize_backwards_compatibility() {
        let original_json = json!({
          "moniker": "a/b",
          "version": 1,
          "data_source": "Logs",
          "payload": {
            "root": {
              "message":{}
            }
          },
          "metadata": {
            "dropped": 0,
            "component_url": "url",
              "severity": "INFO",
              "tags": [],

            "timestamp": 123,
          }
        });
        let expected_data = LogsDataBuilder::new(BuilderArgs {
            component_url: Some("url".into()),
            moniker: ExtendedMoniker::parse_str("a/b").unwrap(),
            severity: Severity::Info,
            timestamp: Timestamp::from_nanos(123),
        })
        .build();
        let original_data: LogsData = serde_json::from_value(original_json).unwrap();
        assert_eq!(original_data, expected_data);
        // We skip deserializing dropped
        assert_eq!(original_data.metadata.dropped, None);
    }

    #[fuchsia::test]
    fn severity_aliases() {
        assert_eq!(Severity::from_str("warn").unwrap(), Severity::Warn);
        assert_eq!(Severity::from_str("warning").unwrap(), Severity::Warn);
    }

    #[fuchsia::test]
    fn test_metadata_merge() {
        let mut meta = InspectMetadata {
            errors: Some(vec![InspectError { message: "error1".to_string() }]),
            name: InspectHandleName::name("test"),
            component_url: "fuchsia-pkg://test".into(),
            timestamp: Timestamp::from_nanos(100),
            escrowed: false,
        };

        meta.merge(InspectMetadata {
            errors: Some(vec![InspectError { message: "error2".to_string() }]),
            name: InspectHandleName::name("test"),
            component_url: "fuchsia-pkg://test".into(),
            timestamp: Timestamp::from_nanos(200),
            escrowed: false,
        });

        assert_eq!(
            meta,
            InspectMetadata {
                errors: Some(vec![
                    InspectError { message: "error1".to_string() },
                    InspectError { message: "error2".to_string() },
                ]),
                name: InspectHandleName::name("test"),
                component_url: "fuchsia-pkg://test".into(),
                timestamp: Timestamp::from_nanos(200),
                escrowed: false,
            }
        );
    }

    #[fuchsia::test]
    fn test_metadata_merge_older_timestamp_noop() {
        let mut meta = InspectMetadata {
            errors: None,
            name: InspectHandleName::name("test"),
            component_url: TEST_URL.into(),
            timestamp: Timestamp::from_nanos(200),
            escrowed: false,
        };
        meta.merge(InspectMetadata {
            errors: None,
            name: InspectHandleName::name("test"),
            component_url: TEST_URL.into(),
            timestamp: Timestamp::from_nanos(100),
            escrowed: false,
        });
        assert_eq!(
            meta,
            InspectMetadata {
                errors: None,
                name: InspectHandleName::name("test"),
                component_url: TEST_URL.into(),
                timestamp: Timestamp::from_nanos(200),
                escrowed: false,
            }
        );
    }

    fn new_test_data(moniker: &str, payload_val: Option<&str>, timestamp: i64) -> InspectData {
        let mut builder = InspectDataBuilder::new(
            moniker.try_into().unwrap(),
            TEST_URL,
            Timestamp::from_nanos(timestamp),
        );
        if let Some(val) = payload_val {
            builder = builder.with_hierarchy(hierarchy! { root: { "key": val } });
        }
        builder.build()
    }

    #[fuchsia::test]
    fn test_data_merge() {
        let mut data = new_test_data("a/b/c", Some("val1"), 100);
        let mut other = new_test_data("a/b/c", Some("val2"), 200);
        other.metadata.errors = Some(vec![InspectError { message: "error".into() }]);

        data.merge(other);

        let expected_payload = hierarchy! { root: { "key": "val2" } };
        assert_eq!(data.payload, Some(expected_payload));
        assert_eq!(data.metadata.timestamp, Timestamp::from_nanos(200));
        assert_eq!(data.metadata.errors, Some(vec![InspectError { message: "error".into() }]));
    }

    #[test_case(new_test_data("a/b/d", Some("v2"), 100); "different moniker")]
    #[test_case(
        {
            let mut d = new_test_data("a/b/c", Some("v2"), 100);
            d.version = 2;
            d
        }; "different version")]
    #[test_case(
        {
            let mut d = new_test_data("a/b/c", Some("v2"), 100);
            d.data_source = DataSource::Logs;
            d
        }; "different data source")]
    #[fuchsia::test]
    fn test_data_merge_noop(other: InspectData) {
        let mut data = new_test_data("a/b/c", Some("v1"), 100);
        let original = data.clone();
        data.merge(other);
        assert_eq!(data, original);
    }

    #[test_case(None, Some("val2"), Some("val2") ; "none_with_some")]
    #[test_case(Some("val1"), None, Some("val1") ; "some_with_none")]
    #[test_case(Some("val1"), Some("val2"), Some("val2") ; "some_with_some")]
    #[fuchsia::test]
    fn test_data_merge_payloads(
        payload: Option<&str>,
        other_payload: Option<&str>,
        expected: Option<&str>,
    ) {
        let mut data = new_test_data("a/b/c", payload, 100);
        let other = new_test_data("a/b/c", other_payload, 100);

        data.merge(other);
        assert_eq!(data, new_test_data("a/b/c", expected, 100));
    }
}
