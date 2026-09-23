// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use argh::FromArgs;
use cm_rust::DictionaryDecl;
use component_events::events::*;
use component_events::matcher::*;
use diagnostics_log_encoding::encode::EncoderOpts;
use diagnostics_log_encoding::parse::parse_record;
use diagnostics_log_encoding::{Argument, Record};
use diagnostics_log_validator_utils as utils;
use fidl::endpoints::RequestStream;
use fidl_fuchsia_diagnostics_types::{Interest, Severity};
use fidl_fuchsia_logger::{
    LogSinkMarker, LogSinkOnInitRequest, LogSinkRequest, LogSinkRequestStream,
    LogSinkWaitForInterestChangeResponder, MAX_DATAGRAM_LEN_BYTES,
};
use fidl_fuchsia_validate_logs::{
    self as fvalidate, LogSinkPuppetMarker, LogSinkPuppetProxy, MAX_ARG_NAME_LENGTH, MAX_ARGS,
    PuppetInfo, RecordSpec,
};
use fuchsia_async::Task;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmInstance, Ref, Route,
};
use futures::channel::mpsc;
use futures::prelude::*;
use log::*;
use proptest::collection::vec;
use proptest::prelude::{Arbitrary, Just, ProptestConfig, Strategy, TestCaseError, any};
use proptest::prop_oneof;
use proptest::test_runner::{Reason, RngAlgorithm, TestRng, TestRunner};
use proptest_derive::Arbitrary;
use ring_buffer::{self, RingBuffer};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::ops::Range;

/// Validate Log VMO formats written by 'puppet' programs controlled by
/// this Validator program.
#[derive(Debug, FromArgs)]
struct Opt {
    /// messages with this tag will be ignored. can be repeated.
    #[argh(option, long = "ignore-tag")]
    ignored_tags: Vec<String>,
    /// if true, invalid unicode will be generated in the initial puppet started message.
    #[argh(switch, long = "test-invalid-unicode")]
    test_invalid_unicode: bool,
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let Opt { ignored_tags, test_invalid_unicode } = argh::from_env();
    Puppet::launch(true, test_invalid_unicode, ignored_tags).await?.test().await
}

struct Puppet {
    start_time: zx::BootInstant,
    ring_buffer: ring_buffer::Reader,
    info: PuppetInfo,
    proxy: LogSinkPuppetProxy,
    _puppet_stopped_watchdog: Task<()>,
    new_file_line_rules: bool,
    ignored_tags: Vec<String>,
    _instance: RealmInstance,
}

async fn demux_fidl(
    stream: &mut LogSinkRequestStream,
) -> Result<(ring_buffer::Reader, LogSinkWaitForInterestChangeResponder), Error> {
    let ring_buffer = RingBuffer::create(zx::system_get_page_size() as usize * 32);

    stream
        .control_handle()
        .send_on_init(LogSinkOnInitRequest {
            buffer: Some(ring_buffer.new_iob_writer(1).unwrap().0),
            ..Default::default()
        })
        .unwrap();

    match stream.next().await.unwrap()? {
        LogSinkRequest::WaitForInterestChange { responder } => Ok((ring_buffer, responder)),
        LogSinkRequest::ConnectStructured { .. } => Err(anyhow!("ConnectStructured unexpected")),
        LogSinkRequest::_UnknownMethod { .. } => unreachable!(),
    }
}

struct ReadRecordArgs {
    new_file_line_rules: bool,
    override_file_line: bool,
}

async fn wait_for_severity(
    stream: &mut LogSinkRequestStream,
) -> Result<LogSinkWaitForInterestChangeResponder, Error> {
    match stream.next().await.unwrap()? {
        LogSinkRequest::WaitForInterestChange { responder } => Ok(responder),
        _ => Err(anyhow!("Unexpected FIDL message")),
    }
}

