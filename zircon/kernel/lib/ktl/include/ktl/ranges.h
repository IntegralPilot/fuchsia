// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_RANGES_H_
#define ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_RANGES_H_

#include <ranges>

namespace ktl {

// This can't be a namespace alias like `views` because <ktl/algorithm.h> also
// adds things to ktl::ranges and redundant namespace aliases aren't allowed.
namespace ranges {
using namespace std::ranges;
}  // namespace ranges

// Only this header defines this namespace, so a simple namespace alias works.
namespace views = std::views;

}  // namespace ktl

#endif  // ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_RANGES_H_
