// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Format, Result, TestBuffers, ToolIO, Writer};
use serde::Serialize;
use std::fmt::Display;
use std::io::Write;

/// Type-safe machine output implementation of [`crate::ToolIO`]
pub struct JsonWriter<T> {
    format: Option<Format>,
    simple_writer: Writer,
    _p: std::marker::PhantomData<fn(T)>,
}

impl<T> JsonWriter<T> {
    /// Create a new writer that doesn't support machine output at all, with the
    /// given streams underlying it.
    pub fn new_buffers<'a, O, E>(format: Option<Format>, stdout: O, stderr: E) -> Self
    where
        O: Write + 'static,
        E: Write + 'static,
    {
        let simple_writer = Writer::new_buffers(stdout, stderr);
        let _p = Default::default();
        Self { format, simple_writer, _p }
    }

    /// Create a new Writer with the specified format.
    ///
    /// Passing None for format implies no output via the machine function.
    pub fn new(format: Option<Format>) -> Self {
        let simple_writer = Writer::new();
        let _p = Default::default();
        Self { format, simple_writer, _p }
    }

    /// Returns a writer backed by string buffers that can be extracted after
    /// the writer is done with
    pub fn new_test(format: Option<Format>, test_buffers: &TestBuffers) -> Self {
        Self::new_buffers(format, test_buffers.stdout.clone(), test_buffers.stderr.clone())
    }

    /// Allow using this writer as a SimpleWriter.
    /// This is useful for calling other tools that do not use JsonWriter.
    pub fn simple_writer(self) -> Writer {
        self.simple_writer
    }

    pub fn stderr(&mut self) -> &mut dyn Write {
        self.simple_writer.stderr()
    }

    // Always returns Some(Json|JsonPretty) or None
    fn get_machine_format(&self) -> Option<Format> {
        match self.format {
            None | Some(Format::Raw) => None,
            j => j,
        }
    }
}

impl<T> JsonWriter<T>
where
    T: Serialize,
{
    /// Write the items from the iterable object to standard output.
    ///
    /// This is a no-op if `is_machine` returns false.
    pub fn machine_many<I: IntoIterator<Item = T>>(&mut self, output: I) -> Result<()> {
        if self.is_machine() {
            for output in output {
                self.machine(&output)?;
            }
        }
        Ok(())
    }

    /// Write the item to standard output.
    ///
    /// This is a no-op if `is_machine` returns false.
    pub fn machine(&mut self, output: &T) -> Result<()> {
        if let Some(format) = self.get_machine_format() {
            format_output(format, &mut self.simple_writer, output)
        } else {
            Ok(())
        }
    }

    /// If this object is outputting machine output, print the item's machine
    /// representation to stdout. Otherwise, print the display item given.
    pub fn machine_or<D: Display>(&mut self, value: &T, or: D) -> Result<()> {
        if let Some(format) = self.get_machine_format() {
            format_output(format, &mut self.simple_writer, value)?
        } else {
            writeln!(self, "{or}")?
        }
        Ok(())
    }

    /// If this object is outputting machine output, print the item's machine
    /// representation to stdout. Otherwise, `write!` the display item given.
    pub fn machine_or_write<D: Display>(&mut self, value: &T, or: D) -> Result<()> {
        if let Some(format) = self.get_machine_format() {
            format_output(format, &mut self.simple_writer, value)?
        } else {
            write!(self, "{or}")?
        }
        Ok(())
    }

    /// If this object is outputting machine output, prints the item's machine
    /// representation to stdout. Otherwise, call the closure with the object
    /// and print the result.
    pub fn machine_or_else<F, R>(&mut self, value: &T, f: F) -> Result<()>
    where
        F: FnOnce() -> R,
        R: Display,
    {
        match self.format {
            Some(format) => format_output(format, &mut self.simple_writer, value)?,
            None => writeln!(self, "{}", f())?,
        }
        Ok(())
    }

    pub fn format(&self) -> Option<Format> {
        self.format
    }

    pub fn formatted<J: Serialize>(&mut self, output: &J) -> Result<()> {
        if let Some(format) = self.get_machine_format() {
            format_output(format, &mut self.simple_writer, output)
        } else {
            Ok(())
        }
    }
}

pub fn format_output<W: Write, T>(format: Format, mut out: W, output: &T) -> Result<()>
where
    T: Serialize,
{
    match format {
        Format::Json => {
            serde_json::to_writer(&mut out, output)?;
            writeln!(out)?;
            out.flush()?;
        }
        Format::JsonPretty => {
            serde_json::to_writer_pretty(&mut out, output)?;
        }
        Format::Raw => {}
    }
    Ok(())
}

