// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, bail};
use ffx_e2e_emu::IsolatedEmulator;
use fuchsia_async::Timer;
use log::info;
use prost::Message;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::Duration;
use tempfile::tempdir;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct StoredSnapshot {
    snapshot_id: u32,
    snapshot_name: String,
    process_koid: u64,
    process_name: String,
}

/// Waits for the collector to report the eight stored snapshots produced by the example component.
async fn wait_at_least_n_stored_snapshots(
    emu: &IsolatedEmulator,
    n: usize,
) -> Result<Vec<StoredSnapshot>> {
    const ONE_SECOND: Duration = Duration::from_secs(1);
    const MAX_ATTEMPTS: usize = 30;

    info!("waiting for the collector to report {} stored snapshots...", n);
    for _ in 0..MAX_ATTEMPTS {
        match emu
            .ffx_output(&["--machine", "json", "profile", "heapdump", "list"])
            .await
            .map(|data| serde_json::from_str::<Vec<StoredSnapshot>>(&data))
        {
            Ok(Ok(json_array)) if json_array.len() >= n => return Ok(json_array),
            _ => Timer::new(ONE_SECOND).await, // try again in one second.
        };
    }

    bail!("Timeout while waiting for at least {} stored snapshots", n);
}

fn load_profile_file(path: &Path) -> Result<pprof::Profile> {
    let file = std::fs::File::open(path).context("opening file")?;
    let mut decoder = flate2::read::GzDecoder::new(file);
    let mut data = Vec::new();
    decoder.read_to_end(&mut data).context("decompressing file")?;
    let result = pprof::Profile::decode(&data[..]).context("decoding protobuf")?;
    Ok(result)
}

