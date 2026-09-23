// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::terminal::Terminal;
use anyhow::Error;
use fuchsia_async::{self as fasync, OnSignals};

pub trait LogClient: 'static + Clone {
    fn create_terminal(&self, id: u32, title: String) -> Result<Terminal, Error>;
    fn request_update(&self, id: u32);
}

pub struct Log;

impl Log {
    pub fn start<T: LogClient>(
        read_only_debuglog: zx::DebugLog,
        client: &T,
        id: u32,
    ) -> Result<(), Error>
where {
        let client = client.clone();
        let terminal =
            client.create_terminal(id, "debuglog".to_string()).expect("failed to create terminal");
        let term = terminal.clone_term();

        // Get our process koid so we can filter out our own debug messages from the log.
        let proc_koid =
            fuchsia_runtime::process_self().koid().expect("failed to get koid for process");

        fasync::Task::local(async move {
            loop {
                let on_signal = OnSignals::new(&read_only_debuglog, zx::Signals::LOG_READABLE);
                on_signal.await.expect("failed to wait for log readable");

                loop {
                    match read_only_debuglog.read() {
                        Ok(record) => {
                            // Don't print log messages from ourself.
                            if record.pid == proc_koid {
                                continue;
                            }

                            let mut parser = term.borrow_mut();

                            // Write prefix with time stamps and ids.
                            let prefix = format!(
                                "\u{001b}[32m{:05}.{:03}\u{001b}[39m] \u{001b}[31m{:05}.\u{001b}[36m{:05}\u{001b}[39m> ",
                                record.timestamp.into_nanos() / 1_000_000_000,
                                (record.timestamp.into_nanos() / 1_000_000) % 1_000,
                                record.pid.raw_koid(),
                                record.tid.raw_koid(),
                            );
                            for byte in prefix.as_bytes() {
                                parser.process(&[*byte]);
                            }

                            // Ignore any trailing newline character.
                            let mut record_data = record.data();
                            if record_data.last() == Some(&b'\n') {
                                record_data = &record_data[..record_data.len() - 1];
                            }

                            // Write record data.
                            for byte in record_data.iter() {
                                parser.process(&[*byte]);
                            }

                            // Write carriage return and newline.
                            for byte in "\r\n".as_bytes() {
                                parser.process(&[*byte]);
                            }

                            // Request terminal update.
                            client.request_update(id);
                        }
                        Err(status) if status == zx::Status::SHOULD_WAIT => {
                            break;
                        }
                        Err(_) => {
                            let mut parser = term.borrow_mut();
                            for byte in "\r\n<<LOG ERROR>>".as_bytes() {
                                parser.process(&[*byte]);
                            }

                            // Request terminal update.
                            client.request_update(id);
                            break;
                        }
                    }
                }
            }
        })
        .detach();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::ColorScheme;
    use fuchsia_async as fasync;

    #[derive(Default, Clone)]
    struct TestLogClient;

    impl LogClient for TestLogClient {
        fn create_terminal(&self, _id: u32, title: String) -> Result<Terminal, Error> {
            Ok(Terminal::new(title, ColorScheme::default(), 1024, None))
        }
        fn request_update(&self, _id: u32) {}
    }

    #[fasync::run_singlethreaded(test)]
    async fn can_start_log() -> Result<(), Error> {
        let resource = zx::Resource::from(zx::NullableHandle::invalid());
        let debuglog = zx::DebugLog::create(&resource, zx::DebugLogOpts::empty()).unwrap();
        let client = TestLogClient::default();
        let _ = Log::start(debuglog, &client, 0)?;
        Ok(())
    }
}
