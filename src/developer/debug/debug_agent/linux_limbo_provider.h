// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_LIMBO_PROVIDER_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_LIMBO_PROVIDER_H_

#include "src/developer/debug/debug_agent/limbo_provider.h"

namespace debug_agent {

// Linux doesn't have Limbo so this object just reports failure.
class LinuxLimboProvider final : public LimboProvider {
 public:
  LinuxLimboProvider() = default;
  virtual ~LinuxLimboProvider() = default;

  // LimboProvider implementation.
  bool Valid() const override { return false; }
  bool IsProcessInLimbo(zx_koid_t process_koid) const override { return false; }
  const RecordMap& GetLimboRecords() const override { return map_; }
  fit::result<debug::Status, RetrievedException> RetrieveException(
      zx_koid_t process_koid) override {
    return fit::error(debug::Status("No Limbo on Linux"));
  }
  debug::Status ReleaseProcess(zx_koid_t process_koid) override {
    return debug::Status("No Limbo on Linux.");
  }

 private:
  RecordMap map_;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_LIMBO_PROVIDER_H_
