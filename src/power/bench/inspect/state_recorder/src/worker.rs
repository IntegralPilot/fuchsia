// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_power_staterecorder_bench::{ControlRequest, ControlRequestStream};
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use state_recorder::{NumericStateRecorder, RecorderOptions, units};

#[fuchsia::main]
async fn main() -> Result<()> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream: ControlRequestStream| stream);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(0, serve_worker).await;
    Ok(())
}

async fn serve_worker(mut stream: ControlRequestStream) {
    let result: Result<()> = async move {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                ControlRequest::RunBenchmark { capacity, entries, lazy_record, responder } => {
                    let options = RecorderOptions {
                        lazy_record,
                        capacity: capacity as usize,
                        ..Default::default()
                    };

                    let mut recorder = NumericStateRecorder::<u32>::new(
                        "bench_recorder".to_string(),
                        c"power_bench",
                        units!(Percent),
                        None,
                        options,
                    )
                    .expect("Failed to create NumericStateRecorder");

                    for i in 0..entries {
                        recorder.record(i);
                    }

                    println!("WAITING FOR MEMORY PROFILING");

                    // Acknowledge before blocking
                    responder.send()?;

                    // Keep component alive for profiling
                    std::future::pending::<()>().await;
                }
                ControlRequest::_UnknownMethod { .. } => unimplemented!(),
            }
        }
        Ok(())
    }
    .await;

    if let Err(err) = result {
        log::error!("Worker server error: {:?}", err);
    }
}
