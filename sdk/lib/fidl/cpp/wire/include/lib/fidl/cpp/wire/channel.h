// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// channel.h is the "entrypoint header" that should be included when using the
// channel transport with LLCPP.

#ifndef LIB_FIDL_CPP_WIRE_CHANNEL_H_
#define LIB_FIDL_CPP_WIRE_CHANNEL_H_

#include <lib/fidl/cpp/wire/client.h>
// IWYU pragma: begin_exports
#include <lib/fidl/cpp/wire/internal/arrow.h>
#include <lib/fidl/cpp/wire/internal/transport.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
// IWYU pragma: end_exports
#include <lib/fidl/cpp/wire/server.h>
#include <lib/fidl/cpp/wire/service_handler.h>
#include <lib/fidl/cpp/wire/sync_call.h>
#include <lib/fidl/cpp/wire/wire_messaging.h>
#include <lib/zx/result.h>

namespace fidl {

template <typename Protocol>
struct Endpoints {
  fidl::ClientEnd<Protocol> client;
  fidl::ServerEnd<Protocol> server;

  // Creates a pair of Zircon channel endpoints speaking the |Protocol| protocol.
  // Whenever interacting with FIDL, using this method should be encouraged over
  // |zx::channel::create|, because this method encodes the precise protocol type
  // into its results at compile time.
  //
  // The return value is struct containing client and server endpoints.
  // Given the following:
  //
  //     auto endpoints = fidl::CreateEndpoints<MyProtocol>();
  //
  //  The channel endpoints may be accessed via:
  //
  //     endpoints->client;
  //     endpoints->server;
  //
  //  Or you may use structured bindings to gain access:
  //
  //     auto [client_end, server_end] = fidl::Endpoints<MyProtocol>::Create();
  static Endpoints<Protocol> Create() {
    zx::channel local, remote;
    ZX_ASSERT(zx::channel::create(0, &local, &remote) == ZX_OK);
    return Endpoints<Protocol>{
        fidl::ClientEnd<Protocol>(std::move(local)),
        fidl::ServerEnd<Protocol>(std::move(remote)),
    };
  }

