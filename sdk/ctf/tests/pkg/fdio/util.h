// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef CTF_TESTS_PKG_FDIO_UTIL_H_
#define CTF_TESTS_PKG_FDIO_UTIL_H_

#include <lib/zx/process.h>

void wait_for_process_exit(const zx::process& process, int64_t* return_code);

#endif  // CTF_TESTS_PKG_FDIO_UTIL_H_
