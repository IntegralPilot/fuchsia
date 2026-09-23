// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{FormatOpts, format};
use std::io::Write;
use std::marker::{Send, Sync};
use std::sync::{Arc, Mutex};

pub trait LogSinkTrait: Send + 'static + Sync {
    fn write_record(&self, opts: &FormatOpts, record: &log::Record<'_>);
    fn flush(&self);
    fn boxed(self) -> Box<dyn LogSinkTrait + Send + Sync + 'static>
    where
        Self: Sized,
        Self: LogSinkTrait + Send + Sync + 'static,
    {
        Box::new(self)
    }
}

pub struct FfxLogSink<W: Write> {
    writable: Arc<Mutex<W>>,
}

impl<W: Write + Send + 'static> FfxLogSink<W> {
    pub fn new(w: Arc<Mutex<W>>) -> Self {
        Self { writable: w }
    }
}

impl<W: Write + Send + 'static> LogSinkTrait for FfxLogSink<W> {
    fn write_record(&self, opts: &FormatOpts, record: &log::Record<'_>) {
        let mut writer = self.writable.lock().unwrap();
        let _ = format::format_record(opts, &mut (*writer), record);
        let mut visitor = StringVisitor::new(&mut *writer);
        let _ = record.key_values().visit(&mut visitor);

        let _ = writeln!(writer);
    }

    fn flush(&self) {
        let _ = self.writable.lock().unwrap().flush();
    }
}

pub(crate) struct StringVisitor<'a, W>(&'a mut W);

impl<'a, W> StringVisitor<'a, W> {
    pub(crate) fn new(writer: &'a mut W) -> Self {
        Self(writer)
    }
}

impl<W: Write> log::kv::VisitSource<'_> for StringVisitor<'_, W> {
    fn visit_pair(
        &mut self,
        key: log::kv::Key<'_>,
        value: log::kv::Value<'_>,
    ) -> Result<(), log::kv::Error> {
        value.visit(StringValueVisitor { buf: self.0, key: key.as_str() })
    }
}

struct StringValueVisitor<'a, W> {
    buf: &'a mut W,
    key: &'a str,
}

impl<W: Write> log::kv::VisitValue<'_> for StringValueVisitor<'_, W> {
    fn visit_any(&mut self, value: log::kv::Value<'_>) -> Result<(), log::kv::Error> {
        write!(self.buf, " {}={}", self.key, value).expect("writing into strings does not fail");
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use log::{Level, Record};

    //////////////////////////////////////////////////////////////////////
    /// Visitors
    ///

    #[test]
    fn test_stringvaluevisitor_writes_nothing_if_no_kv() {
        let mut buf = vec![];
        let mut visitor = StringVisitor::new(&mut buf);
        let record = Record::builder()
            .args(format_args!("Error!"))
            .level(Level::Error)
            .target("myApp")
            .file(Some("server.rs"))
            .line(Some(144))
            .module_path(Some("server"))
            .build();
        record.key_values().visit(&mut visitor).unwrap();
        assert_eq!(buf, vec![]);
    }

    #[test]
    fn test_stringvaluevisitor_kvs() {
        let mut buf = vec![];
        let mut visitor = StringVisitor::new(&mut buf);

        let source = &[("a", 1), ("b", 2), ("c", 3)];
        let record = Record::builder()
            .args(format_args!("Error!"))
            .level(Level::Error)
            .target("myApp")
            .file(Some("server.rs"))
            .line(Some(144))
            .key_values(source)
            .module_path(Some("server"))
            .build();
        record.key_values().visit(&mut visitor).unwrap();
        assert_eq!(String::from_utf8(buf).expect("Valid UTF8"), " a=1 b=2 c=3".to_string());
    }
}
