// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Write;
use std::os::fd::AsFd;
use std::sync::LazyLock;
use zx::{self as zx, ObjectType};

static LOGGER: LazyLock<KernelLogger> = LazyLock::new(KernelLogger::new);

/// KernelLogger is a subscriber implementation for the log crate.
pub struct KernelLogger {
    debuglog: zx::DebugLog,
}

impl KernelLogger {
    /// Make a new `KernelLogger` by cloning our stdout and extracting the debuglog handle from it.
    fn new() -> KernelLogger {
        let debuglog = fdio::clone_fd(std::io::stdout().as_fd()).expect("get handle from stdout");
        assert_eq!(debuglog.basic_info().unwrap().object_type, ObjectType::DEBUGLOG);
        KernelLogger { debuglog: debuglog.into() }
    }

    /// Initialize the global logger to use KernelLogger.
    ///
    /// Registers a panic hook that prints the panic payload to the logger before running the
    /// default panic hook.
    pub fn init() {
        log::set_logger(&*LOGGER).expect("set logger must succeed");
        log::set_max_level(log::LevelFilter::Info);
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log::error!("PANIC {info}");
            previous_hook(info);
        }));
    }
}

impl log::Log for KernelLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        // log levels run the opposite direction of fuchsia severity
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let msg_buffer = format!("{}", record.args());
            let mut visitor = StringVisitor(msg_buffer);
            let _ = record.key_values().visit(&mut visitor);
            let msg_buffer = visitor.0;

            let msg_prefix = format!("[component_manager] {}: ", record.level());

            // &str pointing to the remains of the message.
            let mut msg = msg_buffer.as_str();
            while msg.len() > 0 {
                // Split the message if it contains a newline or is too long for
                // the debug log.
                let mut split_point = if let Some(newline_pos) = msg.find('\n') {
                    newline_pos + 1
                } else {
                    msg.len()
                };
                split_point =
                    std::cmp::min(split_point, zx::sys::ZX_LOG_RECORD_DATA_MAX - msg_prefix.len());

                // Ensure the split point is at a character boundary - splitting
                // in the middle of a unicode character causes a panic.
                while !msg.is_char_boundary(split_point) {
                    split_point -= 1;
                }

                // TODO(https://fxbug.dev/42108144): zx_debuglog_write also accepts options and the possible options include
                // log levels, but they seem to be mostly unused and not displayed today, so we don't pass
                // along log level yet.
                let mut msg_to_write = format!("{}{}", msg_prefix, &msg[..split_point]);

                // If we split at a newline, strip it out.
                if msg_to_write.chars().last() == Some('\n') {
                    msg_to_write.truncate(msg_to_write.len() - 1);
                }

                if let Err(_) = self.debuglog.write(msg_to_write.as_bytes()) {
                    // If we do in fact fail to write to debuglog, then component_manager
                    // has no sink to write messages to. However, it's extremely
                    // unlikely that this error state will ever be hit since
                    // component_manager receives a valid handle from userboot.
                    // Perhaps panicking would be better here?
                }
                msg = &msg[split_point..];
            }
        }
    }

    fn flush(&self) {}
}

struct StringVisitor(String);

impl log::kv::VisitSource<'_> for StringVisitor {
    fn visit_pair(
        &mut self,
        key: log::kv::Key<'_>,
        value: log::kv::Value<'_>,
    ) -> Result<(), log::kv::Error> {
        value.visit(StringValueVisitor { buf: &mut self.0, key: key.as_str() })
    }
}

struct StringValueVisitor<'a> {
    buf: &'a mut String,
    key: &'a str,
}

