// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_DRIVER_CPP_SERVER_H_
#define LIB_FIDL_DRIVER_CPP_SERVER_H_

#include <lib/fdf/dispatcher.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/fidl/cpp/wire/service_handler.h>
#include <lib/fidl_driver/cpp/internal/server_details.h>
#include <lib/fidl_driver/cpp/transport.h>

namespace fdf {

// This class manages a server connection over an fdf channel and its binding to
// an |fdf_dispatcher_t*|, which may be multi-threaded. See the detailed
// documentation on the |BindServer| APIs.
template <typename Protocol>
class ServerBindingRef : public fidl::internal::ServerBindingRefBase {
 public:
  // Triggers an asynchronous unbind operation. If specified, |on_unbound| will
  // be asynchronously run on a dispatcher thread, passing in the endpoint and
  // the unbind reason.
  //
  // On return, the dispatcher will stop monitoring messages on the endpoint,
  // though handling of any already in-flight transactions will continue.
  // Pending completers may be discarded.
  //
  // This may be called from any thread.
  //
  // WARNING: While it is safe to invoke Unbind() from any thread, it is unsafe to wait on the
  // OnUnboundFn from a dispatcher thread, as that will likely deadlock.
  using ServerBindingRefBase::Unbind;

  using ServerBindingRefBase::ServerBindingRefBase;
  explicit ServerBindingRef(fidl::internal::ServerBindingRefBase&& base)
      : ServerBindingRefBase(std::move(base)) {}

  // If the |ServerBindingRef| is bound, tears down the binding and starts closing the server
  // connection with |epitaph|. If unbound does nothing.
  //
  // This may be called from any thread.
  void Close(zx_status_t epitaph) {
    if (auto binding = fidl::internal::ServerBindingRefBase::binding().lock()) {
      binding->Close(std::move(binding), epitaph);
    }
  }
};

// |BindServer| starts handling message on |server_end| using implementation
// |impl|, on a potentially multi-threaded |dispatcher|. Multiple requests may
// be concurrently in-flight, and responded to synchronously or asynchronously.
//
// The behavior of |fdf::BindServer| is identical to |fidl::BindServer|, the
// specialization for channels. Please see documentation in channel.h for more
// details.
//
// Usually all template parameters can be automatically inferred.
template <typename Protocol, typename ServerImpl, typename OnUnbound = std::nullptr_t>
ServerBindingRef<Protocol> BindServer(fdf_dispatcher_t* dispatcher, ServerEnd<Protocol> server_end,
                                      ServerImpl* impl, OnUnbound&& on_unbound = nullptr) {
  static_assert(fidl::IsProtocolV<Protocol>, "|Protocol| must be a FIDL protocol marker");
  return fidl::internal::BindServerImpl<Protocol, ServerImpl>(
      fdf_dispatcher_get_async_dispatcher(dispatcher), std::move(server_end), impl,
      fidl::internal::UnboundThunk(std::move(impl), std::forward<OnUnbound>(on_unbound)));
}

// Overload of |BindServer| that takes ownership of the server as a |unique_ptr|.
// The pointer is destroyed on the same thread as the one calling |on_unbound|,
// and happens right after |on_unbound|.
//
// The behavior of |fdf::BindServer| is identical to |fidl::BindServer|, the
// specialization for channels. Please see documentation in channel.h for more
// details.
template <typename Protocol, typename ServerImpl, typename OnUnbound = std::nullptr_t>
ServerBindingRef<Protocol> BindServer(fdf_dispatcher_t* dispatcher, ServerEnd<Protocol> server_end,
                                      std::unique_ptr<ServerImpl>&& impl,
                                      OnUnbound&& on_unbound = nullptr) {
  static_assert(fidl::IsProtocolV<Protocol>, "|Protocol| must be a FIDL protocol marker");
  ServerImpl* impl_raw = impl.get();
  return fidl::internal::BindServerImpl<Protocol, ServerImpl>(
      fdf_dispatcher_get_async_dispatcher(dispatcher), std::move(server_end), impl_raw,
      fidl::internal::UnboundThunk(std::move(impl), std::forward<OnUnbound>(on_unbound)));
}

// Overload of |BindServer| that shares ownership of the server via a |shared_ptr|.
// The pointer is destroyed on the same thread as the one calling |on_unbound|,
// and happens right after |on_unbound|.
//
// The behavior of |fdf::BindServer| is identical to |fidl::BindServer|, the
// specialization for channels. Please see documentation in channel.h for more
// details.
template <typename Protocol, typename ServerImpl, typename OnUnbound = std::nullptr_t>
ServerBindingRef<Protocol> BindServer(fdf_dispatcher_t* dispatcher, ServerEnd<Protocol> server_end,
                                      std::shared_ptr<ServerImpl> impl,
                                      OnUnbound&& on_unbound = nullptr) {
  static_assert(fidl::IsProtocolV<Protocol>, "|Protocol| must be a FIDL protocol marker");
  ServerImpl* impl_raw = impl.get();
  return fidl::internal::BindServerImpl<Protocol, ServerImpl>(
      fdf_dispatcher_get_async_dispatcher(dispatcher), std::move(server_end), impl_raw,
      fidl::internal::UnboundThunk(std::move(impl), std::forward<OnUnbound>(on_unbound)));
}

// |ServerBinding| binds the implementation of a FIDL protocol to a server
// endpoint.
//
// |ServerBinding| listens for incoming messages on the channel, decodes them,
// and calls the appropriate method on the bound implementation.
//
// When the |ServerBinding| object is destroyed, the binding between the
// protocol endpoint and the server implementation is torn down and the channel
// is closed. Once destroyed, it will not make any method calls on the server
// implementation. Thus the idiomatic usage of a |ServerBinding| is to embed it
// as a member variable of a server implementation, such that they are destroyed
// together.
//
// ## Example
//
//  class Impl : public fdf::Server<fuchsia_my_library::MyProtocol> {
//   public:
//    Impl(fdf::ServerEnd<fuchsia_my_library::Protocol> server_end, fdf_dispatcher_t* dispatcher)
//        : binding_(dispatcher, std::move(server_end), this, std::mem_fn(&Impl::OnFidlClosed)) {}
//
//    void OnFidlClosed(fidl::UnbindInfo info) override {
//      // Handle errors..
//    }
//
//    // More method implementations omitted...
//
//   private:
//    fdf::ServerBinding<fuchsia_my_library::MyProtocol> binding_;
//  };
//
// ## See also
//
//  * |WireClient|, |Client|: which are the client analogues of this class.
//
// ## Thread safety
//
// |ServerBinding| is thread unsafe. Tearing down a |ServerBinding| guarantees
// no more method calls on the borrowed |Impl|. This is only possible when
// the teardown is synchronized with message dispatch. The binding will enforce
// [synchronization guarantees][synchronization-guarantees] at runtime with
// threading checks.
//
// [synchronization-guarantees]:
// https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/sdk/lib/async/README.md#verifying-synchronization-requirements
template <typename FidlProtocol>
class ServerBinding final : public ::fidl::internal::ServerBindingBase<FidlProtocol> {
 private:
  using Base = ::fidl::internal::ServerBindingBase<FidlProtocol>;

