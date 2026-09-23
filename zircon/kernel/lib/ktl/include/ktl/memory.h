// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_MEMORY_H_
#define ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_MEMORY_H_

#include <memory>

namespace ktl {

using std::addressof;
using std::to_address;

using std::align;
using std::assume_aligned;

using std::default_delete;

using std::pointer_traits;

using std::construct_at;
using std::destroy;
using std::destroy_at;
using std::destroy_n;
using std::uninitialized_copy;
using std::uninitialized_copy_n;
using std::uninitialized_default_construct;
using std::uninitialized_default_construct_n;
using std::uninitialized_fill;
using std::uninitialized_fill_n;
using std::uninitialized_move;
using std::uninitialized_move_n;
using std::uninitialized_value_construct;
using std::uninitialized_value_construct_n;

}  // namespace ktl

#endif  // ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_MEMORY_H_