impl Puppet {
    async fn launch(
        new_file_line_rules: bool,
        test_invalid_unicode: bool,
        ignored_tags: Vec<String>,
    ) -> Result<Self, Error> {
        let builder = RealmBuilder::new().await?;
        let puppet = builder.add_child("puppet", "#meta/puppet.cm", ChildOptions::new()).await?;
        let (incoming_log_sink_requests_snd, mut incoming_log_sink_requests) = mpsc::unbounded();
        let mocks_server = builder
            .add_local_child(
                "mocks-server",
                move |handles: LocalComponentHandles| {
                    let snd = incoming_log_sink_requests_snd.clone();
                    Box::pin(async move {
                        let mut fs = ServiceFs::new();
                        fs.dir("svc").add_fidl_service(|s: LogSinkRequestStream| {
                            snd.unbounded_send(s).expect("sent log sink request stream");
                        });
                        fs.serve_connection(handles.outgoing_dir)?;
                        fs.collect::<()>().await;
                        Ok(())
                    })
                },
                ChildOptions::new(),
            )
            .await?;
        builder
            .add_capability(cm_rust::CapabilityDecl::Dictionary(DictionaryDecl {
                name: "diagnostics".parse().unwrap(),
                source_path: None,
            }))
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<LogSinkPuppetMarker>())
                    .from(&puppet)
                    .to(Ref::parent()),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<LogSinkMarker>())
                    .from(&mocks_server)
                    .to(Ref::dictionary(Ref::self_(), "diagnostics"))
                    .to(&puppet),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::dictionary("diagnostics"))
                    .from(Ref::self_())
                    .to(&puppet),
            )
            .await?;

        let mut event_stream = EventStream::open().await.unwrap(); // stopped

        let instance = builder.build().await.expect("create instance");
        let instance_child_name = instance.root.child_name().to_string();
        let _puppet_stopped_watchdog = Task::spawn(async move {
            EventMatcher::ok()
                .moniker(format!("./realm_builder:{instance_child_name}/puppet"))
                .wait::<Stopped>(&mut event_stream)
                .await
                .unwrap();
            let status = event_stream.next().await;
            panic!("puppet should not exit! status: {status:?}");
        });

        let start_time = zx::BootInstant::get();
        let proxy: LogSinkPuppetProxy = instance.root.connect_to_protocol_at_exposed_dir().unwrap();

        info!("Waiting for LogSink connection.");
        let mut stream = incoming_log_sink_requests.next().await.unwrap();

        info!("Using IOBuffer. Waiting for LogSink.WaitForInterestChange");
        let (ring_buffer, interest_listener) = demux_fidl(&mut stream).await?;

        info!("Requesting info from the puppet.");
        let info = proxy.get_info().await?;
        info!("Ensuring we received the init message.");
        let mut puppet = Self {
            ring_buffer,
            proxy,
            info,
            start_time,
            _puppet_stopped_watchdog,
            new_file_line_rules,
            ignored_tags,
            _instance: instance,
        };
        if test_invalid_unicode {
            info!("Testing invalid unicode.");
            assert_eq!(
                puppet
                    .read_record(ReadRecordArgs { new_file_line_rules, override_file_line: true })
                    .await?
                    .unwrap(),
                RecordAssertion::builder(&puppet.info, Severity::Info, new_file_line_rules)
                    .add_string("message", "Puppet started.�(")
                    .build(puppet.start_time..zx::BootInstant::get())
            );
        } else {
            info!("Reading regular record.");
            assert_eq!(
                puppet
                    .read_record(ReadRecordArgs { new_file_line_rules, override_file_line: true })
                    .await?
                    .unwrap(),
                RecordAssertion::builder(&puppet.info, Severity::Info, new_file_line_rules)
                    .add_string("message", "Puppet started.")
                    .build(puppet.start_time..zx::BootInstant::get())
            );
        }
        info!("Testing dot removal.");
        assert_dot_removal(&mut puppet, new_file_line_rules).await?;
        info!("Asserting interest listener");
        assert_interest_listener(&mut puppet, new_file_line_rules, &mut stream, interest_listener)
            .await?;
        Ok(puppet)
    }

    async fn read_record(&mut self, args: ReadRecordArgs) -> Result<Option<TestRecord>, Error> {
        loop {
            let buf = self.ring_buffer.read_message().await.unwrap().1;
            if buf.is_empty() {
                continue;
            }
            return TestRecord::parse(TestRecordParseArgs {
                buf: &buf,
                new_file_line_rules: args.new_file_line_rules,
                ignored_tags: &self.ignored_tags,
                override_file_line: args.override_file_line,
            });
        }
    }

    // For the CPP puppet it's necessary to strip out the TID from the comparison
    // as interest events happen outside the main thread due to HLCPP.
    async fn read_record_no_tid(&mut self, expected_tid: u64) -> Result<Option<TestRecord>, Error> {
        loop {
            let buf = self.ring_buffer.read_message().await.unwrap().1;
            if buf.is_empty() {
                continue;
            }
            let mut record = TestRecord::parse(TestRecordParseArgs {
                buf: &buf,
                new_file_line_rules: self.new_file_line_rules,
                ignored_tags: &self.ignored_tags,
                override_file_line: true,
            })?;
            if let Some(record) = record.as_mut() {
                record.arguments.remove("tid");
                record
                    .arguments
                    .insert("tid".to_string(), fvalidate::Value::UnsignedInt(expected_tid));
            }
            return Ok(record);
        }
    }

    async fn test(&mut self) -> Result<(), Error> {
        info!("Starting the LogSink iob test.");

        let mut runner = TestRunner::new_with_rng(
            ProptestConfig { cases: 2048, failure_persistence: None, ..Default::default() },
            // TODO(https://fxbug.dev/42145628) use TestRng::from_seed
            TestRng::deterministic_rng(RngAlgorithm::ChaCha),
        );

        let (tx_specs, rx_specs) = std::sync::mpsc::channel();
        let (tx_records, rx_records) = std::sync::mpsc::channel();

        // we spawn the testrunner onto a separate thread so that it can synchronously loop through
        // all the test cases and we can still use async to communicate with the puppet.
        // TODO(https://github.com/AltSysrq/proptest/pull/185) proptest async support
        let puppet_info = self.info.clone();
        let new_file_line_rules = self.new_file_line_rules; // needed for lambda capture
        let proptest_thread = std::thread::spawn(move || {
            runner
                .run(&TestVector::arbitrary(), |vector| {
                    let TestCycle { spec, mut assertion } =
                        TestCycle::new(&puppet_info, vector, new_file_line_rules);

                    tx_specs.send(spec).unwrap();
                    let (observed, valid_times) = rx_records.recv().unwrap();
                    let expected = assertion.build(valid_times);

                    if observed == expected {
                        Ok(())
                    } else {
                        Err(TestCaseError::Fail(
                            format!(
                                "unexpected test record: received {observed:?}, expected {expected:?}"
                            )
                            .into(),
                        ))
                    }
                })
                .unwrap();
        });

        for spec in rx_specs {
            let result = self.run_spec(spec, self.new_file_line_rules).await?;
            tx_records.send(result)?;
        }

        proptest_thread.join().unwrap();
        info!("Tested LogSink successfully.");
        Ok(())
    }

    async fn run_spec(
        &mut self,
        spec: RecordSpec,
        new_file_line_rules: bool,
    ) -> Result<(TestRecord, Range<zx::BootInstant>), Error> {
        let before = zx::BootInstant::get();
        self.proxy.emit_log(&spec).await?;
        let after = zx::BootInstant::get();

        // read until we get to a non-ignored record
        let record = loop {
            if let Some(r) = self
                .read_record(ReadRecordArgs { new_file_line_rules, override_file_line: true })
                .await?
            {
                break r;
            };
        };

        Ok((record, before..after))
    }
}

