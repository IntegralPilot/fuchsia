// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use fdomain_fuchsia_tracing::BufferingMode;
use fdomain_fuchsia_tracing_controller::{Action, Trigger};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "trace",
    description = "Collect, aggregate, and visualize traces",
    note = "Tracing is a tool that allows you to collect, aggregate, and visualize diagnostic
tracing information from both userspace processes and the Zircon kernel on a Fuchsia device.

For a full tutorial, visit https://fuchsia.dev/fuchsia-src/development/sdk/ffx/record-traces.
",
    example = "

[Quick Start]:

    $ ffx trace start [--duration <SECONDS> ]

This will record a trace using the default categories, which are suitable for getting a high
level overview.

This will produce a file `trace.fxt` which can be uploaded to ui.perfetto.dev to be viewed."
)]
pub struct TraceCommand {
    #[argh(subcommand)]
    pub sub_cmd: TraceSubCommand,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
#[argh(subcommand)]
pub enum TraceSubCommand {
    ListCategories(ListCategories),
    ListProviders(ListProviders),
    ListCategoryGroups(ListCategoryGroups),
    Start(Start),
    Stop(Stop),
    Status(Status),
    Symbolize(Symbolize),
    Upload(Upload),
    // More commands including `record` and `convert` to follow.
}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
/// List the target's known trace categories.
#[argh(subcommand, name = "list-categories")]
pub struct ListCategories {}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
/// List the target's trace providers.
#[argh(subcommand, name = "list-providers")]
pub struct ListProviders {}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
/// List the builtin and custom category groups.
#[argh(subcommand, name = "list-category-groups")]
pub struct ListCategoryGroups {}

// Work around argh's handling of Vec.  Listing categories as a comma
// separated list of values rather than a repeated keyed option
// is much more concise when dealing with a large set of categories.
pub type TraceCategories = Vec<String>;

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
/// Gets status of all running traces.
#[argh(subcommand, name = "status")]
pub struct Status {}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
/// Symbolizes the provided ordinal
#[argh(subcommand, name = "symbolize")]
pub struct Symbolize {
    /// FIDL ordinal to be symbolized.
    #[argh(option)]
    pub ordinal: Option<u64>,

    /// trace file to be symbolized.
    #[argh(option)]
    pub fxt: Option<String>,

    /// symbolized file path. Defaults to overwriting the input file.
    #[argh(option)]
    pub outfile: Option<String>,

    /// additional IR files to use for symbolization. Path provided must be
    /// absolute or relative to $FUCHSIA_BUILD_DIR. If not provided, only the
    /// files listed in $FUCHSIA_BUILD_DIR/all_fidl_json.txt will be checked.
    #[argh(option)]
    pub ir_path: Vec<String>,

    /// retain raw FIDL-related trace arguments after symbolization. Defaults to false.
    #[argh(switch)]
    pub retain_raw_fidl: bool,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug, Default)]
/// Stops an active running trace.
#[argh(subcommand, name = "stop")]
pub struct Stop {
    /// name of output trace file. Relative or absolute paths are both supported.
    /// If this is not supplied (the default), then the default target will be
    /// used as the method to stop the trace.
    #[argh(option)]
    pub output: Option<String>,

    /// increase verbosity of command output. Defaults to false.
    /// Enabling this prints stats from trace providers including the number
    /// of records dropped, wrapped buffers count, % of durable buffer used and
    /// non durable bytes written.
    #[argh(switch, short = 'v')]
    pub verbose: bool,

    /// prevent symbolization of the trace file. Defaults to false.
    /// If the 'kernel:ipc' category is not included in tracing, this flag does
    /// nothing. Otherwise this flag will prevent the symbolization of the
    /// kernel IPC traces, leaving only the ordinals in the trace.
    #[argh(switch)]
    pub no_symbolize: bool,

    /// prevent checking the trace data when stopping tracing. Defaults to false.
    /// If this flag is enabled, trace result analysis will not be triggered.
    #[argh(switch)]
    pub no_verify_trace: bool,

    /// retain raw FIDL-related trace arguments after symbolization. Defaults to false.
    #[argh(switch)]
    pub retain_raw_fidl: bool,

    /// stop the currently running background trace session. Discard the trace data. Defaults to false.
    #[argh(switch)]
    pub abort: bool,

    /// upload the trace file to GCS and display the Perfetto Trace Viewer URL. Defaults to false.
    #[argh(switch)]
    pub upload: bool,

