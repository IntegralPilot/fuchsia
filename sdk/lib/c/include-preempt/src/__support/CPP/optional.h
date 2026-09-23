// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PREEMPT_SRC___SUPPORT_CPP_OPTIONAL_H_
#define PREEMPT_SRC___SUPPORT_CPP_OPTIONAL_H_

// Just use the real std::optional in place of the llvm-libc polyfill.

#include <optional>

#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {
namespace cpp {

using std::make_optional;
using std::nullopt;
using std::nullopt_t;
using std::optional;

}  // namespace cpp
}  // namespace LIBC_NAMESPACE_DECL

#endif  // PREEMPT_SRC___SUPPORT_CPP_OPTIONAL_H_