  // Creates a pair of Zircon channel endpoints speaking the |Protocol| protocol.
  // Whenever interacting with FIDL, using this method should be encouraged over
  // |zx::channel::create|, because this method encodes the precise protocol type
  // into its results at compile time.
  //
  // This overload of |CreateEndpoints| may lead to more concise code when the
  // caller already has the client endpoint defined as an instance variable.
  // It will replace the destination of |out_client| with a newly created client
  // endpoint, and return the corresponding server endpoint:
  //
  //     // |client_end_| is an instance variable.
  //     auto server_end = fidl::Endpoints<Protocol>::Create(&client_end_);
  static fidl::ServerEnd<Protocol> Create(fidl::ClientEnd<Protocol>* out_client) {
    Endpoints<Protocol> endpoints = Create();
    *out_client = fidl::ClientEnd<Protocol>(std::move(endpoints.client));
    return fidl::ServerEnd<Protocol>(std::move(endpoints.server));
  }
  // Creates a pair of Zircon channel endpoints speaking the |Protocol| protocol.
  // Whenever interacting with FIDL, using this method should be encouraged over
  // |zx::channel::create|, because this method encodes the precise protocol type
  // into its results at compile time.
  //
  // This overload of |CreateEndpoints| may lead to more concise code when the
  // caller already has the server endpoint defined as an instance variable.
  // It will replace the destination of |out_server| with a newly created server
  // endpoint, and return the corresponding client endpoint:
  //
  //     // |server_end_| is an instance variable.
  //     auto client_end = fidl::Endpoints<Protocol>::Create(&server_end_);
  static fidl::ClientEnd<Protocol> Create(fidl::ServerEnd<Protocol>* out_server) {
    Endpoints<Protocol> endpoints = Create();
    *out_server = fidl::ServerEnd<Protocol>(std::move(endpoints.server));
    return fidl::ClientEnd<Protocol>(std::move(endpoints.client));
  }
};

// Creates a pair of Zircon channel endpoints speaking the |Protocol| protocol.
// Whenever interacting with LLCPP, using this method should be encouraged over
// |zx::channel::create|, because this method encodes the precise protocol type
// into its results at compile time.
//
// The return value is a result type wrapping the client and server endpoints.
// Given the following:
//
//     auto endpoints = fidl::CreateEndpoints<MyProtocol>();
//
// The caller should first ensure that |endpoints.is_ok() == true|, after which
// the channel endpoints may be accessed in one of two ways:
//
// - Direct:
//     endpoints->client;
//     endpoints->server;
//
// - Structured Binding:
//     auto [client_end, server_end] = std::move(endpoints.value());
template <typename Protocol>
zx::result<Endpoints<Protocol>> CreateEndpoints() {
  zx::channel local, remote;
  zx_status_t status = zx::channel::create(0, &local, &remote);
  if (status != ZX_OK) {
    return zx::error_result(status);
  }
  return zx::ok(Endpoints<Protocol>{
      fidl::ClientEnd<Protocol>(std::move(local)),
      fidl::ServerEnd<Protocol>(std::move(remote)),
  });
}

// Creates a pair of Zircon channel endpoints speaking the |Protocol| protocol.
// Whenever interacting with LLCPP, using this method should be encouraged over
// |zx::channel::create|, because this method encodes the precise protocol type
// into its results at compile time.
//
// This overload of |CreateEndpoints| may lead to more concise code when the
// caller already has the client endpoint defined as an instance variable.
// It will replace the destination of |out_client| with a newly created client
// endpoint, and return the corresponding server endpoint in a |zx::result|:
//
//     // |client_end_| is an instance variable.
//     auto server_end = fidl::CreateEndpoints(&client_end_);
//     if (server_end.is_ok()) { ... }
template <typename Protocol>
zx::result<fidl::ServerEnd<Protocol>> CreateEndpoints(fidl::ClientEnd<Protocol>* out_client) {
  auto endpoints = fidl::CreateEndpoints<Protocol>();
  if (!endpoints.is_ok()) {
    return endpoints.take_error();
  }
  *out_client = fidl::ClientEnd<Protocol>(std::move(endpoints->client));
  return zx::ok(fidl::ServerEnd<Protocol>(std::move(endpoints->server)));
}

// Creates a pair of Zircon channel endpoints speaking the |Protocol| protocol.
// Whenever interacting with LLCPP, using this method should be encouraged over
// |zx::channel::create|, because this method encodes the precise protocol type
// into its results at compile time.
//
// This overload of |CreateEndpoints| may lead to more concise code when the
// caller already has the server endpoint defined as an instance variable.
// It will replace the destination of |out_server| with a newly created server
// endpoint, and return the corresponding client endpoint in a |zx::result|:
//
//     // |server_end_| is an instance variable.
//     auto client_end = fidl::CreateEndpoints(&server_end_);
//     if (client_end.is_ok()) { ... }
template <typename Protocol>
zx::result<fidl::ClientEnd<Protocol>> CreateEndpoints(fidl::ServerEnd<Protocol>* out_server) {
  auto endpoints = fidl::CreateEndpoints<Protocol>();
  if (!endpoints.is_ok()) {
    return endpoints.take_error();
  }
  *out_server = fidl::ServerEnd<Protocol>(std::move(endpoints->server));
  return zx::ok(fidl::ClientEnd<Protocol>(std::move(endpoints->client)));
}

// This class manages a server connection over a Zircon channel and its binding
// to an |async_dispatcher_t*|, which may be multi-threaded. See the detailed
// documentation on the |BindServer| APIs.
template <typename Protocol>
class ServerBindingRef : public internal::ServerBindingRefBase {
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

  // Triggers an asynchronous unbind operation. Eventually, the epitaph will be sent over the
  // channel which will be subsequently closed. If specified, |on_unbound| will be invoked giving
  // the unbind reason as an argument.
  //
  // This may be called from any thread.
  void Close(zx_status_t epitaph) {
    if (auto binding = ServerBindingRefBase::binding().lock()) {
      binding->Close(std::move(binding), epitaph);
    }
  }

  // Retrieve the implementation used by this |ServerBindingRef| to process incoming messages, and
  // get exclusive const access to it before passing it to a lambda for further introspection.
  template <typename ServerImpl>
  void AsImpl(fit::function<void(const ServerImpl*)> impl_handler) const {
    if (auto binding = ServerBindingRefBase::binding().lock()) {
      impl_handler(
          internal::MessageDispatcherToServerImpl<Protocol, ServerImpl>(binding->interface()));
    }
  }

