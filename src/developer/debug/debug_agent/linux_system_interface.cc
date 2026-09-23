// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/linux_system_interface.h"

#include "src/developer/debug/debug_agent/linux_binary_launcher.h"
#include "src/developer/debug/debug_agent/linux_job_handle.h"

namespace debug_agent {

LinuxSystemInterface::LinuxSystemInterface() : component_manager_(this) {}

uint32_t LinuxSystemInterface::GetNumCpus() const {
  // TODO(brettw) implement this (need to read "/proc").
  return 1;
}

uint64_t LinuxSystemInterface::GetPhysicalMemory() const {
  // TODO(brettw) implement this (need to read "/proc").
  return 1000000000llu;
}

std::unique_ptr<JobHandle> LinuxSystemInterface::GetRootJob() const {
  // Linux doesn't have a job hierarchy so the default LinuxJobHandle is always the root.
  return std::make_unique<LinuxJobHandle>();
}

std::unique_ptr<BinaryLauncher> LinuxSystemInterface::GetLauncher() const {
  return std::make_unique<LinuxBinaryLauncher>();
}

}  // namespace debug_agent