 public:
  // |CloseHandler| is invoked when the endpoint managed by the |ServerBinding|
  // is closed, due to a terminal error or because the user initiated binding
  // teardown.
  //
  // |CloseHandler| is silently discarded if |ServerBinding| is destroyed, to
  // avoid calling into a destroyed server implementation.
  //
  // The handler may have one of these signatures:
  //
  //     void(fidl::UnbindInfo info);
  //     void(Impl* impl, fidl::UnbindInfo info);
  //
  // |info| contains the detailed reason for stopping message dispatch.
  // |impl| is the pointer to the server implementation borrowed by the binding.
  //
  // The second overload allows one to bind the close handler to an instance
  // method on the server implementation, without capturing extra state:
  //
  //     class Impl : fdf::WireServer<Protocol> {
  //      public:
  //       void OnFidlClosed(fidl::UnbindInfo) { /* handle errors */ }
  //     };
  //
  //     fidl::ServerBinding<Protocol> binding(
  //         dispatcher, std::move(server_end), impl,
  //         std::mem_fn(&Impl::OnFidlClosed));
  //
  // In cases where the binding implementation never cares to handle any errors or be notified about
  // binding closure, one can pass |fidl::kIgnoreBindingClosure| as the |CloseHandler|, as follows:
  //
  //     fidl::ServerBinding<Protocol> binding(
  //         dispatcher, std::move(server_end), impl,
  //         fidl::kIgnoreBindingClosure);
  template <typename Impl, typename CloseHandler>
  static void CloseHandlerRequirement() {
    Base::template CloseHandlerRequirement<Impl, CloseHandler>();
  }

  // Tears down the binding and closes the connection with |epitaph|.
  void Close(zx_status_t epitaph) {
    static_cast<ServerBindingRef<FidlProtocol>&>(Base::binding().ref()).Close(epitaph);
  }

