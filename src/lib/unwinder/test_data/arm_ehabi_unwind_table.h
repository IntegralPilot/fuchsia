// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_TEST_DATA_ARM_EHABI_UNWIND_TABLE_H_
#define SRC_LIB_UNWINDER_TEST_DATA_ARM_EHABI_UNWIND_TABLE_H_

// Mark the exported symbols to prevent the linker from stripping them.
#define EXPORT __attribute__((visibility("default")))
#define NOINLINE __attribute__((noinline))

#endif  // SRC_LIB_UNWINDER_TEST_DATA_ARM_EHABI_UNWIND_TABLE_H_
