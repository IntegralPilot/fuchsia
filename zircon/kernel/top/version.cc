// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2013 Google, Inc.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/console.h>
#include <zircon/assert.h>

#include <ktl/byte.h>
#include <ktl/span.h>
#include <lk/init.h>
#include <phys/boot-constants.h>

#include <ktl/enforce.h>

namespace {

int cmd_version(int argc, const cmd_args* argv, uint32_t flags) {
  stdout->Write(kBootConstants.kernel_version_ident.get());
  return 0;
}

}  // namespace

STATIC_COMMAND_START
STATIC_COMMAND("version", "print version", &cmd_version)
STATIC_COMMAND_END(version)
