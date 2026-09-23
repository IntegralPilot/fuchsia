// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use crate::filter::*;
pub use crate::format::*;
pub use crate::log_sink::*;
use std::io::Write;
use std::marker::{Send, Sync};

mod filter;
mod format;
mod log_sink;
pub mod test;

pub struct FfxLog<F> {
    format: FormatOpts,
    sinks: Vec<Box<dyn LogSinkTrait>>,
    main_filter: F,
    level: log::LevelFilter,
    targets_filter: TargetsFilter,
}

impl<F: Filter + Send + Sync + 'static> FfxLog<F> {
    /// Constructs a new `FfxLog` with a minimum severity configured.
    pub fn new(
        sinks: Vec<Box<dyn LogSinkTrait>>,
        format: FormatOpts,
        main_filter: F,
        level: log::LevelFilter,
        targets_filter: TargetsFilter,
    ) -> Self {
        Self { sinks, format, main_filter, level, targets_filter }
    }
}

impl<F: Filter + Sync + Send + 'static> log::Log for FfxLog<F> {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        self.main_filter.should_emit(metadata)
            && self.targets_filter.should_emit(metadata)
            && self.level.should_emit(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            for sink in &self.sinks {
                sink.write_record(&self.format, record);
            }
        }
    }

    fn flush(&self) {
        for sink in &self.sinks {
            sink.flush()
        }
    }
}

#[derive(Default, Debug)]
pub struct TestWriter {
    _p: (),
}

impl TestWriter {
    /// Returns a new `TestWriter` with the default configuration.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Write for TestWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let out_str = String::from_utf8_lossy(buf);
        print!("{}", out_str);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{FfxLog, FfxLogSink, Filter, FormatOpts, LogSinkTrait, TargetsFilter};
    use log::{Level, Log, Record};
    use std::sync::{Arc, Mutex, RwLock};

    //////////////////////////////////////////////////////////////////////
    /// FfxLog
    ///

    struct SwitchFilter {
        should: Arc<RwLock<bool>>,
    }

    impl Filter for SwitchFilter {
        fn should_emit(&self, _metadata: &log::Metadata<'_>) -> bool {
            match *self.should.read().unwrap() {
                true => true,
                false => false,
            }
        }
    }

    #[test]
    fn test_ffx_log_emitting() {
        let mut sinks: Vec<Box<dyn LogSinkTrait>> = vec![];

        let buf1 = Arc::new(Mutex::new(Vec::<u8>::new()));
        let buf2 = Arc::new(Mutex::new(Vec::<u8>::new()));
        sinks.push(FfxLogSink::new(buf1.clone()).boxed());
        sinks.push(FfxLogSink::new(buf2.clone()).boxed());

        let should = Arc::new(RwLock::new(true));
        let switch_filter = SwitchFilter { should: should.clone() };
        let opts = FormatOpts::new(1234);

        let level_filter = log::LevelFilter::Trace;
        let targets_filter = TargetsFilter::new(vec![]);

        let log = FfxLog::new(sinks, opts, switch_filter, level_filter, targets_filter);

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
        log.log(&record);

        let buf1_done = buf1.lock().unwrap();
        let buf2_done = buf2.lock().unwrap();

        assert!(!String::from_utf8((buf1_done).to_vec()).expect("Valid UTF8").is_empty());
        assert!(!String::from_utf8((buf2_done).to_vec()).expect("Valid UTF8").is_empty());
    }

    #[test]
    fn test_ffx_log_no_emitting() {
        let mut sinks: Vec<Box<dyn LogSinkTrait>> = vec![];

        let buf1 = Arc::new(Mutex::new(Vec::<u8>::new()));
        sinks.push(FfxLogSink::new(buf1.clone()).boxed());

        let should = Arc::new(RwLock::new(true));
        let binding = Arc::clone(&should);
        let switch_filter = SwitchFilter { should: Arc::clone(&should) };
        let opts = FormatOpts::new(1234);

        let level_filter = log::LevelFilter::Trace;
        let targets_filter = TargetsFilter::new(vec![]);

        let log = FfxLog::new(sinks, opts, switch_filter, level_filter, targets_filter);

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
        assert!(log.enabled(record.metadata()));
        log.log(&record);
        {
            let mut buf = buf1.lock().unwrap();
            assert!(buf.len() > 0);
            buf.clear();
        }
        {
            let mut sg = binding.write().unwrap();
            *sg = false;
        }
        assert!(!log.enabled(record.metadata()));
        log.log(&record);
        {
            let buf = buf1.lock().unwrap();
            assert!(buf.len() == 0);
        }
    }
}
