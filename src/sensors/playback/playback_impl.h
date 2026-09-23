// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SENSORS_PLAYBACK_PLAYBACK_IMPL_H_
#define SRC_SENSORS_PLAYBACK_PLAYBACK_IMPL_H_

#include <fidl/fuchsia.hardware.sensors/cpp/fidl.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/scope.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include "src/camera/lib/actor/actor_base.h"
#include "src/sensors/playback/playback_controller.h"

namespace sensors::playback {

class PlaybackImpl : public camera::actor::ActorBase,
                     public fidl::Server<fuchsia_hardware_sensors::Playback> {
  using ConfigurePlaybackError = fuchsia_hardware_sensors::ConfigurePlaybackError;
  using FixedValuesPlaybackConfig = fuchsia_hardware_sensors::FixedValuesPlaybackConfig;
  using PlaybackSourceConfig = fuchsia_hardware_sensors::PlaybackSourceConfig;
  using Playback = fuchsia_hardware_sensors::Playback;

 public:
  PlaybackImpl(async_dispatcher_t* dispatcher, PlaybackController& controller);

  void ConfigurePlayback(ConfigurePlaybackRequest& request,
                         ConfigurePlaybackCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<Playback> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  // Server connection management.
  /* LIFECYCLE NOTES:
     Cleaning up when a Driver protocol client disconnects involves waiting for some cleanup to
     happen on the playback controller thread. In order to do this, the version of BindServer which
     accepts a shared_ptr is used so that the DriverImpl instance can be kept alive after the
     OnUnbound callback returns.

     A copy of the shared_ptr is stored in a callback which is given to the controller when it is
     notified of the client's disconnection. When the controller cleanup is done, it will call the
     callback which in turn schedules a promise that resets the shared_ptr (deleting the DriverImpl
     instance). It's not legal for the DriverImpl to delete itself in a task scheduled on it's own
     Executor. Instead, a separate Executor which schedules on the same thread but outlives the
     DriverImpl instance is provided for the final part of the cleanup.
  */
  static void BindSelfManagedServer(async_dispatcher_t* dispatcher,
                                    async::Executor& cleanup_executor,
                                    PlaybackController& controller,
                                    fidl::ServerEnd<Playback> server_end) {
    std::shared_ptr impl = std::make_unique<PlaybackImpl>(dispatcher, controller);
    std::shared_ptr unbind_handle = impl;
    PlaybackImpl* impl_ptr = impl.get();

    fpromise::promise<void> unbind_promise =
        fpromise::make_promise([client_connected = &client_connected_,
                                unbind_handle = std::move(unbind_handle)]() mutable {
          unbind_handle.reset();
          *client_connected = false;
        });

    fit::callback<void()> unbind_callback = [&cleanup_executor,
                                             unbind_promise = std::move(unbind_promise)]() mutable {
      cleanup_executor.schedule_task(fpromise::pending_task(std::move(unbind_promise)));
    };

    fidl::ServerBindingRef binding_ref =
        fidl::BindServer(dispatcher, std::move(server_end), std::move(impl),
                         [unbind_callback = std::move(unbind_callback)](
                             PlaybackImpl* impl, fidl::UnbindInfo info,
                             fidl::ServerEnd<Playback> server_end) mutable {
                           impl->OnUnbound(info, std::move(server_end), std::move(unbind_callback));
                         });

    // Only one client permitted at a time.
    if (client_connected_) {
      FX_LOGS(WARNING) << "Playback client already connected, closing new connection.";
      binding_ref.Close(ZX_ERR_ALREADY_BOUND);
      return;
    }

    impl_ptr->binding_ref_.emplace(std::move(binding_ref));
    client_connected_ = true;
  }

  void OnUnbound(fidl::UnbindInfo info, fidl::ServerEnd<Playback> server_end,
                 fit::callback<void()> unbind_callback);

 private:
  static bool client_connected_;

  std::optional<fidl::ServerBindingRef<Playback>> binding_ref_;

  PlaybackController& controller_;

  // This should always be the last thing in the object. Otherwise scheduled tasks within this scope
  // which reference members of this object may be allowed to run after destruction of this object
  // has started. Keeping this at the end ensures that the scope is destroyed first, cancelling any
  // scheduled tasks before the rest of the members are destroyed.
  fpromise::scope scope_;
};

}  // namespace sensors::playback

#endif  // SRC_SENSORS_PLAYBACK_PLAYBACK_IMPL_H_
