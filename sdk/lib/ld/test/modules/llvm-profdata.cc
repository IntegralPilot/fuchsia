// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/channel.h>
#include <zircon/assert.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <string_view>

#ifndef HAVE_LLVM_PROFDATA
#error "BUILD BUG: HAVE_LLVM_PROFDATA must be 0 or 1"
#endif

constexpr std::string_view kPayload = "llvm-profdata";

extern "C" int64_t TestStart(zx_handle_t bootstrap_channel, void* vdso,
                             zx_handle_t svc_server_end) {
  // The bootstrap handle is a channel where the test that launched this
  // process will wait for a message.
  zx::channel bootstrap{bootstrap_channel};

  if (svc_server_end == ZX_HANDLE_INVALID) {
#if HAVE_LLVM_PROFDATA
    // The startup dynamic linker should have been compiled in the same variant
    // as this test program.  In a variant that enables llvm-profdata, the
    // channel should have been provided by the startup dynamic linker.
    // Return a status that just indicates no channel was provided, which is
    // normal when there is no startup dynamic linker at all.
    return 17;
#else
    // In a variant without llvm-profdata, no channel should have been provided.
    // This exit code tells the test it was skipped intentionally.
    return 77;
#endif
  }

  // Write back on the bootstrap channel to transfer the /svc server end back
  // to the test for validation.  After exit the test will inspect the message
  // from the bootstrap channel.
  return bootstrap.write(0, kPayload.data(), kPayload.size(), &svc_server_end, 1);
}
