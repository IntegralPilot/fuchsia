// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELF_SEARCH_RUST_SRC_WRAPPER_H_
#define SRC_LIB_ELF_SEARCH_RUST_SRC_WRAPPER_H_

#include <elf.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

typedef void (*ElfSearchCallbackFn)(const char* name, size_t name_len, uint64_t vaddr,
                                    const uint8_t* build_id, size_t build_id_len,
                                    const Elf64_Ehdr* ehdr, const Elf64_Phdr* phdrs,
                                    size_t phdrs_len, void* arg);

// Calls the given `callback` for each ELF executable segment in the given process.
zx_status_t elf_search_wrapper(zx_handle_t process_handle, ElfSearchCallbackFn callback,
                               void* callback_arg);

__END_CDECLS

#endif  // SRC_LIB_ELF_SEARCH_RUST_SRC_WRAPPER_H_
