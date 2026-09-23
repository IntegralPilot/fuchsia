// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use argh::FromArgs;
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_inspect as finspect;
use fidl_fuchsia_logger as flogger;
use fidl_fuchsia_sys2 as fsys2;
use fidl_fuchsia_tracing_provider as ftracing;
use fuchsia_async as fasync;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route, SubRealmBuilder,
};
use iquery::command_line::CommandLine;
use iquery::commands::*;
use iquery::types::Error;
use pretty_assertions::assert_eq;
use regex::Regex;
use serde::ser::Serialize;
use serde_json::ser::{PrettyFormatter, Serializer};
use std::path::Path;
use std::{fmt, fs};

const BASIC_COMPONENT_URL: &str = "fuchsia-pkg://fuchsia.com/iquery-tests#meta/basic_component.cm";
const TEST_COMPONENT_URL: &str = "fuchsia-pkg://fuchsia.com/iquery-tests#meta/test_component.cm";
const ARCHIVIST_URL: &str =
    "fuchsia-pkg://fuchsia.com/iquery-tests#meta/archivist-for-embedding.cm";

pub async fn new_test(components: &[TestComponent]) -> TestInExecution {
    let mut builder = TestBuilder::new().await;
    for component in components {
        builder.add_child(component.name(), component.url()).await;
    }
    builder.start().await
}

pub enum TestComponent {
    Basic(&'static str),
    Regular(&'static str),
}

impl TestComponent {
    fn url(&self) -> &str {
        match self {
            Self::Basic(_) => BASIC_COMPONENT_URL,
            Self::Regular(_) => TEST_COMPONENT_URL,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Basic(name) | Self::Regular(name) => name,
        }
    }
}

struct TestBuilder {
    builder: RealmBuilder,
    test_realm: SubRealmBuilder,
}

impl TestBuilder {
    async fn new() -> Self {
        let builder = RealmBuilder::new().await.expect("Created realm builder");
        let test_realm = builder
            .add_child_realm("test", ChildOptions::new().eager())
            .await
            .expect("Create child test realm");
        let archivist = test_realm
            .add_child("archivist", ARCHIVIST_URL, ChildOptions::new().eager())
            .await
            .expect("add child archivist");

        let capabilities_from_parent = Route::new()
            .capability(Capability::protocol::<ftracing::RegistryMarker>().optional())
            .capability(Capability::dictionary("diagnostics"));

        builder
            .add_route(
                capabilities_from_parent
                    .clone()
                    .capability(
                        Capability::event_stream("capability_requested").with_scope(&test_realm),
                    )
                    .from(Ref::parent())
                    .to(&test_realm),
            )
            .await
            .expect("added routes from parent to archivist");

        // Routes for Archivist
        test_realm
            .add_route(
                capabilities_from_parent
                    .capability(Capability::event_stream("capability_requested"))
                    .from(Ref::parent())
                    .to(&archivist),
            )
            .await
            .expect("added routes from parent to archivist");

        test_realm
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fdiagnostics::ArchiveAccessorMarker>())
                    .from(&archivist)
                    .to(Ref::parent()),
            )
            .await
            .expect("added routes from archivist to parent");

        test_realm
            .add_capability(cm_rust::CapabilityDecl::Dictionary(cm_rust::DictionaryDecl {
                name: "diagnostics".parse().unwrap(),
                source_path: None,
            }))
            .await
            .expect("added diagnostics dictionary");

        test_realm
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<finspect::InspectSinkMarker>())
                    .capability(Capability::protocol::<flogger::LogSinkMarker>())
                    .from(&archivist)
                    .to(Ref::dictionary(Ref::self_(), "diagnostics")),
            )
            .await
            .expect("route InspectSink and LogSink to self/diagnostics");

        // The realm query scoped to the test
        test_realm
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fsys2::RealmQueryMarker>())
                    .capability(Capability::protocol::<fsys2::LifecycleControllerMarker>())
                    .from(Ref::framework())
                    .to(Ref::parent()),
            )
            .await
            .expect("Can route realm query to parent");
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fsys2::RealmQueryMarker>())
                    .capability(Capability::protocol::<fsys2::LifecycleControllerMarker>())
                    .from(&test_realm)
                    .to(Ref::parent()),
            )
            .await
            .expect("Can route realm query to parent");

        Self { builder, test_realm }
    }

    async fn start(self) -> TestInExecution {
        let instance = self.builder.build().await.expect("create instance");
        // Ensure archivist has been resolved.
        let lifecycle: fsys2::LifecycleControllerProxy =
            instance.root.connect_to_protocol_at_exposed_dir().expect("can connect to lifecycle");
        lifecycle
            .resolve_instance("archivist")
            .await
            .expect("call lifecycle controller")
            .expect("Archivist is resolved");
        TestInExecution { instance }
    }

    async fn add_child(&mut self, name: &str, url: &str) {
        let child_ref =
            self.test_realm.add_child(name, url, ChildOptions::new().eager()).await.unwrap();

        self.test_realm
            .add_route(
                Route::new()
                    .capability(Capability::dictionary("diagnostics"))
                    .from(Ref::self_())
                    .to(&child_ref),
            )
            .await
            .unwrap();
    }
}

