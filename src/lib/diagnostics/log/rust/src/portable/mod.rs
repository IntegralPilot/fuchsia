// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

use crate::{PublishOptions, Severity};
use fidl_fuchsia_diagnostics_types as fdiagnostics;
use fuchsia_sync::Mutex;
use std::collections::HashSet;
use std::io::Write;
use std::marker::PhantomData;
use std::sync::Once;
use std::time::SystemTime;
use thiserror::Error;

/// Tag derived from metadata.
///
/// Unlike tags, metatags are not represented as strings and instead must be resolved from event
/// metadata. This means that they may resolve to different text for different events.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Metatag {
    /// The location of a span or event.
    ///
    /// The target is typically a module path, but this can be configured by a particular span or
    /// event when it is constructed.
    Target,
}

/// Errors arising while forwarding a diagnostics stream to the environment.
#[derive(Debug, Error)]
pub enum PublishError {
    // Empty for future extensibility.
}

/// Options to configure a `Publisher`.
pub struct PublisherOptions<'t> {
    pub(crate) interest: fdiagnostics::Interest,
    pub(crate) metatags: HashSet<Metatag>,
    pub(crate) tags: &'t [&'t str],
    // Just for compatibility with the fuchsia struct which needs a lifetime.
    _lifetime: PhantomData<&'t ()>,
}

impl Default for PublisherOptions<'_> {
    fn default() -> Self {
        Self {
            interest: fdiagnostics::Interest::default(),
            metatags: HashSet::new(),
            tags: &[],
            _lifetime: PhantomData,
        }
    }
}

impl Default for PublishOptions<'static> {
    fn default() -> Self {
        Self {
            publisher: PublisherOptions::default(),
            install_panic_hook: true,
            panic_prefix: None,
        }
    }
}

/// Initializes logging. This should be called only once.
pub fn initialize(opts: PublishOptions<'_>) -> Result<(), PublishError> {
    static START: Once = Once::new();
    START.call_once(|| {
        let logger = create_logger(&opts, std::io::stderr());
        log::set_boxed_logger(Box::new(logger)).expect("set logger");
        let severity: Severity =
            opts.publisher.interest.min_severity.unwrap_or(fdiagnostics::Severity::Info).into();
        log::set_max_level(severity.into());
        if opts.install_panic_hook {
            crate::install_panic_hook(opts.panic_prefix);
        }
    });
    Ok(())
}

pub(crate) struct HostLogger<W> {
    writable: Mutex<W>,
    format: FormatOpts,
}

impl<W: Write + Send + 'static> HostLogger<W> {
    /// Constructs a new `HostLogger` with a minimum severity configured.
    fn new(writer: W, format: FormatOpts) -> Self
    where
        W: Write + Send + 'static,
    {
        Self { writable: Mutex::new(writer), format }
    }
}

impl<W: Write + Send + 'static> log::Log for HostLogger<W> {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let mut writer = self.writable.lock();
            let _ =
                write!(writer, "{}", chrono::DateTime::<chrono::Local>::from(SystemTime::now()));
            let _ = write!(writer, " {} ", record.metadata().level());

            if !self.format.tags.is_empty() {
                let _ = write!(writer, "[{}] ", self.format.tags.join(", "));
            }

            if self.format.display_module_path {
                let _ = write!(writer, "{}: ", record.metadata().target());
            }

            let _ = write!(writer, "{}", record.args());
            let mut visitor = StringVisitor(&mut *writer);
            let _ = record.key_values().visit(&mut visitor);

            let _ = writeln!(writer);
        }
    }

    fn flush(&self) {
        let _ = self.writable.lock().flush();
    }
}

struct StringVisitor<'a, W>(&'a mut W);

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

/// Sets the global minimum log severity.
/// IMPORTANT: this function can panic if `initialize` wasn't called before.
pub fn set_minimum_severity(severity: impl Into<log::LevelFilter>) {
    log::set_max_level(severity.into());
}