    /// GCS bucket to upload the trace file to.
    #[argh(option)]
    pub bucket: Option<String>,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug, Default)]
/// Upload a trace file to GCS and display the Perfetto Trace Viewer URL.
#[argh(subcommand, name = "upload")]
pub struct Upload {
    /// path to trace file (.fxt) to upload. Defaults to "trace.fxt".
    #[argh(positional)]
    pub trace_file: Option<String>,

    /// GCS bucket to upload the trace file to.
    #[argh(option)]
    pub bucket: Option<String>,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
/// Record a trace.
#[argh(
    subcommand,
    name = "start",
    example = "
[Quick Start]:

    $ ffx trace start [--duration <SECONDS> ]

This will record a trace using the default categories, which are suitable for getting a high
level overview.

This will produce a file `trace.fxt` which can be uploaded to ui.perfetto.dev to be viewed.

[Trace Various Categories]:

    $ ffx trace start --categories '#default,flatland:*,temp'

This will record a trace with all the default categories enabled, 'temp' events enabled, and any
event that starts with 'flatland:'


[Capturing a Sporadic Occurrence]

    $ ffx trace start --buffer-size 64 --buffering-mode circular

This will run the trace with the largest available buffer size, overwriting the old events with new
ones. When the event occurs press <enter> to stop the trace.


[Automatically Capturing a Sporadic Occurrence with a Trigger]

This approach allows you to automatically stop the trace when the event happens, but the traced
code must already be set up to write a trigger event.

    $ ffx trace start --buffer-size 64 --buffering-mode circular --trigger '<alert_name>:terminate'
",
    note = "For a full tutorial, visit https://fuchsia.dev/fuchsia-src/development/sdk/ffx/record-traces"
)]

pub struct Start {
    #[argh(option, default = "BufferingMode::Oneshot", from_str_fn(buffering_mode_from_str))]
    /// the buffering scheme to trace with. Defaults to "oneshot"
    ///
    /// oneshot:   Writes to the tracing buffer until it is full, then ignores all additional trace events.
    ///
    /// circular:  Writes to the tracing buffer until its is full, then new events will replace old events.
    ///
    /// streaming: Forwards tracing events to the trace manager as they arrive. Provides additional
    ///            buffer space with the trade off of some overhead due to occasional ipcs to send the
    ///            events to the trace manager during the trace.
    pub buffering_mode: BufferingMode,

    /// size of per-provider trace buffer in MB.  Defaults to 4.
    #[argh(option, default = "4")]
    pub buffer_size: u32,

    /// comma-separated list of categories to enable.  Defaults to "#default". Run `ffx config get
    /// trace.category_groups.default` to see what categories are included in #default.
    ///
    /// A trailing "*" may be used to indicate a prefix match. For example, "kernel*" includes any
    /// category that starts with kernel.
    ///
    /// A name prefixed with # indicates a category group that will be expanded from ffx config
    /// within the plugin *before* being sent to trace manager. A category group can either be
    /// added to global config by editing data/config.json in the ffx trace plugin, or by using ffx
    /// config set to add/edit a user configured category group. Available category groups can be
    /// discovered by running `ffx config get -s all trace.category_groups`
    ///
    /// A category may be limited to a specific trace provider using a '/'. For example
    /// --categories "scenic.cm/FrameDropped" will only enable the "FrameDropped" category for the
    /// scenic.cm trace provider and no other providers.
    ///
    /// Categories and category groups may be mixed, for example,
    ///
    ///     ffx trace start --categories #default,my_category
    ///
    /// Enables all the default categories as well as "my_category".
    #[argh(option, default = "vec![String::from(\"#default\")]", from_str_fn(parse_categories))]
    pub categories: TraceCategories,

    /// duration of trace capture in seconds.
    #[argh(option)]
    pub duration: Option<u32>,

    /// name of output trace file.  Defaults to trace.fxt. Cannot be used with --background.
    /// To specify the output file with background mode, pass --output to `ffx trace stop` instead."
    #[argh(option)]
    pub output: Option<String>,

    /// whether to run the trace in the background. Defaults to false,
    /// which means the trace will run in "interactive" mode.
    ///
    /// For a trace started in the background for a specified duration,
    /// symbolization is not done by default.
    #[argh(switch)]
    pub background: bool,

