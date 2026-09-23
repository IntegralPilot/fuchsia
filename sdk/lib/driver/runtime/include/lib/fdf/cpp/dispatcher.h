// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FDF_CPP_DISPATCHER_H_
#define LIB_FDF_CPP_DISPATCHER_H_

#include <lib/async/dispatcher.h>
#include <lib/fdf/cpp/unowned.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/stdcompat/string_view.h>
#include <lib/zx/handle.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <string>

namespace fdf_env {
// Forward declaration to support friend declaration.
class DispatcherBuilder;
}  // namespace fdf_env

namespace fdf_internal {
// Forward declaration to support friend declaration.
class TestDispatcherBuilder;
}  // namespace fdf_internal

namespace fdf {

// C++ wrapper for a dispatcher, with RAII semantics. Automatically destroys
// the dispatcher when it goes out of scope. The dispatcher must be completely
// shutdown before it is destroyed. Shutdown completion is notified by the
// |ShutdownHandler| passed to |fdf::SynchronizedDispatcher::Create| or
// |fdf::UnsynchronizedDispatcher::Create|.
//
// # Thread safety
//
// This class is thread-unsafe.
//
// # Example
//
//   void Driver::OnDispatcherShutdown(fdf_dispatcher_t* dispatcher) {
//     // Handle dispatcher shutdown.
//     // It is now safe to destroy |dispatcher|.
//   }
//
//   void Driver::Start() {
//     // TODO(https://fxbug.dev/42166901): update this once scheduler_role is supported.
//     const std::string_view scheduler_role = "";
//     const std::string_view name = "MyDriver";
//
//     auto shutdown_handler = [&]() {
//       OnDispatcherShutdown();
//     };
//     auto dispatcher =
//         fdf::SynchronizedDispatcher::Create(0, name, shutdown_handler, scheduler_role);
//
//     fdf::ChannelRead channel_read;
//     ...
//     zx_status_t status = channel_read->Begin(dispatcher.get());
//
//     // The dispatcher will call the channel_read handler when ready.
//   }
class Dispatcher {
 public:
  using HandleType = fdf_dispatcher_t*;

  // Called when the asynchronous shutdown for |dispatcher| has completed.
  using ShutdownHandler = fit::callback<void(fdf_dispatcher_t* dispatcher)>;

  // Returns the current thread's dispatcher.
  // This will return NULL if not called from a dispatcher managed thread.
  static Unowned<Dispatcher> GetCurrent() {
    return Unowned<Dispatcher>(fdf_dispatcher_get_current_dispatcher());
  }

  // Returns an unowned dispatcher provided an async dispatcher. If |async_dispatcher| was not
  // retrieved via |fdf_dispatcher_get_async_dispatcher|, the call will result in a crash.
  static Unowned<Dispatcher> Downcast(async_dispatcher_t* async_dispatcher) {
    return Unowned<Dispatcher>(fdf_dispatcher_downcast_async_dispatcher(async_dispatcher));
  }

  explicit Dispatcher(fdf_dispatcher_t* dispatcher = nullptr) : dispatcher_(dispatcher) {}

  // Dispatcher cannot be copied.
  Dispatcher(const Dispatcher& to_copy) = delete;
  Dispatcher& operator=(const Dispatcher& other) = delete;

  // Dispatcher can be moved. Once moved, invoking a method on an instance will
  // yield undefined behavior.
  Dispatcher(Dispatcher&& other) noexcept : Dispatcher(other.release()) {}
  Dispatcher& operator=(Dispatcher&& other) noexcept {
    reset(other.release());
    return *this;
  }

  // Begins shutting down the dispatcher. Shutting down is an asynchronous operation.
  //
  // Once |Dispatcher::ShutdownAsync| is called, the dispatcher will no longer
  // accept queueing new |async_dispatcher_t| operations or |ChannelRead| callbacks.
  //
  // The dispatcher will asynchronously wait for all pending |async_dispatcher_t|
  // and |ChannelRead| callbacks to complete. Then it will serially cancel all
  // remaining callbacks with |ZX_ERR_CANCELED| and call the shutdown handler set
  // in |SynchronizedDispatcher::Create| or |UnsynchronizedDispatcher::Create|.
  //
  // If the dispatcher is already shutting down or has completed shutdown, this will do nothing.
  void ShutdownAsync() {
    if (dispatcher_) {
      fdf_dispatcher_shutdown_async(dispatcher_);
    }
  }

