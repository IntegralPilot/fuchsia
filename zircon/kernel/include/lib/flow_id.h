// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_LIB_FLOW_ID_H_
#define ZIRCON_KERNEL_INCLUDE_LIB_FLOW_ID_H_

#include <stdint.h>

// Generates globally unique 64-bit flow IDs for tracing (C-exported).
extern "C" uint64_t flow_id_generate();

#endif  // ZIRCON_KERNEL_INCLUDE_LIB_FLOW_ID_H_