fn severity_to_string(severity: Severity) -> String {
    match severity {
        Severity::Trace => "Trace".to_string(),
        Severity::Debug => "Debug".to_string(),
        Severity::Info => "Info".to_string(),
        Severity::Warn => "Warn".to_string(),
        Severity::Error => "Error".to_string(),
        Severity::Fatal => "Fatal".to_string(),
        Severity::__SourceBreaking { .. } => panic!("unknown severity"),
    }
}

async fn assert_logged_severities(
    puppet: &mut Puppet,
    severities: &[Severity],
    new_file_line_rules: bool,
) -> Result<(), Error> {
    for severity in severities {
        assert_eq!(
            puppet.read_record_no_tid(puppet.info.tid).await?.unwrap(),
            RecordAssertion::builder(&puppet.info, *severity, new_file_line_rules)
                .add_string("message", severity_to_string(*severity))
                .build(puppet.start_time..zx::BootInstant::get())
        );
    }
    Ok(())
}

async fn assert_interest_listener(
    puppet: &mut Puppet,
    new_file_line_rules: bool,
    stream: &mut LogSinkRequestStream,
    listener: LogSinkWaitForInterestChangeResponder,
) -> Result<(), Error> {
    macro_rules! send_log_with_severity {
        ($severity:ident) => {
            let record = RecordSpec {
                file: "test_file.cc".to_string(),
                line: 9001,
                record: fvalidate::Record {
                    arguments: vec![fvalidate::Argument {
                        name: "message".to_string(),
                        value: fvalidate::Value::Text(stringify!($severity).to_string()),
                    }],
                    severity: Severity::$severity,
                    timestamp: zx::BootInstant::ZERO,
                },
            };
            puppet.proxy.emit_log(&record).await?;
        };
    }
    let interest = Interest { min_severity: Some(Severity::Warn), ..Default::default() };
    listener.send(Ok(&interest))?;
    info!("Waiting for interest....");
    assert_eq!(
        puppet.read_record_no_tid(puppet.info.tid).await?.unwrap(),
        RecordAssertion::builder(&puppet.info, Severity::Warn, new_file_line_rules)
            .add_string("message", "Changed severity")
            .build(puppet.start_time..zx::BootInstant::get())
    );

    send_log_with_severity!(Debug);
    send_log_with_severity!(Info);
    send_log_with_severity!(Warn);
    send_log_with_severity!(Error);
    assert_logged_severities(puppet, &[Severity::Warn, Severity::Error], new_file_line_rules)
        .await
        .unwrap();
    info!("Got interest");
    let interest = Interest { min_severity: Some(Severity::Trace), ..Default::default() };
    let listener = wait_for_severity(stream).await?;
    listener.send(Ok(&interest))?;
    assert_eq!(
        puppet.read_record_no_tid(puppet.info.tid).await?.unwrap(),
        RecordAssertion::builder(&puppet.info, Severity::Trace, new_file_line_rules)
            .add_string("message", "Changed severity")
            .build(puppet.start_time..zx::BootInstant::get())
    );
    send_log_with_severity!(Trace);
    send_log_with_severity!(Debug);
    send_log_with_severity!(Info);
    send_log_with_severity!(Warn);
    send_log_with_severity!(Error);

    assert_logged_severities(
        puppet,
        &[Severity::Trace, Severity::Debug, Severity::Info, Severity::Warn, Severity::Error],
        new_file_line_rules,
    )
    .await
    .unwrap();

    let interest = Interest::default();
    let listener = wait_for_severity(stream).await?;
    listener.send(Ok(&interest))?;
    info!("Waiting for reset interest....");
    assert_eq!(
        puppet.read_record_no_tid(puppet.info.tid).await?.unwrap(),
        RecordAssertion::builder(&puppet.info, Severity::Info, new_file_line_rules)
            .add_string("message", "Changed severity")
            .build(puppet.start_time..zx::BootInstant::get())
    );

    send_log_with_severity!(Debug);
    send_log_with_severity!(Info);
    send_log_with_severity!(Warn);
    send_log_with_severity!(Error);

    assert_logged_severities(
        puppet,
        &[Severity::Info, Severity::Warn, Severity::Error],
        new_file_line_rules,
    )
    .await
    .unwrap();

    // Reset severity to TRACE so we get all messages
    // in later tests.
    let interest = Interest { min_severity: Some(Severity::Trace), ..Default::default() };
    let listener = wait_for_severity(stream).await?;
    listener.send(Ok(&interest))?;
    info!("Waiting for interest to change back....");
    assert_eq!(
        puppet.read_record_no_tid(puppet.info.tid).await?.unwrap(),
        RecordAssertion::builder(&puppet.info, Severity::Trace, new_file_line_rules)
            .add_string("message", "Changed severity")
            .build(puppet.start_time..zx::BootInstant::get())
    );
    info!("Changed interest back");

    Ok(())
}

