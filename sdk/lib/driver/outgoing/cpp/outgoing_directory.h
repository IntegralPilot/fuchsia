// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_OUTGOING_CPP_OUTGOING_DIRECTORY_H_
#define LIB_DRIVER_OUTGOING_CPP_OUTGOING_DIRECTORY_H_

#include <lib/component/outgoing/cpp/handlers.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/outgoing/cpp/handlers.h>
#include <lib/fdf/cpp/channel.h>
#include <lib/fdf/cpp/protocol.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fidl/cpp/wire/service_handler.h>
#include <lib/fidl_driver/cpp/transport.h>

#include <unordered_set>

namespace fdf {

// The name of the default FIDL Service instance.
constexpr const std::string_view kDefaultInstance =
    component::OutgoingDirectory::kDefaultServiceInstance;

// Driver specific implementation for an outgoing directory that wraps around
// |component::OutgoingDirectory|.
//
// # Thread safety
//
// This class is thread-unsafe. Instances must be managed and used from tasks
// running on the |fdf_dispatcher_t|, and the dispatcher must be synchronized.
// See
// https://fuchsia.dev/fuchsia-src/development/languages/c-cpp/thread-safe-async#mutual-exclusion-guarantee
class OutgoingDirectory final {
 public:
  static OutgoingDirectory Create(fdf_dispatcher_t* dispatcher) {
    ZX_ASSERT_MSG(dispatcher != nullptr,
                  "OutgoingDirectory::Create received nullptr |dispatcher|.");
    return OutgoingDirectory(dispatcher);
  }

  explicit OutgoingDirectory(fdf_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->get())
      : component_outgoing_dir_(fdf_dispatcher_get_async_dispatcher(dispatcher)),
        dispatcher_(dispatcher) {}

  // OutgoingDirectory can be moved. Once moved, invoking a method on an
  // instance will yield undefined behavior.
  OutgoingDirectory(OutgoingDirectory&&) noexcept;
  OutgoingDirectory& operator=(OutgoingDirectory&&) noexcept;

  // OutgoingDirectory cannot be copied.
  OutgoingDirectory(const OutgoingDirectory&) = delete;
  OutgoingDirectory& operator=(const OutgoingDirectory&) = delete;

  // Adds an instance of a FIDL Service.
  //
  // A |handler| is added to provide an |instance| of a service.
  //
  // The template type |Service| must be the generated type representing a FIDL Service.
  // The template type must be a specialization of |fidl::ServiceInstanceHandler<Transport>|
  // where Transport must be either |fidl::internal::ChannelTransport| for |zx::channel|
  // transports, or |fidl::internal::DriverTransport| for |fdf::Channel| transport. Users
  // should not use this method directly and instead should use the specialization
  // for |component::ServiceInstanceHandler| and |fdf::ServiceInstanceHandler|
  // instead.
  //
  // # Errors
  //
  // ZX_ERR_ALREADY_EXISTS: The instance already exists.
  //
  // ZX_ERR_INVALID_ARGS: |instance| is an empty string or |handler| is empty.
  template <typename TransportHandler, typename Service>
  zx::result<> AddService(TransportHandler handler, std::string_view instance = kDefaultInstance) {
    component::ServiceInstanceHandler component_handler;

    if constexpr (std::is_same_v<TransportHandler, fdf::ServiceInstanceHandler>) {
      // If the driver transport is being used, create a handler that registers the runtime token
      // and add that to the |component_handler|.
      auto handlers = handler.TakeMemberHandlers();
      if (handlers.empty()) {
        return zx::make_result(ZX_ERR_INVALID_ARGS);
      }

      for (auto& [member_name, member_handler] : handlers) {
        zx::result<> add_result = component_handler.AddAnyMember(
            [this, m_handler = std::move(member_handler)](zx::channel server_end) mutable {
              ZX_ASSERT(server_end.is_valid());

              // The received |server_end| is the token channel handle, which needs to be registered
              // with the runtime. Sharing the handler, rather than moving it, as this callback can
              // be called multiple times.
              RegisterRuntimeToken(std::move(server_end), m_handler.share());
            },
            member_name);
        if (add_result.is_error()) {
          return add_result;
        }
      }

      // If one already exists, insert returns the iterator to the existing item instead of
      // resetting it to an empty set.
      auto [it, _inserted] = driver_services_.try_emplace(std::string(Service::Name));
      it->second.insert(std::string(instance));
    } else if constexpr (std::is_same_v<TransportHandler, component::ServiceInstanceHandler>) {
      // If zircon transport is used, just take the user given handler and use that.
      component_handler = std::move(handler);

      // If one already exists, insert returns the iterator to the existing item instead of
      // resetting it to an empty set.
      auto [it, _inserted] = zircon_services_.try_emplace(std::string(Service::Name));
      it->second.insert(std::string(instance));
    } else {
      static_assert(
          false,
          "TransportHandler must be either fidl::internal::ChannelTransport or fidl::internal::DriverTransport");
    }

    return component_outgoing_dir_.AddService<Service>(std::move(component_handler), instance);
  }

