// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_BIT_H_
#define ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_BIT_H_

#include <bit>

namespace ktl {

using std::bit_cast;
using std::bit_ceil;
using std::bit_floor;
using std::bit_width;
using std::countl_one;
using std::countl_zero;
using std::countr_one;
using std::countr_zero;
using std::endian;
using std::has_single_bit;
using std::popcount;
using std::rotl;
using std::rotr;

}  // namespace ktl

#endif  // ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_BIT_H_
