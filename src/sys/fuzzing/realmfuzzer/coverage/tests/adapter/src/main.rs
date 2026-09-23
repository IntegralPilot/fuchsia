// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result};
use fidl_fuchsia_fuzzer as fuzz;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use fuchsia_runtime::process_self;
use futures::{StreamExt, TryFutureExt, TryStreamExt, try_join};
use log::warn;
use zx::Peered;

struct Instrumentation {
    _collector: fuzz::CoverageDataCollectorProxy,
    eventpair: zx::EventPair,
    llvm_module: zx::Vmo,
}

impl Instrumentation {
    async fn connect() -> Result<(Self, fuzz::Options)> {
        let collector = connect_to_protocol::<fuzz::CoverageDataCollectorMarker>()
            .context("failed to connect to fuchsia.fuzzer.CoverageDataCollector")?;
        let (local, eventpair) = zx::EventPair::create();
        let process = process_self()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .context("failed to duplicate process handle")?;
        let options = collector
            .initialize(eventpair, process)
            .await
            .context("fuchsia.fuzzer.CoverageDataCollector/Initialize")?;

        const NUM_COUNTERS: u64 = 5;
        let module = zx::Vmo::create(NUM_COUNTERS).context("failed to create module VMO")?;
        let module_name = zx::Name::new("test-module").expect("Name::new failed");
        module.set_name(&module_name).context("failed to set name on module VMO")?;
        module
            .set_content_size(&NUM_COUNTERS)
            .context("failed to set content size on module VMO")?;
        let inline_8bit_counters = module
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .context("failed to duplicate module VMO handle")?;
        collector
            .add_inline8bit_counters(inline_8bit_counters)
            .await
            .context("fuchsia.fuzzer.CoverageDataCollector/AddLlvmModule")?;

        Ok((Self { _collector: collector, eventpair: local, llvm_module: module }, options))
    }

    async fn collect_coverage(&mut self) -> Result<()> {
        loop {
            fasync::OnSignals::new(&self.eventpair, zx::Signals::USER_0)
                .await
                .context("failed to receive signal from module eventpair")?;
            self.eventpair
                .signal(zx::Signals::USER_0, zx::Signals::NONE)
                .context("failed to clear module eventpair")?;
            let data = "world".as_bytes();
            let content_size = self
                .llvm_module
                .get_content_size()
                .context("failed to get content size for module VMO")?;
            let content_size: usize = content_size.try_into().unwrap();
            assert_eq!(content_size, data.len());
            self.llvm_module.write(data, 0).context("failed to write to module VMO")?;
            self.eventpair
                .signal_peer(zx::Signals::NONE, zx::Signals::USER_1)
                .context("failed to signal module eventpair")?;
        }
    }
}

enum IncomingService {
    TargetAdapter(fuzz::TargetAdapterRequestStream),
}

async fn serve_target_adapter(options: fuzz::Options) -> Result<()> {
    let max_input_size = options.max_input_size.context("`max_input_size` not set")?;
    let max_input_size: usize = max_input_size.try_into().unwrap();
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(IncomingService::TargetAdapter);
    fs.take_and_serve_directory_handle()?;
    const MAX_CONCURRENT: usize = 10_000;
    fs.for_each_concurrent(MAX_CONCURRENT, |IncomingService::TargetAdapter(stream)| {
        run_target_adapter(stream, max_input_size).unwrap_or_else(|e| warn!("{:?}", e))
    })
    .await;
    Ok(())
}

async fn run_target_adapter(
    stream: fuzz::TargetAdapterRequestStream,
    max_input_size: usize,
) -> Result<()> {
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| async move {
            match request {
                fuzz::TargetAdapterRequest::GetParameters { responder } => {
                    let parameters: Vec<String> = Vec::new();
                    responder.send(&parameters).context(
                        "failed to send response for fuchsia.fuzzer.TargetAdapter/GetParameters",
                    )?;
                }
                fuzz::TargetAdapterRequest::Connect { eventpair, test_input, responder } => {
                    responder.send().context(
                        "failed to send response for fuchsia.fuzzer.TargetAdapter/Connect",
                    )?;
                    let mut buf: Vec<u8> = Vec::with_capacity(max_input_size);
                    buf.resize(max_input_size, 0);
                    loop {
                        fasync::OnSignals::new(&eventpair, zx::Signals::USER_0)
                            .await
                            .context("failed to receive signal from test input eventpair")?;
                        eventpair
                            .signal(zx::Signals::USER_0, zx::Signals::NONE)
                            .context("failed to clear adapter eventpair")?;
                        let test_input_size = test_input
                            .get_content_size()
                            .context("failed to get content size from test input VMO")?;
                        let test_input_size: usize = test_input_size.try_into().unwrap();
                        let data = &mut buf[0..test_input_size];
                        test_input.read(data, 0).context("failed to read test input VMO")?;
                        assert_eq!(data, "hello".as_bytes());
                        eventpair
                            .signal_peer(zx::Signals::NONE, zx::Signals::USER_1)
                            .context("failed to signal adapter eventpair")?;
                    }
                }
            }
            Ok(())
        })
        .await
}

#[fuchsia::main(logging = true)]
async fn main() -> Result<()> {
    let (mut instrumentation, options) =
        Instrumentation::connect().await.context("failed to create instrumentation")?;
    try_join!(serve_target_adapter(options), instrumentation.collect_coverage(),)?;
    Ok(())
}
