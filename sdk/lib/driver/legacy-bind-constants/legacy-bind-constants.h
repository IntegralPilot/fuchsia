// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_LEGACY_BIND_CONSTANTS_LEGACY_BIND_CONSTANTS_H_
#define LIB_DRIVER_LEGACY_BIND_CONSTANTS_LEGACY_BIND_CONSTANTS_H_

// LINT.IfChange
// global binding variables at 0x00XX

#define BIND_PCI_TOPO_PACK(bus, dev, func) (((bus) << 8) | (dev << 3) | (func))

// LINT.ThenChange(/src/lib/ddk/include/lib/ddk/binding_priv.h)

#endif  // LIB_DRIVER_LEGACY_BIND_CONSTANTS_LEGACY_BIND_CONSTANTS_H_
