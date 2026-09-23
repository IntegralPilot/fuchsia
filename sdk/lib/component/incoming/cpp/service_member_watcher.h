// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_COMPONENT_INCOMING_CPP_SERVICE_MEMBER_WATCHER_H_
#define LIB_COMPONENT_INCOMING_CPP_SERVICE_MEMBER_WATCHER_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.unknown/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/constants.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/directory_watcher.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/zx/channel.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>

#include <string>
#include <utility>

namespace component {

// Watch for service instances and connect to a member protocol of each instance.
//
// ServiceMemberWatcher and SyncServiceMemberWatcher are templated on a ServiceMember, which
// specifies both the service that it is a part of, and a member protocol of that service.
//
// For a fidl protocol and service defined as:
//   library fidl.examples.echo;
//   protocol DriverEcho {...}
//   service DriverEchoService {
//       echo_device client_end:DriverEcho;
//   };
//
// The ServiceMember would be: fidl_examples_echo::Service::EchoDevice
// Note that the last part of the ServiceMember corresponds to the name of the
// client_end in the service, not the protocol type.
//
// Services can be waited on asynchronously with ServiceMemberWatcher and synchronously with
// SyncServiceMemberWatcher.  If you have a service with multiple protocols, and need to access
// more than one protocol of a service for each instance, you can use component::ServiceWatcher
//
// Define a callback function:
//  void OnInstanceFound(ClientEnd<fidl_examples_echo::DriverEcho> client_end) {...}
// Optionally define an idle function, which will be called when all existing instances have been
// enumerated:
//  void AllExistingEnumerated() {...}
// Create the ServiceMemberWatcher:
// ServiceMemberWatcher<fidl_examples_echo::Service::EchoDevice> watcher;
// watcher->Begin(get_default_dispatcher(), &OnInstanceFound, &AllExistingEnumerated);
//
// The ServiceMemberWatcher will stop upon destruction, or when |Cancel| is called.
template <typename ServiceMember>
class ServiceMemberWatcher {
  static_assert(fidl::IsServiceMemberV<ServiceMember>, "Type must be a member of a service");

 public:
  using Protocol = typename ServiceMember::ProtocolType;
  using ClientCallback = fit::function<void(fidl::ClientEnd<Protocol>)>;
  // Callback function which is invoked when a service instance is found,
  // providing both the client end to the protocol and the instance name.
  using ClientCallbackWithInstance = fit::function<void(fidl::ClientEnd<Protocol>, std::string)>;
  // Callback function which is invoked after the existing service instances have been
  // reported via the client callback, and before newly-arriving service instances are delivered
  // via the client callback.
  using IdleCallback = fit::callback<void()>;

  // Cancels watching for service instances.
  zx::result<> Cancel() {
    client_callback_with_instance_ = nullptr;
    idle_callback_ = nullptr;
    return zx::make_result(directory_watcher_.Cancel());
  }

  // For tests, the service root can be set manually
  explicit ServiceMemberWatcher(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root)
      : svc_dir_(svc_root) {}
  ServiceMemberWatcher()
      : default_svc_dir_(OpenServiceRoot().value()), svc_dir_(default_svc_dir_.borrow()) {}
  // Begins asynchronously waiting for service instances using the given dispatcher.
  //
  // Waits for services in the incoming service directory: /svc/ServiceMember::ServiceName
  //
  // Asynchronously invokes the callback for all existing service instances
  // within the specified aggregate service type, as well as any subsequently added
  // instances until service member watcher is destroyed. Ignores any service
  // named ".".
  //
  // The |idle_callback| is invoked once immediately after all pre-existing
  // service instances have been reported via the callback shortly after creation.
  // After |idle_callback| returns, any newly-arriving instances are reported via
  // the callback.
  // |idle_callback| will be deleted after it is called, so captured context
  // is guaranteed to not be retained.
  zx::result<> Begin(
      async_dispatcher_t* dispatcher, ClientCallback callback, IdleCallback idle_callback = [] {}) {
    return Begin(
        dispatcher,
        [cb = std::move(callback)](fidl::ClientEnd<Protocol> client, std::string /*instance*/) {
          cb(std::move(client));
        },
        std::move(idle_callback));
  }

  // Begins asynchronously waiting for service instances using the given dispatcher.
  //
  // Similar to the other Begin overload, but the callback |callback| is invoked
  // with both the client end and the instance name.
  zx::result<> Begin(
      async_dispatcher_t* dispatcher, ClientCallbackWithInstance callback,
      IdleCallback idle_callback = [] {}) {
    // Begin should only be called once
    ZX_ASSERT(client_callback_with_instance_ == nullptr);
    client_callback_with_instance_ = std::move(callback);
    idle_callback_ = std::move(idle_callback);
    auto service_directory_result = OpenDirectoryAt(svc_dir_, ServiceMember::ServiceName);
    if (service_directory_result.is_error()) {
      return service_directory_result.take_error();
    }
    return zx::make_result(directory_watcher_.Begin(
        std::move(service_directory_result.value()),
        fit::bind_member<&ServiceMemberWatcher::OnWatchedEvent>(this), dispatcher));
  }

