// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use argh::{ArgsInfo, FromArgs, TopLevelCommand};
use chrono::{DateTime, Local, Utc};
use chrono_english::{Dialect, parse_date_string};
#[cfg(not(any(feature = "fdomain", feature = "ctf")))]
use component_debug::query::get_instances_from_query;
#[cfg(feature = "fdomain")]
use component_debug_fdomain::query::get_instances_from_query;
use diagnostics_data::Severity;
use errors::{FfxError, ffx_bail};
use flex_fuchsia_diagnostics::{LogInterestSelector, LogSettingsProxy};
use flex_fuchsia_sys2::RealmQueryProxy;
pub use log_socket_stream::OneOrMany;
use moniker::Moniker;
use selectors::{SelectorExt, sanitize_moniker_for_selectors};
use std::borrow::Cow;
use std::io::Write;
use std::ops::Deref;
use std::str::FromStr;
use std::string::FromUtf8Error;
use std::time::Duration;
use thiserror::Error;
mod filter;
pub mod fxt_streamer;
mod log_formatter;
mod log_socket_stream;
pub use log_formatter::{
    BootTimeAccessor, DefaultLogFormatter, FormatterError, LogData, LogEntry, Symbolize,
    TIMESTAMP_FORMAT, Timestamp, WriterContainer, dump_logs_from_socket,
};
pub use log_socket_stream::{JsonDeserializeError, LogsDataStream};

pub use log_formatter::dump_fxt_logs_from_socket;

// Subcommand for ffx log (either watch or dump).
#[derive(ArgsInfo, FromArgs, Clone, PartialEq, Debug)]
#[argh(subcommand)]
pub enum LogSubCommand {
    Watch(RawWatchCommand),
    Dump(RawDumpCommand),
    SetSeverity(SetSeverityCommand),
}

#[derive(ArgsInfo, FromArgs, Clone, PartialEq, Debug, Default)]
/// Sets the severity, but doesn't view any logs.
#[argh(subcommand, name = "set-severity")]
pub struct SetSeverityCommand {
    /// if true, doesn't persist the interest setting
    /// and blocks forever, keeping the connection open.
    /// Interest settings will be reset when the command exits.
    #[argh(switch)]
    pub no_persist: bool,

    /// if enabled, selectors will be passed directly to Archivist without any filtering.
    /// If disabled and no matching components are found, the user will be prompted to
    /// either enable this or be given a list of selectors to choose from.
    #[argh(switch)]
    pub force: bool,

    /// configure the log settings on the target device for components matching
    /// the given selector. This modifies the minimum log severity level emitted
    /// by components during the logging session.
    /// Specify using the format <component-selector>#<log-level>, with level
    /// as one of FATAL|ERROR|WARN|INFO|DEBUG|TRACE.
    /// May be repeated.
    #[argh(positional, from_str_fn(log_interest_selector))]
    pub interest_selector: Vec<OneOrMany<LogInterestSelector>>,
}

pub fn parse_time(value: &str) -> Result<DetailedDateTime, String> {
    parse_date_string(value, Local::now(), Dialect::Us)
        .map(|time| DetailedDateTime { time, is_now: value == "now" })
        .map_err(|e| format!("invalid date string: {e}"))
}

/// Parses a time string that defaults to UTC. The time returned will be in the local time zone.
pub fn parse_utc_time(value: &str) -> Result<DetailedDateTime, String> {
    parse_date_string(value, Utc::now(), Dialect::Us)
        .map(|time| DetailedDateTime { time: time.into(), is_now: value == "now" })
        .map_err(|e| format!("invalid date string: {e}"))
}

/// Parses a duration from a string. The input is in seconds
/// and the output is a Rust duration.
pub fn parse_seconds_string_as_duration(value: &str) -> Result<Duration, String> {
    Ok(Duration::from_secs(
        value.parse().map_err(|e| format!("value '{value}' is not a number: {e}"))?,
    ))
}

// Time format for displaying logs
#[derive(Clone, Debug, PartialEq)]
pub enum TimeFormat {
    // UTC time
    Utc,
    // Local time
    Local,
    // Boot time
    Boot,
}

impl std::str::FromStr for TimeFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lower = s.to_ascii_lowercase();
        match lower.as_str() {
            "local" => Ok(TimeFormat::Local),
            "utc" => Ok(TimeFormat::Utc),
            "boot" => Ok(TimeFormat::Boot),
            _ => Err(format!("'{s}' is not a valid value: must be one of 'local', 'utc', 'boot'")),
        }
    }
}

/// Encoding format for retrieving logs from archivist
#[derive(Clone, Debug, PartialEq)]
pub enum LogEncoding {
    Json,
    Fxt,
}

impl std::str::FromStr for LogEncoding {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lower = s.to_ascii_lowercase();
        match lower.as_str() {
            "json" => Ok(LogEncoding::Json),
            "fxt" => Ok(LogEncoding::Fxt),
            _ => Err(format!("'{s}' is not a valid value: must be one of 'json', 'fxt'")),
        }
    }
}

/// Date/time structure containing a "now"
/// field, set if it should be interpreted as the
/// current time (used to call Subscribe instead of SnapshotThenSubscribe).
#[derive(PartialEq, Clone, Debug)]
pub struct DetailedDateTime {
    /// The absolute timestamp as specified by the user
    /// or the current timestamp if 'now' is specified.
    pub time: DateTime<Local>,
    /// Whether or not the DateTime was "now".
    /// If the DateTime is "now", logs will be collected in subscribe
    /// mode, instead of SnapshotThenSubscribe.
    pub is_now: bool,
}

impl Deref for DetailedDateTime {
    type Target = DateTime<Local>;

