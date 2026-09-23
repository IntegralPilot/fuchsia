// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_E2E_TESTS_INFERIORS_RUST_PRETTY_TYPES_LIB_H_
#define SRC_DEVELOPER_DEBUG_E2E_TESTS_INFERIORS_RUST_PRETTY_TYPES_LIB_H_

#include <zircon/compiler.h>

#include <cstddef>

// This type is specifically named and placed in the global namespace to conflict with the Rust's
// internal Vec implementation, which eventually includes members that are named "pointer". When we
// pretty print a Vec, we need to traverse these members until there's something we know how to
// print nicely, but we need to make sure that other types that might exist in the global namespace
// of the final Rust program, including all link time dependencies, don't shadow those variable
// names. This was first found in https://fxbug.dev/352125964 but is likely the case for other
// weirdness when trying to implicitly traverse identifiers via pretty printers when there is a
// naming conflict.
typedef size_t pointer;

__EXPORT extern "C" pointer p;

#endif  // SRC_DEVELOPER_DEBUG_E2E_TESTS_INFERIORS_RUST_PRETTY_TYPES_LIB_H_
