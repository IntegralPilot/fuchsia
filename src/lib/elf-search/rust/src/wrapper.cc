// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "wrapper.h"

#include <elf-search.h>
#include <zircon/process.h>

// Calls the given `callback` for each ELF executable segment in the given process.
__EXPORT zx_status_t elf_search_wrapper(zx_handle_t process_handle, ElfSearchCallbackFn callback,
                                        void* callback_arg) {
  zx::unowned_process process(process_handle);
  return elf_search::ForEachModule(*process, [&](const elf_search::ModuleInfo& info) {
    callback(info.name.data(), info.name.length(), info.vaddr, info.build_id.data(),
             info.build_id.size(), &info.ehdr, info.phdrs.data(), info.phdrs.size(), callback_arg);
  });
}