  using ServerBindingRefBase::ServerBindingRefBase;
  explicit ServerBindingRef(internal::ServerBindingRefBase&& base)
      : ServerBindingRefBase(std::move(base)) {}
};

// |BindServer| starts handling message on |server_end| using implementation
// |impl|, on a potentially multi-threaded |dispatcher|. Multiple requests may
// be concurrently in-flight, and responded to synchronously or asynchronously.
//
// |ServerImpl| should implement the abstract base class
// |fidl::WireServer<library::MyProtocol>|, typically generated by the low-level
// C++ backend, corresponding to methods in the protocol |library.MyProtocol|.
//
// This function adds an asynchronous wait to the given |dispatcher| for new
// messages to arrive on |server_end|. When each message arrives, the
// corresponding method handler in |ServerImpl| is called on one of the
// threads of the |dispatcher|.
//
// ## Starting message dispatch
//
// On success, |BindServer| associates |impl| and |server_end| with the
// |dispatcher|, and begins handling messages that arrive on |server_end|. This
// association is called a "binding". The dispatcher owns the |server_end| while
// the binding is active.
//
// The returned |ServerBindingRef| is a reference to the binding; it does not
// own the binding. In particular, the binding is kept alive by the dispatcher
// even if the returned |ServerBindingRef| is dropped. If the binding reference
// is ignored, the server operates in a "self-managed" mode, where it will
// continue listening for messages until an error occurs or if the user tears
// down the connection using a |Completer|.
//
// It is a logic error to invoke |BindServer| on a dispatcher that is
// shutting down or already shut down. Doing so will result in a panic.
//
// If any other error occurs when creating the binding, the |on_unbound| handler
// will be invoked asynchronously with the reason. See the "Unbind" section
// for details on |on_unbound|.
//
// ## Stopping message dispatch
//
// ### Unbind
//
// |ServerBindingRef::Unbind| requests to explicitly disassociate the server
// |impl| and endpoint from the dispatcher, and to retrieve the |server_end|
// endpoint. Note that this is an asynchronous procedure.
//
// |Unbind| is guaranteed to return in a short and bounded amount of time. It
// does not depend on whether there are any in-flight requests. As such, the
// user may safely take locks around an |Unbind| call.
//
// After unbinding completes:
//
// - The |server_end| is detached from the dispatcher; no dispatcher threads
//   will interact with it.
// - Calls on |Completer| objects from in-flight requests will have no effect.
//   Fallible operations will return |ZX_ERR_CANCELED|.
// - Subsequent calls made on the |ServerBindingRef| will be ignored. Fallible
//   operations will return |ZX_ERR_CANCELED|.
// - If |on_unbound| is not specified, the |server_end| is closed.
// - If |on_unbound| is specified, it will be called to signal the completion.
//   Ownership of the |server_end| is transferred to this hook.
//
// |on_unbound| must be a callable of the following signature:
//
//     // |impl| is the pointer to the server implementation.
//     // |info| contains the reason for stopping the message dispatch.
//     // |server_end| is the server channel endpoint.
//     // |ProtocolType| is the type of the FIDL protocol.
//     void OnUnbound(ServerImpl* impl,
//                    fidl::UnbindInfo info,
//                    fidl::ServerEnd<ProtocolType> server_end);
//
// More precisely, if there is a dispatcher thread waiting for incoming messages
// on the channel, it will stop monitoring the channel and detach itself from
// the binding. If there is a thread executing the method handler, the channel
// would be pulled from underneath it, such that it may no longer make any
// replies. When no thread has any active reference to the channel, the
// |on_unbound| callback will be invoked.
//
// |on_unbound| will be executed on a |dispatcher| thread, unless the user shuts
// down the |dispatcher| while there are active bindings associated with it. In
// that case, those bindings will be synchronously unbound, and the |on_unbound|
// callback would be executed on the thread invoking shutdown. |on_unbound|
// hooks must not acquire any locks that could be held during |dispatcher|
// shutdown.
//
// ### Close
//
// |ServerBindingRef::Close| has the same effects as |Unbind| except that it
// sends an epitaph message on the |server_end|.
//
// If specified, the |on_unbound| hook will execute after the epitaph has been
// sent.
//
// ## Server implementation ownership
//
// The server instance |impl| must remain alive while it is bound to the message
// dispatcher. Take special note of |Unbind|, as it returns before the unbind
// operation has completed. It is only safe to destroy the server instance
// within or after |on_unbound|.
//
// This overload borrows the server implementation by raw pointer. There are
// additional overloads of |BindServer| that either takes ownership of an
// implementation via |std::unique_ptr|, or shares the ownership via
// |std::shared_ptr|. Using either of those smart pointer overloads would
// automatically ensure memory safety.
//
// ## Error conditions
//
// The server implementation can call |Close| on the completer to indicate an
// application error during message handling.
//
// The connection will also be automatically closed by the dispatching logic in
// certain conditions:
//
// - If the client-end of the channel is closed (ZX_ERR_PEER_CLOSED).
// - If an error occurs when waiting on, reading from, or writing to the
//   channel.
// - If decoding an incoming message fails or encoding an outgoing message
//   fails.
// - If the message was not defined in the FIDL protocol.
//
// These error conditions lead to the unbinding of the connection. If
// |on_unbound| was specified, it would be called on a |dispatcher| thread with
// a failure reason, allowing the user to process the error.
//
// ## Message ordering
//
// By default, the message dispatch function for a binding will only ever be
// invoked by a single |dispatcher| thread at a time, even if the dispatcher has
// multiple threads. Messages will be dispatched in the order that they are
// received on the channel.
//
// A method handler may call |EnableNextDispatch| on their completer to allow
// another thread to begin dispatching the next message before the current
// method handler returns. Of course, this is only meaningful if the
// |dispatcher| has multiple threads.
//
// If a particular user does not care about ordering, they may invoke
// |EnableNextDispatch| immediately in the message handler. If you have such a
// use case, please file a bug, where we may consider instead providing a mode
// to automatically parallelize.
//
// ## Template Ergonomics
//
// This function is able to infer the type of |Protocol|, |ServerImpl|, and
// |OnUnbound| in most cases. The following code would compile without
// explicitly specializing |BindServer|:
//
//     // Suppose |this| is a server implementation class |Foo|, that
//     // implements the |Bar| FIDL protocol.
//     fidl:ServerEnd<Bar> server_end = ...;
//     fidl::BindServer(dispatcher, std::move(server_end), this,
//                      [](Foo*, fidl::UnbindInfo, fidl::ServerEnd<Bar>) { ... });
//
// TODO(https://fxbug.dev/42145140): Consider using a "DidUnbind" virtual function
// in the server interface to replace the |on_unbound| handler lambda.
template <typename Protocol, typename ServerImpl, typename OnUnbound = std::nullptr_t>
ServerBindingRef<Protocol> BindServer(async_dispatcher_t* dispatcher,
                                      fidl::ServerEnd<Protocol> server_end, ServerImpl* impl,
                                      OnUnbound&& on_unbound = nullptr) {
  static_assert(fidl::IsProtocolV<Protocol>, "|Protocol| must be a FIDL protocol marker");
  return internal::BindServerImpl<Protocol, ServerImpl>(
      dispatcher, std::move(server_end), impl,
      internal::UnboundThunk(std::move(impl), std::forward<OnUnbound>(on_unbound)));
}

// Overload of |BindServer| that takes ownership of the server as a |unique_ptr|.
// The pointer is destroyed on the same thread as the one calling |on_unbound|,
// and happens right after |on_unbound|. See |BindServer| for details.
template <typename Protocol, typename ServerImpl, typename OnUnbound = std::nullptr_t>
ServerBindingRef<Protocol> BindServer(async_dispatcher_t* dispatcher,
                                      fidl::ServerEnd<Protocol> server_end,
                                      std::unique_ptr<ServerImpl>&& impl,
                                      OnUnbound&& on_unbound = nullptr) {
  static_assert(fidl::IsProtocolV<Protocol>, "|Protocol| must be a FIDL protocol marker");
  ServerImpl* impl_raw = impl.get();
  return internal::BindServerImpl<Protocol, ServerImpl>(
      dispatcher, std::move(server_end), impl_raw,
      internal::UnboundThunk(std::move(impl), std::forward<OnUnbound>(on_unbound)));
}

// Overload of |BindServer| that shares ownership of the server via a |shared_ptr|.
// The pointer is destroyed on the same thread as the one calling |on_unbound|,
// and happens right after |on_unbound|. See |BindServer| for details.
template <typename Protocol, typename ServerImpl, typename OnUnbound = std::nullptr_t>
ServerBindingRef<Protocol> BindServer(async_dispatcher_t* dispatcher,
                                      fidl::ServerEnd<Protocol> server_end,
                                      std::shared_ptr<ServerImpl> impl,
                                      OnUnbound&& on_unbound = nullptr) {
  static_assert(fidl::IsProtocolV<Protocol>, "|Protocol| must be a FIDL protocol marker");
  ServerImpl* impl_raw = impl.get();
  return internal::BindServerImpl<Protocol, ServerImpl>(
      dispatcher, std::move(server_end), impl_raw,
      internal::UnboundThunk(std::move(impl), std::forward<OnUnbound>(on_unbound)));
}

// |kIgnoreBindingClosure| can be used in the place of |CloseHandler|s to explicitly acknowledge
// that a binding implementer wants to ignore and drop all notifications of binding closure that
// result in the teardown of a binding.
constexpr auto kIgnoreBindingClosure = ::fidl::internal::IgnoreBindingClosureType::kValue;

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
//  class Impl : public fidl::Server<fuchsia_my_library::MyProtocol> {
//   public:
//    Impl(fidl::ServerEnd<fuchsia_my_library::Protocol> server_end, async_dispatcher_t* dispatcher)
//        : binding_(dispatcher, std::move(server_end), this, std::mem_fn(&Impl::OnFidlClosed)) {}
//
//    void OnFidlClosed(fidl::UnbindInfo info) override {
//      // Handle errors..
//    }
//
//    // More method implementations omitted...
//
//   private:
//    fidl::ServerBinding<fuchsia_my_library::MyProtocol> binding_;
//  };
//
// ## See also
//
//  * |WireClient|, |Client|: which are the client analogues of this class.
//  * |WireSendEvent|, |SendEvent|: which can be used to send events over the
//    bound endpoint.
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
class ServerBinding final : public internal::ServerBindingBase<FidlProtocol> {
 private:
  using Base = internal::ServerBindingBase<FidlProtocol>;

