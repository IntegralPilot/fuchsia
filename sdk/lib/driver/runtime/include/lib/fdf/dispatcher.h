// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FDF_DISPATCHER_H_
#define LIB_FDF_DISPATCHER_H_

#include <lib/async/dispatcher.h>
#include <lib/fdf/types.h>
#include <zircon/availability.h>

__BEGIN_CDECLS

/// Dispatcher for performing asynchronous operations.
///
/// |fdf_dispatcher_t| can be used in conjunction with a |fdf_channel_t|
/// to initiate asynchronous read operations. The dispatcher is in
/// charge of dispatching the read callbacks.
///
/// # Thread safety
///
/// Operations on dispatcher objects are thread-safe.
///
/// # Example
///
///   void shutdown_handler(fdf_dispatcher_t* dispatcher,
///                         fdf_dispatcher_shutdown_observer_t* fdf_observer) {
///     // Handle dispatcher shutdown.
///     // It is now safe to destroy |dispatcher|.
///   }
///
///   struct dispatcher_shutdown_observer {
///     fdf_dispatcher_shutdown_observer_t fdf_observer;
///     my_ctx* ctx;
///   };
///
///   void driver_start(void) {
///     const char* name = "my_driver";
///     const char* scheduler_role = "";
///     struct dispatcher_shutdown_observer my_observer;
///     my_observer.fdf_observer.handler = shutdown_handler;
///     ...
///
///     fdf_dispatcher_t* dispatcher;
///     zx_status_t status = fdf_dispatcher_create(0, name, strlen(name),
///                                                scheduler_role, strlen(scheduler_role),
///                                                &my_observer.fdf_observer, &dispatcher);
///
///     fdf_channel_read_t channel_read;
///     ...
///     status = fdf_channel_wait_async(dispatcher, channel_read, 0);
///
///     // The dispatcher will call the channel_read handler when ready.
///
///     // This begins the dispatcher shutdown process.
///     fdf_dispatcher_shutdown_async(dispatcher);
///   }
typedef struct fdf_dispatcher fdf_dispatcher_t;

typedef struct fdf_dispatcher_shutdown_observer fdf_dispatcher_shutdown_observer_t;

/// Called when the asynchronous shutdown for |dispatcher| has completed.
typedef void(fdf_dispatcher_shutdown_handler_t)(fdf_dispatcher_t* dispatcher,
                                                fdf_dispatcher_shutdown_observer_t* observer);

/// Holds context for the observer which will be called when the dispatcher's
/// asynchronous shutdown has completed.
///
/// After creating the dispatcher, the client is responsible for retaining
/// this structure in memory (and unmodified) until the handler runs.
struct fdf_dispatcher_shutdown_observer {
  fdf_dispatcher_shutdown_handler_t* handler;
};

/// Creates a dispatcher for performing asynchronous operations.
///
/// |options| provides the dispatcher configuration. The following options are supported:
///   * `FDF_DISPATHER_OPTION_SYNCHRONIZED` or `FDF_DISPATCHER_OPTION_UNSYNCHRONIZED` - sets whether
///     parallel callbacks in the callbacks set in the dispatcher are allowed.
///   * `FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS` - the dispatcher may not share zircon threads with
///     other drivers. This may not be set with `FDF_DISPATCHER_OPTION_UNSYNCHRONIZED` or on a
///     scheduler role that has `FDF_SCHEDULER_ROLE_OPTION_NO_SYNC_CALLS` set on it.
///
/// |name| is reported via diagnostics. It is similar to setting the name of a thread.
/// |name_len| is the length of the string, without including the terminated NULL character. If
/// |name| is greater than `ZX_MAX_NAME_LEN`, the length may be truncated.
///
/// |scheduler_role| is a hint. It may or not impact the priority the work scheduler against the
/// dispatcher is handled at. It may or may not impact the ability for other drivers to share zircon
/// threads with the dispatcher.
/// |scheduler_role_len | is the length of the string, without including the terminating
/// NULL character.
///
/// |observer| will be called after |fdf_dispatcher_shutdown_async| has been called,
/// and the dispatcher has completed its asynchronous shutdown.
///
/// # Thread requirements
///
/// This must be called from a thread managed by the driver runtime.
///
/// # Errors
///
/// ZX_ERR_NOT_SUPPORTED: |options| is not a supported configuration, which is any of the following:
///   * `FDF_DISPATCHER_OPTION_UNSYNCHRONIZED` with `FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS`.
///
/// ZX_ERR_INVALID_ARGS: This was not called from a thread managed by the driver runtime.
///
/// ZX_ERR_NO_RESOURCES: A thread needed to be spawned to create this dispatcher, but couldn't be.
/// This is likely because a dispatcher that allows sync calls was requested, but the thread pool is
/// currently at its limit.
///
/// ZX_ERR_BAD_STATE: Dispatchers are currently not allowed to be created, such as when a driver
/// is being shutdown by its driver host.
zx_status_t fdf_dispatcher_create(uint32_t options, const char* name, size_t name_len,
                                  const char* scheduler_role, size_t scheduler_role_len,
                                  fdf_dispatcher_shutdown_observer_t* observer,
                                  fdf_dispatcher_t** out_dispatcher);