impl log::kv::VisitValue<'_> for StringValueVisitor<'_> {
    fn visit_any(&mut self, value: log::kv::Value<'_>) -> Result<(), log::kv::Error> {
        write!(self.buf, " {}={}", self.key, value).expect("writing into strings does not fail");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use fidl_fuchsia_boot as fboot;
    use fuchsia_component::client::connect_channel_to_protocol;
    use log::{error, info, warn};
    use rand::RngExt as _;
    use std::panic;

    const MAX_INFO_LINE_LEN: usize =
        zx::sys::ZX_LOG_RECORD_DATA_MAX - "[component_manager] INFO: ".len();

    fn get_readonlylog() -> zx::DebugLog {
        let (client_end, server_end) = zx::Channel::create();
        connect_channel_to_protocol::<fboot::ReadOnlyLogMarker>(server_end).unwrap();
        let service = fboot::ReadOnlyLogSynchronousProxy::new(client_end);
        let log = service.get(zx::MonotonicInstant::INFINITE).expect("couldn't get read only log");
        log
    }

    // expect_message_in_debuglog will read the last 10000 messages in zircon's debuglog, looking
    // for a message that equals `sent_msg`. If found, the function returns. If the first 10,000
    // messages doesn't contain `sent_msg`, it will panic.
    fn expect_message_in_debuglog(sent_msg: String) {
        let debuglog = get_readonlylog();
        for _ in 0..10000 {
            match debuglog.read() {
                Ok(record) => {
                    let log = record.data().to_string();
                    if log.starts_with(&sent_msg) {
                        // We found our log!
                        return;
                    }
                }
                Err(status) if status == zx::Status::SHOULD_WAIT => {
                    debuglog
                        .wait_one(zx::Signals::LOG_READABLE, zx::MonotonicInstant::INFINITE)
                        .expect("Failed to wait for log readable");
                    continue;
                }
                Err(status) => {
                    panic!("Unexpected error from zx_debuglog_read: {}", status);
                }
            }
        }
        panic!("first 10000 log messages didn't include the one we sent!");
    }

    // Userboot passes component manager a debuglog handle tied to fd 0/1/2, which component
    // manager now uses to retrieve the debuglog handle. To simulate that, we need to bind
    // a handle before calling KernelLogger::init().
    fn init() {
        const STDOUT_FD: i32 = 1;

        let resource = zx::Resource::from(zx::NullableHandle::invalid());
        let debuglog = zx::DebugLog::create(&resource, zx::DebugLogOpts::empty())
            .context("Failed to create debuglog object")
            .unwrap();
        fdio::bind_to_fd(debuglog.into_handle(), STDOUT_FD).unwrap();

        KernelLogger::init();
    }

    fn make_str_with_len(prefix: &str, len: usize) -> String {
        let mut rng = rand::rng();
        let mut res = format!("{}{}{}", prefix, rng.random::<u64>().to_string(), "a".repeat(len));
        res.truncate(len);
        res
    }

    #[test]
    fn log_info_test() {
        let mut rng = rand::rng();
        let logged_value: u64 = rng.random();

        init();
        info!("log_test {}", logged_value);

        expect_message_in_debuglog(format!("[component_manager] INFO: log_test {}", logged_value));
    }

    #[test]
    fn log_info_newline_test() {
        let mut rng = rand::rng();
        let logged_value1: u64 = rng.random();
        let logged_value2: u64 = rng.random();

        init();
        info!("log_test1 {}\nlog_test2 {}", logged_value1, logged_value2);

        expect_message_in_debuglog(format!(
            "[component_manager] INFO: log_test1 {}",
            logged_value1
        ));
        expect_message_in_debuglog(format!(
            "[component_manager] INFO: log_test2 {}",
            logged_value2
        ));
    }

    #[test]
    fn log_many_newlines_test() {
        let mut rng = rand::rng();
        let logged_value1: u64 = rng.random();
        let logged_value2: u64 = rng.random();

        init();
        info!("\n\nmnl_log_test1 {}\n\nmnl_log_test2 {}\n\n", logged_value1, logged_value2);

        expect_message_in_debuglog(format!(
            "[component_manager] INFO: mnl_log_test1 {}",
            logged_value1
        ));
        expect_message_in_debuglog(format!(
            "[component_manager] INFO: mnl_log_test2 {}",
            logged_value2
        ));
    }

    #[test]
    fn log_one_very_long_line_test() {
        let line1: String = make_str_with_len("line1:", MAX_INFO_LINE_LEN);
        let line2: String = make_str_with_len("line2:", MAX_INFO_LINE_LEN);
        let line3: String = make_str_with_len("line3:", MAX_INFO_LINE_LEN);

        init();
        info!("{}{}{}", line1, line2, line3);

        expect_message_in_debuglog(format!("[component_manager] INFO: {}", line1));
        expect_message_in_debuglog(format!("[component_manager] INFO: {}", line2));
        expect_message_in_debuglog(format!("[component_manager] INFO: {}", line3));
    }

    #[test]
    fn log_line_that_would_be_split_without_newline_test() {
        let line1: String = make_str_with_len("line1:", 128);
        let line2: String = make_str_with_len("line2:", 128);

        init();
        info!("{}\n{}", line1, line2);

        expect_message_in_debuglog(format!("[component_manager] INFO: {}", line1));
        expect_message_in_debuglog(format!("[component_manager] INFO: {}", line2));
    }

    #[test]
    fn log_overly_long_line_with_newline_test() {
        let line1: String = make_str_with_len("line1:", MAX_INFO_LINE_LEN);
        let line1part2: String = make_str_with_len("line1part2:", 80);
        let line2: String = make_str_with_len("line2:", 80);

        init();
        info!("{}{}\n{}", line1, line1part2, line2);

        expect_message_in_debuglog(format!("[component_manager] INFO: {}", line1));
        expect_message_in_debuglog(format!("[component_manager] INFO: {}", line1part2));
        expect_message_in_debuglog(format!("[component_manager] INFO: {}", line2));
    }

    #[test]
    fn log_pathological_utf8_data() {
        // Naively, this would split the emoji half-way through, which would
        // cause a panic.
        let line1: String = make_str_with_len("emojiline1:", MAX_INFO_LINE_LEN - 1);

        init();
        info!("{}😈", line1);

        expect_message_in_debuglog(format!("[component_manager] INFO: {}", line1));
        expect_message_in_debuglog(format!("[component_manager] INFO: 😈"));
    }

    #[test]
    fn log_warn_test() {
        let mut rng = rand::rng();
        let logged_value: u64 = rng.random();

        init();
        warn!("log_test {}", logged_value);

        expect_message_in_debuglog(format!("[component_manager] WARN: log_test {}", logged_value));
    }

    #[test]
    fn log_error_test() {
        let mut rng = rand::rng();
        let logged_value: u64 = rng.random();

        init();
        error!("log_test {}", logged_value);

        expect_message_in_debuglog(format!("[component_manager] ERROR: log_test {}", logged_value));
    }

    #[test]
    #[should_panic(expected = "panic_test")]
    // TODO(https://fxbug.dev/42169733): LeakSanitizer flags leaks caused by panic.
    #[cfg_attr(feature = "variant_asan", ignore)]
    #[cfg_attr(feature = "variant_hwasan", ignore)]
    fn log_panic_test() {
        let mut rng = rand::rng();
        let logged_value: u64 = rng.random();

        let old_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            // This will panic again if the message is not found,
            // and the message will not include "panic_test".
            old_hook(info);
            expect_message_in_debuglog(format!("[component_manager] PANIC: panicked at"));
            expect_message_in_debuglog(format!("panic_test {logged_value}"));
        }));

        init();
        panic!("panic_test {}", logged_value);
    }
}
