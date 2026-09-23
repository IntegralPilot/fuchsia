// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_KERNEL_DEBUG_KERNEL_DEBUG_H_
#define LIB_KERNEL_DEBUG_KERNEL_DEBUG_H_

#include <lib/svc/service.h>

const zx_service_provider_t* kernel_debug_get_service_provider();

#endif  // LIB_KERNEL_DEBUG_KERNEL_DEBUG_H_