 public:
  // |CloseHandler| is invoked when the endpoint managed by the |ServerBinding| is closed, due to a
  // terminal error or because the user initiated binding teardown.
  //
  // |CloseHandler| is silently discarded if |ServerBinding| is destroyed, to avoid calling into a
  // destroyed server implementation.
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
  //     class Impl : fidl::WireServer<Protocol> {
  //      public:
  //       void OnFidlClosed(fidl::UnbindInfo) { /* handle errors */ }
  //     };
  //
  //     fidl::ServerBinding<Protocol> binding(
  //         dispatcher, std::move(server_end), impl,
  //         std::mem_fn(&Impl::OnFidlClosed));
  //
  // The close handler will be invoked on a dispatcher thread, unless the user shuts down the async
  // dispatcher while there are active server bindings associated with it. In that case, the handler
  // will be synchronously invoked on the thread calling dispatcher shutdown.
  //
  // In cases where the binding implementation never cares to handle any errors or be notified about
  // binding closure, one can pass |fidl::kIgnoreBindingClosure| as the |CloseHandler|, as follows:
  //
  //     fidl::ServerBinding<Protocol> binding(
  //         dispatcher, std::move(server_end), impl,
  //         fidl::kIgnoreBindingClosure);
  template <typename Impl, typename CloseHandler>
  void CloseHandlerRequirement() {
    Base::template CloseHandlerRequirement<Impl, CloseHandler>();
  }