pub enum AssertionOption {
    Retry,
}

#[derive(Clone, Copy, Debug)]
pub enum IqueryCommand {
    List,
    ListAccessors,
    Selectors,
    Show,
}

impl fmt::Display for IqueryCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::List => "list",
            Self::ListAccessors => "list-accessors",
            Self::Selectors => "selectors",
            Self::Show => "show",
        };
        write!(f, "{s}")
    }
}

pub struct AssertionParameters<'a> {
    pub command: IqueryCommand,
    pub golden_basename: &'static str,
    pub iquery_args: Vec<&'a str>,
    pub opts: Vec<AssertionOption>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    Text,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Json => "json",
            Self::Text => "text",
        };
        write!(f, "{s}")
    }
}

impl Format {
    fn all() -> impl Iterator<Item = Format> {
        vec![Format::Json, Format::Text].into_iter()
    }
}

pub struct TestInExecution {
    instance: RealmInstance,
}

impl TestInExecution {
    pub async fn assert(&self, params: AssertionParameters<'_>) {
        let realm_query = self
            .instance
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("can connect to realm query");
        let provider = ArchiveAccessorProvider::new(realm_query);
        for format in Format::all() {
            let expected = self.read_golden(params.golden_basename, format);
            let mut assertion =
                CommandAssertion::new(params.command, &expected, format, &params.iquery_args);
            for opt in &params.opts {
                match opt {
                    AssertionOption::Retry => assertion.with_retries(),
                }
            }
            assertion.assert(&provider).await;
        }
    }

    /// Execute a command: [command, flags, and, iquery_args]
    pub async fn execute_command(&self, command: &[&str]) -> Result<String, Error> {
        let realm_query = self
            .instance
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("can connect to realm query");
        let provider = ArchiveAccessorProvider::new(realm_query);
        execute_command(command, &provider).await
    }

    fn read_golden(&self, golden_basename: &str, format: Format) -> String {
        let path = format!("/pkg/data/goldens/{golden_basename}.{format}");
        fs::read_to_string(Path::new(&path))
            .unwrap_or_else(|e| panic!("loaded golden {path}: {e:?}"))
    }
}

#[derive(Clone, Debug)]
pub struct CommandAssertion<'a> {
    command: IqueryCommand,
    iquery_args: &'a [&'a str],
    max_retry_time: zx::MonotonicDuration,
    expected: &'a str,
    format: Format,
}

