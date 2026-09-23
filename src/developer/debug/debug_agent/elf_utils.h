// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ELF_UTILS_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ELF_UTILS_H_

#include <stdint.h>

#include <functional>
#include <vector>

#include "src/developer/debug/shared/status.h"
#include "src/lib/elflib/elflib.h"

namespace debug_ipc {
struct AddressRegion;
struct Module;
}  // namespace debug_ipc

namespace debug_agent {

class ProcessHandle;

// Iterates through all modules in the given process, calling the callback for each. The callback
// should return true to keep iterating, false to stop now.
debug::Status WalkElfModules(const ProcessHandle& process, uint64_t dl_debug_addr,
                             std::function<bool(uint64_t base_addr, uint64_t lmap)> cb);

// Computes the modules for the given process.
std::vector<debug_ipc::Module> GetElfModulesForProcess(const ProcessHandle& process,
                                                       uint64_t dl_debug_addr);

namespace internal {

struct ElfSegInfo {
  std::vector<elflib::Elf64_Phdr> segment_headers;
  std::optional<std::string> so_name;
  std::string build_id;
};
void MergeMmapedModules(std::vector<debug_ipc::Module>& modules,
                        const std::vector<debug_ipc::AddressRegion>& mmaps,
                        std::function<std::optional<ElfSegInfo>(uint64_t)> get_elf_info_for_base);

}  // namespace internal

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ELF_UTILS_H_
