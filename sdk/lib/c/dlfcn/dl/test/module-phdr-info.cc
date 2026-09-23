// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "module-phdr-info.h"

#include <zircon/assert.h>

namespace dl::testing {
namespace {

// Call the system dl_iterate_phdr to collect the phdr info for startup modules
// loaded with this unittest: this serves as the source of truth of what is
// loaded when the test is run.
ModuleInfoList GetStartupPhdrInfo() {
  ModuleInfoList phdr_info;
  int rc = dl_iterate_phdr(CollectModulePhdrInfo, &phdr_info);
  ZX_ASSERT(rc == 0);
  return phdr_info;
}

}  // namespace

const ModuleInfoList gStartupPhdrInfo = GetStartupPhdrInfo();

}  // namespace dl::testing
