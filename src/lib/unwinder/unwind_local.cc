// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/unwind_local.h"

#include <link.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <memory>
#include <vector>

#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/third_party/libunwindstack/context.h"
#include "src/lib/unwinder/unwind.h"

namespace unwinder {

namespace {
std::vector<uint64_t> GetLocalModules() {
  std::vector<uint64_t> modules;

  // Find all the modules in the current process.
  using CallbackType = int (*)(dl_phdr_info* info, size_t size, void* modules);
  CallbackType dl_iterate_phdr_callback = [](dl_phdr_info* info, size_t, void* p_modules) {
    auto modules = reinterpret_cast<std::vector<uint64_t>*>(p_modules);
    modules->push_back(info->dlpi_addr);
    return 0;
  };
  dl_iterate_phdr(dl_iterate_phdr_callback, &modules);

  return modules;
}

}  // namespace

std::vector<Frame> UnwindLocal() {
  auto modules = GetLocalModules();
  LocalMemory mem;
  auto frames = Unwind(&mem, modules, GetContext());

  if (frames.empty()) {
    return {};
  }
  // Drop the first frame.
  return {frames.begin() + 1, frames.end()};
}

void UnwindLocalAsync(Memory* local_memory, AsyncMemory::Delegate* delegate,
                      fit::callback<void(std::vector<Frame>)> on_done) {
  auto load_addrs = GetLocalModules();
  std::vector<Module> modules;
  modules.reserve(load_addrs.size());

  for (const auto& addr : load_addrs) {
    modules.emplace_back(addr, local_memory, Module::AddressMode::kProcess);
  }

  constexpr size_t kMaxDepth = 255;

  auto unwinder = std::make_unique<AsyncUnwinder>(delegate, modules);
  unwinder->Unwind(GetContext(), kMaxDepth,
                   [unwinder = std::move(unwinder),
                    on_done = std::move(on_done)](std::vector<Frame> frames) mutable {
                     if (frames.empty()) {
                       return on_done({});
                     }

                     on_done({frames.begin() + 2, frames.end()});
                   });
}

}  // namespace unwinder