  // The dispatcher must be completely shutdown before the dispatcher can be closed.
  // i.e. the shutdown handler set in |SynchronizedDispatcher::Create|
  // or |UnsynchronizedDispatcher::Create| has been called.
  // It is safe to call this from that shutdown handler.
  // TODO(https://fxbug.dev/350026710): consider calling release() automatically
  // on destruction.
  ~Dispatcher() { close(); }

  fdf_dispatcher_t* get() const { return dispatcher_; }

  void reset(fdf_dispatcher_t* dispatcher = nullptr) {
    close();
    dispatcher_ = dispatcher;
  }

  void close() {
    if (dispatcher_) {
      fdf_dispatcher_destroy(dispatcher_);
      dispatcher_ = nullptr;
    }
  }

  fdf_dispatcher_t* release() {
    fdf_dispatcher_t* ret = dispatcher_;
    dispatcher_ = nullptr;
    return ret;
  }

  // Gets the dispatcher's asynchronous dispatch interface.
  async_dispatcher_t* async_dispatcher() const {
    return dispatcher_ ? fdf_dispatcher_get_async_dispatcher(dispatcher_) : nullptr;
  }

  // Returns the options set for this dispatcher.
  std::optional<uint32_t> options() const {
    return dispatcher_ ? std::optional(fdf_dispatcher_get_options(dispatcher_)) : std::nullopt;
  }

  Unowned<Dispatcher> borrow() const { return Unowned<Dispatcher>(dispatcher_); }

  // Removes the allow_sync option that was set on the dispatcher during creation.
  // Calling this will remove the ability of the dispatcher to make any more synchronous calls.
  //
  // # Thread requirements
  //
  // This must be called from the |dispatcher|'s own context.
  //
  // # Errors
  //
  // ZX_ERR_BAD_STATE: Not called from the same context as the given dispatcher or the dispatcher
  // is not in a running state.
  //
  // ZX_ERR_ALREADY_EXISTS: The dispatcher did not contain the allow_sync option. This can happen
  // if this is called more than once, or if it was not created with that option.
  zx::result<> SealAllowSync() {
    return zx::make_result(
        fdf_dispatcher_seal(dispatcher_, FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS));
  }

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
  // Returns the always-on dispatcher interface for this dispatcher.
  // The returned dispatcher is unowned and shares the lifetime of this dispatcher.
  Unowned<Dispatcher> GetAlwaysOnDispatcher() const {
    return Unowned<Dispatcher>(fdf_dispatcher_get_always_on_dispatcher(dispatcher_));
  }
#endif

 protected:
  // Friend declaration is needed because the |DispatcherShutdownContext| is private.
  friend class fdf_env::DispatcherBuilder;
  friend class fdf_internal::TestDispatcherBuilder;

  class DispatcherShutdownContext final {
   public:
    explicit DispatcherShutdownContext(ShutdownHandler handler)
        : observer_{CallHandler}, handler_(std::move(handler)) {}

    fdf_dispatcher_shutdown_observer_t* observer() { return &observer_; }

   private:
    static void CallHandler(fdf_dispatcher_t* dispatcher,
                            fdf_dispatcher_shutdown_observer_t* observer) {
      static_assert(offsetof(DispatcherShutdownContext, observer_) == 0);
      auto self = reinterpret_cast<DispatcherShutdownContext*>(observer);
      self->handler_(dispatcher);

      // Delete the pointer allocated in |SynchronizedDispatcher::Create| or
      // |UnsynchronizedDispatcher::Create|.
      delete self;
    }

    fdf_dispatcher_shutdown_observer_t observer_;
    ShutdownHandler handler_;
  };

  fdf_dispatcher_t* dispatcher_;
};

// Dispatcher that disallows parallel calls into callbacks.
class SynchronizedDispatcher final : public Dispatcher {
 public:
  // Options that may be passed to |fdf::SynchronizedDispatcher::Create|.
  // When using the |SynchronizedDispatcher| class, the dispatcher options must set
  // FDF_DISPATCHER_OPTION_SYNCHRONIZED.
  struct Options {
    // The options to set for the dispatcher. In additional to FDF_DISPATCHER_OPTION_SYNCHRONIZED,
    // the following options are supported:
    //   * `FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS` - blocking calls may be made on this dispatcher.
    uint32_t value = FDF_DISPATCHER_OPTION_SYNCHRONIZED;
    // Specifies a synchronized dispatcher that allows blocking calls to be made.
    static const Options kAllowSyncCalls;
  };

