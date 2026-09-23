// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use errors::ffx_bail;
use ffx_config::EnvironmentContext;
use ffx_tracing::FidlLibraries;
use fxt::{ArgValue, RawArg, RawArgValue, RawEventRecord, SessionParser, StringRef, TraceRecord};
use log::warn;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use termion::{color, style};

static ORDINAL_ARG_NAME: &str = "ordinal";
static BYTES_ARG_NAME: &str = "bytes";
static HANDLES_ARG_NAME: &str = "handles";

/// An iterator over trace files that yields each record in both parsed and raw form.
struct TraceIterator<R> {
    parser: SessionParser<R>,
}

impl<R: std::io::Read> TraceIterator<R> {
    fn new(reader: R) -> Self {
        Self { parser: SessionParser::new(reader) }
    }
}

impl TraceIterator<std::io::Cursor<Vec<u8>>> {
    fn from_bytes(bytes: Vec<u8>) -> Self {
        TraceIterator::new(std::io::Cursor::new(bytes))
    }
}

impl<R: std::io::Read> Iterator for TraceIterator<R> {
    type Item = (Result<TraceRecord, fxt::ParseError>, Vec<u8>, std::ops::Range<usize>);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(record) = self.parser.next() {
            let parsed_bytes = self.parser.parsed_bytes().to_owned();
            Some((record, parsed_bytes, self.parser.record_range()))
        } else {
            None
        }
    }
}

/// Change a raw serialized flow event (begin or end) into a flow step and return the raw bytes.
fn flow_event_into_flow_step(bytes: &[u8], flow_id: u64) -> Result<Vec<u8>> {
    let (_, mut parsed) = fxt::RawEventRecord::parse(bytes)?;
    parsed.set_flow_step_payload(flow_id);
    parsed.serialize().map_err(|e| anyhow!(e))
}

/// Parse a trace session and call a function for each trace record that is an event record.
/// The function takes the parsed event record and the raw bytes and returns raw bytes.
/// This allows the function to examine the event record and modify it if needed.
fn process_event_records<F: FnMut(fxt::EventRecord, Vec<u8>) -> Vec<u8>>(
    input: Vec<u8>,
    mut process: F,
) -> Result<Vec<u8>> {
    let output: Vec<u8> = TraceIterator::from_bytes(input)
        .flat_map(|r| match r {
            (Ok(TraceRecord::Event(event_record)), parsed_bytes, record_range) => {
                assert_eq!(record_range.end, parsed_bytes.len());
                let prefix = &parsed_bytes[..record_range.start];
                if prefix.is_empty() {
                    process(event_record, parsed_bytes)
                } else {
                    let processed = process(event_record, parsed_bytes[record_range].to_vec());
                    [prefix, &processed].concat()
                }
            }
            (_, parsed_bytes, _) => parsed_bytes,
        })
        .collect();
    Ok(output)
}