  // Constructs a binding that dispatches messages from |server_end| to |impl|,
  // using |dispatcher|.
  //
  // |Impl| should implement |fidl::Server<FidlProtocol>| or
  // |fidl::WireServer<FidlProtocol>|.
  //
  // |impl| and any state captured in |close_handler| should outlive the bindings.
  // It's not safe to move |impl| while the binding is still referencing it.
  //
  // |close_handler| is invoked when the endpoint managed by the |ServerBinding|
  // is closed, due to a terminal error or because the user initiated binding
  // teardown. See |CloseHandlerRequirement| for details on the error handler.
  template <typename Impl, typename CloseHandler>
  ServerBinding(async_dispatcher_t* dispatcher, fidl::ServerEnd<FidlProtocol> server_end,
                Impl* impl, CloseHandler&& close_handler)
      : Base(dispatcher, std::move(server_end), impl, std::forward<CloseHandler>(close_handler)) {}

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

  // Tears down the binding and closes the connection with an epitaph.
  //
  // |close_handler| will be called with the appropriate unbind reason.
  void Close(zx_status_t epitaph) {
    static_cast<fidl::ServerBindingRef<FidlProtocol>&>(Base::binding().ref()).Close(epitaph);
  }

  // Retrieve the implementation used by this |ServerBindingRef| to process incoming messages, and
  // get exclusive const access to it before passing it to a lambda for further introspection.
  template <typename ServerImpl>
  void AsImpl(fit::function<void(const ServerImpl*)> impl_handler) const {
    if (auto binding = fidl::internal::BorrowBinding(Base::binding().ref()).lock()) {
      impl_handler(
          internal::MessageDispatcherToServerImpl<FidlProtocol, ServerImpl>(binding->interface()));
    }
  }
};

// |fidl::WireSyncClient| owns a client endpoint and exposes synchronous FIDL
// calls. Prefer using this owning class over |fidl::WireCall| unless one has to
// interface with very low-level functionality (such as making a call over a raw
// zx_handle_t).
//
// Generated FIDL APIs are accessed by 'dereferencing' the client value:
//
//     // Creates a sync client that speaks over |client_end|.
//     fidl::WireSyncClient client(std::move(client_end));
//
//     // Call the |Foo| method synchronously, obtaining the results from the
//     // return value.
//     fidl::WireResult result = client->Foo(args);
//
// |fidl::WireSyncClient| is suitable for code without access to an async
// dispatcher.
//
// ## Thread safety
//
// |WireSyncClient| is generally thread-safe with a few caveats:
//
// - Client objects can be safely sent between threads.
// - One may invoke many FIDL methods in parallel on the same client. However,
//   FIDL method calls must be synchronized with operations that consume or
//   mutate the client object itself:
//
//     - Calling `Bind` or `TakeClientEnd`.
//     - Assigning a new value to the |WireSyncClient| variable.
//     - Moving the |WireSyncClient| to a different location.
//     - Destroying the |WireSyncClient|.
//
// - There can be at most one `HandleOneEvent` call going on at the same time.
template <typename FidlProtocol>
class WireSyncClient {
 public:
  // Creates an uninitialized client that is not bound to a client endpoint.
  //
  // Prefer using the constructor overload that initializes the client
  // atomically during construction. Use this default constructor only when the
  // client must be constructed first before an endpoint could be obtained (for
  // example, if the client is an instance variable).
  //
  // The client may be initialized later via |Bind|.
  WireSyncClient() = default;

