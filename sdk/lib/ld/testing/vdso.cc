// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/ld/testing/vdso.h"

#include <lib/zx/process.h>
#include <zircon/processargs.h>

#include <string>

namespace ld::testing {

elfldltl::Soname<> GetVdsoSoname() {
  static const std::string soname_str = []() {
    // TODO(https://fxbug.dev/42085962): Decode the Vdso name from its VMO.
    return std::string{"libzircon.so"};
  }();
  static const elfldltl::Soname<> soname{soname_str};
  return soname;
}

zx::unowned_vmo GetVdsoVmo() {
  static const zx::vmo vdso{zx_take_startup_handle(PA_HND(PA_VMO_VDSO, 0))};
  return vdso.borrow();
}

}  // namespace ld::testing
