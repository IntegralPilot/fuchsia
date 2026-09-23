// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::{Context as _, Result};
use ffx_config::EnvironmentContext;
use ffx_config::logging::{DisableableFilter, LOGGING_ID, target_levels_log};
use logging::{FfxLog, FfxLogSink, FormatOpts, LogSinkTrait, TargetsFilter};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, Once, RwLock};

static INIT_LOGGER: Once = Once::new();

pub static LOG_SINK: LazyLock<MultiFileLogger> = LazyLock::new(|| MultiFileLogger::new());

/// Sets up logging if it hasn't already been done. This function is a no-op afterwards.
pub fn init_logging(ctx: &EnvironmentContext) {
    INIT_LOGGER.call_once(move || {
        // For the time being this will always be set to debug despite what is in the config. The
        // first thing to set the log level here is going to win, which should be configurable on a
        // per-file basis if possible. For this first-pass implementation we just want any logs
        // available.
        let level = log::LevelFilter::Debug;
        let target_levels = target_levels_log(ctx);
        let format = FormatOpts::new(*LOGGING_ID);
        let toggle_filter = DisableableFilter;
        let targets_filter = TargetsFilter::new(target_levels);
        let logger = FfxLog::new(
            vec![Box::new(LOG_SINK.clone())],
            format,
            toggle_filter,
            level,
            targets_filter,
        );
        let _ = log::set_boxed_logger(Box::new(logger))
            .map(|()| log::set_max_level(log::LevelFilter::Trace));
    });
}

struct LogSinkSubscription {
    subscriber_count: u32,
    log_sink: Box<dyn LogSinkTrait>,
}

// TODO(b/419634189): Add this as a series of ID's to the logging formatter (currently using the
// same ID as the main ffx_config logging crate.
pub fn log_id(env_ctx: &EnvironmentContext) -> u64 {
    let ptr = env_ctx as *const EnvironmentContext;
    ptr as u64
}

impl LogSinkSubscription {
    fn new(log_sink: Box<dyn LogSinkTrait>, _env_ctx: &EnvironmentContext) -> Self {
        Self { subscriber_count: 1, log_sink }
    }

    fn subscribe(&mut self) -> u32 {
        self.subscriber_count += 1;
        self.subscriber_count
    }

    fn unsubscribe(&mut self) -> u32 {
        if self.subscriber_count > 0 {
            self.subscriber_count -= 1;
        }
        self.subscriber_count
    }
}

impl LogSinkTrait for LogSinkSubscription {
    fn write_record(&self, opts: &FormatOpts, record: &log::Record<'_>) {
        self.log_sink.write_record(opts, record)
    }

    fn flush(&self) {
        self.log_sink.flush()
    }
}

type LogSinkContainer = BTreeMap<PathBuf, LogSinkSubscription>;

#[derive(Clone)]
pub struct MultiFileLogger {
    sinks: Arc<RwLock<LogSinkContainer>>,
}

fn default_log_path(ctx: &EnvironmentContext) -> Result<PathBuf> {
    let mut log_path = log_dir(ctx)?;
    log_path.push("fuchsia-controller");
    log_path.set_extension("log");
    Ok(log_path)
}

fn log_dir(ctx: &EnvironmentContext) -> Result<PathBuf> {
    ctx.get("log.dir").map_err(Into::into)
}

impl MultiFileLogger {
    pub fn new() -> Self {
        Self { sinks: Arc::default() }
    }

    pub fn add_log_output(&self, ctx: &EnvironmentContext) -> Result<()> {
        // DO NOT RUN THIS AFTER GETTING THE LOCK.
        let log_dir = match log_dir(ctx) {
            Ok(l) => l,
            // If no log dir has been set then no logging will happen.
            Err(e) => {
                eprintln!(">>> WARNING <<< logging is not enabled for this context: {e:?}");
                return Ok(());
            }
        };
        let log_path = default_log_path(ctx)?;
        // The above code queries the config, which logs, so do not use the config after acquiring
        // the lock, else it will deadlock.
        let mut sinks = self.sinks.write().unwrap();
        match sinks.entry(log_path.clone()) {
            Entry::Vacant(e) => {
                std::fs::create_dir_all(&log_dir)?;
                let log_file = OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(log_path)
                    .context("opening log file")?;
                let logger_impl = FfxLogSink::new(Arc::new(Mutex::new(log_file))).boxed();
                let logger = LogSinkSubscription::new(logger_impl, ctx);
                e.insert(logger);
            }
            Entry::Occupied(mut e) => {
                e.get_mut().subscribe();
            }
        }
        Ok(())
    }

