// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_ZIRCON_H_
#define LIB_LD_ZIRCON_H_

#include <lib/elfldltl/vmar-loader.h>
#include <lib/ld/log-zircon.h>
#include <lib/zx/channel.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include <string_view>

#include "startup-load.h"

namespace ld {

using StartupModule = StartupLoadModule<elfldltl::LocalVmarLoader>;

// This collects the data from the bootstrap channel.
struct StartupData {
  zx::vmo GetLibraryVmo(Diagnostics& diag, std::string_view name) const;
  void ConfigLdsvc(Diagnostics& diag, std::string_view name) const;

  Log log;

  zx::vmar vmar;       // VMAR for allocation and module-loading.
  zx::vmar self_vmar;  // VMAR for the dynamic linker load image.

  zx::vmo executable_vmo;

  zx::channel ldsvc;

  bool ld_debug = false;
};

StartupData ReadBootstrap(zx::unowned_channel bootstrap);

}  // namespace ld

#endif  // LIB_LD_ZIRCON_H_
