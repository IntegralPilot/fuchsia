// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_NEW_H_
#define ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_NEW_H_

#include <new>

namespace ktl {

using std::align_val_t;

using std::destroying_delete;
using std::destroying_delete_t;
using std::nothrow;
using std::nothrow_t;

using std::launder;

}  // namespace ktl

#endif  // ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_NEW_H_