 private:
  void OnWatchedEvent(fuchsia_io::wire::WatchEvent event, std::string instance) {
    if (event == fuchsia_io::wire::WatchEvent::kIdle) {
      idle_callback_();
      return;
    }
    if (event == fuchsia_io::wire::WatchEvent::kRemoved || instance == "." || instance.empty()) {
      return;
    }

    zx::result<fidl::ClientEnd<typename ServiceMember::ProtocolType>> client_result =
        ConnectAtMember<ServiceMember>(svc_dir_, instance);
    // This should not fail, since the directory just gave us the instance.
    ZX_ASSERT(client_result.is_ok());
    client_callback_with_instance_(std::move(client_result.value()), std::move(instance));
  }

  ClientCallbackWithInstance client_callback_with_instance_;
  IdleCallback idle_callback_;
  // for default initialization we hold ownership of the client_end.
  fidl::ClientEnd<fuchsia_io::Directory> default_svc_dir_;
  fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir_;
  component::DirectoryWatcher directory_watcher_;
};

// SyncServiceMemberWatcher allows services to be waited for synchronously.
// Note that the this class is templated on the service member name, not the service name.
// For example:
// SyncServiceMemberWatcher<fidl_examples_echo::Service::EchoDevice> watcher;
// zx::result<ClientEnd<fidl_examples_echo::DriverEcho>> result = watcher.GetNextInstance(true);
template <typename ServiceMember>
class SyncServiceMemberWatcher final : public ServiceMemberWatcher<ServiceMember> {
  static_assert(fidl::IsServiceMemberV<ServiceMember>, "Type must be a member of a service");

 public:
  using Protocol = typename ServiceMember::ProtocolType;
  explicit SyncServiceMemberWatcher(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root)
      : ServiceMemberWatcher<ServiceMember>(svc_root),
        svc_dir_(svc_root),
        sync_dir_watcher_(svc_dir_, ServiceMember::ServiceName) {}
  SyncServiceMemberWatcher()
      : default_svc_dir_(OpenServiceRoot().value()),
        svc_dir_(default_svc_dir_.borrow()),
        sync_dir_watcher_(svc_dir_, ServiceMember::ServiceName) {}

  // Sequentially query for service instances at /svc/ServiceMember::ServiceName
  //
  // This call will block until a service instance is found. When an instance of the given
  // service is detected in the /svc/ServiceMember::ServiceName directory, this function
  // will return a ClientEnd to the protocol specified by ServiceMember::ProtocolType.
  //
  // Subsequent calls to GetNextInstance will return other instances if they exist.
  // GetNextInstance will iterate through all service instances of a given type.
  // When all of the existing service instances have been returned,
  // if |stop_at_idle| is true, GetNextInstance will return a zx::error(ZX_ERR_STOP).
  // Otherwise, GetNextInstance will wait until |deadline| for a new instance to appear.
  zx::result<fidl::ClientEnd<Protocol>> GetNextInstance(bool stop_at_idle,
                                                        zx::time deadline = zx::time::infinite()) {
    return GetNextInstance(stop_at_idle, nullptr, deadline);
  }

  // Sequentially query for service instances at /svc/ServiceMember::ServiceName.
  //
  // Similar to the other GetNextInstance overload, but if |out_instance_name| is
  // not null, it will be populated with the name of the service instance that
  // was connected to.
  zx::result<fidl::ClientEnd<Protocol>> GetNextInstance(bool stop_at_idle,
                                                        std::string* out_instance_name,
                                                        zx::time deadline = zx::time::infinite()) {
    zx::result result = sync_dir_watcher_.GetNextEntry(stop_at_idle, deadline);
    if (result.is_error()) {
      return result.take_error();
    }
    auto client_result = ConnectAtMember<ServiceMember>(svc_dir_, result.value());
    if (client_result.is_ok() && out_instance_name) {
      *out_instance_name = std::move(result.value());
    }
    return client_result;
  }

  // for default initialization we hold ownership of the client_end.
  fidl::ClientEnd<fuchsia_io::Directory> default_svc_dir_;
  fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir_;
  SyncDirectoryWatcher sync_dir_watcher_;
};

}  // namespace component

#endif  // LIB_COMPONENT_INCOMING_CPP_SERVICE_MEMBER_WATCHER_H_
