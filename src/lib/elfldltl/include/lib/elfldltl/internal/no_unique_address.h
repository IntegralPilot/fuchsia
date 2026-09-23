// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_NO_UNIQUE_ADDRESS_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_NO_UNIQUE_ADDRESS_H_

// This is standard in C++20 but the MSVC and Clang's MSVC-compatible Windows
// target modes don't support it.  It's only used here as an optimization.
#if __has_cpp_attribute(no_unique_address) >= 201803L
#define ELFLDLTL_NO_UNIQUE_ADDRESS [[no_unique_address]]
#else
#define ELFLDLTL_NO_UNIQUE_ADDRESS  // Nothing.
#endif

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_NO_UNIQUE_ADDRESS_H_