impl<'a> CommandAssertion<'a> {
    pub fn new(
        command: IqueryCommand,
        expected: &'a str,
        format: Format,
        iquery_args: &'a [&'a str],
    ) -> Self {
        Self { command, iquery_args, max_retry_time: zx::MonotonicDuration::ZERO, expected, format }
    }

    pub fn with_retries(&mut self) {
        self.max_retry_time = zx::MonotonicDuration::from_seconds(120);
    }

    pub(crate) async fn assert(self, provider: &ArchiveAccessorProvider) {
        let started = zx::MonotonicInstant::get();
        let format_str = self.format.to_string();
        let command_str = self.command.to_string();
        let mut command_line = vec!["--format", &format_str, &command_str];
        command_line.append(&mut self.iquery_args.to_vec());
        loop {
            match execute_command(&command_line[..], provider).await {
                Ok(mut result) => {
                    result = self.post_process_output(result);
                    if zx::MonotonicInstant::get() >= started + self.max_retry_time {
                        self.assert_result(&result, self.expected);
                        break;
                    }
                    if self.result_equals_expected(&result, self.expected) {
                        break;
                    }
                }
                Err(e) => {
                    if zx::MonotonicInstant::get() >= started + self.max_retry_time {
                        panic!("Error: {e:?}");
                    }
                }
            }
            fasync::Timer::new(fasync::MonotonicInstant::after(
                zx::MonotonicDuration::from_millis(100),
            ))
            .await;
        }
    }

    fn post_process_output(&self, result: String) -> String {
        match self.format {
            Format::Text => result.trim().to_owned(),
            Format::Json => {
                // Removes the entry in the vector for the archivist
                let result_json: serde_json::Value =
                    serde_json::from_str(&result).expect("expected json");
                // Use 4 spaces for indentation since `fx format-code` enforces that in the
                // goldens.
                let mut buf = Vec::new();
                let mut ser =
                    Serializer::with_formatter(&mut buf, PrettyFormatter::with_indent(b"    "));
                result_json.serialize(&mut ser).unwrap();
                String::from_utf8(buf).unwrap()
            }
        }
    }

    /// Validates that a command result matches the expected json string
    fn assert_result(&self, result: &str, expected: &str) {
        let clean_result = self.cleanup_variable_strings(result);
        if self.format == Format::Json {
            let actual_json: serde_json::Value =
                serde_json::from_str(&clean_result).expect("clean_result is valid json");
            let expected_json: serde_json::Value =
                serde_json::from_str(expected.trim()).expect("expected is valid json");
            pretty_assertions::assert_eq!(actual_json, expected_json);
        } else {
            assert_eq!(&clean_result, expected.trim());
        }
    }

    /// Checks that the result string (cleaned) and the expected string are equal
    fn result_equals_expected(&self, result: &str, expected: &str) -> bool {
        let clean_result = self.cleanup_variable_strings(result);
        if self.format == Format::Json
            && let (Ok(actual_json), Ok(expected_json)) = (
                serde_json::from_str::<serde_json::Value>(&clean_result),
                serde_json::from_str::<serde_json::Value>(expected.trim()),
            )
        {
            return actual_json == expected_json;
        }
        clean_result.trim() == expected.trim()
    }

    /// Cleans-up instances of:
    /// - `"start_timestamp_nanos": 7762005786231` by `"start_timestamp_nanos": TIMESTAMP`
    fn cleanup_variable_strings(&self, string: impl Into<String>) -> String {
        // Replace start_timestamp_nanos in fuchsia.inspect.Health entries and
        // timestamp in metadatas.
        let mut string: String = string.into();
        for value in &["timestamp", "start_timestamp_nanos"] {
            let re = Regex::new(&format!("\"{value}\": \\d+")).unwrap();
            let replacement = format!("\"{value}\": \"TIMESTAMP\"");
            string = re.replace_all(&string, replacement.as_str()).to_string();

            let re = Regex::new(&format!("{value} = \\d+")).unwrap();
            let replacement = format!("{value} = TIMESTAMP");
            string = re.replace_all(&string, replacement.as_str()).to_string();
        }
        string
    }
}

/// Execute a command: [command, flags, and, iquery_args]
async fn execute_command(
    command: &[&str],
    provider: &ArchiveAccessorProvider,
) -> Result<String, Error> {
    let command_line = CommandLine::from_args(&["iquery"], command).expect("create command line");
    command_line.execute(provider).await
}