    pub fn remove_log_output(&self, ctx: &EnvironmentContext) -> Result<()> {
        // DO NOT RUN THIS AFTER GETTING THE LOCK.
        let log_path = match default_log_path(ctx) {
            Ok(l) => l,
            Err(_) => return Ok(()),
        };
        let mut sinks = self.sinks.write().unwrap();
        // The above code queries the config, which logs, so do not use the config after acquiring
        // the lock, else it will deadlock.
        match sinks.entry(log_path.clone()) {
            Entry::Vacant(_) => Err(anyhow::anyhow!(
                "Attempted to remove non-existent logger for path {}",
                log_path.display()
            )),
            Entry::Occupied(mut e) => {
                if e.get_mut().unsubscribe() == 0 {
                    e.remove();
                }
                Ok(())
            }
        }
    }
}

impl LogSinkTrait for MultiFileLogger {
    fn write_record(&self, opts: &FormatOpts, record: &log::Record<'_>) {
        let sinks = self.sinks.read().unwrap();
        for (_, sink) in sinks.iter() {
            sink.write_record(opts, record)
        }
    }

    fn flush(&self) {
        let sinks = self.sinks.read().unwrap();
        for (_, sink) in sinks.iter() {
            sink.flush()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_config::ConfigMap;
    use ffx_config::environment::ExecutableKind;
    use tempfile;

    struct NothingLogger;

    impl LogSinkTrait for NothingLogger {
        fn write_record(&self, _opts: &FormatOpts, _record: &log::Record<'_>) {}
        fn flush(&self) {}
    }

    #[test]
    fn test_subscriber_subscribe() {
        let ctx = EnvironmentContext::strict(ExecutableKind::Test, ConfigMap::new()).unwrap();
        let mut sub = LogSinkSubscription::new(Box::new(NothingLogger), &ctx);
        assert_eq!(sub.subscribe(), 2);
        assert_eq!(sub.unsubscribe(), 1);
        assert_eq!(sub.unsubscribe(), 0);
        assert_eq!(sub.unsubscribe(), 0);
    }

    #[test]
    fn test_file_logger() {
        let log_dir = tempfile::tempdir().unwrap();
        let mut config = ConfigMap::new();
        config.insert(
            "log".to_string(),
            serde_json::json!({"dir": log_dir.path().display().to_string()}),
        );
        let ctx = EnvironmentContext::strict(ExecutableKind::Test, config).unwrap();
        let logger = MultiFileLogger::new();
        logger.add_log_output(&ctx).unwrap();
        assert_eq!(logger.sinks.read().unwrap().keys().len(), 1);
        let mut expect = PathBuf::from(log_dir.path());
        expect.push("fuchsia-controller");
        expect.set_extension("log");
        assert_eq!(logger.sinks.read().unwrap().keys().next().unwrap(), &expect);
        // Adding to the same path shouldn't add additional elements, only additional subscribers.
        logger.add_log_output(&ctx).unwrap();
        assert_eq!(logger.sinks.read().unwrap().keys().len(), 1);
        assert_eq!(logger.sinks.read().unwrap().keys().next().unwrap(), &expect);

        let other_log_dir = tempfile::tempdir().unwrap();
        let mut config = ConfigMap::new();
        config.insert(
            "log".to_string(),
            serde_json::json!({"dir": other_log_dir.path().display().to_string()}),
        );
        let other_ctx = EnvironmentContext::strict(ExecutableKind::Test, config).unwrap();
        logger.add_log_output(&other_ctx).unwrap();
        let mut expect = PathBuf::from(other_log_dir.path());
        expect.push("fuchsia-controller");
        expect.set_extension("log");
        assert_eq!(logger.sinks.read().unwrap().keys().len(), 2);
        assert_eq!(logger.sinks.read().unwrap().get_key_value(&expect).unwrap().0, &expect);
        logger.remove_log_output(&other_ctx).unwrap();
        assert_eq!(logger.sinks.read().unwrap().keys().len(), 1);
        assert!(logger.remove_log_output(&other_ctx).is_err());
        // Multiple removals necessary as there are multiple subscribers.
        logger.remove_log_output(&ctx).unwrap();
        assert_eq!(logger.sinks.read().unwrap().keys().len(), 1);
        logger.remove_log_output(&ctx).unwrap();
        assert_eq!(logger.sinks.read().unwrap().keys().len(), 0);
    }
}
