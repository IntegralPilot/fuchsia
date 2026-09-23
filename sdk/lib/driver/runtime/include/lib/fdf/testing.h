// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FDF_TESTING_H_
#define LIB_FDF_TESTING_H_

#include <lib/fdf/dispatcher.h>
#include <zircon/types.h>

__BEGIN_CDECLS

/// Creates a dispatcher on a unmanaged thread pool. This means that there are no background threads
/// handling this dispatcher and so it has to be ran explicitly using the various run calls.
/// See |fdf_env_dispatcher_create_with_owner| for more information about the parameters and return
/// value.
zx_status_t fdf_testing_create_unmanaged_dispatcher(const void* driver, uint32_t options,
                                                    const char* name, size_t name_len,
                                                    fdf_dispatcher_shutdown_observer_t* observer,
                                                    fdf_dispatcher_t** out_dispatcher);

/// Sets the default driver dispatcher to be returned when the current thread does not have a driver
/// associated with it. This is useful for tests that want to attach a dispatcher to their main
/// test thread, so that they can use objects that contains a synchronization_checker.
/// |dispatcher| should have been created using |fdf_testing_create_unmanaged_dispatcher|, or be a
/// nullptr to remove an existing default.
///
/// Returns |ZX_OK| if set successfully.
/// Returns |ZX_ERR_BAD_STATE| if called from a thread managed by the driver runtime.
zx_status_t fdf_testing_set_default_dispatcher(fdf_dispatcher_t* dispatcher);

/// Runs the unmanaged dispatcher pool on the current thread.
///
/// Dispatches events until the |deadline| expires or it is quit.
/// Use |ZX_TIME_INFINITE| to dispatch events indefinitely.
///
/// If |once| is true, performs a single unit of work then returns.
///
/// Returns |ZX_OK| if the dispatcher returns after one cycle.
/// Returns |ZX_ERR_TIMED_OUT| if the deadline expired.
/// Returns |ZX_ERR_CANCELED| if it quit.
/// Returns |ZX_ERR_BAD_STATE| if the unmanaged dispatcher was shut down or if an unmanaged
/// dispatcher has not been created.
zx_status_t fdf_testing_run(zx_time_t deadline, bool once);

/// Runs the unmanaged dispatcher pool on the current thread. Dispatches events until there are none
/// remaining, and then returns without waiting. This is useful for unit testing, because the
/// behavior doesn't depend on time.
///
/// Returns |ZX_OK| if the dispatcher reaches an idle state.
/// Returns |ZX_ERR_CANCELED| if it is quit.
/// Returns |ZX_ERR_BAD_STATE| if the unmanaged dispatcher was shut down or if an unmanaged
/// dispatcher has not been created.
zx_status_t fdf_testing_run_until_idle();

/// Quits the unmanaged dispatcher pool if one has been created. Otherwise does nothing.
/// Active invocations of |fdf_testing_run()| will eventually terminate upon completion of their
/// current unit of work.
///
/// Subsequent calls to |fdf_testing_run()|
/// will return immediately until |fdf_testing_reset_quit()| is called.
void fdf_testing_quit();

/// Resets the quit state of the unmanaged dispatcher pool so that it can be restarted
/// using |fdf_testing_run()|.
///
/// This function must only be called when the unmanaged dispatcher pool is not running.
/// The caller must ensure all active invocations of |fdf_testing_run()| have terminated before
/// resetting the quit state.
///
/// Returns |ZX_OK| if the unmanaged dispatcher pool's quit state was correctly reset.
/// Returns |ZX_ERR_BAD_STATE| if the unmanaged dispatcher pool was shutting down or if
/// it was currently actively running, or if an unmanaged dispatcher has not been created.
zx_status_t fdf_testing_reset_quit();

__END_CDECLS

#endif  // LIB_FDF_TESTING_H_
