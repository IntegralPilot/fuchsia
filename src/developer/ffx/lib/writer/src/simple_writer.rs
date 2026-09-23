// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Format, Result, ToolIO};
use async_trait::async_trait;
use fho::{FhoEnvironment, TryFromEnv};
use std::io::Write;
use writer::{TestBuffers, Writer};

/// An object that can be used to produce output, with support for the raw machine
/// format.
pub struct SimpleWriter(Writer);

impl From<writer::Writer> for SimpleWriter {
    fn from(value: writer::Writer) -> Self {
        SimpleWriter(value)
    }
}

impl SimpleWriter {
    /// Create a new writer that doesn't support machine output at all, with the
    /// given streams underlying it.
    pub fn new_buffers<'a, O, E>(stdout: O, stderr: E) -> Self
    where
        O: Write + 'static,
        E: Write + 'static,
    {
        Self(Writer::new_buffers(stdout, stderr))
    }

    /// Create a new Writer that doesn't support machine output at all
    pub fn new() -> Self {
        Self(Writer::new())
    }

    /// Returns a writer backed by string buffers that can be extracted after
    /// the writer is done with
    pub fn new_test(test_writer: &TestBuffers) -> Self {
        Self(Writer::new_test(test_writer))
    }
}

impl ToolIO for SimpleWriter {
    type OutputItem = String;

    fn is_machine(&self) -> bool {
        false
    }

    fn item(&mut self, value: &String) -> Result<()> {
        self.line(value)
    }

    fn stderr(&mut self) -> &mut dyn Write {
        self.0.stderr()
    }
}

impl Write for SimpleWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

#[derive(Debug)]
pub struct InvalidMachineFormat;

impl std::fmt::Display for InvalidMachineFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid machine format: only raw supported")
    }
}

impl std::error::Error for InvalidMachineFormat {}

#[async_trait(?Send)]
impl TryFromEnv for SimpleWriter {
    type Error = InvalidMachineFormat;
    async fn try_from_env(env: &FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        let machine_format: Option<writer::Format> =
            env.ffx_command().global.machine.and_then(|mf| mf.into());
        match machine_format {
            Some(Format::Raw) | None => Ok(SimpleWriter::new()),
            Some(_) => Err(InvalidMachineFormat),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_not_machine_is_ok() {
        let test_buffers = TestBuffers::default();
        let mut writer = SimpleWriter::new_test(&test_buffers);
        let res = writer.item(&"ehllo".to_owned());
        assert!(res.is_ok());
    }

    #[test]
    fn test_item_for_test() {
        let test_buffers = TestBuffers::default();
        let mut writer = SimpleWriter::new_test(&test_buffers);
        writer.item(&"hello".to_owned()).unwrap();

        assert_eq!(test_buffers.into_stdout_str(), "hello\n");
    }

    #[test]
    fn test_supports_structured_output() {
        assert!(!SimpleWriter::supports_structured_output());
        assert!(!SimpleWriter::has_schema());
    }

    #[test]
    fn test_is_machine_false() {
        let test_buffers = TestBuffers::default();
        let writer = SimpleWriter::new_test(&test_buffers);
        assert!(!writer.is_machine());
    }

    #[test]
    fn line_writer_for_machine_is_ok() {
        let test_buffers = TestBuffers::default();
        let mut writer = SimpleWriter::new_test(&test_buffers);
        writer.line("hello").unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "hello\n");
        assert_eq!(stderr, "");
    }

    #[test]
    fn writer_print_output_has_no_newline() {
        let test_buffers = TestBuffers::default();
        let mut writer = SimpleWriter::new_test(&test_buffers);
        writer.print("foobar").unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "foobar");
        assert_eq!(stderr, "");
    }

    #[test]
    fn writer_implements_write() {
        let test_buffers = TestBuffers::default();
        let mut writer = SimpleWriter::new_test(&test_buffers);
        writer.write_all(b"foobar").unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "foobar");
        assert_eq!(stderr, "");
    }

    #[test]
    fn writing_errors_goes_to_the_right_stream() {
        let test_buffers = TestBuffers::default();
        let mut writer = SimpleWriter::new_test(&test_buffers);
        writeln!(writer.stderr(), "hello").unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, "hello\n");
    }

    #[fuchsia::test]
    async fn test_try_from_env() {
        let config_env = ffx_config::test_init().unwrap();

        // No machine flag
        let ffx = fho::FfxCommandLine::new(None, &["ffx", "test"]).unwrap();
        let env = fho::FhoEnvironment::new(&config_env.context, &ffx);
        assert!(SimpleWriter::try_from_env(&env).await.is_ok());

        // --machine raw
        let ffx = fho::FfxCommandLine::new(None, &["ffx", "--machine", "raw", "test"]).unwrap();
        let env = fho::FhoEnvironment::new(&config_env.context, &ffx);
        assert!(SimpleWriter::try_from_env(&env).await.is_ok());

        // --machine json (should fail)
        let ffx = fho::FfxCommandLine::new(None, &["ffx", "--machine", "json", "test"]).unwrap();
        let env = fho::FhoEnvironment::new(&config_env.context, &ffx);
        assert!(SimpleWriter::try_from_env(&env).await.is_err());
    }
}
