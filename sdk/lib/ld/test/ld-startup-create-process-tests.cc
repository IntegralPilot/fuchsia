// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ld-startup-create-process-tests.h"

#include <lib/zx/job.h>
#include <zircon/process.h>

#include <gtest/gtest.h>

namespace ld::testing {

// This is only called after CreateProcess(), via some subclass Init().
// But it's before anything has used the root VMAR for anything.
void LdStartupCreateProcessTestsBase::MimicSpawnProcessVmarReservation() {
  zx_info_vmar_t info = RootVmarInfo();

  // Match the system program loader (//src/lib/process_builder) legacy behavior:
  // reserve the lower half of the full address space, not just half of the VMAR
  // length; (base+len) represents the full address space.
  const uint64_t page_size = zx_system_get_page_size();
  const uint64_t top_half_start = ((((info.base + info.len) / 2) + page_size - 1) & -page_size);
  if (info.base >= top_half_start) {
    // Punt if the root VMAR actually starts much higher up, as in a
    // ZX_PROCESS_SHARED process.
    return;
  }

  const uint64_t size = top_half_start - info.base;
  InitVmarReservation({.base = info.base, .len = size});
}

void LdStartupCreateProcessTestsBase::Init(std::initializer_list<std::string_view> args,
                                           std::initializer_list<std::string_view> env) {
  LdLoadZirconLdsvcTestsBase::Init(args, env);

  std::string_view name = process_name();
  ASSERT_NO_FATAL_FAILURE(CreateProcess());
  ASSERT_NO_FATAL_FAILURE(MimicSpawnProcessVmarReservation());

  // Start packing the bootstrap message for the startup dynamic linker.
  // The packing will be completed in Run.
  ASSERT_NO_FATAL_FAILURE(  //
      LdStartupProcArgs(bootstrap(), root_vmar().borrow())
          .AddProcess(process().borrow())
          .AddThread(thread().borrow())
          .AddClonedFd(STDERR_FILENO, process_log_fd().get()));
}

void LdStartupCreateProcessTestsBase::FinishLoad(zx::vmo executable_vmo) {
  // Send the executable VMO.
  ASSERT_NO_FATAL_FAILURE(bootstrap().AddExecutableVmo(std::move(executable_vmo)));

  // Prime the mock loader service from the Needed() calls.
  ASSERT_NO_FATAL_FAILURE(LdsvcExpectNeeded());

  // If a mock loader service has been set up by calls to Needed() et al,
  // send the client end over.
  if (zx::channel ldsvc = TakeLdsvc()) {
    ASSERT_NO_FATAL_FAILURE(bootstrap().AddLdsvc(std::move(ldsvc)));
  }
}

LdStartupCreateProcessTestsBase::~LdStartupCreateProcessTestsBase() = default;

}  // namespace ld::testing