fn create_logger<W>(opts: &PublishOptions<'_>, w: W) -> HostLogger<W>
where
    W: Write + Send + 'static,
{
    HostLogger::new(
        w,
        FormatOpts {
            tags: opts.publisher.tags.iter().map(|s| s.to_string()).collect(),
            display_module_path: opts.publisher.metatags.contains(&Metatag::Target),
        },
    )
}

/// Implements a compact formatter which also knows about tags.
struct FormatOpts {
    tags: Vec<String>,
    display_module_path: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_sync::Mutex;
    use log::{error, info, warn};
    use regex::Regex;
    use std::io;
    use std::sync::{Arc, LazyLock};

    struct MockWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for MockWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.lock().flush()
        }
    }

    static LOGGER: LazyLock<TestLogger> = LazyLock::new(|| {
        let this = TestLogger { inner: Arc::new(Mutex::new(None)) };
        log::set_boxed_logger(Box::new(this.clone())).expect("set logger");
        log::set_max_level(log::LevelFilter::Info);
        this
    });

    #[derive(Clone)]
    struct TestLogger {
        inner: Arc<Mutex<Option<HostLogger<MockWriter>>>>,
    }

    impl TestLogger {
        fn replace(&self, logger: HostLogger<MockWriter>) {
            LOGGER.inner.lock().replace(logger);
        }
    }

    impl log::Log for TestLogger {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            let guard = self.inner.lock();
            guard.as_ref().unwrap().enabled(metadata)
        }

        fn log(&self, record: &log::Record<'_>) {
            let guard = self.inner.lock();
            guard.as_ref().unwrap().log(record);
        }

        fn flush(&self) {
            let guard = self.inner.lock();
            guard.as_ref().unwrap().flush();
        }
    }

    #[test]
    fn minimal_logging() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let buf = buffer.clone();
        LOGGER.replace(create_logger(&PublishOptions::default(), MockWriter(buf.clone())));
        info!(key = 2; "this is a test");
        let buf = buffer.lock();
        let re = Regex::new(".+INFO this is a test key=2\n$").unwrap();
        let result = std::str::from_utf8(buf.as_slice()).unwrap();
        assert!(re.is_match(result), "got: {:?}", result);
    }

    #[test]
    fn supports_tags() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let buf = buffer.clone();
        LOGGER.replace(create_logger(
            &PublishOptions::default().tags(&["hello", "fuchsia"]),
            MockWriter(buf.clone()),
        ));
        info!(key = 2; "this is a test");
        let buf = buffer.lock();
        let re = Regex::new(".+ INFO \\[hello, fuchsia\\] this is a test key=2\n$").unwrap();
        let result = std::str::from_utf8(buf.as_slice()).unwrap();
        assert!(re.is_match(result), "got: {:?}", result);
    }

    #[test]
    fn supports_module_path() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let buf = buffer.clone();
        LOGGER.replace(create_logger(
            &PublishOptions::default().enable_metatag(Metatag::Target),
            MockWriter(buf.clone()),
        ));
        warn!(key = 2; "this is a test");
        let buf = buffer.lock();
        let re =
            Regex::new(".+WARN diagnostics_log_lib_test::portable::tests: this is a test key=2\n$")
                .unwrap();
        let result = std::str::from_utf8(buf.as_slice()).unwrap();
        assert!(re.is_match(result), "got: {:?}", result);
    }

    #[test]
    fn full_logging() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let buf = buffer.clone();
        LOGGER.replace(create_logger(
            &PublishOptions::default().enable_metatag(Metatag::Target).tags(&["hello", "fuchsia"]),
            MockWriter(buf.clone()),
        ));

        error!(key = 2; "this is a test");
        let buf = buffer.lock();
        let re = Regex::new(concat!(
            ".+ERROR \\[hello, fuchsia\\] diagnostics_log_lib_test::portable::tests: ",
            "this is a test key=2\n$"
        ))
        .unwrap();
        let result = std::str::from_utf8(buf.as_slice()).unwrap();
        assert!(re.is_match(result), "got: {:?}", result);
    }
}
