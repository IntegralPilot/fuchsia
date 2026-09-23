// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_DRIVER_EXPORT2_H_
#define LIB_DRIVER_COMPONENT_CPP_DRIVER_EXPORT2_H_

#include <lib/driver/component/cpp/internal/driver_server2.h>

// The given |driver| needs to be a subclass of |fdf::DriverBase2|.
// It must have a constructor in the form of:
// `T(fdf::Context& context, fdf::UnownedSynchronizedDispatcher driver_dispatcher);`
// This MUST only be called once inside of a shared object, and it must be called from the root
// namespace and not nested inside any other namespace.
#define FUCHSIA_DRIVER_EXPORT2(driver)                                                   \
  EXPORT_FUCHSIA_DRIVER_REGISTRATION_V1(fdf_internal::DriverServer2<driver>::initialize, \
                                        fdf_internal::DriverServer2<driver>::destroy)

#endif  // LIB_DRIVER_COMPONENT_CPP_DRIVER_EXPORT2_H_