    /// increase verbosity of output on trace stop. Defaults to false.
    /// This is ignored if duration is not set.
    /// Enabling this prints stats from trace providers including the number
    /// of records dropped, wrapped buffers count, % of durable buffer used and
    /// non durable bytes written.
    #[argh(switch, short = 'v')]
    pub verbose: bool,

    /// send request to start tracing when the device reboots. This trace can be downloaded once
    /// the device is ready using `ffx trace stop`.
    #[argh(switch)]
    pub on_boot: bool,

    /// a trigger consists of an alert leading to an action. An alert
    /// is written into the code being traced, and an action here is
    /// what to do when the alert has happened. The list of supported
    /// actions are: 'terminate'.
    ///
    /// The expected format is "<alert>:<action>" ex:
    ///   -trigger "my_alert:terminate"
    ///
    /// This can be used with a duration, but keep in mind that this
    /// introduces a race between whether the alert leads to an
    /// action, and when the trace stops from the duration being
    /// reached.
    ///
    /// Triggers can only be used in the background outside of
    /// interactive mode.
    #[argh(option, from_str_fn(trigger_from_str))]
    pub trigger: Vec<Trigger>,

    /// prevent symbolization of the trace file. Defaults to false.
    /// If the 'kernel:ipc' category is not included in tracing, this flag does
    /// nothing. Otherwise this flag will prevent the symbolization of the
    /// kernel IPC traces, leaving the ordinals as is in the trace.
    ///
    /// This flag is only used if tracing is manually stopped.
    #[argh(switch)]
    pub no_symbolize: bool,

    /// prevent checking the trace data when stopping tracing. Defaults to false.
    /// If this flag is enabled, trace result analysis will not be triggered.
    #[argh(switch)]
    pub no_verify_trace: bool,

    /// retain raw FIDL-related trace arguments after symbolization. Defaults to false.
    #[argh(switch)]
    pub retain_raw_fidl: bool,

    /// disable zstd compression during transfer. Defaults to false.
    #[argh(switch)]
    pub nocompress: bool,

    /// upload the trace file to GCS and display the Perfetto Trace Viewer URL. Defaults to false.
    #[argh(switch)]
    pub upload: bool,

    /// GCS bucket to upload the trace file to.
    #[argh(option)]
    pub bucket: Option<String>,
}

impl Default for Start {
    fn default() -> Self {
        Self {
            buffering_mode: BufferingMode::Oneshot,
            buffer_size: 4,
            categories: vec![String::from("#default")],
            duration: None,
            output: None,
            background: false,
            verbose: false,
            on_boot: false,
            trigger: vec![],
            no_symbolize: false,
            no_verify_trace: false,
            retain_raw_fidl: false,
            nocompress: false,
            upload: false,
            bucket: None,
        }
    }
}

fn try_string_to_action(s: &str) -> Result<Action, String> {
    match s {
        "terminate" => Ok(Action::Terminate),
        e => Err(format!("unsupported action: \"{}\"", e)),
    }
}

fn trigger_from_str(val: &str) -> Result<Trigger, String> {
    let (alert, action) = val
        .split_once(":")
        .ok_or("triggers must be delimited by ':'. Example: '-trigger \"my_alert:terminate\"")?;
    let action = Some(try_string_to_action(action)?);
    let alert = Some(alert.to_owned());
    Ok(Trigger { alert, action, ..Default::default() })
}

fn buffering_mode_from_str(val: &str) -> Result<BufferingMode, String> {
    match val {
        "o" | "oneshot" => Ok(BufferingMode::Oneshot),
        "c" | "circular" => Ok(BufferingMode::Circular),
        "s" | "streaming" => Ok(BufferingMode::Streaming),
        _ => Err(
            "Unrecognized value. Possible values are \"oneshot\", \"circular\", or \"streaming\""
                .to_owned(),
        ),
    }
}

fn parse_categories(value: &str) -> Result<TraceCategories, String> {
    let mut cats = Vec::new();

    // Remove opening and closing double smart quote
    let value = value.replace(['“', '”'], "");

    if value.is_empty() {
        return Err("no categories specified".to_string());
    }

    for cat in value.split(",").map(|category| category.trim()) {
        if cat.is_empty() {
            return Err("empty category specified".to_string());
        }
        cats.push(String::from(cat));
    }

    Ok(cats)
}

