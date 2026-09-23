// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ELF_SEARCH_H_
#define ELF_SEARCH_H_

#include <elf.h>
#include <lib/fit/function.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/process.h>
#include <stdint.h>

#include <memory>
#include <string_view>

namespace elf_search {

struct ModuleInfo {
  std::string_view name;
  uintptr_t vaddr;
  cpp20::span<const uint8_t> build_id;
  const Elf64_Ehdr& ehdr;
  cpp20::span<const Elf64_Phdr> phdrs;
};

using ModuleAction = fit::function<void(const ModuleInfo&)>;
extern zx_status_t ForEachModule(const zx::process&, ModuleAction);

// A class that allows reusing the same buffer for multiple ForEachModule calls.
class Searcher {
 public:
  // Resize the internal buffer to ensure there are at least N available zx_info_maps.
  zx_status_t Reserve(size_t);

  zx_status_t ForEachModule(const zx::process&, ModuleAction);

 private:
  // Store the buffer and capacity ourselves since vector implementations tend to either write over
  // the values provided by the kernel or require extra initialization.
  std::unique_ptr<zx_info_maps[]> maps_;
  size_t capacity_ = 0;
};

}  // namespace elf_search

#endif  // ELF_SEARCH_H_
