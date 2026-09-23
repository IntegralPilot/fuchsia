// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{MetricValue, missing, syntax_error};
use crate::config::{DataFetcher, DiagnosticData, Source};
use anyhow::{Context, Error, Result, anyhow, bail};
use diagnostics_hierarchy::{DiagnosticsHierarchy, SelectResult};
use fidl_fuchsia_diagnostics::Selector;
use fidl_fuchsia_inspect::DEFAULT_TREE_NAME;
use moniker::ExtendedMoniker;
use regex_lite::Regex;
use selectors::{SelectorExt, VerboseError};
use serde::Serialize;
use serde_derive::Deserialize;
use serde_json::Value as JsonValue;
use serde_json::map::Map as JsonMap;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::LazyLock;

/// [Fetcher] is a source of values to feed into the calculations. It may contain data either
/// from snapshot.zip files (e.g. inspect.json data that can be accessed via "select" entries)
/// or supplied in the specification of a trial.
#[derive(Clone, Debug)]
pub enum Fetcher<'a> {
    FileData(FileDataFetcher<'a>),
    TrialData(TrialDataFetcher<'a>),
}

/// [FileDataFetcher] contains fetchers for data in snapshot.zip files.
#[derive(Clone, Debug)]
pub struct FileDataFetcher<'a> {
    pub inspect: &'a InspectFetcher,
    pub syslog: &'a TextFetcher,
    pub klog: &'a TextFetcher,
    pub bootlog: &'a TextFetcher,
    pub annotations: &'a KeyValueFetcher,
}

impl<'a> FileDataFetcher<'a> {
    pub fn new(data: &'a [DiagnosticData]) -> FileDataFetcher<'a> {
        let mut fetcher = FileDataFetcher {
            inspect: InspectFetcher::ref_empty(),
            syslog: TextFetcher::ref_empty(),
            klog: TextFetcher::ref_empty(),
            bootlog: TextFetcher::ref_empty(),
            annotations: KeyValueFetcher::ref_empty(),
        };
        for DiagnosticData { source, data, .. } in data.iter() {
            match source {
                Source::Inspect => {
                    if let DataFetcher::Inspect(data) = data {
                        fetcher.inspect = data;
                    }
                }
                Source::Syslog => {
                    if let DataFetcher::Text(data) = data {
                        fetcher.syslog = data;
                    }
                }
                Source::Klog => {
                    if let DataFetcher::Text(data) = data {
                        fetcher.klog = data;
                    }
                }
                Source::Bootlog => {
                    if let DataFetcher::Text(data) = data {
                        fetcher.bootlog = data;
                    }
                }
                Source::Annotations => {
                    if let DataFetcher::KeyValue(data) = data {
                        fetcher.annotations = data;
                    }
                }
            }
        }
        fetcher
    }

    pub(crate) fn fetch(&self, selector: &SelectorString) -> MetricValue {
        match selector.selector_type {
            // Selectors return a vector. Non-wildcarded Inspect selectors will usually return
            // a vector with a single element, but when a component exposes multiple
            // `fuchsia.inspect.Tree`s, a non-wildcarded selector may match multiple properties.
            SelectorType::Inspect => MetricValue::Vector(self.inspect.fetch(selector)),
        }
    }

    // Return a vector of errors encountered by contained fetchers.
    pub fn errors(&self) -> Vec<String> {
        self.inspect.component_errors.iter().map(|e| format!("{e}")).collect()
    }
}

/// [TrialDataFetcher] stores the key-value lookup for metric names whose values are given as
/// part of a trial (under the "test" section of the .triage files).
#[derive(Clone, Debug)]
pub struct TrialDataFetcher<'a> {
    values: &'a HashMap<String, JsonValue>,
    pub(crate) klog: &'a TextFetcher,
    pub(crate) syslog: &'a TextFetcher,
    pub(crate) bootlog: &'a TextFetcher,
    pub(crate) annotations: &'a KeyValueFetcher,
}

static EMPTY_JSONVALUES: LazyLock<HashMap<String, JsonValue>> = LazyLock::new(HashMap::new);

