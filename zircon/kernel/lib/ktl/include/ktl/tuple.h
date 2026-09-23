// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_TUPLE_H_
#define ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_TUPLE_H_

#include <tuple>

namespace ktl {

using std::apply;
using std::forward_as_tuple;
using std::get;
using std::ignore;
using std::make_from_tuple;
using std::make_tuple;
using std::tie;
using std::tuple;
using std::tuple_cat;
using std::tuple_element;
using std::tuple_element_t;
using std::tuple_size;
using std::tuple_size_v;

}  // namespace ktl

#endif  // ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_TUPLE_H_
