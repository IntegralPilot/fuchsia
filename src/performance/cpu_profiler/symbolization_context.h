// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_CPU_PROFILER_SYMBOLIZATION_CONTEXT_H_
#define SRC_PERFORMANCE_CPU_PROFILER_SYMBOLIZATION_CONTEXT_H_

#include <lib/zx/process.h>
#include <lib/zx/result.h>

#include <map>
#include <string>
#include <vector>

namespace profiler {
struct Segment {
  uintptr_t p_vaddr;
  uintptr_t p_memsz;
  uint64_t p_flags;
};

struct Module {
  std::string module_name;
  uintptr_t vaddr;
  std::vector<Segment> loads;
};

struct SymbolizationContext {
  std::map<zx_koid_t, std::map<std::vector<std::byte>, Module>> process_contexts;
};
}  // namespace profiler
#endif  // SRC_PERFORMANCE_CPU_PROFILER_SYMBOLIZATION_CONTEXT_H_
