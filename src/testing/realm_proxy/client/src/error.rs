// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_testing_harness as fharness;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("operation error {0:?}")]
    OperationError(fharness::OperationError),
}