impl<'a> TrialDataFetcher<'a> {
    pub fn new(values: &'a HashMap<String, JsonValue>) -> TrialDataFetcher<'a> {
        TrialDataFetcher {
            values,
            klog: TextFetcher::ref_empty(),
            syslog: TextFetcher::ref_empty(),
            bootlog: TextFetcher::ref_empty(),
            annotations: KeyValueFetcher::ref_empty(),
        }
    }

    pub fn new_empty() -> TrialDataFetcher<'static> {
        TrialDataFetcher {
            values: &EMPTY_JSONVALUES,
            klog: TextFetcher::ref_empty(),
            syslog: TextFetcher::ref_empty(),
            bootlog: TextFetcher::ref_empty(),
            annotations: KeyValueFetcher::ref_empty(),
        }
    }

    pub fn set_syslog(&mut self, fetcher: &'a TextFetcher) {
        self.syslog = fetcher;
    }

    pub fn set_klog(&mut self, fetcher: &'a TextFetcher) {
        self.klog = fetcher;
    }

    pub fn set_bootlog(&mut self, fetcher: &'a TextFetcher) {
        self.bootlog = fetcher;
    }

    pub fn set_annotations(&mut self, fetcher: &'a KeyValueFetcher) {
        self.annotations = fetcher;
    }

    pub(crate) fn fetch(&self, name: &str) -> MetricValue {
        match self.values.get(name) {
            Some(value) => MetricValue::from(value),
            None => syntax_error(format!("Value {name} not overridden in test")),
        }
    }

    pub(crate) fn has_entry(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }
}

/// Selector type used to determine how to query target file.
#[derive(Deserialize, Debug, Clone, PartialEq, Serialize)]
pub enum SelectorType {
    /// Selector for Inspect Tree ("inspect.json" files).
    Inspect,
}

