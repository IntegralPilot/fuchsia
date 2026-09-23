// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Wrappers around the mechanisms of driver registration for the driver
//! framework for implementing startup and shutdown of the driver in rust.

#![warn(missing_docs, unsafe_op_in_unsafe_fn)]

use core::future::Future;

mod context;
mod error;
mod incoming;
pub mod macros;
mod node;
mod server;
pub mod testing;

pub use context::*;
pub use error::DriverError;
pub use incoming::*;
pub use node::*;

/// Entry points into a driver for starting and stopping.
///
/// Driver authors should implement this trait, taking information from the [`DriverContext`]
/// passed to the [`Driver::start`] method to set up, and then tearing down any resources they use
/// in the [`Driver::stop`] method.
pub trait Driver: Sized + Send + 'static {
    /// The name of the driver as it will appear in logs
    const NAME: &str;

    /// This will be called when the driver is started.
    ///
    /// The given [`DriverContext`] contains information and functionality necessary to get at the
    /// driver's incoming and outgoing namespaces, add child nodes in the driver topology, and
    /// manage dispatchers.
    ///
    /// In order for the driver to be properly considered started, it must return `Ok`
    /// and bind the client end for the [`DriverStartArgs::node`] given in
    /// [`DriverContext::start_args`].
    fn start(context: DriverContext) -> impl Future<Output = Result<Self, DriverError>> + Send;

    /// This will be called when the driver has been asked to stop, and should do any
    /// asynchronous cleanup necessary before the driver is fully shut down.
    ///
    /// Note: The driver will not be considered fully stopped until the node client end bound in
    /// [`Driver::start`] has been closed.
    fn stop(&self) -> impl Future<Output = ()> + Send;

    /// Called when the driver has been asked to suspend.
    ///
    /// This method is invoked after the driver runtime has suspended the driver's dispatchers
    /// and waited for all executing power-managed (normal) tasks to complete.
    ///
    /// The driver should use this opportunity to put its hardware into a low-power state.
    ///
    /// Note: This will only be called after the driver has successfully finished [`Driver::start`].
    /// If a stop is initiated while the driver is suspended, the driver will be fully resumed
    /// (via [`Driver::system_resume`]) before [`Driver::stop`] is invoked.
    ///
    /// Only called when `power_managed_dispatchers_enabled` is set to `"true"` in the
    /// driver's component manifest.
    fn system_suspend(&self) -> impl Future<Output = Result<(), DriverError>> + Send {
        async { Ok(()) }
    }

    /// Called when the driver has been asked to resume.
    ///
    /// This method is invoked when the system resumes or a registered wake vector triggers.
    /// It executes *before* anything else. The driver should use this opportunity to bring
    /// its hardware out of its low-power state. Any task queued for the driver to execute will
    /// only run after this function completes, starting with the wake vector's task if there
    /// was one.
    ///
    /// If the resume was triggered by a wake vector, `lease` will contain a lease token
    /// representing the lease associated with the wakeup. The driver can retain this lease
    /// token to keep the driver active and prevent it from suspending again.
    ///
    /// Note: This will only be called after the driver has successfully finished [`Driver::start`].
    /// If the driver is suspended when a stop is initiated, this method is called to resume the
    /// driver first, ensuring the driver is in a running state during the shutdown hook [`Driver::stop`].
    ///
    /// Only called when `power_managed_dispatchers_enabled` is set to `"true"` in the
    /// driver's component manifest.
    fn system_resume(
        &self,
        _lease: Option<zx::EventPair>,
    ) -> impl Future<Output = Result<(), DriverError>> + Send {
        async { Ok(()) }
    }
}
