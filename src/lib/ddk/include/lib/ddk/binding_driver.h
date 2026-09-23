// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DDK_INCLUDE_LIB_DDK_BINDING_DRIVER_H_
#define SRC_LIB_DDK_INCLUDE_LIB_DDK_BINDING_DRIVER_H_

#include <lib/ddk/binding_priv.h>

#define ZIRCON_DRIVER(Driver, Ops, VendorName, Version) \
  ZIRCON_DRIVER_PRIV(Driver, Ops, VendorName, Version)

#endif  // SRC_LIB_DDK_INCLUDE_LIB_DDK_BINDING_DRIVER_H_
