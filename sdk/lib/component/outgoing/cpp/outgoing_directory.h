// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_COMPONENT_OUTGOING_CPP_OUTGOING_DIRECTORY_H_
#define LIB_COMPONENT_OUTGOING_CPP_OUTGOING_DIRECTORY_H_

#include <fidl/fuchsia.io/cpp/markers.h>
#include <lib/async/cpp/sequence_checker.h>
#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/component/outgoing/cpp/handlers.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fidl/cpp/wire/traits.h>
#include <lib/fit/function.h>
#include <lib/svc/dir.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <map>
#include <memory>
#include <stack>
#include <type_traits>

namespace component {

// The directory containing handles to capabilities this component provides.
// Entries served from this outgoing directory should correspond to the
// component manifest's `capabilities` declarations.
//
// The outgoing directory contains one special subdirectory, named `svc`. This
// directory contains the FIDL Services and Protocols offered by this component
// to other components. For example the FIDL Protocol `fuchsia.foo.Bar` will be
// hosted under the path `/svc/fuchsia.foo.Bar`.
//
// # Thread safety
//
// This class is thread-unsafe. Instances must be managed and used from a
// [synchronized async dispatcher][synchronized-dispatcher]. See
// https://fuchsia.dev/fuchsia-src/development/languages/c-cpp/thread-safe-async#synchronized-dispatcher
//
// # Server lifetimes
//
// This class supports a simpler usage style where it owns the server
// implementations published to the directory, and an advanced usage style
// where it borrows the server and where the user manages the server lifetimes.
//
// When the outgoing directory owns the server implementation (e.g. see
// |AddProtocol|), removing the published capability or destroying the outgoing
// directory will destroy the server object and teardown all connections to it.
//
// When the outgoing directory borrows the server implementation (usually via
// callbacks e.g. see |AddUnmanagedProtocol|), removing the published
// capability or destroying the outgoing directory synchronously stops all
// future attempts to connect to the server. Thereafter, the user may teardown
// existing connections and destroy the server.
//
// # Maintainer's Note
//
// This class' API is semantically identical to the one found in
// `//sdk/lib/sys/cpp`. This exists in order to offer equivalent facilities to
// the new C++ bindings. The other class is designed for the older, HLCPP
// (High-Level C++) FIDL bindings. It is expected that once all clients of HLCPP
// are migrated to the new C++ bindings, that library will be removed.
class OutgoingDirectory final {
 public:
  // The name of the default FIDL Service instance.
  static constexpr const char kDefaultServiceInstance[] = "default";

  // The path referencing the outgoing services directory.
  static constexpr const char kServiceDirectory[] = "svc";

  // Creates an OutgoingDirectory which will serve requests when
  // |Serve| or |ServeFromStartupInfo()| is called.
  //
  // |dispatcher| must not be nullptr. If it is, this method will panic.
  explicit OutgoingDirectory(async_dispatcher_t* dispatcher);

  OutgoingDirectory() = delete;

  // OutgoingDirectory can be moved. Once moved, invoking a method on an
  // instance will yield undefined behavior.
  OutgoingDirectory(OutgoingDirectory&&) noexcept;
  OutgoingDirectory& operator=(OutgoingDirectory&&) noexcept;

  // OutgoingDirectory cannot be copied.
  OutgoingDirectory(const OutgoingDirectory&) = delete;
  OutgoingDirectory& operator=(const OutgoingDirectory&) = delete;

  // Destroying the directory will stop any future attempts to connect to the
  // services and protocols published within.
  ~OutgoingDirectory();

  // Starts serving the outgoing directory on the given channel.
  //
  // This should be invoked after the outgoing directory has been populated,
  // i.e. after |AddProtocol|. While |OutgoingDirectory| does not require
  // calling |AddProtocol| before |Serve|, if you call them in the other order
  // there is a chance that requests that arrive in between will be dropped.
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
  zx::result<> Serve(fidl::ServerEnd<fuchsia_io::Directory> directory_server_end);

  // Starts serving the outgoing directory on the channel provided to this
  // process at startup as |PA_DIRECTORY_REQUEST|.
  //
  // This object will implement the |fuchsia.io.Directory| interface using this
  // channel.
  //
  // # Errors
  //
  // ZX_ERR_BAD_HANDLE: the process did not receive a |PA_DIRECTORY_REQUEST|
  // startup handle or it was already taken.
  //
  // ZX_ERR_ACCESS_DENIED: The |PA_DIRECTORY_REQUEST| handle has insufficient
  // rights.
  zx::result<> ServeFromStartupInfo();