  // Creates a dispatcher for performing asynchronous operations.
  //
  // |options| provides the dispatcher configuration. This will panic if options does not
  // set FDF_DISPATCHER_OPTION_SYNCHRONIZED.
  //
  // |name| is reported via diagnostics. It is similar to setting the name of a thread. Names longer
  // than `ZX_MAX_NAME_LEN` may be truncated.
  //
  // |scheduler_role| is a hint. It may or not impact the priority the work scheduler against the
  // dispatcher is handled at. It may or may not impact the ability for other drivers to share
  // zircon threads with the dispatcher.
  //
  // |shutdown_handler| will be called after |ShutdownAsync| has been called, and the dispatcher
  // has completed its asynchronous shutdown. The client must keep any pointers that are
  // referenced in |shutdown_handler| alive until the handler runs.
  //
  // # Thread requirements
  //
  // This must be called from a thread managed by the driver runtime.
  //
  // # Errors
  //
  // ZX_ERR_NOT_SUPPORTED: |options| is not a supported configuration, which is any of the
  // following:
  //   * `FDF_DISPATCHER_OPTION_UNSYNCHRONIZED` with `FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS`.
  //
  // ZX_ERR_INVALID_ARGS: This was not called from a thread managed by the driver runtime.
  //
  // ZX_ERR_NO_RESOURCES: A thread needed to be spawned to create this dispatcher, but couldn't be.
  // This is likely because a dispatcher that allows sync calls was requested, but the thread pool
  // is currently at its limit.
  //
  // ZX_ERR_BAD_STATE: Dispatchers are currently not allowed to be created, such as when a driver
  // is being shutdown by its driver host.
  static zx::result<SynchronizedDispatcher> Create(Options options, std::string_view name,
                                                   ShutdownHandler shutdown_handler,
                                                   std::string_view scheduler_role = {}) {
    ZX_ASSERT_MSG((options.value & FDF_DISPATCHER_OPTION_SYNCHRONIZATION_MASK) ==
                      FDF_DISPATCHER_OPTION_SYNCHRONIZED,
                  "options.value=%u, needs to have FDF_DISPATCHER_OPTION_SYNCHRONIZED",
                  options.value);
    ZX_ASSERT_MSG(shutdown_handler, "Cannot provide a null shutdown handler.");
    // We need to create an additional shutdown context in addition to the fdf::Dispatcher
    // object, as the fdf::SynchronizedDispatcher may be destructed before the shutdown handler
    // is called. This can happen if the raw pointer is released from the
    // fdf::SynchronizedDispatcher.
    auto dispatcher_shutdown_context =
        std::make_unique<DispatcherShutdownContext>(std::move(shutdown_handler));
    fdf_dispatcher_t* dispatcher;
    zx_status_t status =
        fdf_dispatcher_create(FDF_DISPATCHER_OPTION_SYNCHRONIZED | options.value, name.data(),
                              name.size(), scheduler_role.data(), scheduler_role.size(),
                              dispatcher_shutdown_context->observer(), &dispatcher);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    dispatcher_shutdown_context.release();
    return zx::ok(SynchronizedDispatcher(dispatcher));
  }

  explicit SynchronizedDispatcher(fdf_dispatcher_t* dispatcher = nullptr) : Dispatcher(dispatcher) {
    if (dispatcher) {
      ZX_ASSERT((fdf_dispatcher_get_options(dispatcher) &
                 FDF_DISPATCHER_OPTION_SYNCHRONIZATION_MASK) == FDF_DISPATCHER_OPTION_SYNCHRONIZED);
    }
  }

  Unowned<SynchronizedDispatcher> borrow() const {
    return Unowned<SynchronizedDispatcher>(dispatcher_);
  }
};

inline constexpr SynchronizedDispatcher::Options SynchronizedDispatcher::Options::kAllowSyncCalls =
    {FDF_DISPATCHER_OPTION_SYNCHRONIZED | FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS};

// Dispatcher that allows parallel calls into callbacks.
class UnsynchronizedDispatcher final : public Dispatcher {
 public:
  // Options that may be passed to |fdf::UnsynchronizedDispatcher::Create|.
  // When using the |UnsynchronizedDispatcher| class, the dispatcher options must set
  // FDF_DISPATCHER_OPTION_UNSYNCHRONIZED.
  struct Options {
    // The options to set for the dispatcher. Currently no additional options are supported.
    uint32_t value = FDF_DISPATCHER_OPTION_UNSYNCHRONIZED;
  };

