// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_VFS_CPP_SERVICE_H_
#define LIB_VFS_CPP_SERVICE_H_

#include <lib/async/default.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/fit/function.h>
#include <lib/fit/traits.h>
#include <lib/vfs/cpp/node.h>

#include <type_traits>

namespace vfs {

// A node which binds a channel to a service implementation when opened.
//
// This class is thread-safe.
class Service final : public Node {
 public:
  // Handler callback which binds a channel to a service instance (HLCPP).
  using Connector = fit::function<void(zx::channel channel, async_dispatcher_t* dispatcher)>;

  // Handler callback which binds a channel to a service instance.
  using Callback = fit::function<void(zx::channel)>;

  template <typename Protocol>
  using TypedCallback = fit::function<void(fidl::ServerEnd<Protocol>)>;

  // Untyped constructor that invokes `connector` upon requests to connect to this service node.
  //
  // If the `connector` is null, then incoming connection requests will be dropped.
  explicit Service(Callback connector) : Node(MakeService(std::move(connector))) {}

 private:
  struct Traits {
    // Matches cases where `T` could be a `TypedCallback`.
    template <typename T>
    static constexpr bool kIsTypedCallback =
        std::conjunction_v<std::negation<std::is_same<std::decay_t<T>, Service>>,
                           std::negation<std::is_convertible<T, Callback>>,
                           std::negation<std::is_convertible<T, Connector>>>;

    // Extracts `Protocol` from a `Callable` with signature `void(fidl::ServerEnd<Protocol>)`.
    template <typename Callable>
    using ProtocolFrom =
        typename fit::callable_traits<std::decay_t<Callable>>::args::template at<0>::ProtocolType;
  };

 public:
  // Creates a service with the typed `connector` callback. The callback is invoked upon requests
  // to connect to this service node. For example:
  //
  //    auto service = std::make_unique<vfs::Service>(
  //      [](fidl::ServerEnd<Protocol> server_end) {
  //        // Serve requests here, e.g. using fidl::BindServer.
  //      });
  //
  // Or using a managed `fidl::Server` or `fidl::WireServer` instance:
  //
  //    auto instance = (...);  // Must outlive nodes that reference the service.
  //    auto service = std::make_unique<vfs::Service>(instance.bind_handler(...));
  //
  // If `connector` is null, then incoming connection requests will be dropped.
  template <typename Callable, std::enable_if_t<Traits::kIsTypedCallback<Callable>, bool> = true>
  explicit Service(Callable&& connector)
      : Service([connector = std::forward<Callable>(connector)](zx::channel channel) mutable {
          connector(fidl::ServerEnd<Traits::ProtocolFrom<Callable>>{std::move(channel)});
        }) {}

 private:
  static vfs_internal_node_t* MakeService(Callback connector) {
    vfs_internal_node_t* svc;
    vfs_internal_svc_context_t context{
        .cookie = new Callback(std::move(connector)),
        .connect = &Connect,
        .destroy = &DestroyCookie,
    };
    ZX_ASSERT(vfs_internal_service_create(&context, &svc) == ZX_OK);
    return svc;
  }

  static zx_status_t Connect(const void* cookie, zx_handle_t request) {
    (*static_cast<const Callback*>(cookie))(zx::channel{request});
    return ZX_OK;
  }

  static void DestroyCookie(void* cookie) { delete static_cast<Callback*>(cookie); }

  // * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * *
  // Deprecated HLCPP Signatures
  // * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * *
  //
  // TODO(https://fxbug.dev/336617685): Mark the following signatures as deprecated once all callers
  // have migrated to the above LLCPP signatures. This might be difficult since these signatures
  // are still relied upon by //sdk/lib/sys/cpp.

 public:
  explicit Service(Connector connector) : Node(MakeServiceDeprecated(std::move(connector))) {}

  // TOOD(https://fxbug.dev/336617685): Deprecate and provide LLCPP replacement.
  template <typename Interface>
  explicit Service(fidl::InterfaceRequestHandler<Interface> handler)
      : Service(
            [handler = std::move(handler)](zx::channel channel, async_dispatcher_t* dispatcher) {
              handler(fidl::InterfaceRequest<Interface>(std::move(channel)));
            }) {}

 private:
  static vfs_internal_node_t* MakeServiceDeprecated(Connector connector) {
    vfs_internal_node_t* svc;
    vfs_internal_svc_context_t context{
        .cookie = new Connector(std::move(connector)),
        .connect = &ConnectDeprecated,
        .destroy = &DestroyCookie,
    };
    ZX_ASSERT(vfs_internal_service_create(&context, &svc) == ZX_OK);
    return svc;
  }

  static void DestroyCookieDeprecated(void* cookie) { delete static_cast<Connector*>(cookie); }

  static zx_status_t ConnectDeprecated(const void* cookie, zx_handle_t request) {
    (*static_cast<const Connector*>(cookie))(zx::channel{request}, async_get_default_dispatcher());
    return ZX_OK;
  }
};
}  // namespace vfs

#endif  // LIB_VFS_CPP_SERVICE_H_
