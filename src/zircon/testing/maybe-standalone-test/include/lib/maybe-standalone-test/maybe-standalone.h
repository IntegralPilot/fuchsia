// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_ZIRCON_TESTING_MAYBE_STANDALONE_TEST_INCLUDE_LIB_MAYBE_STANDALONE_TEST_MAYBE_STANDALONE_H_
#define SRC_ZIRCON_TESTING_MAYBE_STANDALONE_TEST_INCLUDE_LIB_MAYBE_STANDALONE_TEST_MAYBE_STANDALONE_H_

#include <lib/zx/channel.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls/resource.h>

#include <optional>
#include <string_view>

// Forward declaration for <lib/boot-options/boot-options.h>.
struct BootOptions;

namespace maybe_standalone {

// This returns the invalid handle if not built standalone.
zx::unowned_resource GetIoportResource();
zx::unowned_resource GetIrqResource();
zx::unowned_resource GetMmioResource();
zx::unowned_resource GetSystemResource();

// Creates and returns upon success a specific system resource given a |base|.
zx::result<zx::resource> GetSystemResourceWithBase(zx::unowned_resource& system_resource,
                                                   uint64_t base);

zx::unowned_vmo GetVmo(std::string_view name);
zx::unowned_channel GetNsDir(std::string_view name);

// This returns nullptr if not built standalone.
const BootOptions* GetBootOptions();

}  // namespace maybe_standalone

#endif  // SRC_ZIRCON_TESTING_MAYBE_STANDALONE_TEST_INCLUDE_LIB_MAYBE_STANDALONE_TEST_MAYBE_STANDALONE_H_
