// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of the [`driver_register`] macro for registering driver implementations
//! with the driver host.

use crate::Driver;
use crate::server::DriverServer;

/// These re-exports are for the use of the macro and so should not be surfaced in documentation.
#[doc(hidden)]
pub use fdf_sys::{
    DRIVER_REGISTRATION_VERSION_1, DriverRegistration, DriverRegistration_driver_registration_v1,
    fdf_dispatcher_get_current_dispatcher,
};

/// Called by the macro [`driver_register`] to create the driver registration struct
/// without exposing any of the internal implementation as a public API.
#[doc(hidden)]
pub const fn make_driver_registration<T: Driver>() -> DriverRegistration {
    DriverRegistration {
        version: DRIVER_REGISTRATION_VERSION_1,
        v1: DriverRegistration_driver_registration_v1 {
            initialize: Some(DriverServer::<T>::initialize),
            destroy: Some(DriverServer::<T>::destroy),
        },
    }
}

/// Macro for declaring a driver's implementation of the [`Driver`] trait.
///
/// # Example
///
/// ```
/// use fdf_server::{driver_register, Driver, DriverContext};
/// use log::info;
/// use zx::Status;
///
/// #[derive(Default)]
/// struct TestDriver;
///
/// impl Driver for TestDriver {
///     async fn start(context: DriverContext) -> Result<Self, Status> {
///         info!("driver starting!");
///         // implement binding the node client, creating children, etc. here.
///         Ok(Self)
///     }
///     async fn stop(&self) {
///         info!("driver stop message");
///     }
/// }
///
/// driver_register!(TestDriver);
/// ```
#[macro_export]
macro_rules! driver_register {
    ($ty:ty) => {
        // SAFETY: There is no other global function of this name
        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".driver_registration")]
        #[allow(non_upper_case_globals)]
        pub static __fuchsia_driver_registration__: $crate::macros::DriverRegistration =
            $crate::macros::make_driver_registration::<$ty>();
    };
}
