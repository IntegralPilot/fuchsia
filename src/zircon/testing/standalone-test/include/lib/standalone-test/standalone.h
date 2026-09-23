// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_ZIRCON_TESTING_STANDALONE_TEST_INCLUDE_LIB_STANDALONE_TEST_STANDALONE_H_
#define SRC_ZIRCON_TESTING_STANDALONE_TEST_INCLUDE_LIB_STANDALONE_TEST_STANDALONE_H_

#include <lib/zx/channel.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls/resource.h>

#include <string>
#include <string_view>

// Forward declaration for <lib/boot-options/boot-options.h>.
struct BootOptions;

namespace standalone {

struct Option {
  std::string_view prefix;
  std::string option = {};
};

void GetOptions(std::initializer_list<std::reference_wrapper<Option>> opts);

zx::unowned_resource GetIoportResource();
zx::unowned_resource GetIrqResource();
zx::unowned_resource GetMmioResource();
zx::unowned_resource GetSystemResource();

// Creates and returns upon success a specific system resource given a |base|.
zx::result<zx::resource> GetSystemResourceWithBase(zx::unowned_resource& system_resource,
                                                   uint64_t base);
zx::unowned_vmo GetVmo(std::string_view name);
zx::unowned_channel GetNsDir(std::string_view name);

const BootOptions& GetBootOptions();

// This is also wired up as write on STDOUT_FILENO or STDERR_FILENO.
// It does line-buffering and at '\n' boundaries it writes to the debuglog.
void LogWrite(std::string_view str);

// This can be used as the main() function to run zxtest tests.
int TestMain();

}  // namespace standalone

#endif  // SRC_ZIRCON_TESTING_STANDALONE_TEST_INCLUDE_LIB_STANDALONE_TEST_STANDALONE_H_