  // Creates a dispatcher for performing asynchronous operations.
  //
  // |options| provides the dispatcher configuration. This will panic if options does not
  // set FDF_DISPATCHER_OPTION_UNSYNCHRONIZED.
  //
  // |name| is reported via diagnostics. It is similar to setting the name of a thread. Names longer
  // than `ZX_MAX_NAME_LEN` may be truncated.
  //
  // |scheduler_role| is a hint. It may or not impact the priority the work scheduler against the
  // dispatcher is handled at. It may or may not impact the ability for other drivers to share
  // zircon threads with the dispatcher.
  //
  // |shutdown_handler| will be called after |ShutdownAsync| has been called, and the dispatcher
  // has completed its asynchronous shutdown. The client must keep any pointers that are
  // referenced in |shutdown_handler| alive until the handler runs.
  //
  // # Thread requirements
  //
  // This must be called from a thread managed by the driver runtime.
  //
  // # Errors
  //
  // ZX_ERR_NOT_SUPPORTED: |options| is not a supported configuration, which is any of the
  // following:
  //   * `FDF_DISPATCHER_OPTION_UNSYNCHRONIZED` with `FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS`.
  //
  // ZX_ERR_INVALID_ARGS: This was not called from a thread managed by the driver runtime.
  //
  // ZX_ERR_NO_RESOURCES: A thread needed to be spawned to create this dispatcher, but couldn't be.
  // This is likely because a dispatcher that allows sync calls was requested, but the thread pool
  // is currently at its limit.
  //
  // ZX_ERR_BAD_STATE: Dispatchers are currently not allowed to be created, such as when a driver
  // is being shutdown by its driver host.
  static zx::result<UnsynchronizedDispatcher> Create(Options options, std::string_view name,
                                                     ShutdownHandler shutdown_handler,
                                                     std::string_view scheduler_role = {}) {
    ZX_ASSERT_MSG((options.value & FDF_DISPATCHER_OPTION_SYNCHRONIZATION_MASK) ==
                      FDF_DISPATCHER_OPTION_UNSYNCHRONIZED,
                  "options.value=%u, needs to have FDF_DISPATCHER_OPTION_UNSYNCHRONIZED",
                  options.value);
    ZX_ASSERT_MSG(shutdown_handler, "Cannot provide a null shutdown handler.");
    // We need to create an additional shutdown context in addition to the fdf::Dispatcher
    // object, as the fdf::UnsynchronizedDispatcher may be destructed before the shutdown handler
    // is called. This can happen if the raw pointer is released from the
    // fdf::UnsynchronizedDispatcher.
    auto dispatcher_shutdown_context =
        std::make_unique<DispatcherShutdownContext>(std::move(shutdown_handler));
    fdf_dispatcher_t* dispatcher;
    zx_status_t status =
        fdf_dispatcher_create(FDF_DISPATCHER_OPTION_UNSYNCHRONIZED | options.value, name.data(),
                              name.size(), scheduler_role.data(), scheduler_role.size(),
                              dispatcher_shutdown_context->observer(), &dispatcher);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    dispatcher_shutdown_context.release();
    return zx::ok(UnsynchronizedDispatcher(dispatcher));
  }

  explicit UnsynchronizedDispatcher(fdf_dispatcher_t* dispatcher = nullptr)
      : Dispatcher(dispatcher) {
    if (dispatcher) {
      ZX_ASSERT(
          (fdf_dispatcher_get_options(dispatcher) & FDF_DISPATCHER_OPTION_SYNCHRONIZATION_MASK) ==
          FDF_DISPATCHER_OPTION_UNSYNCHRONIZED);
    }
  }

  Unowned<UnsynchronizedDispatcher> borrow() const {
    return Unowned<UnsynchronizedDispatcher>(dispatcher_);
  }
};

using UnownedDispatcher = Unowned<Dispatcher>;
using UnownedSynchronizedDispatcher = Unowned<SynchronizedDispatcher>;
using UnownedUnsynchronizedDispatcher = Unowned<UnsynchronizedDispatcher>;

}  // namespace fdf

#endif  // LIB_FDF_CPP_DISPATCHER_H_
