// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdatomic.h>
#include <stdint.h>
#include <zircon/types.h>

// TODO(https://fxbug.dev/440105800): Remove this helper file and go back to
// using `std::atomic_ref` once this bug has been fixed.

void wake_report_fetch_add(zx_futex_t* val_addr, zx_futex_t amt) { atomic_fetch_add(val_addr, 1u); }
zx_futex_t wake_report_load(zx_futex_t* val_addr) { return atomic_load(val_addr); }
