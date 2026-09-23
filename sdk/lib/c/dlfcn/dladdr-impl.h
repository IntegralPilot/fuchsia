// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DLADDR_IMPL_H_
#define LIB_C_DLFCN_DLADDR_IMPL_H_

#include <dlfcn.h>
#include <lib/elfldltl/symbol.h>
#include <lib/ld/module.h>

#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

// This implements the body of dladdr.  If no module is found, the module
// pointer is nullptr and *info is not touched at all.  The other return values
// will be used by dladdr1.

struct DladdrResult {
  const ld::abi::Abi<>::Module* module = nullptr;
  const elfldltl::Elf<>::Sym* sym = nullptr;
};

DladdrResult DladdrImpl(const void* __restrict addr, Dl_info* __restrict info);

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_DLFCN_DLADDR_IMPL_H_
