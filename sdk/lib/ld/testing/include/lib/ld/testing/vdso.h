// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TESTING_VDSO_H_
#define LIB_LD_TESTING_VDSO_H_

#include <lib/elfldltl/soname.h>
#include <lib/zx/vmo.h>

namespace ld::testing {

elfldltl::Soname<> GetVdsoSoname();

zx::unowned_vmo GetVdsoVmo();

}  // namespace ld::testing

#endif  // LIB_LD_TESTING_VDSO_H_
