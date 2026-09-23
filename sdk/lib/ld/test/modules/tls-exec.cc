// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <lib/ld/module.h>
#include <lib/ld/tls.h>
#include <stdint.h>

#include "test-start.h"

[[gnu::used, gnu::retain]] alignas(64) constinit thread_local int tls_data = 1;

extern "C" int64_t TestStart() {
  const auto modules = ld::AbiLoadedModules(ld::abi::_ld_abi);

  const auto& exec_module = *modules.begin();

  if (exec_module.tls_modid != 1) {
    return 1;
  }

  return 17;
}