/// Returns the asynchronous dispatch interface.
async_dispatcher_t* fdf_dispatcher_get_async_dispatcher(fdf_dispatcher_t* dispatcher);

/// Returns an unowned dispatcher provided an async dispatcher. If |async_dispatcher| was not
/// retrieved via |fdf_dispatcher_get_async_dispatcher|, the call will result in a crash.
fdf_dispatcher_t* fdf_dispatcher_downcast_async_dispatcher(async_dispatcher_t* async_dispatcher);

/// Returns the current thread's dispatcher.
/// This will return NULL if not called from a dispatcher managed thread.
fdf_dispatcher_t* fdf_dispatcher_get_current_dispatcher(void);

/// Returns the options set for this dispatcher.
uint32_t fdf_dispatcher_get_options(const fdf_dispatcher_t* dispatcher);

/// Begins shutting down the dispatcher. Shutting down is an asynchronous operation.
///
/// Once |fdf_dispatcher_shutdown_async| is called, the dispatcher will no longer
/// accept queueing new |async_dispatcher_t| operations or |ChannelRead| callbacks.
///
/// The dispatcher will asynchronously wait for all pending |async_dispatcher_t|
/// and |ChannelRead| callbacks to complete. Then it will serially cancel all
/// remaining callbacks with |ZX_ERR_CANCELED| and call the shutdown handler set
/// in |fdf_dispatcher_create|.
///
/// If the dispatcher is already shutting down or has completed shutdown, this will do nothing.
void fdf_dispatcher_shutdown_async(fdf_dispatcher_t* dispatcher);

/// Informs the runtime that |dispatcher| may be destroyed.
///
/// The dispatcher must be completely shutdown before this is called,
/// i.e. the shutdown handler set in |fdf_dispatcher_create| has been called.
/// It is safe to call this from that shutdown handler.
///
/// |dispatcher| must not be used after calling this function.
void fdf_dispatcher_destroy(fdf_dispatcher_t* dispatcher);

/// Removes an option set on the dispatcher during creation. The provided |option| parameter
/// is removed from the |dispatcher| given. For example if the dispatcher was created with options
/// that contained `FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS`, calling this with that option will
/// remove the ability of the dispatcher to make any more synchronous calls.
///
/// The following options are supported for sealing (aka removal):
///  - `FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS`
///
/// # Thread requirements
///
/// This must be called from the |dispatcher|'s own context.
///
/// # Errors
///
/// ZX_ERR_INVALID_ARGS: The |option| being removed is not a supported option.
///
/// ZX_ERR_BAD_STATE: Not called from the same context as the given dispatcher or the dispatcher
/// is not in a running state.
///
/// ZX_ERR_ALREADY_EXISTS: The option did not exist on the dispatcher. This can happen if this is
/// called more than once with the same option, or if it was not created with the option.
zx_status_t fdf_dispatcher_seal(fdf_dispatcher_t* dispatcher, uint32_t option);

/// Returns the always-on dispatcher interface for the given dispatcher.
///
/// The primary user of this is the driver's fuchsia.driver.framework/Driver FIDL server
/// which executes the driver lifecycle methods (e.g. Start, Stop).
///
/// The secondary user of this is for bindings and tasks that require execution during
/// suspend/resume lifecycle hooks. The driver host only calls these hooks when its safe
/// to do so (dependent elements still exist).
///
/// WARNING: Any other use will lead to execution while suspended, which may be unexpected.
fdf_dispatcher_t* fdf_dispatcher_get_always_on_dispatcher(fdf_dispatcher_t* dispatcher)
    ZX_AVAILABLE_SINCE(32);

/// Registers a wake vector. Callbacks for |signals| received on this handle while suspended,
/// wake the driver and execute after the driver's Resume hook, but before anything else.
///
/// The handle wait does not need to be registered before this is called.
/// This is a sticky registration; all future waits registered with this handle will
/// automatically inherit this wakeability.
///
/// Registration is cumulative; new signals are combined with any existing wake signals
/// registered for this handle.
///
/// |handle| is borrowed
/// |dispatcher| is the dispatcher that the wake vector applies to.
/// |signals| are the signals that will wake the driver
///
/// # Errors
///
/// ZX_ERR_BAD_STATE: The dispatcher is suspended.
zx_status_t fdf_dispatcher_register_wake_vector(fdf_dispatcher_t* dispatcher, zx_handle_t handle,
                                                zx_signals_t signals) ZX_AVAILABLE_SINCE(32);

/// Unregisters the signals on the given handle as wake vectors.
///
/// If the |signals| provided don't exactly match the signals that are currently
/// registered, the delta between the two will still remain registered.
///
/// |handle| is borrowed
/// |dispatcher| is the dispatcher that the wake vector is removed from.
/// |signals| are the signals that will be unregistered
///
/// # Errors
///
/// ZX_ERR_NOT_FOUND: The |handle| was never registered.
zx_status_t fdf_dispatcher_unregister_wake_vector(fdf_dispatcher_t* dispatcher, zx_handle_t handle,
                                                  zx_signals_t signals) ZX_AVAILABLE_SINCE(32);

__END_CDECLS

#endif  // LIB_FDF_DISPATCHER_H_