async fn assert_dot_removal(puppet: &mut Puppet, new_file_line_rules: bool) -> Result<(), Error> {
    let record = RecordSpec {
        file: "../../test_file.cc".to_string(),
        line: 9001,
        record: fvalidate::Record {
            arguments: vec![fvalidate::Argument {
                name: "key".to_string(),
                value: fvalidate::Value::Text("value".to_string()),
            }],
            severity: Severity::Error,
            timestamp: zx::BootInstant::ZERO,
        },
    };
    puppet.proxy.emit_log(&record).await?;
    info!("Waiting for dot message");
    assert_eq!(
        puppet
            .read_record(ReadRecordArgs { new_file_line_rules, override_file_line: false })
            .await?
            .unwrap(),
        RecordAssertion::builder(&puppet.info, Severity::Error, false)
            .add_string("file", "test_file.cc")
            .add_string("key", "value")
            .add_unsigned("line", 9001u64)
            .build(puppet.start_time..zx::BootInstant::get())
    );
    info!("Dot removed");
    Ok(())
}

#[derive(Debug, Arbitrary)]
#[proptest(filter = "vector_filter")]
struct TestVector {
    #[proptest(strategy = "severity_strategy()")]
    severity: Severity,
    #[proptest(strategy = "args_strategy()")]
    args: Vec<(String, fvalidate::Value)>,
}

