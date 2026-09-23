// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_log::{OnInterestChanged, PublisherOptions, Severity, TestRecord};
use fidl_fuchsia_validate_logs::{
    LogSinkPuppetRequest, LogSinkPuppetRequestStream, PuppetInfo, RecordSpec,
};
use fuchsia_async::Task;
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use log::{debug, error, info, trace, warn};
use {diagnostics_log_validator_utils as utils, fuchsia_runtime as rt};

#[fuchsia::main(logging = false)]
async fn main() {
    let publisher = diagnostics_log::Publisher::new_async(
        PublisherOptions::default()
            .log_file_line_info(true)
            .register_global_logger(true)
            .listen_for_interest_updates(true),
    )
    .await
    .unwrap();

    publisher.set_interest_listener(Listener);

    log::info!("Puppet started.");

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(|r: LogSinkPuppetRequestStream| r);
    fs.take_and_serve_directory_handle().unwrap();

    while let Some(incoming) = fs.next().await {
        Task::spawn(run_puppet(incoming, publisher.clone())).detach();
    }
}

struct Listener;

impl OnInterestChanged for Listener {
    fn on_changed(&self, severity: Severity) {
        match severity {
            Severity::Trace => {
                trace!("Changed severity");
            }
            Severity::Debug => {
                debug!("Changed severity");
            }
            Severity::Info => {
                info!("Changed severity");
            }
            Severity::Warn => {
                warn!("Changed severity");
            }
            Severity::Error => {
                error!("Changed severity");
            }
            Severity::Fatal => {
                panic!("Changed severity");
            }
        }
    }
}

async fn run_puppet(
    mut requests: LogSinkPuppetRequestStream,
    publisher: diagnostics_log::Publisher,
) {
    while let Some(next) = requests.try_next().await.unwrap() {
        match next {
            LogSinkPuppetRequest::GetInfo { responder } => {
                let info = PuppetInfo {
                    tag: None,
                    pid: rt::process_self().koid().unwrap().raw_koid(),
                    tid: rt::with_thread_self(|thread| thread.koid().unwrap()).raw_koid(),
                };
                responder.send(&info).unwrap();
            }
            LogSinkPuppetRequest::EmitLog {
                responder,
                spec: RecordSpec { file, line, mut record },
            } => {
                if record.timestamp == zx::BootInstant::ZERO {
                    record.timestamp = zx::BootInstant::get();
                }
                let mut record = utils::fidl_to_record(record);
                if record.timestamp == zx::BootInstant::ZERO {
                    record.timestamp = zx::BootInstant::get();
                }
                let test_record = TestRecord::from(&file, line, &record);
                publisher.event_for_testing(test_record);
                responder.send().unwrap();
            }
        }
    }
}