impl<T> ToolIO for JsonWriter<T>
where
    T: Serialize,
{
    type OutputItem = T;

    fn supports_structured_output() -> bool {
        true
    }

    fn is_machine(&self) -> bool {
        match self.format {
            Some(Format::Json) | Some(Format::JsonPretty) => true,
            _ => false,
        }
    }

    fn stderr(&mut self) -> &mut dyn Write {
        self.stderr()
    }

    fn item(&mut self, value: &T) -> Result<()>
    where
        T: Display,
    {
        if self.is_machine() { self.machine(value) } else { self.line(value) }
    }
}

impl<T> Write for JsonWriter<T>
where
    T: Serialize,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.is_machine() { Ok(buf.len()) } else { self.simple_writer.write(buf) }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.is_machine() { Ok(()) } else { self.simple_writer.flush() }
    }
}
#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_not_machine_is_ok() {
        let test_buffers = TestBuffers::default();
        let mut writer: JsonWriter<&str> = JsonWriter::new_test(None, &test_buffers);
        let res = writer.machine(&"ehllo");
        assert!(res.is_ok());
    }

    #[test]
    fn test_machine_valid_json_is_ok() {
        let test_buffers = TestBuffers::default();
        let mut writer: JsonWriter<&str> = JsonWriter::new_test(Some(Format::Json), &test_buffers);
        let res = writer.machine(&"ehllo");
        assert!(res.is_ok());
    }

    #[test]
    fn writer_implements_write() {
        let test_buffers = TestBuffers::default();
        let mut writer: JsonWriter<&str> = JsonWriter::new_test(None, &test_buffers);
        writer.write_all(b"foobar").unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "foobar");
        assert_eq!(stderr, "");
    }

    #[test]
    fn writer_implements_write_ignored_on_machine() {
        let test_buffers = TestBuffers::default();
        let mut writer: JsonWriter<&str> = JsonWriter::new_test(Some(Format::Json), &test_buffers);
        writer.write_all(b"foobar").unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, "");
    }

    #[test]
    fn test_item_for_test_as_machine() {
        let test_buffers = TestBuffers::default();
        let mut writer: JsonWriter<&str> = JsonWriter::new_test(Some(Format::Json), &test_buffers);
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
        let mut writer: JsonWriter<&str> = JsonWriter::new_test(None, &test_buffers);
        writer.item(&"hello").unwrap();
        writer.machine_or(&"hello again", "but what if").unwrap();
        writer.machine_or_else(&"hello forever", || "but what if else").unwrap();

        assert_eq!(test_buffers.into_stdout_str(), "hello\nbut what if\nbut what if else\n");
    }

    #[test]
    fn test_machine_for_test() {
        let test_buffers = TestBuffers::default();
        let mut writer: JsonWriter<&str> = JsonWriter::new_test(Some(Format::Json), &test_buffers);
        writer.machine(&"hello").unwrap();

        assert_eq!(test_buffers.into_stdout_str(), "\"hello\"\n");
    }

    #[test]
    fn test_not_machine_for_test_is_empty() {
        let test_buffers = TestBuffers::default();
        let mut writer: JsonWriter<&str> = JsonWriter::new_test(None, &test_buffers);
        writer.machine(&"hello").unwrap();

        assert!(test_buffers.into_stdout_str().is_empty());
    }

    #[test]
    fn test_machine_makes_is_machine_true() {
        let test_buffers = TestBuffers::default();
        let writer: JsonWriter<&str> = JsonWriter::new_test(Some(Format::Json), &test_buffers);
        assert!(writer.is_machine());
    }

    #[test]
    fn test_not_machine_makes_is_machine_false() {
        let test_buffers = TestBuffers::default();
        let writer: JsonWriter<&str> = JsonWriter::new_test(None, &test_buffers);
        assert!(!writer.is_machine());
    }

    #[test]
    fn line_writer_for_machine_is_ok() {
        let test_buffers = TestBuffers::default();
        let mut writer: JsonWriter<&str> = JsonWriter::new_test(Some(Format::Json), &test_buffers);
        writer.line("hello").unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, "");
    }

    #[test]
    fn writer_write_for_machine_is_ok() {
        let test_buffers = TestBuffers::default();
        let mut writer: JsonWriter<&str> = JsonWriter::new_test(Some(Format::Json), &test_buffers);
        writer.print("foobar").unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, "");
    }

    #[test]
    fn writer_print_output_has_no_newline() {
        let test_buffers = TestBuffers::default();
        let mut writer: JsonWriter<&str> = JsonWriter::new_test(None, &test_buffers);
        writer.print("foobar").unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "foobar");
        assert_eq!(stderr, "");
    }

    #[test]
    fn writing_errors_goes_to_the_right_stream() {
        let test_buffers = TestBuffers::default();
        let mut writer: JsonWriter<&str> = JsonWriter::new_test(None, &test_buffers);
        writeln!(writer.stderr(), "hello").unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, "hello\n");
    }

    #[test]
    fn test_machine_writes_pretty_json() {
        let test_buffers = TestBuffers::default();
        let mut writer: JsonWriter<serde_json::Value> =
            JsonWriter::new_test(Some(Format::JsonPretty), &test_buffers);
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
}
