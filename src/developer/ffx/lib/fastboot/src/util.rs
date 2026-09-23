// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use chrono::TimeDelta;
use ffx_fastboot_interface::fastboot_interface::{UploadProgress, Variable};

#[derive(Debug, PartialEq)]
pub enum Event {
    Unlock(UnlockEvent),
    Upload(UploadProgress),
    Locked,
    RebootStarted,
    FlashProduct { product_name: String, partition_count: usize },
    FlashPartition { partition_name: String },
    FlashPartitionFinished { partition_name: String, duration: TimeDelta },
    Rebooted(TimeDelta),
    Variable(Variable),
    Oem { oem_command: String },
}

#[derive(Debug, PartialEq)]
pub enum UnlockEvent {
    SearchingForCredentials,
    FoundCredentials(TimeDelta),
    GeneratingToken,
    FinishedGeneratingToken(TimeDelta),
    BeginningUploadOfToken,
    Done,
}