  // Constructs a binding that dispatches messages from |server_end| to |impl|,
  // using |dispatcher|.
  //
  // |Impl| should implement |fdf::Server<FidlProtocol>| or
  // |fdf::WireServer<FidlProtocol>|.
  //
  // |impl| and any state captured in |close_handler| should outlive the bindings.
  // It's not safe to move |impl| while the binding is still referencing it.
  //
  // |close_handler| is invoked when the endpoint managed by the |ServerBinding|
  // is closed, due to a terminal error or because the user initiated binding
  // teardown. See |CloseHandlerRequirement| for details on the error handler.
  template <typename Impl, typename CloseHandler>
  ServerBinding(fdf_dispatcher_t* dispatcher, ServerEnd<FidlProtocol> server_end, Impl* impl,
                CloseHandler&& close_handler)
      : Base(fdf_dispatcher_get_async_dispatcher(dispatcher), std::move(server_end), impl,
             std::forward<CloseHandler>(close_handler)) {}

  // The usual usage style of |ServerBinding| puts it as a member variable of a
  // server object, to which it unsafely borrows. Thus it's unsafe to move the
  // server objects. As a precaution, we do not allow moving the bindings. If
  // one needs to move a server object, consider wrapping it in a
  // |std::unique_ptr|.
  ServerBinding(ServerBinding&& other) noexcept = delete;
  ServerBinding& operator=(ServerBinding&& other) noexcept = delete;

  ServerBinding(const ServerBinding& other) noexcept = delete;
  ServerBinding& operator=(const ServerBinding& other) noexcept = delete;

