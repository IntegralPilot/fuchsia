// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_SYMBOLS_SYMBOLS_H_
#define LIB_DRIVER_SYMBOLS_SYMBOLS_H_

#include <lib/fdf/dispatcher.h>
#include <lib/fdf/types.h>

/// The |DriverRegistration| is the ABI for drivers to expose themselves to the Driver Framework.
/// The driver is loaded in as a shared library (also referred to as a DSO), and the global symbol
/// `__fuchsia_driver_registration__` is used by the Driver Framework to locate this
/// DriverRegistration in the driver library. The framework will use this to initiate a FIDL based
/// connection over the driver transport into the driver.
struct DriverRegistration {
  /// The version is not expected to change since the evolution of the driver APIs will happen
  /// on the FIDL side. This simply exists in case we need to evolve the initialization signature
  /// for any reason.
  uint64_t version;

  struct driver_registration_v1 {
    /// Pointer to a function that can initialize the driver server on the |server_handle|.
    ///
    /// Errors to initialize the driver server can be propagated by closing the |server_handle| with
    /// an epitaph, but driver start failures should use the corresponding FIDL protocol.
    ///
    /// This function is executed on the default driver dispatcher. This dispatcher is synchronized
    /// and runs on a shared driver thread within a `driver_host`. The driver can access this
    /// dispatcher using |fdf_dispatcher_get_current_dispatcher()| or in C++ with
    /// |fdf::Dispatcher::GetCurrent()|. This can be stored in an
    /// |fdf::UnownedSynchronizedDispatcher| as the driver host is the owner of the dispatcher.
    ///
    /// |server_handle| is the server end for the |fuchsia.driver.framework/Driver| protocol that
    /// the driver should begin serving on. This should be served on the driver dispatcher
    /// that this function is executed on.
    ///
    /// The return result is a `void*` token that the driver framework will pass into |destroy|.
    void* (*initialize)(fdf_handle_t server_handle);

    /// Pointer to a function that can free the resources that were allocated as part of
    /// |initialize|. Generally this will just be the object allocated to serve the
    /// |fuchsia.driver.framework/Driver| protocol.
    ///
    /// This function is executed after all of the dispatchers belonging to the driver have fully
    /// been shutdown, including the default dispatcher that |initialize| was executed on.
    ///
    /// |token| is the return result of the |initialize| function.
    void (*destroy)(void* token);
  } v1;
};

static const uint64_t DRIVER_REGISTRATION_VERSION_1 = 1;

static const uint64_t DRIVER_REGISTRATION_VERSION_MAX = DRIVER_REGISTRATION_VERSION_1;

#define FUCHSIA_DRIVER_REGISTRATION_V1(initialize, destroy) \
  {                                                         \
      .version = DRIVER_REGISTRATION_VERSION_1,             \
      .v1 = {initialize, destroy},                          \
  }

#define EXPORT_FUCHSIA_DRIVER_REGISTRATION_V1(initialize, destroy)             \
  extern "C" const DriverRegistration __fuchsia_driver_registration__ __EXPORT \
  FUCHSIA_DRIVER_REGISTRATION_V1(initialize, destroy)

#endif  // LIB_DRIVER_SYMBOLS_SYMBOLS_H_