    fn deref(&self) -> &Self::Target {
        &self.time
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum SymbolizeMode {
    /// Disable all symbolization
    Off,
    /// Use prettified symbolization
    Pretty,
    /// Use classic (non-prettified) symbolization
    Classic,
}

impl SymbolizeMode {
    pub fn is_prettification_disabled(&self) -> bool {
        matches!(self, SymbolizeMode::Classic)
    }

    pub fn is_symbolize_disabled(&self) -> bool {
        matches!(self, SymbolizeMode::Off)
    }
}

/// Helper macro to merge individual `LogFilterArgs` fields based on outer field type.
#[doc(hidden)]
macro_rules! overlay_field {
    (Vec, $self:ident, $child:ident, $field:ident) => {
        $self.$field.extend($child.$field.into_iter());
    };
    (Option, $self:ident, $child:ident, $field:ident) => {
        if $child.$field.is_some() {
            $self.$field = $child.$field;
        }
    };
    (bool, $self:ident, $child:ident, $field:ident) => {
        $self.$field |= $child.$field;
    };
}

/// Helper Token-Tree (TT) Muncher macro for [`define_log_filter_args!`](define_log_filter_args!).
///
/// # Purpose
/// Rust pattern destructuring and struct field initialization cannot contain documentation
/// comments (`///`) or non-`cfg` attributes (`#[argh(...)]`). Doing so produces syntax errors.
/// However, conditionally compiled fields (`#[cfg(...)]`) *must* be preserved in pattern
/// destructuring and struct literals so that conditional flags compile correctly.
///
/// This macro uses a Token-Tree (TT) Muncher pattern to inspect all field attributes, stripping non-`cfg`
/// attributes (`#[argh]`, `#[doc]`) while preserving `#[cfg(...)]` annotations.
///
/// # TT-Muncher Stages
/// - `[ $(#[$attr])* ]`: Accumulated attributes for the field being processed.
/// - `[ $(#[$cfg])* ]`: Accumulated `#[cfg(...)]` attributes for the field being processed.
/// - `[ $($fields)* ]`: Output tuples of stripped fields `( [attrs] [cfgs] vis field : ty )`.
#[doc(hidden)]
macro_rules! __define_log_filter_args_helper {
    (
        [ $(#[$attr:meta])* ]
        [ $(#[$cfg:meta])* ]
        [ $($fields:tt)* ]
        #[cfg $($cfg_args:tt)*]
        $($rest:tt)*
    ) => {
        __define_log_filter_args_helper! {
            [ $(#[$attr])* #[cfg $($cfg_args)*] ]
            [ $(#[$cfg])* #[cfg $($cfg_args)*] ]
            [ $($fields)* ]
            $($rest)*
        }
    };

    (
        [ $(#[$attr:meta])* ]
        [ $(#[$cfg:meta])* ]
        [ $($fields:tt)* ]
        #[$other_attr:meta]
        $($rest:tt)*
    ) => {
        __define_log_filter_args_helper! {
            [ $(#[$attr])* #[$other_attr] ]
            [ $(#[$cfg])* ]
            [ $($fields)* ]
            $($rest)*
        }
    };

    (
        [ $(#[$attr:meta])* ]
        [ $(#[$cfg:meta])* ]
        [ $($fields:tt)* ]
        $vis:vis $field:ident : $ty_outer:ident $( < $ty_inner:ty > )? $(, $($rest:tt)*)?
    ) => {
        __define_log_filter_args_helper! {
            [ ]
            [ ]
            [
                $($fields)*
                (
                    [ $(#[$attr])* ]
                    [ $(#[$cfg])* ]
                    $vis $field : $ty_outer $( < $ty_inner > )?
                )
            ]
            $($($rest)*)?
        }
    };

    (
        [ ]
        [ ]
        [
            $(
                (
                    [ $(#[$all_attr:meta])* ]
                    [ $(#[$cfg_attr:meta])* ]
                    $vis:vis $field:ident : $ty_outer:ident $( < $ty_inner:ty > )?
                )
            )*
        ]
    ) => {
        /// Container for log filtering and display arguments.
        #[derive(Clone, Debug, PartialEq)]
        pub struct LogFilterArgs {
            $(
                $(#[$cfg_attr])*
                $vis $field: $ty_outer $( < $ty_inner > )?,
            )*
        }

        impl Default for LogFilterArgs {
            fn default() -> Self {
                LogFilterArgs {
                    $(
                        $(#[$cfg_attr])*
                        $field: Default::default(),
                    )*
                }
            }
        }

        impl LogFilterArgs {
            /// Merges `other` filter arguments into `self`, extending list filters and overlaying non-default scalar/flag values.
            pub fn merge(&mut self, other: LogFilterArgs) {
                $(
                    $(#[$cfg_attr])*
                    overlay_field!($ty_outer, self, other, $field);
                )*
            }
        }

        #[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
        /// Raw command line arguments for `ffx log` before subcommand flag merging.
        #[argh(
            subcommand,
            name = "log",
            description = "Display logs from a target device",
            note = "Logs are retrieved from the target at the moment this command is called.\n\nYou may see some additional information attached to the log line:\n\n- `dropped=N`: this means that N logs attributed to the component were dropped when the component\n  wrote to the log socket. This can happen when archivist cannot keep up with the rate of logs being\n  emitted by the component and the component filled the log socket buffer in the kernel.\n\n- `rolled=N`: this means that N logs rolled out from the archivist buffer and ffx never saw them.\n  This can happen when more logs are being ingested by the archivist across all components and the\n  ffx couldn't retrieve them fast enough.\n\nSymbolization is performed in the background using the symbolizer host tool. You can pass\nadditional arguments to the symbolizer tool (for example, to add a remote symbol server) using:\n  $ ffx config set proactive_log.symbolize.extra_args \"--symbol-server gs://some-url/path --symbol-server gs://some-other-url/path ...\"\n\nTo learn more about configuring the log viewer, visit https://fuchsia.dev/fuchsia-src/development/tools/ffx/commands/log",
            example = "Dump the most recent logs and stream new ones as they happen:\n  $ ffx log\n\nStream new logs starting from the current time, filtering for severity of at least \"WARN\":\n  $ ffx log --severity warn --since now\n\nStream logs where the source moniker, component url and message do not include \"sys\":\n  $ ffx log --exclude sys\n\nStream ERROR logs with source moniker, component url or message containing either\n\"netstack\" or \"remote-control.cm\", but not containing \"sys\":\n  $ ffx log --severity error --filter netstack --filter remote-control.cm --exclude sys\n\nDump all available logs where the source moniker, component url, or message contains\n\"remote-control\":\n  $ ffx log --filter remote-control dump\n\nDump all logs from the last 30 minutes logged before 5 minutes ago:\n  $ ffx log --since \"30m ago\" --until \"5m ago\" dump\n\nEnable DEBUG logs from the \"core/audio\" component while logs are streaming:\n  $ ffx log --set-severity core/audio#DEBUG"
        )]
        pub struct RawLogCommand {
            #[argh(subcommand)]
            pub sub_command: Option<LogSubCommand>,

            /// configure the log settings on the target device for components matching
            /// the given selector. This modifies the minimum log severity level emitted
            /// by components during the logging session.
            /// Specify using the format <component-selector>#<log-level>, with level
            /// as one of FATAL|ERROR|WARN|INFO|DEBUG|TRACE.
            /// May be repeated and it's also possible to pass multiple comma-separated
            /// strings per invocation.
            /// Cannot be used in conjunction with the set-severity subcommand.
            #[argh(option, from_str_fn(log_interest_selector))]
            pub set_severity: Vec<OneOrMany<LogInterestSelector>>,

            $(
                $(#[$all_attr])*
                $vis $field: $ty_outer $( < $ty_inner > )?,
            )*
        }

        impl RawLogCommand {
            pub fn into_log_command(self) -> LogCommand {
                LogCommand {
                    sub_command: self.sub_command,
                    set_severity: self.set_severity,
                    filters: LogFilterArgs {
                        $(
                            $(#[$cfg_attr])*
                            $field: self.$field,
                        )*
                    },
                }
            }
        }

        #[derive(ArgsInfo, FromArgs, Clone, PartialEq, Debug)]
        /// Dumps all logs from a given target's session.
        #[argh(subcommand, name = "dump")]
        pub struct RawDumpCommand {
            /// return only the last N log lines.
            #[argh(option)]
            pub tail: Option<usize>,

            $(
                $(#[$all_attr])*
                $vis $field: $ty_outer $( < $ty_inner > )?,
            )*
        }

        impl Default for RawDumpCommand {
            fn default() -> Self {
                let filters = LogFilterArgs::default();
                Self {
                    tail: None,
                    $(
                        $(#[$cfg_attr])*
                        $field: filters.$field,
                    )*
                }
            }
        }

        impl RawDumpCommand {
            pub fn into_filter_args(self) -> LogFilterArgs {
                LogFilterArgs {
                    $(
                        $(#[$cfg_attr])*
                        $field: self.$field,
                    )*
                }
            }
        }

        #[derive(ArgsInfo, FromArgs, Clone, PartialEq, Debug)]
        /// Watches for and prints logs from a target. Default if no sub-command is specified.
        #[argh(subcommand, name = "watch")]
        pub struct RawWatchCommand {
            $(
                $(#[$all_attr])*
                $vis $field: $ty_outer $( < $ty_inner > )?,
            )*
        }

        impl Default for RawWatchCommand {
            fn default() -> Self {
                let filters = LogFilterArgs::default();
                Self {
                    $(
                        $(#[$cfg_attr])*
                        $field: filters.$field,
                    )*
                }
            }
        }

        impl RawWatchCommand {
            pub fn into_filter_args(self) -> LogFilterArgs {
                LogFilterArgs {
                    $(
                        $(#[$cfg_attr])*
                        $field: self.$field,
                    )*
                }
            }
        }
    };
}

/// Macro to define `LogFilterArgs` and all derivative raw CLI command structs.
///
/// # Purpose
/// `ffx log` supports filtering options both at the root command level (e.g. `ffx log --severity warn`)
/// and at the subcommand level (e.g. `ffx log dump --severity warn`).
///
/// `argh` requires CLI flags to be defined directly on the struct corresponding to a command/subcommand.
/// To avoid manually duplicating 30+ filter flags across `RawLogCommand`, `RawDumpCommand`, and `RawWatchCommand`,
/// this macro generates:
/// 1. `LogFilterArgs`: Container struct holding all active filter criteria.
/// 2. `RawLogCommand`: Top-level CLI `argh` parser struct.
/// 3. `RawDumpCommand`: `dump` subcommand `argh` parser struct.
/// 4. `RawWatchCommand`: `watch` subcommand `argh` parser struct.
/// 5. Conversion methods (`into_log_command`, `into_filter_args`).
/// 6. `LogFilterArgs::merge()`: Method to merge subcommand filter overrides.
///
/// # Adding New Log Filter Flags
/// To add a new CLI flag to `ffx log`:
/// 1. Locate the `define_log_filter_args!` invocation in `src/diagnostics/lib/log-command/src/lib.rs`.
/// 2. Add the field with doc comments (`///`) and `argh` attributes (`#[argh(option)]` or `#[argh(switch)]`).
/// 3. The macro will automatically propagate the field to `LogFilterArgs`, all `Raw*Command` structs,
///    their respective `into_*` conversion functions, and `LogFilterArgs::merge()`.
/// 4. Add an encapsulated getter accessor method to `impl LogCommand`.
///
/// # Syntax
/// ```rust,ignore
/// define_log_filter_args! {
///     /// Filter description for CLI help output.
///     #[argh(option)]
///     pub my_flag: Option<String>,
/// }
/// ```
macro_rules! define_log_filter_args {
    ($($tokens:tt)*) => {
        __define_log_filter_args_helper! {
            [ ]
            [ ]
            [ ]
            $($tokens)*
        }
    };
}

define_log_filter_args! {
    /// filter for a string in either the message, component or url.
    /// May be repeated.
    #[argh(option)]
    pub filter: Vec<String>,

    /// DEPRECATED: use --component
    #[argh(option)]
    pub moniker: Vec<String>,

    /// fuzzy search for a component by moniker or url.
    /// May be repeated.
    #[argh(option)]
    pub component: Vec<String>,

    /// exclude a string in either the message, component or url.
    /// May be repeated.
    #[argh(option)]
    pub exclude: Vec<String>,

    /// exclude logs matching a regular expression. May be repeated.
    #[argh(option)]
    pub exclude_regex: Vec<String>,

    /// path to a file containing regular expressions, one per line, to exclude.
    #[argh(option)]
    pub exclude_regex_file: Option<String>,

    /// filter for only logs with a given tag. May be repeated.
    #[argh(option)]
    pub tag: Vec<String>,

    /// exclude logs with a given tag. May be repeated.
    #[argh(option)]
    pub exclude_tags: Vec<String>,

    /// set the minimum severity. Accepted values (from lower to higher) are: trace, debug, info,
    /// warn (or warning), error, fatal. This field is case insensitive.
    #[argh(option)]
    pub severity: Option<Severity>,

    /// outputs only kernel logs, unless combined with --component.
    #[argh(switch)]
    pub kernel: bool,

    /// show only logs after a certain time (exclusive)
    #[argh(option, from_str_fn(parse_time))]
    pub since: Option<DetailedDateTime>,

    /// show only logs after a certain time (as a boot
    /// timestamp: seconds from the target's boot time).
    #[argh(option, from_str_fn(parse_seconds_string_as_duration))]
    pub since_boot: Option<Duration>,

    /// show only logs until a certain time (exclusive)
    #[argh(option, from_str_fn(parse_time))]
    pub until: Option<DetailedDateTime>,

    /// show only logs until a certain time (as a boot
    /// timestamp: seconds since the target's boot time).
    #[argh(option, from_str_fn(parse_seconds_string_as_duration))]
    pub until_boot: Option<Duration>,

    /// hide the tag field from output (does not exclude any log messages)
    #[argh(switch)]
    pub hide_tags: bool,

    /// hide the file and line number field from output (does not exclude any log messages)
    #[argh(switch)]
    pub hide_file: bool,

    /// disable coloring logs according to severity.
    /// Note that you can permanently disable this with
    /// `ffx config set log_cmd.color false`
    #[argh(switch)]
    pub no_color: bool,

    /// if enabled, text filtering options are case-sensitive
    /// this applies to --filter, --exclude, --tag, and --exclude-tags.
    #[argh(switch)]
    pub case_sensitive: bool,

    /// shows process-id and thread-id in log output
    #[argh(switch)]
    pub show_metadata: bool,

    /// shows the full moniker in log output. By default this is false and only the last segment
    /// of the moniker is printed.
    #[argh(switch)]
    pub show_full_moniker: bool,

    /// if enabled, prefer using the component URL for the component name over the moniker.
    #[argh(switch)]
    pub prefer_url_component_name: bool,

    /// hide the moniker field from output (does not exclude any log messages)
    #[argh(switch)]
    pub hide_moniker: bool,

    /// how to display log timestamps.
    /// Options are "utc", "local", or "boot" (i.e. nanos since target boot).
    /// Default is boot.
    #[argh(option)]
    pub clock: Option<TimeFormat>,

    /// configure symbolization options. Valid options are:
    /// - pretty (default): pretty concise symbolization
    /// - off: disables all symbolization
    /// - classic: traditional, non-prettified symbolization
    #[cfg(not(target_os = "fuchsia"))]
    #[argh(option)]
    pub symbolize: Option<SymbolizeMode>,

    /// filters by pid
    #[argh(option)]
    pub pid: Option<u64>,

    /// filters by tid
    #[argh(option)]
    pub tid: Option<u64>,

    /// if enabled, selectors will be passed directly to Archivist without any filtering.
    /// If disabled and no matching components are found, the user will be prompted to
    /// either enable this or be given a list of selectors to choose from.
    /// This applies to both --set-severity and the set-severity subcommand.
    #[argh(switch)]
    pub force_set_severity: bool,

    /// EXPERIMENTAL/SUBJECT TO REMOVAL: select the encoding used to retrieve logs from the
    /// archivist. Options are "json" or "fxt". Default is "json".
    #[cfg(target_os = "fuchsia")]
    #[argh(option)]
    pub encoding: Option<LogEncoding>,

    /// enables structured JSON logs.
    #[cfg(target_os = "fuchsia")]
    #[argh(switch)]
    pub json: bool,

    /// disable automatic reconnect
    #[cfg(not(target_os = "fuchsia"))]
    #[argh(switch)]
    pub disable_reconnect: bool,
}

#[derive(Default, Clone, Debug, PartialEq)]
/// Consolidated log command representation containing merged filter criteria and subcommands.
pub struct LogCommand {
    pub sub_command: Option<LogSubCommand>,
    pub set_severity: Vec<OneOrMany<LogInterestSelector>>,
    pub filters: LogFilterArgs,
}

impl LogCommand {
    /// Merges subcommand filter overrides (e.g. `dump` or `watch`) into `self.filters`.
    pub fn merge_subcommand(&mut self) {
        match &self.sub_command {
            Some(LogSubCommand::Dump(raw_dump)) => {
                self.filters.merge(raw_dump.clone().into_filter_args());
            }
            Some(LogSubCommand::Watch(raw_watch)) => {
                self.filters.merge(raw_watch.clone().into_filter_args());
            }
            _ => {}
        }
    }
}

impl FromArgs for LogCommand {
    fn from_args(command_name: &[&str], args: &[&str]) -> Result<Self, argh::EarlyExit> {
        let cli = match RawLogCommand::from_args(command_name, args) {
            Ok(cli) => cli,
            Err(mut early_exit) => {
                if early_exit.status.is_err() {
                    if args.iter().any(|arg| arg.starts_with("--dump")) {
                        early_exit.output.push_str(
                            "\nNote: 'dump' is a sub-command of 'ffx log', not a flag. Use: ffx log dump [options]\n",
                        );
                    } else if args.contains(&"dump") {
                        if args.iter().any(|arg| arg.starts_with("--limit")) {
                            early_exit.output.push_str(
                                "\nNote: 'ffx log dump' does not take --limit. To limit output lines, use '--tail <count>' or pipe to head/tail.\n",
                            );
                        }
                        if args.iter().any(|arg| arg.starts_with("--grep")) {
                            early_exit.output.push_str(
                                "\nNote: 'ffx log dump' does not take --grep. To filter log snapshot output, use '--filter <pattern>' or pipe to grep: ffx log dump | grep <pattern>\n",
                            );
                        }
                    }
                }
                return Err(early_exit);
            }
        };
        let mut cmd = cli.into_log_command();
        cmd.merge_subcommand();
        Ok(cmd)
    }
}

impl ArgsInfo for LogCommand {
    fn get_args_info() -> argh::CommandInfoWithArgs {
        RawLogCommand::get_args_info()
    }
}

impl argh::SubCommand for LogCommand {
    const COMMAND: &'static argh::CommandInfo = RawLogCommand::COMMAND;
}

/// Result returned from processing logs
#[derive(PartialEq, Debug)]
pub enum LogProcessingResult {
    /// The caller should exit
    Exit,
    /// The caller should continue processing logs
    Continue,
}

impl FromStr for SymbolizeMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_lowercase();
        match s.as_str() {
            "off" => Ok(SymbolizeMode::Off),
            "pretty" => Ok(SymbolizeMode::Pretty),
            "classic" => Ok(SymbolizeMode::Classic),
            other => Err(format_err!("invalid symbolize flag: {}", other)),
        }
    }
}

#[derive(Error, Debug)]
pub enum LogError {
    #[error(transparent)]
    UnknownError(#[from] anyhow::Error),
    #[error("No boot timestamp")]
    NoBootTimestamp,
    #[error(transparent)]
    IOError(#[from] std::io::Error),
    #[error(transparent)]
    RegexError(#[from] regex_lite::Error),
    #[error("Cannot use dump with --since now")]
    DumpWithSinceNow,
    #[error("No symbolizer configuration provided")]
    NoSymbolizerConfig,
    #[error(transparent)]
    FfxError(#[from] FfxError),
    #[error(transparent)]
    Utf8Error(#[from] FromUtf8Error),
    #[error(transparent)]
    FidlError(#[from] fidl::Error),
    #[error(transparent)]
    FormatterError(#[from] FormatterError),
    #[error("Deprecated flag: `{flag}`, use: `{new_flag}`")]
    DeprecatedFlag { flag: &'static str, new_flag: &'static str },
    #[error(
        "Fuzzy matching for '{query}' failed due to too many matches, please re-try with one of these:\n{matches}"
    )]
    FuzzyMatchTooManyMatches { query: String, matches: String },
    #[error(
        "No running components were found matching {0}. Please ensure the component is running and the moniker is correct. Run 'ffx component list' to see running components."
    )]
    SearchParameterNotFound(String),
}

impl LogError {
    fn too_many_fuzzy_matches(matches: impl Iterator<Item = String>, query: &str) -> Self {
        let mut result = String::new();
        for component in matches {
            result.push_str(&component);
            result.push('\n');
        }

        Self::FuzzyMatchTooManyMatches { matches: result, query: query.to_string() }
    }

    pub fn is_broken_pipe(&self) -> bool {
        match self {
            LogError::IOError(error) => error.kind() == std::io::ErrorKind::BrokenPipe,
            LogError::FormatterError(formatter_error) => formatter_error.is_broken_pipe(),
            LogError::UnknownError(err) => {
                if let Some(writer_err) = err.downcast_ref::<writer::Error>() {
                    writer_err.is_broken_pipe()
                } else if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
                    io_err.kind() == std::io::ErrorKind::BrokenPipe
                } else {
                    false
                }
            }

            LogError::NoBootTimestamp
            | LogError::DumpWithSinceNow
            | LogError::NoSymbolizerConfig
            | LogError::RegexError(_)
            | LogError::FfxError(_)
            | LogError::Utf8Error(_)
            | LogError::FidlError(_)
            | LogError::DeprecatedFlag { .. }
            | LogError::FuzzyMatchTooManyMatches { .. }
            | LogError::SearchParameterNotFound(_) => false,
        }
    }
}

/// Trait used to get available instances given a moniker query.
#[async_trait::async_trait(?Send)]
pub trait InstanceGetter {
    async fn get_monikers_from_query(&self, query: &str) -> Result<Vec<Moniker>, LogError>;
}

#[cfg(not(feature = "ctf"))]
#[async_trait::async_trait(?Send)]
impl InstanceGetter for RealmQueryProxy {
    async fn get_monikers_from_query(&self, query: &str) -> Result<Vec<Moniker>, LogError> {
        Ok(get_instances_from_query(query, self)
            .await?
            .into_iter()
            .map(|value| value.moniker)
            .collect())
    }
}

#[cfg(feature = "ctf")]
#[async_trait::async_trait(?Send)]
impl InstanceGetter for RealmQueryProxy {
    async fn get_monikers_from_query(&self, _query: &str) -> Result<Vec<Moniker>, LogError> {
        unreachable!("get_monikers_from_query is not supported in CTF tests.");
    }
}

impl LogCommand {
    /// Returns the minimum log severity.
    #[must_use]
    pub fn severity(&self) -> Severity {
        self.filters.severity.unwrap_or(Severity::Info)
    }

    /// Returns the timestamp display format.
    #[must_use]
    pub fn clock(&self) -> TimeFormat {
        self.filters.clock.clone().unwrap_or(TimeFormat::Boot)
    }

    /// Returns the symbolization mode.
    #[cfg(not(target_os = "fuchsia"))]
    #[must_use]
    pub fn symbolize(&self) -> SymbolizeMode {
        self.filters.symbolize.clone().unwrap_or(SymbolizeMode::Pretty)
    }

    /// Returns the log encoding format.
    #[cfg(target_os = "fuchsia")]
    #[must_use]
    pub fn encoding(&self) -> LogEncoding {
        self.filters.encoding.clone().unwrap_or(LogEncoding::Json)
    }

    /// Returns the log text filter patterns.
    #[must_use]
    pub fn filter(&self) -> &[String] {
        &self.filters.filter
    }

    /// Returns the deprecated moniker filter patterns.
    #[must_use]
    pub fn moniker(&self) -> &[String] {
        &self.filters.moniker
    }

    /// Returns the component filter patterns.
    #[must_use]
    pub fn component(&self) -> &[String] {
        &self.filters.component
    }

    /// Returns the text exclusion patterns.
    #[must_use]
    pub fn exclude(&self) -> &[String] {
        &self.filters.exclude
    }

    /// Returns the regular expression exclusion patterns.
    #[must_use]
    pub fn exclude_regex(&self) -> &[String] {
        &self.filters.exclude_regex
    }

    /// Returns the tag filter patterns.
    #[must_use]
    pub fn tag(&self) -> &[String] {
        &self.filters.tag
    }

    /// Returns the tag exclusion patterns.
    #[must_use]
    pub fn exclude_tags(&self) -> &[String] {
        &self.filters.exclude_tags
    }

    /// Returns whether tags are hidden from output.
    #[must_use]
    pub fn hide_tags(&self) -> bool {
        self.filters.hide_tags
    }

    /// Returns whether colored output is disabled.
    #[must_use]
    pub fn no_color(&self) -> bool {
        self.filters.no_color
    }

    /// Sets whether colored output is disabled.
    pub fn set_no_color(&mut self, no_color: bool) {
        self.filters.no_color = no_color;
    }

    /// Returns whether PID and TID metadata are displayed.
    #[must_use]
    pub fn show_metadata(&self) -> bool {
        self.filters.show_metadata
    }

    /// Returns whether file and line number locations are hidden.
    #[must_use]
    pub fn hide_file(&self) -> bool {
        self.filters.hide_file
    }

    /// Returns whether monikers are hidden from output.
    #[must_use]
    pub fn hide_moniker(&self) -> bool {
        self.filters.hide_moniker
    }

    /// Returns whether full monikers are displayed.
    #[must_use]
    pub fn show_full_moniker(&self) -> bool {
        self.filters.show_full_moniker
    }

    /// Returns whether component URL is preferred over moniker for display.
    #[must_use]
    pub fn prefer_url_component_name(&self) -> bool {
        self.filters.prefer_url_component_name
    }

    /// Returns the starting timestamp filter.
    #[must_use]
    pub fn since(&self) -> Option<&DetailedDateTime> {
        self.filters.since.as_ref()
    }

    /// Returns the ending timestamp filter.
    #[must_use]
    pub fn until(&self) -> Option<&DetailedDateTime> {
        self.filters.until.as_ref()
    }

    /// Returns the starting boot duration filter.
    #[must_use]
    pub fn since_boot(&self) -> Option<Duration> {
        self.filters.since_boot
    }

    /// Returns the ending boot duration filter.
    #[must_use]
    pub fn until_boot(&self) -> Option<Duration> {
        self.filters.until_boot
    }

    /// Returns whether JSON output is enabled.
    #[cfg(target_os = "fuchsia")]
    #[must_use]
    pub fn json(&self) -> bool {
        self.filters.json
    }

    /// Returns the path to the regex exclusion file, if set.
    #[must_use]
    pub fn exclude_regex_file(&self) -> Option<&str> {
        self.filters.exclude_regex_file.as_deref()
    }

    /// Returns whether only kernel logs should be displayed.
    #[must_use]
    pub fn kernel(&self) -> bool {
        self.filters.kernel
    }

    /// Returns whether severity selectors bypass ambiguity checks.
    #[must_use]
    pub fn force_set_severity(&self) -> bool {
        self.filters.force_set_severity
    }

    /// Returns whether text filtering is case-sensitive.
    #[must_use]
    pub fn case_sensitive(&self) -> bool {
        self.filters.case_sensitive
    }

    /// Returns the process ID filter, if set.
    #[must_use]
    pub fn pid(&self) -> Option<u64> {
        self.filters.pid
    }

    /// Returns the thread ID filter, if set.
    #[must_use]
    pub fn tid(&self) -> Option<u64> {
        self.filters.tid
    }

    /// Returns whether automatic reconnection is disabled.
    #[cfg(not(target_os = "fuchsia"))]
    #[must_use]
    pub fn disable_reconnect(&self) -> bool {
        self.filters.disable_reconnect
    }

    async fn map_interest_selectors<'a>(
        realm_query: &impl InstanceGetter,
        interest_selectors: impl Iterator<Item = &'a LogInterestSelector>,
    ) -> Result<impl Iterator<Item = Cow<'a, LogInterestSelector>>, LogError> {
        let selectors = Self::get_selectors_and_monikers(interest_selectors);
        let mut translated_selectors = vec![];
        for (moniker, selector) in selectors {
            // Attempt to translate to a single instance
            let instances = realm_query.get_monikers_from_query(moniker.as_str()).await?;
            // If exactly one match, perform rewrite
            if instances.len() == 1 {
                let mut translated_selector = selector.clone();
                translated_selector.selector = instances[0].clone().into_component_selector();
                translated_selectors.push((Cow::Owned(translated_selector), instances));
            } else {
                translated_selectors.push((Cow::Borrowed(selector), instances));
            }
        }
        if translated_selectors.iter().any(|(_, matches)| matches.len() > 1) {
            let mut err_output = vec![];
            writeln!(
                &mut err_output,
                "WARN: One or more of your selectors appears to be ambiguous"
            )?;
            writeln!(&mut err_output, "and may not match any components on your system.\n")?;
            writeln!(
                &mut err_output,
                "If this is unintentional you can explicitly match using the"
            )?;
            writeln!(&mut err_output, "following command:\n")?;
            writeln!(&mut err_output, "ffx log \\")?;
            let mut output = vec![];
            for (oselector, instances) in translated_selectors {
                for selector in instances {
                    writeln!(
                        output,
                        "\t--set-severity {}#{} \\",
                        sanitize_moniker_for_selectors(selector.to_string().as_str())
                            .replace("\\", "\\\\"),
                        format!("{:?}", oselector.interest.min_severity.unwrap()).to_uppercase()
                    )?;
                }
            }
            // Intentionally ignored, removes the newline, space, and \
            let _ = output.pop();
            let _ = output.pop();
            let _ = output.pop();

            writeln!(&mut err_output, "{}", String::from_utf8(output).unwrap())?;
            writeln!(&mut err_output, "\nIf this is intentional, you can disable this with")?;
            writeln!(&mut err_output, "ffx log --force-set-severity.")?;

            ffx_bail!("{}", String::from_utf8(err_output)?);
        }
        Ok(translated_selectors.into_iter().map(|(selector, _)| selector))
    }

    pub fn validate_cmd_flags_with_warnings(&mut self) -> Result<Vec<&'static str>, LogError> {
        let mut warnings = vec![];

        if !self.filters.moniker.is_empty() {
            warnings.push("WARNING: --moniker is deprecated, use --component instead");
            if self.filters.component.is_empty() {
                self.filters.component = std::mem::take(&mut self.filters.moniker);
            } else {
                warnings.push("WARNING: ignoring --moniker arguments in favor of --component");
            }
        }

        Ok(warnings)
    }

    /// Sets interest based on configured selectors.
    /// If a single ambiguous match is found, the monikers in the selectors
    /// are automatically re-written.
    pub async fn maybe_set_interest(
        &self,
        log_settings_client: &LogSettingsProxy,
        realm_query: &impl InstanceGetter,
    ) -> Result<(), LogError> {
        let (set_severity, force_set_severity, persist) =
            if let Some(LogSubCommand::SetSeverity(options)) = &self.sub_command {
                // No other argument can exist in conjunction with SetSeverity
                let default_cmd = LogCommand {
                    sub_command: Some(LogSubCommand::SetSeverity(options.clone())),
                    ..Default::default()
                };
                if &default_cmd != self {
                    ffx_bail!("Cannot combine set-severity with other options.");
                }
                (&options.interest_selector, options.force, !options.no_persist)
            } else {
                (&self.set_severity, self.filters.force_set_severity, false)
            };

        if persist || !set_severity.is_empty() {
            let selectors = if force_set_severity {
                set_severity.clone().into_iter().flatten().collect::<Vec<_>>()
            } else {
                let new_selectors =
                    Self::map_interest_selectors(realm_query, set_severity.iter().flatten())
                        .await?
                        .map(|s| s.into_owned())
                        .collect::<Vec<_>>();
                if new_selectors.is_empty() {
                    set_severity.clone().into_iter().flatten().collect::<Vec<_>>()
                } else {
                    new_selectors
                }
            };
            log_settings_client
                .set_component_interest(
                    &flex_fuchsia_diagnostics::LogSettingsSetComponentInterestRequest {
                        selectors: Some(selectors),
                        persist: Some(persist),
                        ..Default::default()
                    },
                )
                .await?;
        }

        Ok(())
    }

    fn get_selectors_and_monikers<'a>(
        interest_selectors: impl Iterator<Item = &'a LogInterestSelector>,
    ) -> Vec<(String, &'a LogInterestSelector)> {
        let mut selectors = vec![];
        for selector in interest_selectors {
            let segments = selector.selector.moniker_segments.as_ref().unwrap();
            let mut full_moniker = String::new();
            for segment in segments {
                match segment {
                    flex_fuchsia_diagnostics::StringSelector::ExactMatch(segment) => {
                        if full_moniker.is_empty() {
                            full_moniker.push_str(segment);
                        } else {
                            full_moniker.push('/');
                            full_moniker.push_str(segment);
                        }
                    }
                    _ => {
                        // If the user passed a non-exact match we assume they
                        // know what they're doing and skip this logic.
                        return vec![];
                    }
                }
            }
            selectors.push((full_moniker, selector));
        }
        selectors
    }
}

impl TopLevelCommand for LogCommand {}

fn log_interest_selector(s: &str) -> Result<OneOrMany<LogInterestSelector>, String> {
    if s.contains(",") {
        let many: Result<Vec<LogInterestSelector>, String> = s
            .split(",")
            .map(|value| selectors::parse_log_interest_selector(value).map_err(|e| e.to_string()))
            .collect();
        Ok(OneOrMany::Many(many?))
    } else {
        Ok(OneOrMany::One(selectors::parse_log_interest_selector(s).map_err(|s| s.to_string())?))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use fidl::endpoints::create_proxy;
    use flex_fuchsia_diagnostics::{LogSettingsMarker, LogSettingsRequest};
    use futures_util::StreamExt;
    use futures_util::future::Either;
    use futures_util::stream::FuturesUnordered;
    use selectors::parse_log_interest_selector;

    #[derive(Default)]
    struct FakeInstanceGetter {
        output: Vec<Moniker>,
        expected_selector: Option<String>,
    }

    #[async_trait(?Send)]
    impl InstanceGetter for FakeInstanceGetter {
        async fn get_monikers_from_query(&self, query: &str) -> Result<Vec<Moniker>, LogError> {
            if let Some(expected) = &self.expected_selector {
                assert_eq!(expected, query);
            }
            Ok(self.output.clone())
        }
    }

    #[fuchsia::test]
    async fn test_symbolize_mode_from_str() {
        assert_matches!(SymbolizeMode::from_str("off"), Ok(value) if value == SymbolizeMode::Off);
        assert_matches!(
            SymbolizeMode::from_str("pretty"),
            Ok(value) if value == SymbolizeMode::Pretty
        );
        assert_matches!(
            SymbolizeMode::from_str("classic"),
            Ok(value) if value == SymbolizeMode::Classic
        );
    }

    #[fuchsia::test]
    async fn maybe_set_interest_errors_additional_arguments_passed_to_set_interest() {
        let (settings_proxy, settings_server) = create_proxy::<LogSettingsMarker>();
        let getter = FakeInstanceGetter {
            expected_selector: Some("ambiguous_selector".into()),
            output: vec![
                Moniker::try_from("core/some/ambiguous_selector:thing/test").unwrap(),
                Moniker::try_from("core/other/ambiguous_selector:thing/test").unwrap(),
            ],
        };
        // Main should return an error

        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::SetSeverity(SetSeverityCommand {
                interest_selector: vec![OneOrMany::One(
                    parse_log_interest_selector("ambiguous_selector#INFO").unwrap(),
                )],
                force: false,
                no_persist: false,
            })),
            filters: LogFilterArgs { hide_file: true, ..LogFilterArgs::default() },
            ..LogCommand::default()
        };
        let mut set_interest_result = None;

        let mut scheduler = FuturesUnordered::new();
        scheduler.push(Either::Left(async {
            set_interest_result = Some(cmd.maybe_set_interest(&settings_proxy, &getter).await);
            drop(settings_proxy);
        }));
        scheduler.push(Either::Right(async {
            let request = settings_server.into_stream().next().await;
            // The channel should be closed without sending any requests.
            assert_matches!(request, None);
        }));
        while scheduler.next().await.is_some() {}
        drop(scheduler);

        let error = format!("{}", set_interest_result.unwrap().unwrap_err());

        const EXPECTED_INTEREST_ERROR: &str = "Cannot combine set-severity with other options.";
        assert_eq!(error, EXPECTED_INTEREST_ERROR);
    }

    #[fuchsia::test]
    async fn maybe_set_interest_errors_if_ambiguous_selector() {
        let (settings_proxy, settings_server) = create_proxy::<LogSettingsMarker>();
        let getter = FakeInstanceGetter {
            expected_selector: Some("ambiguous_selector".into()),
            output: vec![
                Moniker::try_from("core/some/ambiguous_selector:thing/test").unwrap(),
                Moniker::try_from("core/other/ambiguous_selector:thing/test").unwrap(),
            ],
        };
        // Main should return an error

        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            set_severity: vec![OneOrMany::One(
                parse_log_interest_selector("ambiguous_selector#INFO").unwrap(),
            )],
            ..LogCommand::default()
        };
        let mut set_interest_result = None;

        let mut scheduler = FuturesUnordered::new();
        scheduler.push(Either::Left(async {
            set_interest_result = Some(cmd.maybe_set_interest(&settings_proxy, &getter).await);
            drop(settings_proxy);
        }));
        scheduler.push(Either::Right(async {
            let request = settings_server.into_stream().next().await;
            // The channel should be closed without sending any requests.
            assert_matches!(request, None);
        }));
        while scheduler.next().await.is_some() {}
        drop(scheduler);

        let error = format!("{}", set_interest_result.unwrap().unwrap_err());

        const EXPECTED_INTEREST_ERROR: &str = r#"WARN: One or more of your selectors appears to be ambiguous
and may not match any components on your system.

If this is unintentional you can explicitly match using the
following command:

ffx log \
	--set-severity core/some/ambiguous_selector\\:thing/test#INFO \
	--set-severity core/other/ambiguous_selector\\:thing/test#INFO

If this is intentional, you can disable this with
ffx log --force-set-severity.
"#;
        assert_eq!(error, EXPECTED_INTEREST_ERROR);
    }

    #[fuchsia::test]
    async fn logger_translates_selector_if_one_match() {
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            set_severity: vec![OneOrMany::One(
                parse_log_interest_selector("ambiguous_selector#INFO").unwrap(),
            )],
            ..LogCommand::default()
        };
        let mut set_interest_result = None;
        let getter = FakeInstanceGetter {
            expected_selector: Some("ambiguous_selector".into()),
            output: vec![Moniker::try_from("core/some/ambiguous_selector").unwrap()],
        };
        let mut scheduler = FuturesUnordered::new();
        let (settings_proxy, settings_server) = create_proxy::<LogSettingsMarker>();
        scheduler.push(Either::Left(async {
            set_interest_result = Some(cmd.maybe_set_interest(&settings_proxy, &getter).await);
            drop(settings_proxy);
        }));
        scheduler.push(Either::Right(async {
            let request = settings_server.into_stream().next().await;
            let (payload, responder) = assert_matches!(
                request,
                Some(Ok(LogSettingsRequest::SetComponentInterest { payload, responder })) =>
                (payload, responder)
            );
            responder.send().unwrap();
            assert_eq!(
                payload.selectors,
                Some(vec![
                    parse_log_interest_selector("core/some/ambiguous_selector#INFO").unwrap()
                ])
            );
        }));
        while scheduler.next().await.is_some() {}
        drop(scheduler);
        assert_matches!(set_interest_result, Some(Ok(())));
    }

    #[fuchsia::test]
    async fn logger_uses_specified_selectors_if_no_results_returned() {
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            set_severity: vec![OneOrMany::One(
                parse_log_interest_selector("core/something/a:b/elements:main/otherstuff:*#DEBUG")
                    .unwrap(),
            )],
            ..LogCommand::default()
        };
        let mut set_interest_result = None;
        let getter = FakeInstanceGetter {
            expected_selector: Some("core/something/a:b/elements:main/otherstuff:*#DEBUG".into()),
            output: vec![],
        };
        let scheduler = FuturesUnordered::new();
        let (settings_proxy, settings_server) = create_proxy::<LogSettingsMarker>();
        scheduler.push(Either::Left(async {
            set_interest_result = Some(cmd.maybe_set_interest(&settings_proxy, &getter).await);
            drop(settings_proxy);
        }));
        scheduler.push(Either::Right(async {
            let request = settings_server.into_stream().next().await;
            let (payload, responder) = assert_matches!(
                request,
                Some(Ok(LogSettingsRequest::SetComponentInterest { payload, responder })) =>
                (payload, responder)
            );
            responder.send().unwrap();
            assert_eq!(
                payload.selectors,
                Some(vec![
                    parse_log_interest_selector(
                        "core/something/a:b/elements:main/otherstuff:*#DEBUG"
                    )
                    .unwrap()
                ])
            );
        }));
        scheduler.map(|_| Ok(())).forward(futures::sink::drain()).await.unwrap();
        assert_matches!(set_interest_result, Some(Ok(())));
    }

    #[fuchsia::test]
    async fn logger_prints_ignores_ambiguity_if_force_set_severity_is_used() {
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::SetSeverity(SetSeverityCommand {
                no_persist: true,
                interest_selector: vec![OneOrMany::One(
                    parse_log_interest_selector("ambiguous_selector#INFO").unwrap(),
                )],
                force: true,
            })),
            ..LogCommand::default()
        };
        let getter = FakeInstanceGetter {
            expected_selector: Some("ambiguous_selector".into()),
            output: vec![
                Moniker::try_from("core/some/ambiguous_selector:thing/test").unwrap(),
                Moniker::try_from("core/other/ambiguous_selector:thing/test").unwrap(),
            ],
        };
        let mut set_interest_result = None;
        let mut scheduler = FuturesUnordered::new();
        let (settings_proxy, settings_server) = create_proxy::<LogSettingsMarker>();
        scheduler.push(Either::Left(async {
            set_interest_result = Some(cmd.maybe_set_interest(&settings_proxy, &getter).await);
            drop(settings_proxy);
        }));
        scheduler.push(Either::Right(async {
            let request = settings_server.into_stream().next().await;
            let (payload, responder) = assert_matches!(
                request,
                Some(Ok(LogSettingsRequest::SetComponentInterest { payload, responder })) =>
                (payload, responder)
            );
            responder.send().unwrap();
            assert_eq!(
                payload.selectors,
                Some(vec![parse_log_interest_selector("ambiguous_selector#INFO").unwrap()])
            );
        }));
        while scheduler.next().await.is_some() {}
        drop(scheduler);
        assert_matches!(set_interest_result, Some(Ok(())));
    }

    #[fuchsia::test]
    async fn logger_prints_ignores_ambiguity_if_force_set_severity_is_used_persistent() {
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::SetSeverity(SetSeverityCommand {
                no_persist: false,
                interest_selector: vec![log_socket_stream::OneOrMany::One(
                    parse_log_interest_selector("ambiguous_selector#INFO").unwrap(),
                )],
                force: true,
            })),
            ..LogCommand::default()
        };
        let getter = FakeInstanceGetter {
            expected_selector: Some("ambiguous_selector".into()),
            output: vec![
                Moniker::try_from("core/some/ambiguous_selector:thing/test").unwrap(),
                Moniker::try_from("core/other/ambiguous_selector:thing/test").unwrap(),
            ],
        };
        let mut set_interest_result = None;
        let mut scheduler = FuturesUnordered::new();
        let (settings_proxy, settings_server) = create_proxy::<LogSettingsMarker>();
        scheduler.push(Either::Left(async {
            set_interest_result = Some(cmd.maybe_set_interest(&settings_proxy, &getter).await);
            drop(settings_proxy);
        }));
        scheduler.push(Either::Right(async {
            let request = settings_server.into_stream().next().await;
            let (payload, responder) = assert_matches!(
                request,
                Some(Ok(LogSettingsRequest::SetComponentInterest { payload, responder })) =>
                (payload, responder)
            );
            responder.send().unwrap();
            assert_eq!(
                payload.selectors,
                Some(vec![parse_log_interest_selector("ambiguous_selector#INFO").unwrap()])
            );
            assert_eq!(payload.persist, Some(true));
        }));
        while scheduler.next().await.is_some() {}
        drop(scheduler);
        assert_matches!(set_interest_result, Some(Ok(())));
    }

    #[fuchsia::test]
    async fn logger_prints_ignores_ambiguity_if_machine_output_is_used() {
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            set_severity: vec![OneOrMany::One(
                parse_log_interest_selector("ambiguous_selector#INFO").unwrap(),
            )],
            filters: LogFilterArgs { force_set_severity: true, ..LogFilterArgs::default() },
        };
        let getter = FakeInstanceGetter {
            expected_selector: Some("ambiguous_selector".into()),
            output: vec![
                Moniker::try_from("core/some/collection:thing/test").unwrap(),
                Moniker::try_from("core/other/collection:thing/test").unwrap(),
            ],
        };
        let mut set_interest_result = None;
        let mut scheduler = FuturesUnordered::new();
        let (settings_proxy, settings_server) = create_proxy::<LogSettingsMarker>();
        scheduler.push(Either::Left(async {
            set_interest_result = Some(cmd.maybe_set_interest(&settings_proxy, &getter).await);
            drop(settings_proxy);
        }));
        scheduler.push(Either::Right(async {
            let request = settings_server.into_stream().next().await;
            let (payload, responder) = assert_matches!(
                request,
                Some(Ok(LogSettingsRequest::SetComponentInterest { payload, responder })) =>
                (payload, responder)
            );
            responder.send().unwrap();
            assert_eq!(
                payload.selectors,
                Some(vec![parse_log_interest_selector("ambiguous_selector#INFO").unwrap()])
            );
        }));
        while scheduler.next().await.is_some() {}
        drop(scheduler);
        assert_matches!(set_interest_result, Some(Ok(())));
    }
    #[test]
    fn test_parse_selector() {
        assert_eq!(
            log_interest_selector("core/audio#DEBUG").unwrap(),
            OneOrMany::One(parse_log_interest_selector("core/audio#DEBUG").unwrap())
        );
    }

    #[test]
    fn test_parse_selector_with_commas() {
        assert_eq!(
            log_interest_selector("core/audio#DEBUG,bootstrap/archivist#TRACE").unwrap(),
            OneOrMany::Many(vec![
                parse_log_interest_selector("core/audio#DEBUG").unwrap(),
                parse_log_interest_selector("bootstrap/archivist#TRACE").unwrap()
            ])
        );
    }

    #[test]
    fn test_parse_time() {
        assert!(parse_time("now").unwrap().is_now);
        let date_string = "04/20/2020";
        let res = parse_time(date_string).unwrap();
        assert!(!res.is_now);
        assert_eq!(
            res.date_naive(),
            parse_date_string(date_string, Local::now(), Dialect::Us).unwrap().date_naive()
        );
    }

    #[test]
    fn test_log_error_is_broken_pipe() {
        assert!(
            LogError::IOError(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe"))
                .is_broken_pipe()
        );
        assert!(
            LogError::UnknownError(anyhow::Error::new(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "broken pipe"
            )))
            .is_broken_pipe()
        );
        assert!(!LogError::IOError(std::io::Error::other("other")).is_broken_pipe());
        assert!(!LogError::NoBootTimestamp.is_broken_pipe());
    }

    #[test]
    fn test_raw_dump_command_into_filter_args() {
        let raw_dump = RawDumpCommand {
            tail: Some(42),
            filter: vec!["foo".to_string()],
            severity: Some(Severity::Warn),
            hide_tags: true,
            ..RawDumpCommand::default()
        };
        let filters = raw_dump.into_filter_args();
        assert_eq!(filters.filter, vec!["foo".to_string()]);
        assert_eq!(filters.severity, Some(Severity::Warn));
        assert!(filters.hide_tags);
    }

    #[test]
    fn test_raw_watch_command_into_filter_args() {
        let raw_watch = RawWatchCommand {
            tag: vec!["my_tag".to_string()],
            no_color: true,
            ..RawWatchCommand::default()
        };
        let filters = raw_watch.into_filter_args();
        assert_eq!(filters.tag, vec!["my_tag".to_string()]);
        assert!(filters.no_color);
    }

    #[test]
    fn test_merge_subcommand_dump_overlay() {
        let cmd = LogCommand::from_args(
            &["log"],
            &[
                "--severity",
                "info",
                "--filter",
                "top_filter",
                "dump",
                "--tail",
                "10",
                "--severity",
                "debug",
                "--filter",
                "sub_filter",
            ],
        )
        .unwrap();

        assert_eq!(cmd.severity(), Severity::Debug);
        assert_eq!(cmd.filter(), &["top_filter".to_string(), "sub_filter".to_string()]);
        if let Some(LogSubCommand::Dump(dump)) = &cmd.sub_command {
            assert_eq!(dump.tail, Some(10));
        } else {
            panic!("expected Dump subcommand");
        }
    }

    #[test]
    fn test_merge_subcommand_watch_overlay() {
        let cmd = LogCommand::from_args(
            &["log"],
            &["--tag", "t1", "watch", "--tag", "t2", "--hide-tags"],
        )
        .unwrap();

        assert_eq!(cmd.tag(), &["t1".to_string(), "t2".to_string()]);
        assert!(cmd.hide_tags());
    }

    #[test]
    fn test_merge_subcommand_validate_warnings_on_merged_moniker() {
        let mut cmd =
            LogCommand::from_args(&["log"], &["dump", "--moniker", "my_moniker"]).unwrap();

        assert_eq!(cmd.moniker(), &["my_moniker".to_string()]);
        let warnings = cmd.validate_cmd_flags_with_warnings().unwrap();
        assert!(!warnings.is_empty());
        assert_eq!(cmd.component(), &["my_moniker".to_string()]);
        assert!(cmd.moniker().is_empty());
    }

    #[cfg(not(target_os = "fuchsia"))]
    #[fuchsia::test]
    async fn test_symbolize_accessor() {
        let cmd_default = LogCommand::from_args(&["ffx", "log"], &["dump"]).unwrap();
        assert_eq!(cmd_default.symbolize(), SymbolizeMode::Pretty);

        let cmd_custom =
            LogCommand::from_args(&["ffx", "log"], &["dump", "--symbolize", "off"]).unwrap();
        assert_eq!(cmd_custom.symbolize(), SymbolizeMode::Off);
    }

    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    async fn test_encoding_accessor() {
        let cmd_default = LogCommand::from_args(&["ffx", "log"], &["dump"]).unwrap();
        assert_eq!(cmd_default.encoding(), LogEncoding::Json);

        let cmd_custom =
            LogCommand::from_args(&["ffx", "log"], &["dump", "--encoding", "fxt"]).unwrap();
        assert_eq!(cmd_custom.encoding(), LogEncoding::Fxt);
    }

    #[fuchsia::test]
    async fn test_subcommand_moniker_deprecation() {
        let mut cmd =
            LogCommand::from_args(&["ffx", "log"], &["dump", "--moniker", "foo"]).unwrap();
        assert_eq!(cmd.filters.moniker, vec!["foo"]);

        let warnings = cmd.validate_cmd_flags_with_warnings().unwrap();
        assert_eq!(warnings, vec!["WARNING: --moniker is deprecated, use --component instead"]);
        assert_eq!(cmd.filters.component, vec!["foo"]);
        assert!(cmd.filters.moniker.is_empty());

        let mut cmd_both = LogCommand::from_args(
            &["ffx", "log"],
            &["dump", "--moniker", "foo", "--component", "bar"],
        )
        .unwrap();
        let warnings_both = cmd_both.validate_cmd_flags_with_warnings().unwrap();
        assert_eq!(
            warnings_both,
            vec![
                "WARNING: --moniker is deprecated, use --component instead",
                "WARNING: ignoring --moniker arguments in favor of --component"
            ]
        );
        assert_eq!(cmd_both.filters.component, vec!["bar"]);
    }

    #[test]
    fn test_subcommand_enum_overrides() {
        let cmd =
            LogCommand::from_args(&["log"], &["--severity", "warn", "dump", "--severity", "info"])
                .unwrap();
        assert_eq!(cmd.severity(), Severity::Info);
        assert_eq!(cmd.filters.severity, Some(Severity::Info));
    }

    #[test]
    fn test_merge_subcommand_no_subcommand_or_set_severity() {
        let initial_filters = LogFilterArgs {
            severity: Some(Severity::Warn),
            filter: vec!["test_filter".into()],
            no_color: true,
            ..Default::default()
        };
        let mut cmd_none = LogCommand {
            sub_command: None,
            set_severity: vec![],
            filters: initial_filters.clone(),
        };
        cmd_none.merge_subcommand();
        assert_eq!(cmd_none.filters, initial_filters);
        assert_eq!(cmd_none.sub_command, None);

        let set_severity_cmd =
            SetSeverityCommand { no_persist: true, force: true, interest_selector: vec![] };
        let mut cmd_set_sev = LogCommand {
            sub_command: Some(LogSubCommand::SetSeverity(set_severity_cmd.clone())),
            set_severity: vec![],
            filters: initial_filters.clone(),
        };
        cmd_set_sev.merge_subcommand();
        assert_eq!(cmd_set_sev.filters, initial_filters);
        assert_eq!(cmd_set_sev.sub_command, Some(LogSubCommand::SetSeverity(set_severity_cmd)));
    }

    #[test]
    fn test_macro_raw_command_conversions() {
        #[cfg(not(target_os = "fuchsia"))]
        let raw_dump = RawDumpCommand::from_args(
            &["dump"],
            &[
                "--tail",
                "50",
                "--severity",
                "error",
                "--filter",
                "dump_filter",
                "--symbolize",
                "off",
                "--disable-reconnect",
            ],
        )
        .unwrap();

        #[cfg(target_os = "fuchsia")]
        let raw_dump = RawDumpCommand::from_args(
            &["dump"],
            &[
                "--tail",
                "50",
                "--severity",
                "error",
                "--filter",
                "dump_filter",
                "--encoding",
                "fxt",
                "--json",
            ],
        )
        .unwrap();

        assert_eq!(raw_dump.tail, Some(50));
        let dump_filters = raw_dump.into_filter_args();
        assert_eq!(dump_filters.severity, Some(Severity::Error));
        assert_eq!(dump_filters.filter, vec!["dump_filter"]);
        #[cfg(not(target_os = "fuchsia"))]
        {
            assert_eq!(dump_filters.symbolize, Some(SymbolizeMode::Off));
            assert!(dump_filters.disable_reconnect);
        }
        #[cfg(target_os = "fuchsia")]
        {
            assert_eq!(dump_filters.encoding, Some(LogEncoding::Fxt));
            assert!(dump_filters.json);
        }

        #[cfg(not(target_os = "fuchsia"))]
        let raw_watch = RawWatchCommand::from_args(
            &["watch"],
            &["--severity", "warn", "--tag", "watch_tag", "--symbolize", "classic"],
        )
        .unwrap();

        #[cfg(target_os = "fuchsia")]
        let raw_watch = RawWatchCommand::from_args(
            &["watch"],
            &["--severity", "warn", "--tag", "watch_tag", "--encoding", "json"],
        )
        .unwrap();

        let watch_filters = raw_watch.into_filter_args();
        assert_eq!(watch_filters.severity, Some(Severity::Warn));
        assert_eq!(watch_filters.tag, vec!["watch_tag"]);
        #[cfg(not(target_os = "fuchsia"))]
        assert_eq!(watch_filters.symbolize, Some(SymbolizeMode::Classic));
        #[cfg(target_os = "fuchsia")]
        assert_eq!(watch_filters.encoding, Some(LogEncoding::Json));
    }

    #[test]
    fn test_dump_help_text() {
        let help_err = RawDumpCommand::from_args(&["dump"], &["--help"]).unwrap_err();
        let help_output = help_err.output;
        assert!(help_output.contains("--tail"), "dump help should include --tail");
        assert!(help_output.contains("--severity"), "dump help should include --severity");
        assert!(help_output.contains("--filter"), "dump help should include --filter");
        assert!(help_output.contains("--since"), "dump help should include --since");
        assert!(help_output.contains("--until"), "dump help should include --until");
    }

    #[test]
    fn test_watch_help_text() {
        let help_err = RawWatchCommand::from_args(&["watch"], &["--help"]).unwrap_err();
        let help_output = help_err.output;
        assert!(help_output.contains("--severity"), "watch help should include --severity");
        assert!(help_output.contains("--filter"), "watch help should include --filter");
        assert!(help_output.contains("--since"), "watch help should include --since");
        assert!(help_output.contains("--until"), "watch help should include --until");
    }

    #[test]
    fn test_option_field_subcommand_overlay() {
        // 1. Subcommand `Some` overrides top-level `None`
        let cmd_sub_some = LogCommand::from_args(
            &["log"],
            &[
                "dump",
                "--pid",
                "123",
                "--tid",
                "456",
                "--since",
                "10m ago",
                "--until",
                "5m ago",
                "--exclude-regex-file",
                "/path/sub.txt",
            ],
        )
        .unwrap();

        assert_eq!(cmd_sub_some.pid(), Some(123));
        assert_eq!(cmd_sub_some.tid(), Some(456));
        assert!(cmd_sub_some.since().is_some());
        assert!(cmd_sub_some.until().is_some());
        assert_eq!(cmd_sub_some.exclude_regex_file(), Some("/path/sub.txt"));

        // 2. Subcommand `Some` overrides top-level `Some`
        let cmd_sub_override = LogCommand::from_args(
            &["log"],
            &[
                "--pid",
                "11",
                "--tid",
                "22",
                "--since",
                "20m ago",
                "--until",
                "15m ago",
                "--exclude-regex-file",
                "/path/top.txt",
                "dump",
                "--pid",
                "123",
                "--tid",
                "456",
                "--since",
                "10m ago",
                "--until",
                "5m ago",
                "--exclude-regex-file",
                "/path/sub.txt",
            ],
        )
        .unwrap();

        assert_eq!(cmd_sub_override.pid(), Some(123));
        assert_eq!(cmd_sub_override.tid(), Some(456));
        assert_eq!(cmd_sub_override.exclude_regex_file(), Some("/path/sub.txt"));
        assert!(cmd_sub_override.since().is_some());
        assert!(cmd_sub_override.until().is_some());

        // 3. Subcommand `None` preserves top-level `Some`
        let cmd_top_some = LogCommand::from_args(
            &["log"],
            &[
                "--pid",
                "11",
                "--tid",
                "22",
                "--since",
                "20m ago",
                "--until",
                "15m ago",
                "--exclude-regex-file",
                "/path/top.txt",
                "dump",
            ],
        )
        .unwrap();

        assert_eq!(cmd_top_some.pid(), Some(11));
        assert_eq!(cmd_top_some.tid(), Some(22));
        assert_eq!(cmd_top_some.exclude_regex_file(), Some("/path/top.txt"));
        assert!(cmd_top_some.since().is_some());
        assert!(cmd_top_some.until().is_some());
    }

    #[test]
    fn test_enum_subcommand_overlay_and_preservation() {
        // Top-level non-default enum preserved when subcommand uses defaults
        #[cfg(not(target_os = "fuchsia"))]
        let cmd = LogCommand::from_args(
            &["log"],
            &["--severity", "warn", "--clock", "local", "--symbolize", "off", "dump"],
        )
        .unwrap();

        #[cfg(target_os = "fuchsia")]
        let cmd = LogCommand::from_args(
            &["log"],
            &["--severity", "warn", "--clock", "local", "--encoding", "fxt", "dump"],
        )
        .unwrap();

        assert_eq!(cmd.severity(), Severity::Warn);
        assert_eq!(cmd.clock(), TimeFormat::Local);
        #[cfg(not(target_os = "fuchsia"))]
        assert_eq!(cmd.symbolize(), SymbolizeMode::Off);
        #[cfg(target_os = "fuchsia")]
        assert_eq!(cmd.encoding(), LogEncoding::Fxt);

        // Subcommand non-default enum overrides top-level non-default enum
        #[cfg(not(target_os = "fuchsia"))]
        let cmd_override = LogCommand::from_args(
            &["log"],
            &[
                "--severity",
                "warn",
                "--clock",
                "local",
                "--symbolize",
                "off",
                "dump",
                "--severity",
                "error",
                "--clock",
                "utc",
                "--symbolize",
                "classic",
            ],
        )
        .unwrap();

        #[cfg(target_os = "fuchsia")]
        let cmd_override = LogCommand::from_args(
            &["log"],
            &[
                "--severity",
                "warn",
                "--clock",
                "local",
                "dump",
                "--severity",
                "error",
                "--clock",
                "utc",
                "--encoding",
                "fxt",
            ],
        )
        .unwrap();

        assert_eq!(cmd_override.severity(), Severity::Error);
        assert_eq!(cmd_override.clock(), TimeFormat::Utc);
        #[cfg(not(target_os = "fuchsia"))]
        assert_eq!(cmd_override.symbolize(), SymbolizeMode::Classic);
        #[cfg(target_os = "fuchsia")]
        assert_eq!(cmd_override.encoding(), LogEncoding::Fxt);
    }

    #[test]
    fn test_boolean_flag_cumulative_or() {
        let cmd = LogCommand::from_args(
            &["log"],
            &["--no-color", "--case-sensitive", "dump", "--hide-file", "--kernel"],
        )
        .unwrap();

        assert!(cmd.no_color());
        assert!(cmd.case_sensitive());
        assert!(cmd.hide_file());
        assert!(cmd.kernel());
        assert!(!cmd.hide_tags());
        assert!(!cmd.show_metadata());

        // Verify OR combining when set on both top-level and subcommand
        let cmd_both = LogCommand::from_args(&["log"], &["--kernel", "dump", "--kernel"]).unwrap();

        assert!(cmd_both.kernel());
    }

    #[test]
    fn test_log_command_accessors() {
        // Test default state
        let default_cmd = LogCommand::default();
        assert_eq!(default_cmd.pid(), None);
        assert_eq!(default_cmd.tid(), None);
        assert!(!default_cmd.hide_file());
        #[cfg(target_os = "fuchsia")]
        assert!(!default_cmd.json());
        assert!(default_cmd.exclude().is_empty());
        assert!(default_cmd.exclude_regex().is_empty());
        assert_eq!(default_cmd.since(), None);
        assert_eq!(default_cmd.until(), None);
        assert!(!default_cmd.kernel());
        assert!(!default_cmd.case_sensitive());
        #[cfg(not(target_os = "fuchsia"))]
        assert!(!default_cmd.disable_reconnect());

        // Test with non-default values set via CLI args
        #[cfg(not(target_os = "fuchsia"))]
        let args = &[
            "--pid",
            "123",
            "--tid",
            "456",
            "--hide-file",
            "--exclude",
            "bad_tag",
            "--exclude",
            "other_tag",
            "--exclude-regex",
            "^error.*",
            "--since",
            "10m ago",
            "--until",
            "5m ago",
            "--kernel",
            "--case-sensitive",
            "--disable-reconnect",
        ];

        #[cfg(target_os = "fuchsia")]
        let args = &[
            "--pid",
            "123",
            "--tid",
            "456",
            "--hide-file",
            "--exclude",
            "bad_tag",
            "--exclude",
            "other_tag",
            "--exclude-regex",
            "^error.*",
            "--since",
            "10m ago",
            "--until",
            "5m ago",
            "--kernel",
            "--case-sensitive",
            "--json",
        ];

        let cmd = LogCommand::from_args(&["log"], args).unwrap();

        assert_eq!(cmd.pid(), Some(123));
        assert_eq!(cmd.tid(), Some(456));
        assert!(cmd.hide_file());
        #[cfg(target_os = "fuchsia")]
        assert!(cmd.json());
        assert_eq!(cmd.exclude(), &["bad_tag".to_string(), "other_tag".to_string()]);
        assert_eq!(cmd.exclude_regex(), &["^error.*".to_string()]);
        assert!(cmd.since().is_some());
        assert!(cmd.until().is_some());
        assert!(cmd.kernel());
        assert!(cmd.case_sensitive());
        #[cfg(not(target_os = "fuchsia"))]
        assert!(cmd.disable_reconnect());
    }

    #[test]
    fn test_dump_diagnostics_unsupported_flags() {
        let err = LogCommand::from_args(&["ffx", "log"], &["--dump"]).unwrap_err();
        assert!(err.status.is_err());
        assert!(err.output.contains("Unrecognized argument: --dump"));
        assert!(err.output.contains(
            "Note: 'dump' is a sub-command of 'ffx log', not a flag. Use: ffx log dump [options]"
        ));

        let err = LogCommand::from_args(&["ffx", "log"], &["dump", "--limit", "100"]).unwrap_err();
        assert!(err.status.is_err());
        assert!(err.output.contains("Unrecognized argument: --limit"));
        assert!(err.output.contains(
            "Note: 'ffx log dump' does not take --limit. To limit output lines, use '--tail <count>' or pipe to head/tail."
        ));

        let err =
            LogCommand::from_args(&["ffx", "log"], &["dump", "--limit", "100", "--limit", "200"])
                .unwrap_err();
        assert!(err.status.is_err());
        assert_eq!(err.output.matches("Note: 'ffx log dump' does not take --limit.").count(), 1);

        let err = LogCommand::from_args(&["ffx", "log"], &["dump", "--grep", "hello"]).unwrap_err();
        assert!(err.status.is_err());
        assert!(err.output.contains("Unrecognized argument: --grep"));
        assert!(err.output.contains(
            "Note: 'ffx log dump' does not take --grep. To filter log snapshot output, use '--filter <pattern>' or pipe to grep: ffx log dump | grep <pattern>"
        ));
    }
}
