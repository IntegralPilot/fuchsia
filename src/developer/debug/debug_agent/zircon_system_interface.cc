// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/zircon_system_interface.h"

#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>

#include "src/developer/debug/debug_agent/zircon_binary_launcher.h"
#include "src/developer/debug/debug_agent/zircon_job_handle.h"
#include "src/developer/debug/shared/logging/logging.h"

namespace debug_agent {

namespace {

// Returns an !is_valid() job object on failure.
zx::job GetRootZxJob() {
  auto res = component::Connect<fuchsia_kernel::RootJob>();
  if (res.is_error()) {
    LOGS(Error) << "Failed to connect to fuchsia.kernel.RootJob: " << res.error_value();
    return zx::job();
  }

  fidl::SyncClient root_job_ptr(std::move(*res));

  auto get_res = root_job_ptr->Get();
  if (get_res.is_error()) {
    LOGS(Error) << "Failed to get root job: " << get_res.error_value().FormatDescription();
    return zx::job();
  }
  return std::move(get_res->job());
}

}  // namespace

ZirconSystemInterface::ZirconSystemInterface()
    : svc_dir_(*component::OpenServiceRoot()), component_manager_(this), limbo_provider_(svc_dir_) {
  if (zx::job zx_root = GetRootZxJob(); zx_root.is_valid())
    root_job_ = std::make_unique<ZirconJobHandle>(std::move(zx_root));
}

uint32_t ZirconSystemInterface::GetNumCpus() const { return zx_system_get_num_cpus(); }

uint64_t ZirconSystemInterface::GetPhysicalMemory() const { return zx_system_get_physmem(); }

std::unique_ptr<JobHandle> ZirconSystemInterface::GetRootJob() const {
  if (root_job_)
    return std::make_unique<ZirconJobHandle>(*root_job_);
  return nullptr;
}

std::unique_ptr<BinaryLauncher> ZirconSystemInterface::GetLauncher() const {
  return std::make_unique<ZirconBinaryLauncher>(svc_dir_);
}

ComponentManager& ZirconSystemInterface::GetComponentManager() { return component_manager_; }

std::string ZirconSystemInterface::GetSystemVersion() { return zx_system_get_version_string(); }

}  // namespace debug_agent
