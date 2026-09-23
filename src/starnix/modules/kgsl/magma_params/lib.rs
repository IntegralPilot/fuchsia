// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod kgsl_magma_params;

pub use kgsl_magma_params::{AdrenoKgslParams, MAGMA_QCOM_ADRENO_QUERY_KGSL_PARAMS};
pub use starnix_uapi::uapi::{
    KGSL_FLAGS_ACTIVE, KGSL_FLAGS_INITIALIZED, KGSL_FLAGS_INITIALIZED0, KGSL_FLAGS_NORMALMODE,
    KGSL_FLAGS_PER_CONTEXT_TIMESTAMPS, KGSL_FLAGS_RESERVED0, KGSL_FLAGS_RESERVED1,
    KGSL_FLAGS_RESERVED2, KGSL_FLAGS_SAFEMODE, KGSL_FLAGS_SOFT_RESET, KGSL_FLAGS_STARTED,
};