// Note: Instantiating an IsolatedEmulator is a costly operation. For this reason, all the tests
// are implemented as a single function sharing the same IsolatedEmulator instance.
#[fuchsia::test(logging = true)]
async fn test_ffx_profile_heapdump() {
    let scratch_dir = tempdir().expect("Failed to create a temporary directory");
    let emu = IsolatedEmulator::start("test-ffx-profile-heapdump").await.unwrap();

    // Enable heapdump's experimental plugin.
    emu.ffx(&["config", "set", "ffx_profile_heapdump", "true"]).await.unwrap();

    info!("Starting heapdump's example component...");
    let moniker = "/core/ffx-laboratory:heapdump-example";
    let url = "fuchsia-pkg://fuchsia.com/heapdump-example#meta/heapdump-example.cm";
    emu.ffx(&["component", "run", moniker, url]).await.unwrap();

    // Wait for the example component to have completed its execution *and* the collector to have
    // received all the eight stored snapshots it produces. Note that, after generating the eight
    // stored snapshots, the test program enters pause() so that it stays alive until killed.
    const NUM_EXPECTED_SNAPSHOTS: usize = 8;
    let stored_snapshots =
        wait_at_least_n_stored_snapshots(&emu, NUM_EXPECTED_SNAPSHOTS).await.unwrap();

    // Verify that the snapshot names match those produced by the example program.
    info!("Validating list of stored snapshots...");
    for (i, snapshot) in stored_snapshots.iter().enumerate() {
        assert_eq!(snapshot.snapshot_name, format!("fib-{}", i));
        assert_eq!(snapshot.process_name, "heapdump-example.cm");
    }

    // Verify that a stored snapshot can be downloaded and parsed successfully.
    info!("Validating the last stored snapshot...");
    {
        let snapshot_id = stored_snapshots.last().unwrap().snapshot_id;
        let profile_path = scratch_dir.path().join("stored-snapshot.pb.gz");
        emu.ffx(&[
            "profile",
            "heapdump",
            "download",
            "--snapshot-id",
            &snapshot_id.to_string(),
            "--output-file",
            profile_path.to_str().unwrap(),
        ])
        .await
        .expect("Failed to download stored snapshot");

        load_profile_file(&profile_path).expect("Failed to load the generated profile");
    }

    // Take a live snapshot and verify that it can be read back.
    info!("Taking a live snapshot...");
    {
        let profile_path = scratch_dir.path().join("live-snapshot.pb.gz");
        emu.ffx(&[
            "profile",
            "heapdump",
            "snapshot",
            "--by-name",
            "heapdump-example.cm",
            "--output-file",
            profile_path.to_str().unwrap(),
        ])
        .await
        .expect("Failed to take a live snapshot");

        load_profile_file(&profile_path).expect("Failed to load the generated profile");
    }

    // Take a live snapshot which includes the contents of each allocated block, and verify that:
    // - the generated directory includes as many files as allocated memory blocks.
    // - a block with known contents is present.
    info!("Taking a live snapshot with contents...");
    {
        let profile_path = scratch_dir.path().join("live-snapshot-with-contents.pb.gz");
        let contents_dir = scratch_dir.path().join("contents-dir");
        emu.ffx(&[
            "profile",
            "heapdump",
            "snapshot",
            "--by-name",
            "heapdump-example.cm",
            "--output-file",
            profile_path.to_str().unwrap(),
            "--output-contents-dir",
            contents_dir.to_str().unwrap(),
        ])
        .await
        .expect("Failed to take a live snapshot");

        let profile =
            load_profile_file(&profile_path).expect("Failed to load the generated profile");
        let contents_files = std::fs::read_dir(&contents_dir)
            .expect("Failed to read the output directory")
            .collect::<std::io::Result<Vec<_>>>()
            .expect("Failed to read one or more entries in the output directory");
        assert_eq!(contents_files.len(), profile.sample.len());

        // The example program leaks this string into the heap. Verify that it has been dumped.
        const KNOWN_BLOCK_CONTENTS: &[u8; 16] = b"This is a leak!\0";
        let known_block_found = contents_files.iter().any(|dir_entry| {
            let contents = std::fs::read(dir_entry.path()).expect("Failed to read dumped block");
            contents == KNOWN_BLOCK_CONTENTS
        });
        assert!(known_block_found);
    }

    // Take another live snapshot, this time with symbolization enabled, and verify the names of the
    // functions on the stack match the expectations.
    info!("Taking a symbolized snapshot...");
    {
        let profile_path = scratch_dir.path().join("symbolized-snapshot.pb.gz");
        emu.ffx(&[
            "profile",
            "heapdump",
            "snapshot",
            "--symbolize",
            "--by-name",
            "heapdump-example.cm",
            "--output-file",
            profile_path.to_str().unwrap(),
        ])
        .await
        .expect("Failed to take a live snapshot");

        let profile =
            load_profile_file(&profile_path).expect("Failed to load the generated profile");
        let mapping_by_id: HashMap<u64, &pprof::Mapping> =
            profile.mapping.iter().map(|mapping| (mapping.id, mapping)).collect();
        let location_by_id: HashMap<u64, &pprof::Location> =
            profile.location.iter().map(|location| (location.id, location)).collect();
        let function_by_id: HashMap<u64, &pprof::Function> =
            profile.function.iter().map(|function| (function.id, function)).collect();

        // For each sample, count the number of "fibonacci" occurrences in its call stack.
        let mut distrib = HashMap::<usize, usize>::new(); // distribution of the counters
        for sample in &profile.sample {
            let mut counter = 0;

            for location_id in &sample.location_id {
                let location = location_by_id.get(location_id).unwrap();

                // Validate that the mapping is marked as resolved.
                let mapping = mapping_by_id.get(&location.mapping_id).unwrap();
                assert!(mapping.has_functions);
                assert!(mapping.has_filenames);
                assert!(mapping.has_line_numbers);
                assert!(mapping.has_inline_frames);

                for line in &location.line {
                    let function = function_by_id.get(&line.function_id).unwrap();

                    let name = profile.string_table.get(function.name as usize).unwrap();
                    let filename = profile.string_table.get(function.filename as usize).unwrap();

                    if name.contains("fibonacci(")
                        && filename.ends_with("src/performance/memory/heapdump/example/main.c")
                    {
                        counter += 1;
                    }
                }
            }

            *distrib.entry(counter).or_default() += 1;
        }

        // Validate some simple properties on the distribution to check that it's sound.
        assert!(distrib.get(&7).is_none());
        assert!(distrib.get(&6).is_some());
        assert!(distrib.get(&5).is_some());
        assert!(distrib.get(&4).is_some());
        assert!(distrib.get(&3).is_some());
        assert!(distrib.get(&2).is_some());
        assert!(distrib.get(&1).is_some());
    }
}
