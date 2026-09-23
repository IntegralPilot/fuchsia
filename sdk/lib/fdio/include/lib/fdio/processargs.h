// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FDIO_PROCESSARGS_H_
#define LIB_FDIO_PROCESSARGS_H_

#include <stdint.h>

// Flag on handle info in <zircon/processargs.h> protocol, instructing that
// this fd should be dup'd to 0/1/2 and be used for all of stdio.
#define FDIO_FLAG_USE_FOR_STDIO ((uint32_t)0x8000)

#endif  // LIB_FDIO_PROCESSARGS_H_
