// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::{MachineWriter, Result, SimpleWriter, TestBuffers, ToolIO};
use async_trait::async_trait;
use fho::{FhoEnvironment, TryFromEnv};
use serde::Serialize;
use serde_json::Value;
use std::fmt::Display;
use std::io::Write;
use writer::{Format, format_output};

/// Structured output writer with schema.
/// [`VerifiedMachineWriter`] is used to provide
/// structured output and a schema that describes the structure.
pub struct VerifiedMachineWriter<T> {
    machine_writer: MachineWriter<T>,
}

impl<T> VerifiedMachineWriter<T>
where
    T: Serialize + schemars::JsonSchema,
{
    pub fn new_buffers<'a, O, E>(format: Option<Format>, stdout: O, stderr: E) -> Self
    where
        O: Write + 'static,
        E: Write + 'static,
    {
        Self { machine_writer: MachineWriter::new_buffers(format, stdout, stderr) }
    }

    /// Create a new Writer with the specified format.
    ///
    /// Passing None for format implies no output via the machine function.
    pub fn new(format: Option<Format>) -> Self {
        Self { machine_writer: MachineWriter::new(format) }
    }

    /// Returns a writer backed by string buffers that can be extracted after
    /// the writer is done with
    pub fn new_test(format: Option<Format>, test_buffers: &TestBuffers) -> Self {
        Self::new_buffers(format, test_buffers.stdout.clone(), test_buffers.stderr.clone())
    }

    /// Write the items from the iterable object to standard output.
    ///
    /// This is a no-op if `is_machine` returns false.
    pub fn machine_many<I: IntoIterator<Item = T>>(&mut self, output: I) -> Result<()> {
        self.machine_writer.machine_many(output)
    }

    /// Write the item to standard output.
    ///
    /// This is a no-op if `is_machine` returns false.
    pub fn machine(&mut self, output: &T) -> Result<()> {
        self.machine_writer.machine(output)
    }

    /// If this object is outputting machine output, print the item's machine
    /// representation to stdout. Otherwise, print the display item given.
    pub fn machine_or<D: Display>(&mut self, value: &T, or: D) -> Result<()> {
        self.machine_writer.machine_or(value, or)
    }

    /// If this object is outputting machine output, print the item's machine
    /// representation to stdout. Otherwise, `write!` the display item given.
    pub fn machine_or_write<D: Display>(&mut self, value: &T, or: D) -> Result<()> {
        self.machine_writer.machine_or_write(value, or)
    }

    /// If this object is outputting machine output, prints the item's machine
    /// representation to stdout. Otherwise, call the closure with the object
    /// and print the result.
    pub fn machine_or_else<F, R>(&mut self, value: &T, f: F) -> Result<()>
    where
        F: FnOnce() -> R,
        R: Display,
    {
        self.machine_writer.machine_or_else(value, f)
    }

    /// Allow using this writer as a SimpleWriter.
    /// This is useful for calling other tools that do not use MachineWriter.
    pub fn simple_writer(self) -> SimpleWriter {
        self.machine_writer.simple_writer()
    }

    /// Validates the passed in data object against the schema
    /// for the writer type.
    pub fn verify_schema(data: &Value) -> Result<()> {
        let s = schemars::schema_for!(T);
        let mut raw_schema: Vec<u8> = vec![];
        format_output(Format::JsonPretty, &mut raw_schema, &s)
            .map_err(|e| crate::Error::SchemaFailure(format!("err: {e:?} for {s:?}")))?;
        let schema_val: Value = serde_json::from_slice(&raw_schema).map_err(|e| {
            crate::Error::SchemaFailure(format!(
                "err: {e:?} for {:?}",
                String::from_utf8(raw_schema)
            ))
        })?;

        let mut scope = valico::json_schema::Scope::new();
        let schema = scope
            .compile_and_return(schema_val, false)
            .map_err(|e| crate::Error::SchemaFailure(format!("Error compiling schema: {e:?}")))?;
        let state = schema.validate(data);

        if !state.is_valid() {
            return Err(crate::Error::SchemaFailure(format!("{:?}", state.errors)));
        }
        Ok(())
    }
}

