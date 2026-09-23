// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/syscalls.h>

#include <zxtest/zxtest.h>

namespace {

TEST(MembarrierSyncProcessData, Basic) { zx_membarrier_sync_process_data(); }

TEST(MembarrierSyncProcessInsn, Basic) { zx_membarrier_sync_process_insn(); }

}  // namespace
