// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result, bail};
use fidl_fuchsia_fuzzer as fuzz;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use std::str::from_utf8;
use zx::{Peered, Task};

#[fuchsia::main(logging = true)]
async fn main() -> Result<()> {
    // Register the configured options.
    let provider = connect_to_protocol::<fuzz::CoverageDataProviderMarker>()
        .context("failed to connect to fuchsia.fuzzer.CoverageDataProvider")?;
    const MAX_INPUT_SIZE: u64 = 256;
    let options = fuzz::Options {
        runs: Some(1000),
        max_input_size: Some(MAX_INPUT_SIZE),
        ..Default::default()
    };
    provider.set_options(&options).context("fuchsia.fuzzer.CoverageDataProvider/SetOptions")?;

    // Connect to the adapter.
    let adapter = connect_to_protocol::<fuzz::TargetAdapterMarker>()
        .context("failed to connect to fuchsia.fuzzer.TargetAdapter")?;
    let (local, remote) = zx::EventPair::create();
    let test_input = zx::Vmo::create(MAX_INPUT_SIZE).context("failed to create adapter VMO")?;
    let test_input_dup = test_input
        .duplicate_handle(zx::Rights::SAME_RIGHTS)
        .context("failed to duplicate adapter VMO")?;
    adapter
        .connect(remote, test_input_dup)
        .await
        .context("fuchsia.fuzzer.TargetAdapter/Connect")?;

    // Wait for the adapter to publish its module.
    let mut remote = None;
    let mut shared = None;
    while remote.is_none() || shared.is_none() {
        let batch = provider
            .watch_coverage_data()
            .await
            .context("fuchsia.fuzzer.CoverageDataProvider/WatchCoverageData")?;
        for coverage in batch {
            match coverage.data {
                fuzz::Data::Instrumented(instrumented) => {
                    remote = Some(instrumented);
                }
                fuzz::Data::Inline8bitCounters(inline_8bit_counters) => {
                    shared = Some(inline_8bit_counters);
                }
                fuzz::DataUnknown!() => bail!("unknown coverage data type"),
            }
        }
    }
    let remote = remote.unwrap();
    let shared = shared.unwrap();

    // Simulate sending a test input to the adapter and performing a fuzzing run.
    let data = "hello".as_bytes();
    let content_size: u64 = data.len().try_into().unwrap();
    test_input.write(data, 0).context("failed to write to adapter VMO")?;
    test_input
        .set_content_size(&content_size)
        .context("failed to set content size for adapter VMO")?;
    local
        .signal_peer(zx::Signals::NONE, zx::Signals::USER_0)
        .context("failed to signal adapter eventpair")?;
    fasync::OnSignals::new(&local, zx::Signals::USER_1)
        .await
        .context("failed to receive signal from adapter eventpair")?;
    local
        .signal(zx::Signals::USER_1, zx::Signals::NONE)
        .context("failed to clear adapter eventpair")?;

    // Simulate collecting coverage data from the module.
    remote
        .eventpair
        .signal_peer(zx::Signals::NONE, zx::Signals::USER_0)
        .context("failed to signal module eventpair")?;
    fasync::OnSignals::new(&remote.eventpair, zx::Signals::USER_1)
        .await
        .context("failed to receive signal from module eventpair")?;
    remote
        .eventpair
        .signal(zx::Signals::USER_1, zx::Signals::NONE)
        .context("failed to clear module eventpair")?;

    let content_size =
        shared.get_content_size().context("failed to get content size from module VMO")?;
    let content_size: usize = content_size.try_into().unwrap();
    let mut buf: Vec<u8> = Vec::with_capacity(content_size);
    buf.resize(content_size, 0);
    let data = &mut buf[..];
    shared.read(data, 0).context("failed to read from module VMO")?;
    let s = from_utf8(data).context("invalid UTF-8")?;
    assert_eq!(s, "world");

    // Stop the adapter process.
    remote.process.kill().context("failed to kill subprocess")?;
    Ok(())
}