  // Adds a FIDL Protocol server instance.
  //
  // |impl| will be used to handle requests for this protocol.
  // |name| is used to determine where to host the protocol. This protocol will
  // be hosted under the path /svc/{name}, where `name` is the discoverable name
  // of the protocol, by default. A pointer to |impl| is returned on the success
  // case of this function call. This pointer will be valid until either this
  // |OutgoingDirectory| is destroyed or |RemoveProtocol| is invoked with the
  // provided |name|.
  //
  // Note, if and when |RemoveProtocol| is called for the provided |name|, this
  // object will asynchronously close down the associated server end channel and
  // stop receiving requests. This method provides no facilities for waiting
  // until teardown is complete. If such control is desired, then the
  // |TypedHandler| overload of this method listed below ought to be used.
  //
  // # Errors
  //
  // ZX_ERR_ALREADY_EXISTS: An entry already exists for this protocol.
  //
  // ZX_ERR_INVALID_ARGS: |impl| is nullptr or |name| is an empty string.
  //
  // # Examples
  //
  // See sample use cases in test case(s) located at
  // //sdk/lib/component/tests/outgoing_directory_test.cc
  template <typename Protocol, typename ServerImpl,
            typename = std::enable_if_t<fidl::IsProtocolV<Protocol>>>
  zx::result<> AddProtocol(std::unique_ptr<ServerImpl> impl,
                           std::string_view name = fidl::DiscoverableProtocolName<Protocol>) {
    return AddProtocolAt<Protocol>(kServiceDirectory, std::move(impl), name);
  }

  // Same as |AddProtocol| except that a typed handler is used. The
  // |handler| is a callback that handles connection requests for this
  // particular protocol.
  //
  // # Note
  //
  // Active connections are never torn down when/if |RemoveProtocol| is invoked
  // with the same |name|. Users of this method should manage teardown of
  // all active connections.
  template <typename Protocol, typename = std::enable_if_t<fidl::IsProtocolV<Protocol>>>
  zx::result<> AddUnmanagedProtocol(
      TypedHandler<Protocol> handler,
      std::string_view name = fidl::DiscoverableProtocolName<Protocol>) {
    static_assert(fidl::IsProtocol<Protocol>(), "Type of |Protocol| must be FIDL protocol");

    return AddUnmanagedProtocolAt<Protocol>(kServiceDirectory, std::move(handler), name);
  }

  // Same as above but is untyped. This method is generally discouraged but
  // is made available if a generic handler needs to be provided.
  zx::result<> AddUnmanagedProtocol(AnyHandler handler, std::string_view name);

  // Same as |AddProtocol| but allows setting the parent directory in
  // which the protocol will be installed.
  template <typename Protocol, typename ServerImpl,
            typename = std::enable_if_t<fidl::IsProtocolV<Protocol>>>
  zx::result<> AddProtocolAt(std::string_view path, std::unique_ptr<ServerImpl> impl,
                             std::string_view name = fidl::DiscoverableProtocolName<Protocol>) {
    static_assert(fidl::IsProtocol<Protocol>(), "Type of |Protocol| must be FIDL protocol");
    if (impl == nullptr || inner().dispatcher_ == nullptr) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    return AddUnmanagedProtocolAt<Protocol>(
        path,
        [dispatcher = inner().dispatcher_, impl = std::move(impl),
         unbind_protocol_callbacks = &inner().unbind_protocol_callbacks_,
         name = std::string(name)](fidl::ServerEnd<Protocol> request) {
          fidl::ServerBindingRef<Protocol> server =
              fidl::BindServer(dispatcher, std::move(request), impl.get());

          auto cb = [server = std::move(server)]() mutable { server.Unbind(); };
          // We don't have to check for entry existing because the |AddProtocol|
          // overload being invoked here will do that internally.
          AppendUnbindConnectionCallback(unbind_protocol_callbacks, name, std::move(cb));
        },
        name);
  }

  // Same as |AddProtocol| but uses a typed handler and allows the usage of
  // setting the parent directory in which the protocol will be installed.
  template <typename Protocol, typename = std::enable_if_t<fidl::IsProtocolV<Protocol>>>
  zx::result<> AddUnmanagedProtocolAt(
      std::string_view path, TypedHandler<Protocol> handler,
      std::string_view name = fidl::DiscoverableProtocolName<Protocol>) {
    static_assert(fidl::IsProtocol<Protocol>(), "Type of |Protocol| must be FIDL protocol");

    auto bridge_func = [handler = std::move(handler)](zx::channel request) {
      fidl::ServerEnd<Protocol> server_end(std::move(request));
      (void)handler(std::move(server_end));
    };

    return AddUnmanagedProtocolAt(std::move(bridge_func), path, name);
  }