impl TestVector {
    fn record(&self) -> fvalidate::Record {
        let mut record = fvalidate::Record {
            arguments: vec![],
            severity: self.severity,
            timestamp: zx::BootInstant::ZERO,
        };
        for (name, value) in &self.args {
            record.arguments.push(fvalidate::Argument { name: name.clone(), value: value.clone() });
        }
        record
    }
}

fn vector_filter(vector: &TestVector) -> bool {
    // check to make sure the generated message is small enough
    let record = utils::fidl_to_record(vector.record());
    // TODO(https://fxbug.dev/42145630) avoid this overallocation by supporting growth of the vec
    let mut buf = Cursor::new(vec![0; 1_000_000]);
    {
        diagnostics_log_encoding::encode::Encoder::new(&mut buf, EncoderOpts::default())
            .write_record(record)
            .unwrap();
    }

    buf.position() < MAX_DATAGRAM_LEN_BYTES as u64
}

fn severity_strategy() -> impl Strategy<Value = Severity> {
    prop_oneof![
        Just(Severity::Trace),
        Just(Severity::Debug),
        Just(Severity::Info),
        Just(Severity::Warn),
        Just(Severity::Error),
    ]
}

fn args_strategy() -> impl Strategy<Value = Vec<(String, fvalidate::Value)>> {
    let key_strategy = any::<String>().prop_filter(Reason::from("key too large"), move |s| {
        s.len() <= MAX_ARG_NAME_LENGTH as usize
    });

    let value_strategy = prop_oneof![
        any::<String>().prop_map(fvalidate::Value::Text),
        any::<i64>().prop_map(fvalidate::Value::SignedInt),
        any::<u64>().prop_map(fvalidate::Value::UnsignedInt),
        any::<f64>().prop_map(fvalidate::Value::Floating),
        any::<bool>().prop_map(fvalidate::Value::Boolean),
    ];

    vec((key_strategy, value_strategy), 0..=MAX_ARGS as usize)
}

struct TestCycle {
    spec: RecordSpec,
    assertion: RecordAssertionBuilder,
}

impl TestCycle {
    fn new(info: &PuppetInfo, vector: TestVector, new_file_line_rules: bool) -> Self {
        let spec = RecordSpec { file: "test".to_string(), line: 25, record: vector.record() };
        let mut assertion = RecordAssertion::builder(info, vector.severity, new_file_line_rules);
        for (name, value) in vector.args {
            match value {
                fvalidate::Value::Text(t) => assertion.add_string(&name, t.to_string()),
                fvalidate::Value::SignedInt(n) => assertion.add_signed(&name, n),
                fvalidate::Value::UnsignedInt(n) => assertion.add_unsigned(&name, n),
                fvalidate::Value::Floating(f) => assertion.add_floating(&name, f),
                fvalidate::Value::Boolean(f) => assertion.add_boolean(&name, f),
                _ => {
                    unreachable!("we don't generate unknown values")
                }
            };
        }

        Self { spec, assertion }
    }
}

const STUB_ERROR_FILENAME: &str = "path/to/puppet";
const STUB_ERROR_LINE: u64 = 0x1A4;

#[derive(Debug, PartialEq)]
struct TestRecord {
    timestamp: zx::BootInstant,
    severity: Severity,
    arguments: BTreeMap<String, fvalidate::Value>,
}

struct TestRecordParseArgs<'a> {
    buf: &'a [u8],
    new_file_line_rules: bool,
    ignored_tags: &'a [String],
    override_file_line: bool,
}