#[cfg(test)]
mod tests {
    use super::*;
    const START_CMD_NAME: &'static [&'static str] = &["start"];

    #[test]
    fn test_parse_categories() {
        assert_eq!(parse_categories(&"a"), Ok(vec!["a".to_string()]));

        assert_eq!(
            parse_categories(&"a,b,c:d"),
            Ok(vec!["a".to_string(), "b".to_string(), "c:d".to_string()])
        );

        assert_eq!(parse_categories(&""), Err("no categories specified".to_string()));
        assert_eq!(parse_categories(&"a,,b"), Err("empty category specified".to_string()));
    }

    #[test]
    fn test_default_categories() {
        // This tests the code in a string that is passed as a default to argh. It is compile
        // checked because of the generated code, but this ensures that it is functionally correct.
        let cmd = Start::from_args(START_CMD_NAME, &[]).unwrap();
        assert_eq!(vec!["#default"], cmd.categories);
    }

    #[test]
    fn test_buffering_mode_from_str() {
        assert_eq!(buffering_mode_from_str("o"), Ok(BufferingMode::Oneshot));
        assert_eq!(buffering_mode_from_str("oneshot"), Ok(BufferingMode::Oneshot));
        assert_eq!(buffering_mode_from_str("c"), Ok(BufferingMode::Circular));
        assert_eq!(buffering_mode_from_str("circular"), Ok(BufferingMode::Circular));
        assert_eq!(buffering_mode_from_str("s"), Ok(BufferingMode::Streaming));
        assert_eq!(buffering_mode_from_str("streaming"), Ok(BufferingMode::Streaming));
    }

    #[test]
    fn test_unsupported_buffering_mode_from_str() {
        assert!(buffering_mode_from_str("roundrobin").is_err());
    }

    #[test]
    fn test_trigger_from_str_no_colon() {
        assert!(trigger_from_str("foobar").is_err());
    }

    #[test]
    fn test_trigger_from_str_invalid_action() {
        assert!(trigger_from_str("foobar:storp").is_err());
    }

    #[test]
    fn test_trigger_from_str_valid() {
        assert_eq!(
            trigger_from_str("foobar:terminate").unwrap(),
            Trigger {
                alert: Some("foobar".to_owned()),
                action: Some(Action::Terminate),
                ..Default::default()
            },
        );
    }

    #[test]
    fn test_stop_args_upload_and_bucket() {
        let stop = Stop::from_args(&["stop"], &["--upload", "--bucket", "test-bucket"]).unwrap();
        assert!(stop.upload);
        assert_eq!(stop.bucket.as_deref(), Some("test-bucket"));

        let default_stop = Stop::from_args(&["stop"], &[]).unwrap();
        assert!(!default_stop.upload);
        assert_eq!(default_stop.bucket, None);
    }

    #[test]
    fn test_upload_args() {
        let default_upload = Upload::from_args(&["upload"], &[]).unwrap();
        assert_eq!(default_upload.trace_file, None);
        assert_eq!(default_upload.bucket, None);

        let upload_with_file = Upload::from_args(&["upload"], &["some/path/trace.fxt"]).unwrap();
        assert_eq!(upload_with_file.trace_file.as_deref(), Some("some/path/trace.fxt"));
        assert_eq!(upload_with_file.bucket, None);

        let upload_with_bucket =
            Upload::from_args(&["upload"], &["--bucket", "test-bucket"]).unwrap();
        assert_eq!(upload_with_bucket.trace_file, None);
        assert_eq!(upload_with_bucket.bucket.as_deref(), Some("test-bucket"));

        let upload_full =
            Upload::from_args(&["upload"], &["trace.fxt", "--bucket", "test-bucket"]).unwrap();
        assert_eq!(upload_full.trace_file.as_deref(), Some("trace.fxt"));
        assert_eq!(upload_full.bucket.as_deref(), Some("test-bucket"));

        let upload_full_reversed =
            Upload::from_args(&["upload"], &["--bucket", "test-bucket", "trace.fxt"]).unwrap();
        assert_eq!(upload_full_reversed.trace_file.as_deref(), Some("trace.fxt"));
        assert_eq!(upload_full_reversed.bucket.as_deref(), Some("test-bucket"));
    }

    #[test]
    fn test_start_args_upload_and_bucket() {
        let start = Start::from_args(&["start"], &["--upload", "--bucket", "test-bucket"]).unwrap();
        assert!(start.upload);
        assert_eq!(start.bucket.as_deref(), Some("test-bucket"));

        let default_start = Start::from_args(&["start"], &[]).unwrap();
        assert!(!default_start.upload);
        assert_eq!(default_start.bucket, None);
    }
}
