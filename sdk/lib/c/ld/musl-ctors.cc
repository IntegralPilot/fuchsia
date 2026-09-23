// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../startup/start-main.h"
#include "dynlink.h"
#include "libc.h"

namespace LIBC_NAMESPACE_DECL {

void StartupSanitizerModuleLoaded() { _dl_finish_startup(); }

void StartupCtors() { __libc_start_init(); }

}  // namespace LIBC_NAMESPACE_DECL