impl FromStr for SelectorType {
    type Err = anyhow::Error;
    fn from_str(selector_type: &str) -> Result<Self, Self::Err> {
        match selector_type {
            "INSPECT" => Ok(SelectorType::Inspect),
            incorrect => bail!("Invalid selector type '{}' - must be INSPECT", incorrect),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SelectorString {
    pub(crate) full_selector: String,
    pub selector_type: SelectorType,
    body: String,

    #[serde(skip_serializing)]
    parsed_selector: Selector,
}

impl SelectorString {
    pub fn body(&self) -> &str {
        &self.body
    }
}

impl TryFrom<String> for SelectorString {
    type Error = anyhow::Error;

    fn try_from(full_selector: String) -> Result<Self, Self::Error> {
        let mut string_parts = full_selector.splitn(2, ':');
        let selector_type =
            SelectorType::from_str(string_parts.next().ok_or_else(|| anyhow!("Empty selector"))?)?;
        let body = string_parts.next().ok_or_else(|| anyhow!("Selector needs a :"))?.to_owned();
        let parsed_selector = selectors::parse_selector::<VerboseError>(&body)?;
        Ok(SelectorString { full_selector, selector_type, body, parsed_selector })
    }
}

#[derive(Debug)]
pub struct ComponentInspectInfo {
    processed_data: DiagnosticsHierarchy,
    moniker: ExtendedMoniker,
    tree_name: String,
}

impl ComponentInspectInfo {
    fn matches_selector(&self, selector: &Selector) -> bool {
        self.moniker
            .match_against_selectors_and_tree_name(&self.tree_name, Some(selector))
            .next()
            .is_some()
    }
}

#[derive(Default, Debug)]
pub struct KeyValueFetcher {
    pub map: JsonMap<String, JsonValue>,
}

impl TryFrom<&str> for KeyValueFetcher {
    type Error = anyhow::Error;

    fn try_from(json_text: &str) -> Result<Self, Self::Error> {
        let raw_json =
            json_text.parse::<JsonValue>().context("Couldn't parse KeyValue text as JSON.")?;
        match raw_json {
            JsonValue::Object(map) => Ok(KeyValueFetcher { map }),
            _ => bail!("Bad json KeyValue data needs to be Object (map)."),
        }
    }
}

impl TryFrom<&JsonMap<String, JsonValue>> for KeyValueFetcher {
    type Error = anyhow::Error;

    fn try_from(map: &JsonMap<String, JsonValue>) -> Result<Self, Self::Error> {
        // This doesn't fail today, but that's an implementation detail; don't count on it.
        Ok(KeyValueFetcher { map: map.clone() })
    }
}

static EMPTY_KEY_VALUE_FETCHER: LazyLock<KeyValueFetcher> = LazyLock::new(KeyValueFetcher::default);

impl KeyValueFetcher {
    pub fn ref_empty() -> &'static Self {
        &EMPTY_KEY_VALUE_FETCHER
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn fetch(&self, key: &str) -> MetricValue {
        match self.map.get(key) {
            Some(value) => MetricValue::from(value),
            None => missing(format!("Key '{key}' not found in annotations")),
        }
    }
}

#[derive(Default, Debug)]
pub struct TextFetcher {
    pub lines: Vec<String>,
}

impl From<&str> for TextFetcher {
    fn from(log_buffer: &str) -> Self {
        TextFetcher { lines: log_buffer.split('\n').map(|s| s.to_string()).collect::<Vec<_>>() }
    }
}

static EMPTY_TEXT_FETCHER: LazyLock<TextFetcher> = LazyLock::new(TextFetcher::default);

impl TextFetcher {
    pub fn ref_empty() -> &'static Self {
        &EMPTY_TEXT_FETCHER
    }

    pub fn contains(&self, pattern: &str) -> bool {
        let re = match Regex::new(pattern) {
            Ok(re) => re,
            _ => return false,
        };
        self.lines.iter().any(|s| re.is_match(s))
    }
}

#[derive(Default, Debug)]
pub struct InspectFetcher {
    pub components: Vec<ComponentInspectInfo>,
    pub component_errors: Vec<anyhow::Error>,
}

impl TryFrom<&str> for InspectFetcher {
    type Error = anyhow::Error;

    fn try_from(json_text: &str) -> Result<Self, Self::Error> {
        let raw_json =
            json_text.parse::<JsonValue>().context("Couldn't parse Inspect text as JSON.")?;
        match raw_json {
            JsonValue::Array(list) => Self::try_from(list),
            _ => bail!("Bad json inspect data needs to be array."),
        }
    }
}

impl TryFrom<Vec<JsonValue>> for InspectFetcher {
    type Error = anyhow::Error;

    fn try_from(component_vec: Vec<JsonValue>) -> Result<Self, Self::Error> {
        fn extract_json_value(component: &mut JsonValue, key: &'_ str) -> Result<JsonValue, Error> {
            Ok(component
                .get_mut(key)
                .ok_or_else(|| anyhow!("'{}' not found in Inspect component", key))?
                .take())
        }

        fn moniker_from(component: &mut JsonValue) -> Result<ExtendedMoniker, anyhow::Error> {
            let value = extract_json_value(component, "moniker")
                .or_else(|_| bail!("'moniker' not found in Inspect component"))?;
            let moniker = ExtendedMoniker::parse_str(
                value
                    .as_str()
                    .ok_or_else(|| anyhow!("Inspect component path wasn't a valid string"))?,
            )?;
            Ok(moniker)
        }

        let components = component_vec.into_iter().map(|mut raw_component| {
            let moniker = moniker_from(&mut raw_component)?;
            let tree_name = match extract_json_value(
                &mut extract_json_value(&mut raw_component, "metadata")?,
                "name",
            ) {
                Ok(n) => n.as_str().unwrap_or(DEFAULT_TREE_NAME).to_string(),
                // the "name" field might be missing from older systems
                Err(_) => DEFAULT_TREE_NAME.to_string(),
            };
            let raw_contents = extract_json_value(&mut raw_component, "payload").or_else(|_| {
                extract_json_value(&mut raw_component, "contents").or_else(|_| {
                    bail!("Neither 'payload' nor 'contents' found in Inspect component")
                })
            })?;
            let processed_data: DiagnosticsHierarchy = match raw_contents {
                v if v.is_null() => {
                    // If the payload is null, leave the hierarchy empty.
                    DiagnosticsHierarchy::new_root()
                }
                raw_contents => serde_json::from_value(raw_contents).with_context(|| {
                    format!(
                        "Unable to deserialize Inspect contents for {moniker} to node hierarchy",
                    )
                })?,
            };
            Ok(ComponentInspectInfo { moniker, processed_data, tree_name })
        });

        let mut component_errors = vec![];
        let components = components
            .filter_map(|v| match v {
                Ok(component) => Some(component),
                Err(e) => {
                    component_errors.push(e);
                    None
                }
            })
            .collect::<Vec<_>>();
        Ok(Self { components, component_errors })
    }
}

static EMPTY_INSPECT_FETCHER: LazyLock<InspectFetcher> = LazyLock::new(InspectFetcher::default);

impl InspectFetcher {
    pub fn ref_empty() -> &'static Self {
        &EMPTY_INSPECT_FETCHER
    }

    fn try_fetch(&self, selector_string: &SelectorString) -> Result<Vec<MetricValue>, Error> {
        let mut intermediate_results = Vec::new();
        let mut found_component = false;
        for component in &self.components {
            if !component.matches_selector(&selector_string.parsed_selector) {
                continue;
            }
            found_component = true;
            let selector = selector_string.parsed_selector.clone();
            intermediate_results.push(diagnostics_hierarchy::select_from_hierarchy(
                &component.processed_data,
                &selector,
            )?);
        }

        if !found_component {
            return Ok(vec![missing(format!(
                "No component found matching selector {}",
                selector_string.body,
            ))]);
        }

        let mut result = vec![];
        for r in intermediate_results {
            match r {
                SelectResult::Properties(p) => {
                    result.extend(p.into_iter().map(|p| MetricValue::from(p.into_owned())))
                }
                SelectResult::Nodes(n) => {
                    for node in n {
                        for _ in &node.children {
                            result.push(MetricValue::Node);
                        }

                        for prop in &node.properties {
                            result.push(MetricValue::from(prop.clone()));
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    pub fn fetch(&self, selector: &SelectorString) -> Vec<MetricValue> {
        match self.try_fetch(selector) {
            Ok(v) => v,
            Err(e) => vec![syntax_error(format!("Fetch {selector:?} -> {e}"))],
        }
    }

    #[cfg(test)]
    fn fetch_str(&self, selector_str: &str) -> Vec<MetricValue> {
        match SelectorString::try_from(selector_str.to_owned()) {
            Ok(selector) => self.fetch(&selector),
            Err(e) => vec![syntax_error(format!("Bad selector {selector_str}: {e}"))],
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::metrics::variable::VariableName;
    use crate::metrics::{Metric, MetricState, Problem, ValueSource};
    use crate::{assert_problem, make_metrics};
    use assert_matches::assert_matches;
    use serde_json::Value as JsonValue;

    static LOCAL_M: LazyLock<HashMap<String, JsonValue>> = LazyLock::new(|| {
        let mut m = HashMap::new();
        m.insert("foo".to_owned(), JsonValue::from(42));
        m.insert("a::b".to_owned(), JsonValue::from(7));
        m
    });
    static FOO_42_AB_7_TRIAL_FETCHER: LazyLock<TrialDataFetcher<'static>> =
        LazyLock::new(|| TrialDataFetcher::new(&LOCAL_M));
    static LOCAL_F: LazyLock<Vec<DiagnosticData>> = LazyLock::new(|| {
        let s = r#"[
            {
                "data_source": "Inspect",
                "moniker": "bar",
                "metadata": {},
                "payload": { "root": { "bar": 99 }}
            },
            {
                "data_source": "Inspect",
                "moniker": "bar2",
                "metadata": {},
                "payload": { "root": { "bar": 90 }}
            }

            ]"#;
        vec![DiagnosticData::new("i".to_string(), Source::Inspect, s.to_string()).unwrap()]
    });
    static BAR_99_FILE_FETCHER: LazyLock<FileDataFetcher<'static>> =
        LazyLock::new(|| FileDataFetcher::new(&LOCAL_F));
    static BAR_SELECTOR: LazyLock<SelectorString> =
        LazyLock::new(|| SelectorString::try_from("INSPECT:bar:root:bar".to_owned()).unwrap());
    static NEW_BAR_SELECTOR: LazyLock<SelectorString> =
        LazyLock::new(|| SelectorString::try_from("INSPECT:bar2:root:bar".to_owned()).unwrap());
    static BAD_COMPONENT_SELECTOR: LazyLock<SelectorString> = LazyLock::new(|| {
        SelectorString::try_from("INSPECT:bad_component:root:bar".to_owned()).unwrap()
    });
    static WRONG_SELECTOR: LazyLock<SelectorString> =
        LazyLock::new(|| SelectorString::try_from("INSPECT:bar:root:oops".to_owned()).unwrap());
    static LOCAL_DUPLICATES_F: LazyLock<Vec<DiagnosticData>> = LazyLock::new(|| {
        let s = r#"[
                {
                    "data_source": "Inspect",
                    "moniker": "bootstrap/foo",
                    "metadata": {},
                    "payload": null
                },
                {
                    "data_source": "Inspect",
                    "moniker": "bootstrap/foo",
                    "metadata": {},
                    "payload": {"root": {"bar": 10}}
                }
            ]"#;
        vec![DiagnosticData::new("i".to_string(), Source::Inspect, s.to_string()).unwrap()]
    });
    static LOCAL_DUPLICATES_FETCHER: LazyLock<FileDataFetcher<'static>> =
        LazyLock::new(|| FileDataFetcher::new(&LOCAL_DUPLICATES_F));
    static DUPLICATE_SELECTOR: LazyLock<SelectorString> = LazyLock::new(|| {
        SelectorString::try_from("INSPECT:bootstrap/foo:root:bar".to_owned()).unwrap()
    });

    macro_rules! variable {
        ($name:expr) => {
            &VariableName::new($name.to_string())
        };
    }

    #[fuchsia::test]
    fn test_file_fetch() {
        assert_eq!(
            BAR_99_FILE_FETCHER.fetch(&BAR_SELECTOR),
            MetricValue::Vector(vec![MetricValue::Int(99)])
        );
        assert_eq!(BAR_99_FILE_FETCHER.fetch(&WRONG_SELECTOR), MetricValue::Vector(vec![]),);
    }

    #[fuchsia::test]
    fn test_duplicate_file_fetch() {
        assert_eq!(
            LOCAL_DUPLICATES_FETCHER.fetch(&DUPLICATE_SELECTOR),
            MetricValue::Vector(vec![MetricValue::Int(10)])
        );
    }

    #[fuchsia::test]
    fn test_trial_fetch() {
        assert!(FOO_42_AB_7_TRIAL_FETCHER.has_entry("foo"));
        assert!(FOO_42_AB_7_TRIAL_FETCHER.has_entry("a::b"));
        assert!(!FOO_42_AB_7_TRIAL_FETCHER.has_entry("a:b"));
        assert!(!FOO_42_AB_7_TRIAL_FETCHER.has_entry("oops"));
        assert_eq!(FOO_42_AB_7_TRIAL_FETCHER.fetch("foo"), MetricValue::Int(42));
        assert_problem!(
            FOO_42_AB_7_TRIAL_FETCHER.fetch("oops"),
            "SyntaxError: Value oops not overridden in test"
        );
    }

    #[fuchsia::test]
    fn test_eval_with_file() {
        let metrics = make_metrics!({
            "bar_file":{
                eval: {
                    "bar_plus_one": "bar + 1",
                    "oops_plus_one": "oops + 1"
                }
                select: {
                    "bar": [BAR_SELECTOR],
                    "wrong_or_bar": [WRONG_SELECTOR, BAR_SELECTOR],
                    "wrong_or_wrong":  [WRONG_SELECTOR, WRONG_SELECTOR],
                    "wrong_or_new_bar_or_bar": [WRONG_SELECTOR, NEW_BAR_SELECTOR, BAR_SELECTOR],
                    "bad_component_or_bar": [BAD_COMPONENT_SELECTOR, BAR_SELECTOR]
                }
            },
            "other_file":{
                eval: {
                    "bar": "42"
                }
            }
        });

        let file_state =
            MetricState::new(&metrics, Fetcher::FileData(BAR_99_FILE_FETCHER.clone()), None);
        assert_eq!(
            file_state.evaluate_variable("bar_file", variable!("bar_plus_one")),
            MetricValue::Int(100)
        );
        assert_problem!(
            file_state.evaluate_variable("bar_file", variable!("oops_plus_one")),
            "SyntaxError: Metric 'oops' Not Found in 'bar_file'"
        );
        assert_eq!(
            file_state.evaluate_variable("bar_file", variable!("bar")),
            MetricValue::Vector(vec![MetricValue::Int(99)])
        );
        assert_eq!(
            file_state.evaluate_variable("other_file", variable!("bar")),
            MetricValue::Int(42)
        );
        assert_eq!(
            file_state.evaluate_variable("other_file", variable!("other_file::bar")),
            MetricValue::Int(42)
        );
        assert_eq!(
            file_state.evaluate_variable("other_file", variable!("bar_file::bar")),
            MetricValue::Vector(vec![MetricValue::Int(99)])
        );
        assert_eq!(
            file_state.evaluate_variable("bar_file", variable!("bar")),
            file_state.evaluate_variable("bar_file", variable!("wrong_or_bar")),
        );
        assert_eq!(
            file_state.evaluate_variable("bar_file", variable!("wrong_or_wrong")),
            MetricValue::Vector(vec![]),
        );
        assert_eq!(
            file_state.evaluate_variable("bar_file", variable!("wrong_or_new_bar_or_bar")),
            MetricValue::Vector(vec![MetricValue::Int(90)])
        );
        assert_eq!(
            file_state.evaluate_variable("bar_file", variable!("bad_component_or_bar")),
            MetricValue::Vector(vec![MetricValue::Int(99)])
        );
        assert_problem!(
            file_state.evaluate_variable("other_file", variable!("bar_plus_one")),
            "SyntaxError: Metric 'bar_plus_one' Not Found in 'other_file'"
        );
        assert_problem!(
            file_state.evaluate_variable("missing_file", variable!("bar_plus_one")),
            "SyntaxError: Bad namespace 'missing_file'"
        );
        assert_problem!(
            file_state.evaluate_variable("bar_file", variable!("other_file::bar_plus_one")),
            "SyntaxError: Metric 'bar_plus_one' Not Found in 'other_file'"
        );
    }

    #[fuchsia::test]
    fn test_eval_with_trial() {
        // The (broken) "foo" selector should be ignored in favor of the "foo" fetched value.
        // The file "a" should be completely ignored when testing foo_file.
        let metrics = make_metrics!({
            "a":{
                eval: {
                "b": "2",
                "c": "3",
                "foo": "4",
                }
            },
            "foo_file":{
                eval: {
                    "foo_plus_one": "foo + 1",
                    "oops_plus_one": "oops + 1",
                    "ab_plus_one": "a::b + 1",
                    "ac_plus_one": "a::c + 1"
                }
                select: {
                "foo": [BAR_SELECTOR]
                }
            }
        });

        let trial_state =
            MetricState::new(&metrics, Fetcher::TrialData(FOO_42_AB_7_TRIAL_FETCHER.clone()), None);

        // foo from values shadows foo selector.
        assert_eq!(
            trial_state.evaluate_variable("foo_file", variable!("foo")),
            MetricValue::Int(42)
        );
        // Value shadowing also works in expressions.
        assert_eq!(
            trial_state.evaluate_variable("foo_file", variable!("foo_plus_one")),
            MetricValue::Int(43)
        );
        // foo can shadow eval as well as selector.
        assert_eq!(trial_state.evaluate_variable("a", variable!("foo")), MetricValue::Int(42));
        // A value that's not there should be "SyntaxError" (e.g. not crash)
        assert_problem!(
            trial_state.evaluate_variable("foo_file", variable!("oops_plus_one")),
            "SyntaxError: Metric 'oops' Not Found in 'foo_file'"
        );
        // a::b ignores the "b" in file "a" and uses "a::b" from values.
        assert_eq!(
            trial_state.evaluate_variable("foo_file", variable!("ab_plus_one")),
            MetricValue::Int(8)
        );
        // a::c should return Missing, not look up c in file a.
        assert_problem!(
            trial_state.evaluate_variable("foo_file", variable!("ac_plus_one")),
            "SyntaxError: Name a::c not in test values and refers outside the file"
        );
    }

    #[fuchsia::test]
    fn inspect_fetcher_new_works() -> Result<(), Error> {
        assert!(InspectFetcher::try_from("foo").is_err(), "'foo' isn't valid JSON");
        assert!(InspectFetcher::try_from(r#"{"a":5}"#).is_err(), "Needed an array");
        assert!(InspectFetcher::try_from("[]").is_ok(), "A JSON array should have worked");
        Ok(())
    }

    #[fuchsia::test]
    fn test_fetch_with_tree_names() {
        let cases = &[
            (
                "INSPECT:core/*:[name=root]root:foo",
                vec![MetricValue::String("bar".to_string())],
                r#"[
  {
    "data_source": "Inspect",
    "metadata": {
      "name": "root",
      "component_url": "fuchsia-pkg://fuchsia.com/foo#meta/foo.cm",
      "timestamp": 6532507441581
    },
    "moniker": "core/foo",
    "payload": {
      "root": {
        "foo": "bar"
      }
    }
  },
  {
    "data_source": "Inspect",
    "metadata": {
      "name": "root",
      "component_url": "fuchsia-pkg://fuchsia.com/baz#meta/baz.cm",
      "timestamp": 6532507441581
    },
    "moniker": "core/baz",
    "payload": {
      "root": {
        "baz": ""
      }
    }
  }
]
"#,
            ),
            (
                "INSPECT:core/*:[name=foo-is-bar]root:foo",
                vec![MetricValue::String("bar".to_string())],
                r#"[
  {
    "data_source": "Inspect",
    "metadata": {
      "name": "foo-is-bar",
      "component_url": "fuchsia-pkg://fuchsia.com/foo#meta/foo.cm",
      "timestamp": 6532507441581
    },
    "moniker": "core/foo",
    "payload": {
      "root": {
        "foo": "bar"
      }
    }
  },
  {
    "data_source": "Inspect",
    "metadata": {
      "name": "foo-is-qux",
      "component_url": "fuchsia-pkg://fuchsia.com/foo#meta/foo.cm",
      "timestamp": 6532507441581
    },
    "moniker": "core/foo",
    "payload": {
      "root": {
        "foo": "qux"
      }
    }
  },
  {
    "data_source": "Inspect",
    "metadata": {
      "name": "root",
      "component_url": "fuchsia-pkg://fuchsia.com/baz#meta/baz.cm",
      "timestamp": 6532507441581
    },
    "moniker": "core/baz",
    "payload": {
      "root": {
        "baz": ""
      }
    }
  }
]
"#,
            ),
        ];

        for (selector, expected, json) in cases {
            let fetcher = InspectFetcher::try_from(*json).unwrap();
            let metric = fetcher.fetch_str(selector);
            assert_eq!(expected, &metric, "component list: {:#?}", fetcher.components);
        }
    }

    #[fuchsia::test]
    fn test_fetch() -> Result<(), Error> {
        // This tests both the moniker/payload and path/content (old-style) Inspect formats.
        let json_options = vec![
            r#"[
        {"moniker":"asdf/foo/qwer", "metadata": {},
         "payload":{"root":{"dataInt":5, "child":{"dataFloat":2.3}}}},
        {"moniker":"zxcv/bar/hjkl", "metadata": {},
         "payload":{"base":{"dataInt":42, "array":[2,3,4], "yes": true}}},
        {"moniker":"fail_component", "metadata": {},
         "payload": ["a", "b"]},
        {"moniker":"missing_component", "metadata": {},
         "payload": null}
        ]"#,
            r#"[
        {"moniker":"asdf/foo/qwer", "metadata": {},
         "payload":{"root":{"dataInt":5, "child":{"dataFloat":2.3}}}},
        {"moniker":"zxcv/bar/hjkl", "metadata": {},
         "contents":{"base":{"dataInt":42, "array":[2,3,4], "yes": true}}},
        {"moniker":"fail_component", "metadata": {},
         "payload": ["a", "b"]},
        {"moniker":"missing_component", "metadata": {},
         "payload": null}
        ]"#,
        ];

        for json in json_options.into_iter() {
            let inspect = InspectFetcher::try_from(json)?;
            assert_eq!(
                vec!["Unable to deserialize Inspect contents for fail_component to node hierarchy"],
                inspect.component_errors.iter().map(|e| format!("{e}")).collect::<Vec<_>>()
            );
            macro_rules! assert_wrong {
                ($selector:expr, $error:expr) => {
                    let error = inspect.fetch_str($selector);
                    assert_eq!(error.len(), 1);
                    assert_problem!(&error[0], $error);
                };
            }
            assert_wrong!(
                "INSPET:*/foo/*:root:dataInt",
                "SyntaxError: Bad selector INSPET:*/foo/*:root:dataInt: Invalid selector type \'INSPET\' - must be INSPECT"
            );
            assert_eq!(
                inspect.fetch_str("INSPECT:*/foo/*:root:dataInt"),
                vec![MetricValue::Int(5)]
            );
            assert_eq!(
                inspect.fetch_str("INSPECT:*/foo/*:root/child:dataFloat"),
                vec![MetricValue::Float(2.3)]
            );
            assert_eq!(
                inspect.fetch_str("INSPECT:zxcv/*/hjk*:base:yes"),
                vec![MetricValue::Bool(true)]
            );
            assert_eq!(inspect.fetch_str("INSPECT:*/foo/*:root.dataInt"), vec![]);
            assert_wrong!(
                "INSPECT:*/fo/*:root.dataInt",
                "Missing: No component found matching selector */fo/*:root.dataInt"
            );

            assert_eq!(inspect.fetch_str("INSPECT:*/foo/*:root/kid:dataInt"), vec![]);
            assert_eq!(inspect.fetch_str("INSPECT:*/bar/*:base/array:dataInt"), vec![]);
            assert_eq!(
                inspect.fetch_str("INSPECT:*/bar/*:base:array"),
                vec![MetricValue::Vector(vec![
                    MetricValue::Int(2),
                    MetricValue::Int(3),
                    MetricValue::Int(4)
                ])]
            );
        }
        Ok(())
    }

    #[fuchsia::test]
    fn inspect_ref_empty() -> Result<(), Error> {
        // Make sure it doesn't crash, can be called multiple times and they both work right.
        let fetcher1 = InspectFetcher::ref_empty();
        let fetcher2 = InspectFetcher::ref_empty();

        match fetcher1.try_fetch(&SelectorString::try_from("INSPECT:a:b:c".to_string())?).unwrap()
            [0]
        {
            MetricValue::Problem(Problem::Missing(_)) => {}
            _ => bail!("Should have Missing'd a valid selector"),
        }
        match fetcher2.try_fetch(&SelectorString::try_from("INSPECT:a:b:c".to_string())?).unwrap()
            [0]
        {
            MetricValue::Problem(Problem::Missing(_)) => {}
            _ => bail!("Should have Missing'd a valid selector"),
        }
        Ok(())
    }

    #[fuchsia::test]
    fn text_fetcher_works() {
        let fetcher = TextFetcher::from("abcfoo\ndefgfoo");
        assert!(fetcher.contains("d*g"));
        assert!(fetcher.contains("foo"));
        assert!(!fetcher.contains("food"));
        // Make sure ref_empty() doesn't crash and can be used multiple times.
        let fetcher1 = TextFetcher::ref_empty();
        let fetcher2 = TextFetcher::ref_empty();
        assert!(!fetcher1.contains("a"));
        assert!(!fetcher2.contains("a"));
    }

    #[fuchsia::test]
    fn test_selector_string_parse() -> Result<(), Error> {
        // Test correct shape of SelectorString and verify no errors on parse for valid selector.
        let full_selector = "INSPECT:bad_component:root:bar".to_string();
        let selector_type = SelectorType::Inspect;
        let body = "bad_component:root:bar".to_string();
        let parsed_selector = selectors::parse_selector::<VerboseError>(&body)?;

        assert_eq!(
            SelectorString::try_from("INSPECT:bad_component:root:bar".to_string())?,
            SelectorString { full_selector, selector_type, body, parsed_selector }
        );

        // Test that a selector that does not follow the correct syntax results in parse error.
        assert_matches!(SelectorString::try_from("INSPECT:not a selector".to_string()), Err(_));

        // Test that an invalid selector results in a parse error.
        assert_matches!(
            SelectorString::try_from("INSPECT:*/foo/*:root:data:Int".to_string()),
            Err(_)
        );

        Ok(())
    }

    // KeyValueFetcher is tested in metrics::test::annotations_work()
}