  // Same as |AddProtocol| but is untyped and allows the usage of setting the
  // parent directory in which the protocol will be installed.
  zx::result<> AddUnmanagedProtocolAt(AnyHandler handler, std::string_view path,
                                      std::string_view name);

  // Adds an instance of a FIDL Service.
  //
  // A |handler| is added to provide an |instance| of a service.
  //
  // The template type |Service| must be the generated type representing a FIDL Service.
  // The generated class |Service::Handler| helps the caller populate a
  // |ServiceInstanceHandler|.
  //
  // # Errors
  //
  // ZX_ERR_ALREADY_EXISTS: The instance already exists.
  //
  // ZX_ERR_INVALID_ARGS: |instance| is an empty string or |handler| is empty.
  //
  // # Example
  //
  // See sample use cases in test case(s) located at
  // //sdk/lib/sys/component/cpp/outgoing_directory_test.cc
  template <typename Service, typename = std::enable_if_t<fidl::IsServiceV<Service>>>
  zx::result<> AddService(ServiceInstanceHandler handler,
                          std::string_view instance = kDefaultServiceInstance) {
    static_assert(fidl::IsService<Service>(), "Type of |Service| must be FIDL service");

    return AddService(std::move(handler), Service::Name, instance);
  }

  // Same as above but is untyped.
  zx::result<> AddService(ServiceInstanceHandler handler, std::string_view service,
                          std::string_view instance = kDefaultServiceInstance);

  template <typename Service, typename = std::enable_if_t<fidl::IsServiceV<Service>>>
  zx::result<> AddServiceAt(ServiceInstanceHandler handler, std::string_view path,
                            std::string_view instance = kDefaultServiceInstance) {
    static_assert(fidl::IsService<Service>(), "Type of |Service| must be FIDL service");

    return AddServiceAt(std::move(handler), path, Service::Name, instance);
  }

  // Same as above but is untyped.
  zx::result<> AddServiceAt(ServiceInstanceHandler handler, std::string_view path,
                            std::string_view service,
                            std::string_view instance = kDefaultServiceInstance);

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
                            std::string_view directory_name);

  // Same as |AddDirectory| but allows setting the parent directory
  // in which the directory will be installed.
  zx::result<> AddDirectoryAt(fidl::ClientEnd<fuchsia_io::Directory> remote_dir,
                              std::string_view path, std::string_view directory_name);

  // Removes a FIDL Protocol entry with the path `/svc/{name}`.
  //
  // Removing the protocol will stop any future attempts to connect to this
  // protocol.
  //
  // # Errors
  //
  // ZX_ERR_NOT_FOUND: The protocol entry was not found.
  //
  // # Example
  //
  // ```
  // outgoing.RemoveProtocol<lib_example::MyProtocol>();
  // ```
  template <typename Protocol, typename = std::enable_if_t<fidl::IsProtocolV<Protocol>>>
  zx::result<> RemoveProtocol(std::string_view name = fidl::DiscoverableProtocolName<Protocol>) {
    return RemoveProtocol(name);
  }

  // Same as above but untyped.
  zx::result<> RemoveProtocol(std::string_view name);

  // Removes a FIDL Protocol entry located in the provided |directory|.
  // Unlike |RemoveProtocol| which looks for the protocol to remove in the
  // path `/svc/{name}`, this method uses the directory name provided, e.g.
  // `/{path}/{name}`.
  //
  // # Errors
  //
  // ZX_ERR_NOT_FOUND: The protocol entry was not found.
  //
  // # Example
  //
  // ```
  // outgoing.RemoveProtocolAt<lib_example::MyProtocol>("diagnostics");
  // ```
  template <typename Protocol, typename = std::enable_if_t<fidl::IsProtocolV<Protocol>>>
  zx::result<> RemoveProtocolAt(std::string_view path,
                                std::string_view name = fidl::DiscoverableProtocolName<Protocol>) {
    return RemoveProtocolAt(path, name);
  }

  // Same as |RemoveProtocolAt| but is untyped and allows the usage of setting the
  // parent directory in which the protocol will be removed.
  zx::result<> RemoveUnmanagedProtocolAt(std::string_view path, std::string_view name);

  // Same as above but untyped.
  zx::result<> RemoveProtocolAt(std::string_view directory, std::string_view name);

  // Removes an instance of a FIDL Service.
  //
  // Removing the service will stop any future attempts to connect to members
  // within service. Connections to intermediate directories will be closed.
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
  template <typename Service, typename = std::enable_if_t<fidl::IsServiceV<Service>>>
  zx::result<> RemoveService(std::string_view instance = kDefaultServiceInstance) {
    return RemoveService(Service::Name, instance);
  }