  // Creates an initialized client. FIDL calls will be made on |client_end|.
  //
  // Similar to |fidl::WireClient|, the client endpoint must be valid.
  //
  // To just make a FIDL call uniformly on a client endpoint that may or may not
  // be valid, use the |fidl::WireCall(client_end)| helper. We may extend
  // |fidl::WireSyncClient<P>| with richer features hinging on having a valid
  // endpoint in the future.
  explicit WireSyncClient(::fidl::ClientEnd<FidlProtocol> client_end)
      : client_end_(std::move(client_end)) {
    ZX_ASSERT(is_valid());
  }

  ~WireSyncClient() = default;
  WireSyncClient(WireSyncClient&&) noexcept = default;
  WireSyncClient& operator=(WireSyncClient&&) noexcept = default;

  // Whether the client is initialized.
  bool is_valid() const { return client_end_.is_valid(); }
  explicit operator bool() const { return is_valid(); }

  // Borrows the underlying client endpoint. The client must have been
  // initialized.
  const ::fidl::ClientEnd<FidlProtocol>& client_end() const {
    ZX_ASSERT(is_valid());
    return client_end_;
  }

  // Initializes the client with a |client_end|. FIDL calls will be made on this
  // endpoint.
  //
  // It is not allowed to call |Bind| on an initialized client. To rebind a
  // |WireSyncClient| to a different endpoint, simply replace the
  // |WireSyncClient| variable with a new instance.
  void Bind(::fidl::ClientEnd<FidlProtocol> client_end) {
    ZX_ASSERT(!is_valid());
    client_end_ = std::move(client_end);
    ZX_ASSERT(is_valid());
  }