impl<T> ToolIO for VerifiedMachineWriter<T>
where
    T: Serialize + schemars::JsonSchema,
{
    type OutputItem = T;

    fn supports_structured_output() -> bool {
        true
    }

    fn has_schema() -> bool {
        true
    }

    fn is_machine(&self) -> bool {
        self.machine_writer.is_machine()
    }

    fn try_print_schema(&mut self) -> Result<()> {
        let s = schemars::schema_for!(T);
        self.machine_writer.formatted(&s)
    }

    fn item(&mut self, value: &T) -> Result<()>
    where
        T: Display,
    {
        self.machine_writer.item(value)
    }

    fn stderr(&mut self) -> &mut dyn Write {
        self.machine_writer.stderr()
    }
}

impl<T> Write for VerifiedMachineWriter<T>
where
    T: Serialize,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.machine_writer.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.machine_writer.flush()
    }
}

#[async_trait(?Send)]
impl<T: serde::Serialize + schemars::JsonSchema> TryFromEnv for VerifiedMachineWriter<T> {
    type Error = std::convert::Infallible;
    async fn try_from_env(env: &FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        let format = env.ffx_command().global.machine.and_then(|mf| mf.into());
        let format = match format {
            Some(Format::Raw) => None,
            f => f,
        };
        Ok(VerifiedMachineWriter::new(format))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use schemars::JsonSchema;

    #[test]
    fn test_not_machine_is_ok() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(None, &test_buffers);
        let res = writer.machine(&"ehllo");
        assert!(res.is_ok());
    }

