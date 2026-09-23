// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_MODULES_ENSURE_TEST_THREAD_POINTER_H_
#define LIB_LD_TEST_MODULES_ENSURE_TEST_THREAD_POINTER_H_

// Make sure that accessing the thread pointer in the normal way is valid, such
// that forming the address of a `thread_local` variable will work.  Note this
// does not mean that the address so formed will be valid for memory access.
// It means only that it will be valid to take the address of a variable via
// the compiler and it will be valid to use ld::TpRelative() so the offset
// between the two can be calculated.  This can't always be done, so a false
// return value says that it could not be ensured.
bool EnsureTestThreadPointer();

#endif  // LIB_LD_TEST_MODULES_ENSURE_TEST_THREAD_POINTER_H_