  // Extracts the underlying endpoint from the client. After this operation, the
  // client goes back to an uninitialized state.
  //
  // It is not safe to invoke this method while there are ongoing FIDL calls.
  ::fidl::ClientEnd<FidlProtocol> TakeClientEnd() {
    ZX_ASSERT(is_valid());
    return std::move(client_end_);
  }

  // Returns an interface for making FIDL calls with managed memory.
  internal::SyncEndpointManagedVeneer<internal::WireSyncClientImpl<FidlProtocol>> operator->()
      const {
    ZX_ASSERT(is_valid());
    return internal::SyncEndpointManagedVeneer<internal::WireSyncClientImpl<FidlProtocol>>(
        fidl::internal::MakeAnyUnownedTransport(client_end_.handle()));
  }

  // Returns an interface which exposes the caller-allocating API, using
  // the provided |resource| to allocate buffers necessary for each call.
  // See documentation on |SyncEndpointVeneer::buffer| for detailed behavior.
  template <typename MemoryResource>
  auto buffer(MemoryResource&& resource) const {
    ZX_ASSERT(is_valid());
    return internal::SyncEndpointBufferVeneer<internal::WireSyncBufferClientImpl<FidlProtocol>>{
        internal::MakeAnyUnownedTransport(client_end_.handle()),
        internal::MakeAnyBufferAllocator(std::forward<MemoryResource>(resource))};
  }

  // Handle all possible events defined in this protocol.
  //
  // Blocks to consume exactly one message from the channel, then call the corresponding virtual
  // method defined in |event_handler|. If the message was unknown or malformed, returns an
  // error without calling any virtual method.
  ::fidl::Status HandleOneEvent(fidl::WireSyncEventHandler<FidlProtocol>& event_handler) const {
    return event_handler.HandleOneEvent(client_end());
  }