  // Same as above but strictly for services using the driver channel transport.
  template <typename Service>
  zx::result<> AddService(fdf::ServiceInstanceHandler handler,
                          std::string_view instance = kDefaultInstance) {
    return AddService<fdf::ServiceInstanceHandler, Service>(std::move(handler), instance);
  }

  // Same as above but strictly for services using the Zircon channel transport.
  template <typename Service>
  zx::result<> AddService(component::ServiceInstanceHandler handler,
                          std::string_view instance = kDefaultInstance) {
    return AddService<component::ServiceInstanceHandler, Service>(std::move(handler), instance);
  }

  //
  // Wrappers around |component::OutgoingDirectory|.
  //
  // Protocols are not supported. Drivers should use |AddService| instead.
  //

  // Starts serving the outgoing directory on the given channel. This should
  // be invoked after the outgoing directory has been populated, e.g. via
  // |AddService|.
  //
  // This object will implement the |fuchsia.io.Directory| interface using this
  // channel. Note that this method returns immediately and that the |dispatcher|
  // provided to the constructor will be responsible for processing messages
  // sent to the server endpoint.
  //
  // # Errors
  //
  // ZX_ERR_BAD_HANDLE: |directory_server_end| is not a valid handle.
  //
  // ZX_ERR_ACCESS_DENIED: |directory_server_end| has insufficient rights.
  zx::result<> Serve(fidl::ServerEnd<fuchsia_io::Directory> directory_server_end) {
    return component_outgoing_dir_.Serve(std::move(directory_server_end));
  }

  // Serve a subdirectory at the root of this outgoing directory.
  //
  // The directory will be installed under the path |directory_name|. When
  // a request is received under this path, then it will be forwarded to
  // |remote_dir|.
  //
  // # Errors
  //
  // ZX_ERR_ALREADY_EXISTS: An entry with the provided name already exists.
  //
  // ZX_ERR_BAD_HANDLE: |remote_dir| is an invalid handle.
  //
  // ZX_ERR_INVALID_ARGS: |directory_name| is an empty string.
  zx::result<> AddDirectory(fidl::ClientEnd<fuchsia_io::Directory> remote_dir,
                            std::string_view directory_name) {
    return component_outgoing_dir_.AddDirectory(std::move(remote_dir), directory_name);
  }

  // Same as |AddDirectory| but allows setting the parent directory
  // in which the directory will be installed.
  zx::result<> AddDirectoryAt(fidl::ClientEnd<fuchsia_io::Directory> remote_dir,
                              std::string_view path, std::string_view directory_name) {
    return component_outgoing_dir_.AddDirectoryAt(std::move(remote_dir), path, directory_name);
  }

  // Removes an instance of a FIDL Service.
  //
  // # Errors
  //
  // ZX_ERR_NOT_FOUND: The instance was not found.
  //
  // # Example
  //
  // ```
  // outgoing.RemoveService<lib_example::MyService>("my-instance");
  // ```
  template <typename Service>
  zx::result<> RemoveService(std::string_view instance = kDefaultInstance) {
    return RemoveService(Service::Name, instance);
  }

  // Same as above but untyped.
  zx::result<> RemoveService(std::string_view service,
                             std::string_view instance = kDefaultInstance) {
    return component_outgoing_dir_.RemoveService(service, instance);
  }

  // Removes the subdirectory on the provided |directory_name|.
  //
  // # Errors
  //
  // ZX_ERR_NOT_FOUND: No entry was found with provided name.
  zx::result<> RemoveDirectory(std::string_view directory_name) {
    return component_outgoing_dir_.RemoveDirectory(directory_name);
  }

  // Same as |RemoveDirectory| but allows specifying the parent directory
  // that the directory will be removed from. The parent directory, |path|,
  // will not be removed.
  zx::result<> RemoveDirectoryAt(std::string_view path, std::string_view directory_name) {
    return component_outgoing_dir_.RemoveDirectoryAt(path, directory_name);
  }

  std::unordered_map<std::string, std::unordered_set<std::string>> GetDriverServices() {
    return driver_services_;
  }

  std::unordered_map<std::string, std::unordered_set<std::string>> GetZirconServices() {
    return zircon_services_;
  }

  // Get the underlying component outgoing directory. These APIs will only support
  // FIDL, they do not support DriverTransport.
  component::OutgoingDirectory& component() { return component_outgoing_dir_; }

 private:
  // Registers |token| with the driver runtime. When the client attempts to connect using the
  // token channel peer, we will call |handler|.
  void RegisterRuntimeToken(zx::channel token, AnyHandler handler);

  component::OutgoingDirectory component_outgoing_dir_;

  fdf::UnownedSynchronizedDispatcher dispatcher_;

  std::unordered_map<std::string, std::unordered_set<std::string>> driver_services_;
  std::unordered_map<std::string, std::unordered_set<std::string>> zircon_services_;
};

}  // namespace fdf

#endif  // LIB_DRIVER_OUTGOING_CPP_OUTGOING_DIRECTORY_H_