  // Same as above but untyped.
  zx::result<> RemoveService(std::string_view service,
                             std::string_view instance = kDefaultServiceInstance);

  template <typename Service, typename = std::enable_if_t<fidl::IsServiceV<Service>>>
  zx::result<> RemoveServiceAt(std::string_view path,
                               std::string_view instance = kDefaultServiceInstance) {
    return RemoveServiceAt(path, Service::Name, instance);
  }

  zx::result<> RemoveServiceAt(std::string_view path, std::string_view service,
                               std::string_view instance);

  // Removes the subdirectory on the provided |directory_name|.
  //
  // # Errors
  //
  // ZX_ERR_NOT_FOUND: No entry was found with provided name.
  zx::result<> RemoveDirectory(std::string_view directory_name);

  // Same as |RemoveDirectory| but allows specifying the parent directory
  // that the directory will be removed from. The parent directory, |path|,
  // will not be removed.
  zx::result<> RemoveDirectoryAt(std::string_view path, std::string_view directory_name);

 private:
  // |svc_dir_add_service_by_path| takes in a void* |context| that is passed to
  // the |handler| callback passed as the last argument to the function call.
  // This library will pass in a casted void* pointer to this object, and when
  // the `svc` library invokes this library's connection handler, the |context|
  // will be casted back to |OnConnectContext*|.
  struct OnConnectContext {
    AnyHandler handler;
  };

  // Function pointer that matches type of |svc_dir_add_service| handler.
  // Internally, it calls the |AnyMemberHandler| instance populated via |AddAnyMember|.
  static void OnConnect(void* context, const char* service_name, zx_handle_t handle);

  // Callback invoked during teardown of a FIDL protocol entry. This callback
  // will close all active connections on the associated channel.
  using UnbindConnectionCallback = fit::callback<void()>;
  using UnbindCallbackMap = std::map<std::string, std::vector<UnbindConnectionCallback>>;

  static void AppendUnbindConnectionCallback(UnbindCallbackMap* unbind_protocol_callbacks,
                                             const std::string& name,
                                             UnbindConnectionCallback callback);

  void UnbindAllConnections(std::string_view name);

  static std::string MakePath(std::vector<std::string_view> strings);

  struct Inner {
    Inner(async_dispatcher_t* dispatcher, svc_dir_t* root);
    ~Inner();
    Inner(Inner&&) noexcept = delete;
    Inner& operator=(Inner&&) noexcept = delete;

    async_dispatcher_t* dispatcher_;

    async::synchronization_checker checker_;

    // This task will be scheduled on the async dispatcher to let |checker|
    // run additional verifications inside a dispatcher task.
    std::optional<async::Task> synchronization_check_dispatcher_task_;

    // |root_| is the outgoing directory implementation.
    // It is thread-unsafe, hence guarded by our synchronization checker using
    // clang thread-safety annotations. The annotations would help the compiler
    // statically verify that all accesses are checked for mutual exclusion.
    svc_dir_t* root_ __TA_GUARDED(checker_);

    // Mapping of all registered protocol handlers. Key represents a path to
    // the directory in which the protocol ought to be installed. For example,
    // a path may look like `svc/fuchsia.FooService/some_instance`.
    // The value contains a map of each of the entry's handlers.
    //
    // For FIDL Protocols, entries will be stored under "svc" entry
    // of this type, and then their name will be used as a key for the internal
    // map.
    //
    // For FIDL Services, entries will be stored by instance,
    // e.g. `svc/fuchsia.FooService/default`, and then the member names will be
    // used as the keys for the internal maps.
    //
    // The OnConnectContext has to be stored in the heap because its pointer
    // is used by |OnConnect|, a static function, during channel connection attempt.
    std::map<std::string, std::map<std::string, std::unique_ptr<OnConnectContext>>>
        registered_handlers_ = {};

    // Protocol bindings used to initiate teardown when protocol is removed. We
    // store this in a callback as opposed to a map of fidl::ServerBindingRef<T>
    // because that object is template parameterized and therefore can't be
    // stored in a homogeneous container.
    UnbindCallbackMap unbind_protocol_callbacks_;
  };

  Inner& inner() { return *inner_; }
  const Inner& inner() const { return *inner_; }

  // Wrapped in |unique_ptr| so that we can capture in a lambda without risk of
  // it becoming invalid.
  std::unique_ptr<Inner> inner_;
};

}  // namespace component

#endif  // LIB_COMPONENT_OUTGOING_CPP_OUTGOING_DIRECTORY_H_