  // Tears down the binding and closes the connection.
  //
  // After the binding destructs, it will release references on |impl|.
  // Destroying the binding will discard the |close_handler| without calling it.
  ~ServerBinding() = default;
};

// |ServerBindingGroup| manages a collection of FIDL |ServerBinding|s. It does not own the |impl|s
// backing those bindings. All members of a |ServerBindingGroup| collection must implement a common
// FIDL protocol, but implementations themselves may be distinct from one another.
//
// Destroying a |ServerBindingGroup| will close all managed connections and release the references
// to the implementations. If one does not require per-connection state, a common pattern is to
// have a common server implementation own its |ServerBindingGroup|.
//
// ## Example
//
//     // Define the protocol implementation.
//     class Impl : public fdf::Server<fuchsia_lib::MyProtocol> {
//      public:
//       fidl::ProtocolHandler<fuchsia_lib::MyProtocol> GetHandler() {
//         return bindings_.CreateHandler(this, dispatcher, std::mem_fn(&Impl::OnClosed));
//       }
//
//      private:
//       void OnClosed(fidl::UnbindInfo info) {
//         // Called when a connection to this server is closed.
//         // This is provided to the binding group during |CreateHandler|.
//       }
//
//       fdf::ServerBindingGroup<fuchsia_lib::MyProtocol> bindings_;
//     };
//
//     // Instantiate the server.
//     Impl impl;
//
//     // Publish the server to an outgoing directory.
//     auto outgoing = fdf::OutgoingDirectory::Create(dispatcher);
//     outgoing.AddService(fuchsia_lib::MyService::InstanceHandler {
//         .member = impl.GetHandler(),
//     });
//
// One may also explicitly let the binding group manage new connections using |AddBinding|:
//
//     // Instantiate some servers.
//     auto a = ImplA{...};
//     auto b = ImplB{...};
//
//     // Create the group.
//     fdf::ServerBindingGroup<fuchsia_lib::MyProtocol> bindings;
//
//     // Add server endpoints and implementations to the group.
//     fdf::Endpoints<fuchsia_lib::MyProtocol>() endpoints1 = ...;
//     group.AddBinding(dispatcher, endpoints1->server, &a, OnClosed);
//
//     fdf::Endpoints<fuchsia_lib::MyProtocol>() endpoints2 = ...;
//     group.AddBinding(dispatcher, endpoints2->server, &a, OnClosed);
//
// # Thread safety
//
// This class is thread-unsafe. Instances must be managed and used from a
// synchronized async dispatcher. See
// https://fuchsia.dev/fuchsia-src/development/languages/c-cpp/thread-safe-async#synchronized-dispatcher
template <typename FidlProtocol>
class ServerBindingGroup final
    : private ::fidl::internal::ServerBindingGroupBase<FidlProtocol,
                                                       ::fidl::internal::DriverTransport> {
 private:
  using Base =
      ::fidl::internal::ServerBindingGroupBase<FidlProtocol, ::fidl::internal::DriverTransport>;
  using BindingUid = typename Base::BindingUid;
  using Binding = typename Base::Binding;
  using StorageType = typename Base::StorageType;

 public:
  ServerBindingGroup() = default;
  ServerBindingGroup(const ServerBindingGroup&) = delete;
  ServerBindingGroup(ServerBindingGroup&&) = delete;
  ServerBindingGroup& operator=(const ServerBindingGroup&) = delete;
  ServerBindingGroup& operator=(ServerBindingGroup&&) = delete;

  // Add a binding to an unowned impl to the group.
  //
  // |CloseHandler| is silently discarded if |ServerBindingGroup| is destroyed, to avoid calling
  // into a destroyed server implementation.
  //
  // The handler may have one of these signatures:
  //
  //     void(fidl::UnbindInfo info);
  //     void(Impl* impl, fidl::UnbindInfo info);
  //
  // |info| contains the detailed reason for stopping message dispatch. |impl| is the pointer to the
  // server implementation borrowed by the binding.
  //
  // This method allows one to bind a |CloseHandler| to the newly created server binding instance.
  // See |ServerBinding| for more information on the behavior of the |CloseHandler|, when and how it
  // is called, etc. This is particularly useful when passing in a |std::unique_ptr<ServerImpl>|,
  // because one does not have to capture the server implementation pointer again:
  //
  //     // Define and instantiate the impl.
  //     class Impl : fdf::WireServer<Protocol> {
  //      public:
  //       void OnFidlClosed(fidl::UnbindInfo) { /* handle errors */ }
  //     };
  //
  //     auto impl = Impl(...);
  //     fdf::ServerBindingGroup<Protocol> binding_group;
  //
  //     // Bind the server endpoint to the |Impl| instance, and hook up the
  //     // |CloseHandler| to its |OnFidlClosed| member function.
  //     binding_group.AddBinding(
  //         dispatcher, std::move(server_end), &impl,
  //         std::mem_fn(&Impl::OnFidlClosed));
  //
  // In cases where the binding implementation never cares to handle any errors or be notified about
  // binding closure, one can pass |fidl::kIgnoreBindingClosure| as the |close_handler|.
  template <typename ServerImpl, typename CloseHandler>
  void AddBinding(fdf_dispatcher_t* dispatcher, ServerEnd<FidlProtocol> server_end,
                  ServerImpl* impl, CloseHandler&& close_handler) {
    Base::AddBinding(dispatcher, std::move(server_end), impl,
                     std::forward<CloseHandler>(close_handler));
  }

  // Returns a protocol handler that binds the incoming |ServerEnd| to the passed in |impl|.
  // All bindings will use the same |CloseHandler|.
  template <typename ServerImpl, typename CloseHandler>
  fidl::ProtocolHandler<FidlProtocol> CreateHandler(ServerImpl* impl, fdf_dispatcher_t* dispatcher,
                                                    CloseHandler&& close_handler) {
    return Base::CreateHandler(impl, dispatcher, std::forward<CloseHandler>(close_handler));
  }

  // Iterate over the bindings stored in this group.
  void ForEachBinding(fit::function<void(const Binding&)> visitor) {
    Base::ForEachBinding(std::move(visitor));
  }

  // Removes all bindings associated with a particular |impl| without calling their close handlers.
  // None of the removed bindings will have its close handler called. Returns true if at least one
  // binding was removed.
  template <class ServerImpl>
  bool RemoveBindings(const ServerImpl* impl) {
    return Base::RemoveBindings(impl);
  }

  // Removes all bindings. None of the removed bindings close handlers' is called. Returns true if
  // at least one binding was removed.
  bool RemoveAll() { return Base::RemoveAll(); }

  // Closes all bindings associated with the specified |impl|. The supplied epitaph is passed to
  // each closed binding's close handler, which is called in turn. Returns true if at least one
  // binding was closed. The teardown operation is asynchronous, and will not necessarily have been
  // completed by the time this function returns.
  template <class ServerImpl>
  bool CloseBindings(const ServerImpl* impl, zx_status_t epitaph_value) {
    return Base::CloseBindings(impl, epitaph_value);
  }

  // Closes all bindings. All of the closed bindings' close handlers are called. Returns true if at
  // least one binding was closed.
  bool CloseAll(zx_status_t epitaph_value) { return Base::CloseAll(epitaph_value); }

  // The number of active bindings in this |ServerBindingGroup|.
  size_t size() const { return Base::size(); }

  // Called when a previously full |ServerBindingGroup| has been emptied. A |ServerBindingGroup| is
  // "empty" once it contains no active bindings, and all closed bindings that it held since the
  // last time it was empty have finished their tear down routines.
  //
  // This function is not called by |~ServerBindingGroup|.
  void set_empty_set_handler(fit::closure empty_set_handler) {
    Base::set_empty_set_handler(std::move(empty_set_handler));
  }
};

}  // namespace fdf

#endif  // LIB_FIDL_DRIVER_CPP_SERVER_H_
