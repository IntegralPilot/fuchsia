// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This test logs its own message at TRACE severity and then waits for it to be observable over
//! ArchiveAccessor. The key bit we're testing here is the BUILD.gn's test_specs.min_log_severity,
//! which should enable the lower level logging which is disabled by default. Without having that
//! flag correctly passed through test metadata to ffx/run-test-suite, this test should fail.

use diagnostics_reader::ArchiveReader;
use futures::StreamExt;

#[fuchsia::test]
async fn hello_world() {
    let tag = "hello_world".to_string();
    tracing::trace!("TRACE LEVEL MESSAGE");
    log::trace!("TRACE LEVEL MESSAGE 2");

    let logs = ArchiveReader::logs().snapshot_then_subscribe().unwrap().map(|r| r.unwrap());
    let msgs = logs
        .filter(|log| {
            futures::future::ready(match log.tags() {
                Some(tags) => tags.contains(&tag),
                None => false,
            })
        })
        .map(|log| log.msg().unwrap().to_owned())
        .take(2)
        .collect::<Vec<_>>()
        .await;

    assert_eq!(msgs[0], "TRACE LEVEL MESSAGE");
    assert_eq!(msgs[1], "TRACE LEVEL MESSAGE 2");
}
