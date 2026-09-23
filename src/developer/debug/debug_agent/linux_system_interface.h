// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_SYSTEM_INTERFACE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_SYSTEM_INTERFACE_H_

#include <memory>

#include "src/developer/debug/debug_agent/linux_component_manager.h"
#include "src/developer/debug/debug_agent/linux_limbo_provider.h"
#include "src/developer/debug/debug_agent/system_interface.h"

namespace debug_agent {

class LinuxSystemInterface final : public SystemInterface {
 public:
  LinuxSystemInterface();

  // SystemInterface implementation:
  uint32_t GetNumCpus() const override;
  uint64_t GetPhysicalMemory() const override;
  std::unique_ptr<JobHandle> GetRootJob() const override;
  std::unique_ptr<BinaryLauncher> GetLauncher() const override;
  ComponentManager& GetComponentManager() override { return component_manager_; }
  LimboProvider& GetLimboProvider() override { return limbo_provider_; }
  std::string GetSystemVersion() override { return "Unknown"; }

 private:
  LinuxComponentManager component_manager_;
  LinuxLimboProvider limbo_provider_;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_SYSTEM_INTERFACE_H_