/// Process a trace file.
/// Optionally symbolize and apply a category check.
pub fn process_trace_file(
    trace_file: impl AsRef<Path>,
    outfile: impl AsRef<Path>,
    symbolize: bool,
    retain_raw_fidl: bool,
    category_check: Option<Vec<String>>,
    context: &EnvironmentContext,
) -> Result<Vec<String>> {
    let mut warning_list = Vec::new();

    let mut symbolizer = if symbolize {
        let libs = match FidlLibraries::from_context(context) {
            Ok(libs) => libs,
            Err(err) => {
                // Fall back to an empty symbolization map, but warn the user.
                warning_list.push(format!(
                    "{}WARNING: FIDL ordinals not loaded: {:?}{}",
                    color::Fg(color::Yellow),
                    err,
                    style::Reset
                ));
                FidlLibraries::default()
            }
        };

        Some(Symbolizer::new(libs))
    } else {
        None
    };
    let mut category_counter = if let Some(input_categories) = category_check {
        Some(CategoryCounter::new(input_categories))
    } else {
        None
    };

    // Read the trace into memory.
    let trace = std::fs::read(&trace_file)?;

    // Process all of the event records.
    let mut modified = false;
    let mut trace: Vec<u8> = process_event_records(trace, |event_record, parsed_bytes| {
        // This closure runs over every event record.

        // Count the event categories.
        if let Some(category_counter) = &mut category_counter {
            category_counter.increment_category(&event_record.category);
        }

        // Symbolize FIDL IPC traces.
        if let Some(symbolizer) = &mut symbolizer {
            if let Some(symbolized_bytes) =
                symbolizer.symbolize_event_record(&event_record, &parsed_bytes)
            {
                modified = true;
                symbolized_bytes
            } else {
                parsed_bytes
            }
        } else {
            parsed_bytes
        }
    })?;

    // Check that the categories which were specified were actually captured.
    if let Some(category_counter) = &mut category_counter {
        if trace.is_empty() {
            ffx_bail!(
                "{}WARNING: The trace file is empty. Please verify that the input categories are valid. Input categories are: {:?}.{}",
                color::Fg(color::Yellow),
                category_counter.input_categories,
                style::Reset
            );
        }
        let invalid_category_list = category_counter.get_invalid_category_list();
        if !invalid_category_list.is_empty() {
            warning_list.push(format!(
                "{}WARNING: Categories {:?} were manually specified, but not found in the resulting trace. Check the spelling of your categories or the status of your trace provider{}",
                color::Fg(color::Yellow),
                invalid_category_list,
                style::Reset
            ));
        }
    }

    // Fix-up two-way method traces so that the round-trips are a single flow.
    // For channel_call the kernel takes care of this, for async IPC we do it now.
    if let Some(symbolizer) = symbolizer {
        // We can ignore any flows that have steps because the kernel's already taken care of it.
        let mut flow_state: BTreeMap<u64, Option<CallState>> = BTreeMap::from_iter(
            symbolizer.flow_has_steps.iter().filter_map(|(flow_id, has_steps)| {
                if *has_steps { None } else { Some((*flow_id, None)) }
            }),
        );

        // If there are flows to fix up then run through the trace again:
        if !flow_state.is_empty() {
            trace = process_event_records(trace, |event_record, mut parsed_bytes| {
                // If this is a flow event...
                if let Some((flow_id, flow_stage)) = event_flow_info(&event_record) {
                    // If this is a flow event for a flow we care about...
                    if event_record.category.as_str() != "kernel:ipc" {
                        return parsed_bytes;
                    }
                    if let Some(call_state) = flow_state.get_mut(&flow_id) {
                        // Flows with steps should have been excluded above...
                        assert_ne!(flow_stage, FlowStage::Step,);

                        let call_state = call_state.get_or_insert_with(||
                            // Guess the initial state...
                            if flow_stage == FlowStage::Begin { CallState::WriteRequest } else { CallState::ReadResponse }
                        );

                        if matches!(call_state, CallState::ReadRequest | CallState::WriteResponse) {
                            // These should turn into FlowStep records.
                            parsed_bytes = flow_event_into_flow_step(&parsed_bytes, flow_id)
                                .expect("modifying flow event");
                        }
                        *call_state = call_state.next();
                    }
                }
                if !retain_raw_fidl {
                    // Find which arguments are for "ordinal", "bytes" and "handles".
                    let remove_args: Vec<usize> = event_record
                        .args
                        .iter()
                        .enumerate()
                        .filter_map(|(i, arg)| {
                            if arg.name == ORDINAL_ARG_NAME
                                || arg.name == BYTES_ARG_NAME
                                || arg.name == HANDLES_ARG_NAME
                            {
                                Some(i)
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !remove_args.is_empty() {
                        let (_, mut parsed) = fxt::RawEventRecord::parse(&parsed_bytes)
                            .expect("Removing raw FIDL-related args");
                        for i in remove_args.iter().rev() {
                            parsed.args.remove(*i);
                        }
                        parsed_bytes = parsed.serialize().expect("serializing modified record");
                    }
                }
                parsed_bytes
            })?;
        }
    }

    // Write the trace if there's a different output path specified or if the processing actually modified the trace.
    if trace_file.as_ref() != outfile.as_ref() || modified {
        std::fs::write(outfile, trace)?;
    }
    Ok(warning_list)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallState {
    WriteRequest,
    ReadRequest,
    WriteResponse,
    ReadResponse,
}
impl CallState {
    fn next(&self) -> Self {
        use CallState::*;
        match self {
            WriteRequest => ReadRequest,
            ReadRequest => WriteResponse,
            WriteResponse => ReadResponse,
            ReadResponse => WriteRequest,
        }
    }
}

/// Determine if this flow id is for a message with a txid.
/// See: //zircon/kernel/object/channel_dispatcher.cc
fn is_two_way_message(flow_id: u64) -> bool {
    (flow_id >> 31) & 1 == 1
}

#[derive(PartialEq, Debug, Eq)]
enum FlowStage {
    Begin,
    Step,
    End,
}

/// Extract the flow id and stage from an event record, if it holds a flow event.
fn event_flow_info(event_record: &fxt::EventRecord) -> Option<(u64, FlowStage)> {
    use FlowStage::*;
    match event_record.payload {
        fxt::EventPayload::FlowBegin { id } => Some((id, Begin)),
        fxt::EventPayload::FlowStep { id } => Some((id, Step)),
        fxt::EventPayload::FlowEnd { id } => Some((id, End)),
        _ => None,
    }
}

/// Look for a named argument to an event record.
fn event_record_arg_value<'a>(
    event_record: &'a fxt::EventRecord,
    arg_name: &str,
) -> Option<&'a ArgValue> {
    for arg in &event_record.args {
        if arg.name == arg_name {
            return Some(&arg.value);
        }
    }
    None
}

/// Implements FIDL method symbolization for IPC traces.
pub struct Symbolizer {
    libs: FidlLibraries,
    flow_has_steps: BTreeMap<u64, bool>,
}

impl Symbolizer {
    pub fn new(libs: FidlLibraries) -> Self {
        Self { libs, flow_has_steps: BTreeMap::new() }
    }

    /// Apply FIDL method name symbolization to an event record.
    /// If the record was modified then set the modified reference.
    fn symbolize_event_record(
        &mut self,
        event_record: &fxt::EventRecord,
        parsed_bytes: &[u8],
    ) -> Option<Vec<u8>> {
        if event_record.category.as_str() == "kernel:ipc" {
            // Count how many times we've seen flow begin, step and end for two-way messages.
            if let Some((flow_id, flow_stage)) = event_flow_info(&event_record) {
                if is_two_way_message(flow_id) {
                    // If we see a flow message make sure that we have an entry for its flow id.
                    let flow_has_steps = self.flow_has_steps.entry(flow_id).or_default();

                    // If it's a FlowStep message record that.
                    if flow_stage == FlowStage::Step {
                        *flow_has_steps = true;
                    }
                }
            }

            // Attach method names to DurationBegin events with ordinals.
            if let Some(ArgValue::Unsigned64(ordinal)) =
                event_record_arg_value(&event_record, ORDINAL_ARG_NAME)
            {
                let ordinal = *ordinal;

                let payload = if let Some(ArgValue::Blob(bytes)) =
                    event_record_arg_value(&event_record, BYTES_ARG_NAME)
                {
                    let handles = if let Some(ArgValue::Blob(handles)) =
                        event_record_arg_value(&event_record, HANDLES_ARG_NAME)
                    {
                        handles.to_owned()
                    } else {
                        vec![]
                    };
                    Some(self.libs.decode_message(ordinal, &bytes, &handles))
                } else {
                    None
                };

                if let Some(method_name) = self.libs.get(ordinal) {
                    return match symbolize_fidl_call(
                        &parsed_bytes,
                        ordinal,
                        &method_name,
                        payload,
                        false,
                    ) {
                        Ok(bytes) => Some(bytes),
                        Err(e) => {
                            warn!("Failed to symbolize fidl call: {:#}", e);
                            None
                        }
                    };
                }
            }
        }
        None
    }
}

fn symbolize_fidl_call<'a>(
    bytes: &[u8],
    ordinal: u64,
    method: &'a str,
    payload: Option<String>,
    _preserve_raw_message: bool,
) -> anyhow::Result<Vec<u8>> {
    let (_, mut raw_event_record) =
        RawEventRecord::parse(bytes).context("Unable to parse event record")?;
    raw_event_record.name = StringRef::Inline(method);
    let mut new_args = vec![];
    for arg in &raw_event_record.args {
        if let &RawArgValue::Unsigned64(arg_value) = &arg.value {
            if arg_value == ordinal {
                let symbolized_arg = RawArg {
                    name: StringRef::Inline("method"),
                    value: RawArgValue::String(StringRef::Inline(method)),
                };
                new_args.push(symbolized_arg);
            }
        }
        new_args.push(arg.clone());
    }
    if let Some(payload) = payload.as_ref() {
        new_args.push(RawArg {
            name: StringRef::Inline("payload"),
            value: RawArgValue::String(StringRef::Inline(payload)),
        });
    }

    raw_event_record.args = new_args;
    Ok(raw_event_record.serialize()?)
}

pub struct CategoryCounter {
    category_counter: HashMap<String, usize>,
    input_categories: Vec<String>,
    wildcard_categories: Vec<String>,
}

impl CategoryCounter {
    pub fn new(input_categories: Vec<String>) -> Self {
        let mut category_counter = HashMap::new();
        let mut wildcard_categories = Vec::new();
        for category in &input_categories {
            // Skip categories starting with '#' since they represent groups.
            // We don't check them because it's not explicitly specified.
            if !category.starts_with("#") {
                category_counter.insert(category.clone(), 0);
            }
            // Handle wildcard categories.
            if category.ends_with("*") {
                wildcard_categories.push(category.clone());
            }
        }
        // "kernel:meta" and "kernel:sched" produces metadata which doesn't contain a category, so
        // we ignore them.
        category_counter.remove("kernel:meta");
        category_counter.remove("kernel:sched");
        // "starnix:atrace" delivers events via Perfetto blobs, so the category check will fail.
        category_counter.remove("starnix:atrace");
        Self { category_counter, input_categories, wildcard_categories }
    }

    fn increment_category(&mut self, category: &str) {
        *self.category_counter.entry(category.to_string()).or_insert(0) += 1;

        // Handle wildcard categories.
        for wildcard_category in &self.wildcard_categories {
            if category.starts_with(&wildcard_category[..wildcard_category.len() - 1]) {
                *self.category_counter.entry(wildcard_category.clone()).or_insert(0) += 1;
            }
        }

        // The "kernel" category is a special meta category that enables all "kernel:*" categories.
        // If we see any "kernel:" category, also consider "kernel" to be seen.
        if category.starts_with("kernel:") {
            *self.category_counter.entry("kernel".into()).or_insert(0) += 1;
        }
    }

    fn get_invalid_category_list(&self) -> Vec<String> {
        self.category_counter
            .iter()
            .filter(|&(_, &count)| count == 0)
            .map(|(category, _)| category.clone())
            .collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[fuchsia::test]
    async fn test_verify_missing() {
        let mut counter =
            CategoryCounter::new(vec!["some".into(), "other".into(), "categories".into()]);
        counter.increment_category("some");
        counter.increment_category("other");
        let missing = counter.get_invalid_category_list();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0], "categories");
    }

    #[fuchsia::test]
    async fn test_verify_kernel_category() {
        let mut counter = CategoryCounter::new(vec![
            "kernel".into(),
            "some".into(),
            "other".into(),
            "categories".into(),
        ]);
        counter.increment_category("some");
        counter.increment_category("other");
        counter.increment_category("categories");
        counter.increment_category("kernel:ipc");
        assert!(counter.get_invalid_category_list().is_empty());
    }

    #[fuchsia::test]
    async fn test_verify_kernel_meta() {
        // Kernel:meta and kernel:sched don't produce events with categories, so we should ignore
        // them.
        let mut counter = CategoryCounter::new(vec!["kernel:meta".into(), "kernel:sched".into()]);
        counter.increment_category("some");
        counter.increment_category("other");
        counter.increment_category("categories");
        assert!(counter.get_invalid_category_list().is_empty());
    }

    #[fuchsia::test]
    async fn test_verify_starnix_atrace() {
        // starnix:atrace doesn't produce events with categories, so we should ignore it.
        let mut counter = CategoryCounter::new(vec!["starnix:atrace".into()]);
        counter.increment_category("some");
        counter.increment_category("other");
        counter.increment_category("categories");
        assert!(counter.get_invalid_category_list().is_empty());
    }

    #[fuchsia::test]
    async fn test_verify_wildcard_categories() {
        let mut counter = CategoryCounter::new(vec![
            "starnix:*".into(),
            "some:*".into(),
            "other:category".into(),
        ]);
        counter.increment_category("starnix:atrace");
        counter.increment_category("some:ipc");
        let missing = counter.get_invalid_category_list();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0], "other:category");
    }
}