impl TestRecord {
    fn parse(args: TestRecordParseArgs<'_>) -> Result<Option<Self>, Error> {
        let Record { timestamp, severity, arguments } = parse_record(args.buf)?.0;

        let mut sorted_args = BTreeMap::new();

        let error_or_file_line_rules_apply = (severity >= Severity::Error.into_primitive()
            || args.new_file_line_rules)
            && args.override_file_line;
        for argument in arguments {
            let name = argument.name().to_string();
            match argument {
                Argument::Tag(tag) => {
                    // check for ignored tags
                    if args.ignored_tags.iter().any(|t| t.as_str() == tag) {
                        return Ok(None);
                    }
                }
                Argument::File(_) if error_or_file_line_rules_apply => {
                    sorted_args.insert(name, fvalidate::Value::Text(STUB_ERROR_FILENAME.into()));
                }
                arg @ Argument::File(_) => {
                    sorted_args.insert(name, utils::value_to_fidl(arg.value()));
                }

                Argument::Line(_) if error_or_file_line_rules_apply => {
                    sorted_args.insert(name, fvalidate::Value::UnsignedInt(STUB_ERROR_LINE));
                }
                arg @ Argument::Line(_) => {
                    sorted_args.insert(name, utils::value_to_fidl(arg.value()));
                }
                _ => {
                    sorted_args.insert(name, utils::value_to_fidl(argument.value()));
                }
            }
        }

        Ok(Some(Self {
            timestamp,
            severity: Severity::from_primitive(severity).unwrap(),
            arguments: sorted_args,
        }))
    }
}
impl PartialEq<RecordAssertion> for TestRecord {
    fn eq(&self, rhs: &RecordAssertion) -> bool {
        rhs.eq(self)
    }
}

#[derive(Debug)]
struct RecordAssertion {
    valid_times: Range<zx::BootInstant>,
    severity: Severity,
    arguments: BTreeMap<String, fvalidate::Value>,
}

impl RecordAssertion {
    fn builder(
        info: &PuppetInfo,
        severity: Severity,
        new_file_line_rules: bool,
    ) -> RecordAssertionBuilder {
        let mut builder = RecordAssertionBuilder { severity, arguments: BTreeMap::new() };
        if let Some(tag) = &info.tag {
            builder.add_string("tag", tag);
        }
        builder.add_unsigned("pid", info.pid);
        builder.add_unsigned("tid", info.tid);
        if severity >= Severity::Error || new_file_line_rules {
            builder.add_string("file", STUB_ERROR_FILENAME);
            builder.add_unsigned("line", STUB_ERROR_LINE);
        }
        builder
    }
}

impl PartialEq<TestRecord> for RecordAssertion {
    fn eq(&self, rhs: &TestRecord) -> bool {
        self.valid_times.contains(&rhs.timestamp)
            && self.severity == rhs.severity
            && self.arguments == rhs.arguments
    }
}

struct RecordAssertionBuilder {
    severity: Severity,
    arguments: BTreeMap<String, fvalidate::Value>,
}

impl RecordAssertionBuilder {
    fn build(&mut self, valid_times: Range<zx::BootInstant>) -> RecordAssertion {
        RecordAssertion {
            valid_times,
            severity: self.severity,
            arguments: std::mem::take(&mut self.arguments),
        }
    }

    fn add_string(&mut self, name: &str, value: impl Into<String>) -> &mut Self {
        self.arguments.insert(name.to_owned(), fvalidate::Value::Text(value.into()));
        self
    }

    fn add_unsigned(&mut self, name: &str, value: u64) -> &mut Self {
        self.arguments.insert(name.to_owned(), fvalidate::Value::UnsignedInt(value));
        self
    }

    fn add_signed(&mut self, name: &str, value: i64) -> &mut Self {
        self.arguments.insert(name.to_owned(), fvalidate::Value::SignedInt(value));
        self
    }

    fn add_floating(&mut self, name: &str, value: f64) -> &mut Self {
        self.arguments.insert(name.to_owned(), fvalidate::Value::Floating(value));
        self
    }

    fn add_boolean(&mut self, name: &str, value: bool) -> &mut Self {
        self.arguments.insert(name.to_owned(), fvalidate::Value::Boolean(value));
        self
    }
}
