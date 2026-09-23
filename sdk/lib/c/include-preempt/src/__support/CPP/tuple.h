// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PREEMPT_SRC___SUPPORT_CPP_TUPLE_H_
#define PREEMPT_SRC___SUPPORT_CPP_TUPLE_H_

// The llvm-libc "src/__support/CPP/tuple.h" header defines some names in
// `namespace std` to support compiler features that magically require those
// exact qualified names.  This collides with libc++ <tuple> definitions.  The
// llvm-libc code never uses libc++ headers, but Fuchsia libc code does.
//
// It's not important that `std::tuple` et al actually be just `using` aliases
// to `LIBC_NAMESPACE:std::tuple` et al as they are in llvm-libc.  So the only
// really _required_ purpose of this preempting header is just to act as if
// "src/__support/CPP/tuple.h" just uses libc++ <tuple> for these few `std`
// definitions.  That could be done with some shenanigans involving doing
// `#include_next` inside a wrapper namespace and then doing a lot of aliases
// back and forth before and after.  But that is pretty hairy and fragile.
//
// The purpose of `LIBC_NAMESPACE:cpp::tuple` et al is just to be exact
// polyfills for `std::tuple` et al.  So it's far simpler here to just make
// them plain aliases for the <tuple> ones and avoid the upstream llvm-libc
// "src/__support/CPP/tuple.h" header altogether.

#include <tuple>

#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {
namespace cpp {

using std::get;
using std::make_tuple;
using std::tie;
using std::tuple;
using std::tuple_cat;
using std::tuple_element;
using std::tuple_size;

}  // namespace cpp
}  // namespace LIBC_NAMESPACE_DECL

#endif  // PREEMPT_SRC___SUPPORT_CPP_TUPLE_H_