 private:
  ::fidl::ClientEnd<FidlProtocol> client_end_;
};

template <typename FidlProtocol>
WireSyncClient(fidl::ClientEnd<FidlProtocol>) -> WireSyncClient<FidlProtocol>;

// |WireCall| is used to make method calls directly on a |fidl::ClientEnd|
// without having to set up a client. Call it like:
//
//     fidl::WireCall(client_end)->Method(args...);
//
template <typename FidlProtocol>
internal::SyncEndpointVeneer<internal::WireSyncClientImpl, FidlProtocol> WireCall(
    const fidl::ClientEnd<FidlProtocol>& client_end) {
  return internal::SyncEndpointVeneer<internal::WireSyncClientImpl, FidlProtocol>(
      fidl::internal::MakeAnyUnownedTransport(client_end.borrow().handle()));
}

// |WireCall| is used to make method calls directly on a |fidl::ClientEnd|
// without having to set up a client. Call it like:
//
//     fidl::WireCall(client_end)->Method(args...);
//
template <typename FidlProtocol>
internal::SyncEndpointVeneer<internal::WireSyncClientImpl, FidlProtocol> WireCall(
    const fidl::UnownedClientEnd<FidlProtocol>& client_end) {
  return internal::SyncEndpointVeneer<internal::WireSyncClientImpl, FidlProtocol>(
      fidl::internal::MakeAnyUnownedTransport(client_end.handle()));
}

// Return an interface for sending FIDL events containing wire domain objects
// over the endpoint managed by |binding_ref|. Call it like:
//
//     fidl::WireSendEvent(server_binding_ref)->FooEvent(args...);
//
template <typename FidlProtocol>
internal::WeakEventSenderVeneer<internal::WireWeakEventSender, FidlProtocol> WireSendEvent(
    const ServerBindingRef<FidlProtocol>& binding_ref) {
  return internal::WeakEventSenderVeneer<internal::WireWeakEventSender, FidlProtocol>(
      internal::BorrowBinding(
          static_cast<const fidl::internal::ServerBindingRefBase&>(binding_ref)));
}

// Return an interface for sending FIDL events containing wire domain objects
// over the endpoint managed by |binding|. Call it like:
//
//     fidl::ServerBinding<SomeProtocol> server_binding_{...};
//     fidl::WireSendEvent(server_binding_)->FooEvent(args...);
//
template <typename FidlProtocol>
internal::WeakEventSenderVeneer<internal::WireWeakEventSender, FidlProtocol> WireSendEvent(
    const ServerBinding<FidlProtocol>& binding) {
  return internal::WeakEventSenderVeneer<internal::WireWeakEventSender, FidlProtocol>(
      internal::BorrowBinding(
          static_cast<const fidl::internal::ServerBindingBase<FidlProtocol>&>(binding)));
}

// Return an interface for sending FIDL events containing wire domain objects
// over |server_end|. Call it like:
//
//     fidl::WireSendEvent(server_end)->FooEvent(args...);
//
template <typename FidlProtocol>
internal::SyncEndpointVeneer<internal::WireEventSender, FidlProtocol> WireSendEvent(
    const ServerEnd<FidlProtocol>& server_end) {
  return internal::SyncEndpointVeneer<internal::WireEventSender, FidlProtocol>(
      fidl::internal::MakeAnyUnownedTransport(server_end.channel()));
}

// Return an interface for sending FIDL events containing wire domain objects
// over |server_end|. Call it like:
//
//     fidl::WireSendEvent(server_end)->FooEvent(args...);
//
template <typename FidlProtocol>
internal::SyncEndpointVeneer<internal::WireEventSender, FidlProtocol> WireSendEvent(
    UnownedServerEnd<FidlProtocol> server_end) {
  return internal::SyncEndpointVeneer<internal::WireEventSender, FidlProtocol>(
      fidl::internal::MakeAnyUnownedTransport(server_end.handle()));
}

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
//     class Impl : public fidl::Server<fuchsia_lib::MyProtocol> {
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
//       fidl::ServerBindingGroup<fuchsia_lib::MyProtocol> bindings_;
//     };
//
//     // Instantiate the server.
//     Impl impl;
//
//     // Publish the server to an outgoing directory.
//     component::OutgoingDirectory outgoing{dispatcher};
//     outgoing.AddUnmanagedProtocol<fuchsia_lib::MyProtocol>(impl.GetHandler());
//
// One may also explicitly let the binding group manage new connections using |AddBinding|:
//
//     // Instantiate some servers.
//     auto a = ImplA{...};
//     auto b = ImplB{...};
//
//     // Create the group.
//     fidl::ServerBindingGroup<fuchsia_lib::MyProtocol> bindings;
//
//     // Add server endpoints and implementations to the group.
//     fidl::Endpoints<fuchsia_lib::MyProtocol>() endpoints1 = ...;
//     group.AddBinding(dispatcher, endpoints1->server, &a, OnClosed);
//
//     fidl::Endpoints<fuchsia_lib::MyProtocol>() endpoints2 = ...;
//     group.AddBinding(dispatcher, endpoints2->server, &a, OnClosed);
//
// # Thread safety
//
// This class is thread-unsafe. Instances must be managed and used from a
// synchronized async dispatcher. See
// https://fuchsia.dev/fuchsia-src/development/languages/c-cpp/thread-safe-async#synchronized-dispatcher
template <typename FidlProtocol>
class ServerBindingGroup final
    : private internal::ServerBindingGroupBase<FidlProtocol, internal::ChannelTransport> {
 private:
  using Base = internal::ServerBindingGroupBase<FidlProtocol, internal::ChannelTransport>;
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
  //     class Impl : fidl::WireServer<Protocol> {
  //      public:
  //       void OnFidlClosed(fidl::UnbindInfo) { /* handle errors */ }
  //     };
  //
  //     auto impl = Impl(...);
  //     fidl::ServerBindingGroup<Protocol> binding_group;
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
  void AddBinding(async_dispatcher_t* dispatcher, fidl::ServerEnd<FidlProtocol> server_end,
                  ServerImpl* impl, CloseHandler&& close_handler) {
    Base::AddBinding(dispatcher, std::move(server_end), impl,
                     std::forward<CloseHandler>(close_handler));
  }

  // Returns a protocol handler that binds the incoming |ServerEnd| to the passed in |impl|.
  // All bindings will use the same |CloseHandler|.
  template <typename ServerImpl, typename CloseHandler>
  fidl::ProtocolHandler<FidlProtocol> CreateHandler(ServerImpl* impl,
                                                    async_dispatcher_t* dispatcher,
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

}  // namespace fidl

#endif  // LIB_FIDL_CPP_WIRE_CHANNEL_H_
