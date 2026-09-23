// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_HOST2_ENTRY_POINT_H_
#define SRC_DEVICES_BIN_DRIVER_HOST2_ENTRY_POINT_H_

#include <stdint.h>
#include <zircon/types.h>

extern "C" [[gnu::visibility("default")]] int64_t Start(zx_handle_t bootstrap, void* vdso);

#endif  // SRC_DEVICES_BIN_DRIVER_HOST2_ENTRY_POINT_H_