    #[test]
    fn test_machine_valid_json_is_ok() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(Some(Format::Json), &test_buffers);
        let res = writer.machine(&"ehllo");
        assert!(res.is_ok());
    }

    #[test]
    fn writer_implements_write() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(None, &test_buffers);
        writer.write_all(b"foobar").unwrap();
        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "foobar");
        assert_eq!(stderr, "");
    }

    #[test]
    fn writer_implements_write_ignored_on_machine() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(Some(Format::Json), &test_buffers);
        writer.write_all(b"foobar").unwrap();
        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, "");
    }

    #[test]
    fn test_item_for_test_as_machine() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(Some(Format::Json), &test_buffers);
        writer.item(&"hello").unwrap();
        writer.machine_or(&"hello again", "but what if").unwrap();
        writer.machine_or_else(&"hello forever", || "but what if else").unwrap();
        assert_eq!(
            test_buffers.into_stdout_str(),
            "\"hello\"\n\"hello again\"\n\"hello forever\"\n"
        );
    }

    #[test]
    fn test_item_for_test_as_not_machine() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(None, &test_buffers);
        writer.item(&"hello").unwrap();
        writer.machine_or(&"hello again", "but what if").unwrap();
        writer.machine_or_else(&"hello forever", || "but what if else").unwrap();
        assert_eq!(test_buffers.into_stdout_str(), "hello\nbut what if\nbut what if else\n");
    }

    #[test]
    fn test_item_for_test_as_machine_write() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(Some(Format::Json), &test_buffers);
        writer.item(&"hello").unwrap();
        writer.machine_or_write(&"hello again", "but what if").unwrap();
        assert_eq!(test_buffers.into_stdout_str(), "\"hello\"\n\"hello again\"\n");
    }

    #[test]
    fn test_item_for_test_as_not_machine_write() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(None, &test_buffers);
        writer.item(&"hello").unwrap();
        writer.machine_or_write(&"hello again", "but what if").unwrap();
        assert_eq!(test_buffers.into_stdout_str(), "hello\nbut what if");
    }

    #[test]
    fn test_machine_for_test() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(Some(Format::Json), &test_buffers);
        writer.machine(&"hello").unwrap();
        assert_eq!(test_buffers.into_stdout_str(), "\"hello\"\n");
    }
    #[test]
    fn test_not_machine_for_test_is_empty() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(None, &test_buffers);
        writer.machine(&"hello").unwrap();
        assert!(test_buffers.into_stdout_str().is_empty());
    }

    #[test]
    fn test_machine_makes_is_machine_true() {
        let test_buffers = TestBuffers::default();
        let writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(Some(Format::Json), &test_buffers);
        assert!(writer.is_machine());
    }

    #[test]
    fn test_not_machine_makes_is_machine_false() {
        let test_buffers = TestBuffers::default();
        let writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(None, &test_buffers);
        assert!(!writer.is_machine());
    }

    #[test]
    fn line_writer_for_machine_is_ok() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(Some(Format::Json), &test_buffers);
        writer.line("hello").unwrap();
        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, "");
    }

    #[test]
    fn writer_write_for_machine_is_ok() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(Some(Format::Json), &test_buffers);
        writer.print("foobar").unwrap();
        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, "");
    }

    #[test]
    fn writer_print_output_has_no_newline() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(None, &test_buffers);
        writer.print("foobar").unwrap();
        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "foobar");
        assert_eq!(stderr, "");
    }

    #[test]
    fn writing_errors_goes_to_the_right_stream() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<&str> =
            VerifiedMachineWriter::new_test(None, &test_buffers);
        writeln!(writer.stderr(), "hello").unwrap();
        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, "hello\n");
    }

    #[test]
    fn test_machine_writes_pretty_json() {
        let test_buffers = TestBuffers::default();
        let mut writer: VerifiedMachineWriter<serde_json::Value> =
            VerifiedMachineWriter::new_test(Some(Format::JsonPretty), &test_buffers);
        let test_input = serde_json::json!({
            "object1": {
                "line1": "hello",
                "line2": "foobar"
            }
        });
        writer.machine(&test_input).unwrap();
        assert_eq!(
            test_buffers.into_stdout_str(),
            r#"{
  "object1": {
    "line1": "hello",
    "line2": "foobar"
  }
}"#
        );
    }

    #[derive(Serialize, JsonSchema)]
    struct SampleData {
        pub name: String,
        pub size: Option<u16>,
        pub error: Option<String>,
    }

    #[test]
    fn test_verified_machine_writer() {
        let test_buffers = TestBuffers::default();
        let data = SampleData { name: "The Name".into(), size: Some(10), error: None };
        let mut writer =
            VerifiedMachineWriter::<SampleData>::new_test(Some(Format::JsonPretty), &test_buffers);
        writer.machine(&data).expect("machine data written");
        let s = schemars::schema_for!(SampleData);
        let mut schema: Vec<u8> = vec![];
        format_output(Format::JsonPretty, &mut schema, &s).expect("schema as string");
        let schema_val = serde_json::from_slice(&schema).expect("string to schema value");
        let mut scope = valico::json_schema::Scope::new();
        let schema = scope
            .compile_and_return(schema_val, false)
            .unwrap_or_else(|e| panic!("Schema  is invalid {e:?}: {s:?}"));
        let input = test_buffers.into_stdout_str();
        let input_val = serde_json::from_str(&input).expect("json string to Value");
        let state = schema.validate(&input_val);
        assert!(state.is_valid(), "Unexpected validation errors: {:?}", state.errors);
    }

    #[test]
    fn test_verified_machine_writer_schema() {
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<SampleData>::new_test(Some(Format::JsonPretty), &test_buffers);
        writer.try_print_schema().expect("printed schema");
        let actual = test_buffers.into_stdout_str();
        let expected = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "SampleData",
  "type": "object",
  "required": [
    "name"
  ],
  "properties": {
    "name": {
      "type": "string"
    },
    "size": {
      "type": [
        "integer",
        "null"
      ],
      "format": "uint16",
      "minimum": 0.0
    },
    "error": {
      "type": [
        "string",
        "null"
      ]
    }
  }
}"#;
        assert_eq!(actual, expected);
    }
}
