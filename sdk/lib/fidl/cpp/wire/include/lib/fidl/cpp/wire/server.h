// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_WIRE_SERVER_H_
#define LIB_FIDL_CPP_WIRE_SERVER_H_

#include <lib/fidl/cpp/wire/internal/arrow.h>
#include <lib/fidl/cpp/wire/internal/endpoints.h>
#include <lib/fidl/cpp/wire/internal/server_details.h>
#include <lib/fidl/cpp/wire/service_handler.h>
#include <lib/fidl/cpp/wire/wire_messaging_declarations.h>
#include <lib/fit/function.h>
#include <zircon/types.h>

#include <memory>
#include <optional>
#include <type_traits>
#include <unordered_map>
#include <utility>

namespace fidl {

namespace internal {

enum class IgnoreBindingClosureType { kValue };

class AsyncServerBinding;
class ServerBindingRefBase;

template <typename FidlProtocol>
class ServerBindingBase;

// |UniqueServerBindingOwner| tears down the managed binding when it destructs.
//
// There must be at most one unique owner of a binding.
class UniqueServerBindingOwner {
 public:
  explicit UniqueServerBindingOwner(ServerBindingRefBase&& ref) : ref_(std::move(ref)) {}
  ~UniqueServerBindingOwner() { ref_.Unbind(); }

  UniqueServerBindingOwner(UniqueServerBindingOwner&&) = default;
  UniqueServerBindingOwner& operator=(UniqueServerBindingOwner&&) = default;

  ServerBindingRefBase& ref() { return ref_; }
  const ServerBindingRefBase& ref() const { return ref_; }

 private:
  ServerBindingRefBase ref_;
};

using SimpleErrorHandler = fit::callback<void(UnbindInfo)>;
using SimpleCloseHandler = fit::callback<void(UnbindInfo)>;
template <typename Impl>
using InstanceCloseHandler = fit::callback<void(Impl*, UnbindInfo)>;

template <typename FidlProtocol>
class ServerBindingBase {
 public:
  template <typename Impl, typename CloseHandler>
  static constexpr void CloseHandlerRequirement() {
    // TODO(https://fxbug.dev/42063969): Cannot use |std::is_invocable_v| as that fails on the latest clang.
    static_assert(std::is_convertible_v<CloseHandler, IgnoreBindingClosureType> ||
                      std::is_convertible_v<CloseHandler, SimpleCloseHandler> ||
                      std::is_convertible_v<CloseHandler, InstanceCloseHandler<Impl>>,
                  "The close handler must have a signature of "
                  "void(fidl::UnbindInfo) or void(Impl*, fidl::UnbindInfo)");
  }

  template <typename Impl, typename CloseHandler>
  ServerBindingBase(async_dispatcher_t* dispatcher,
                    fidl::internal::ServerEndType<FidlProtocol> server_end, Impl* impl,
                    CloseHandler&& close_handler) {
    CloseHandlerRequirement<Impl, CloseHandler>();
    lifetime_ = std::make_shared<Lifetime>();
    binding_.emplace(internal::UniqueServerBindingOwner(BindServerImpl<FidlProtocol>(
        dispatcher, std::move(server_end), impl,
        [error_handler = std::forward<CloseHandler>(close_handler),
         weak_lifetime = std::weak_ptr(lifetime_)](
            Impl* impl, fidl::UnbindInfo info,
            fidl::internal::ServerEndType<FidlProtocol> endpoint) mutable {
          if (weak_lifetime.expired()) {
            // Binding is already destructed. Don't call the error handler to avoid
            // calling into a destructed server.
            return;
          }

          // Close the endpoint before notifying the user of connection closure.
          endpoint.reset();

          if constexpr (std::is_convertible_v<CloseHandler,
                                              ::fidl::internal::IgnoreBindingClosureType>) {
            // The implementer has explicitly chosen to drop errors, so do nothing.
            //
            // Suppress warnings of unused |CloseHandler|.
            (void)error_handler;
          } else if constexpr (std::is_convertible_v<CloseHandler, SimpleErrorHandler>) {
            error_handler(info);
          } else {
            error_handler(impl, info);
          }
        },
        ThreadingPolicy::kCreateAndTeardownFromDispatcherThread)));
  }

  ServerBindingBase(ServerBindingBase&& other) noexcept = delete;
  ServerBindingBase& operator=(ServerBindingBase&& other) noexcept = delete;

  ServerBindingBase(const ServerBindingBase& other) noexcept = delete;
  ServerBindingBase& operator=(const ServerBindingBase& other) noexcept = delete;

  ~ServerBindingBase() = default;

 protected:
  internal::UniqueServerBindingOwner& binding() { return *binding_; }
  const internal::UniqueServerBindingOwner& binding() const { return *binding_; }

 private:
  template <typename P>
  friend WeakServerBindingRef BorrowBinding(const ServerBindingBase<P>&);
  struct Lifetime {};

  std::optional<internal::UniqueServerBindingOwner> binding_;
  std::shared_ptr<Lifetime> lifetime_;
};

template <typename FidlProtocol>
inline WeakServerBindingRef BorrowBinding(const ServerBindingBase<FidlProtocol>& binding) {
  return BorrowBinding(binding.binding_.value().ref());
}

template <typename FidlProtocol, typename Transport>
class ServerBindingGroupBase {
 public:
  using BindingUid = size_t;
  using Binding = typename Transport::template ServerBinding<FidlProtocol>;
  using StorageType = std::unordered_map<BindingUid, std::unique_ptr<Binding>>;

  ServerBindingGroupBase() = default;
  ServerBindingGroupBase(const ServerBindingGroupBase&) = delete;
  ServerBindingGroupBase(ServerBindingGroupBase&&) = delete;
  ServerBindingGroupBase& operator=(const ServerBindingGroupBase&) = delete;
  ServerBindingGroupBase& operator=(ServerBindingGroupBase&&) = delete;

  template <typename Dispatcher, typename ServerImpl, typename CloseHandler>
  void AddBinding(Dispatcher* dispatcher, fidl::internal::ServerEndType<FidlProtocol> server_end,
                  ServerImpl* impl, CloseHandler&& close_handler) {
    ProtocolMatchesImplRequirement<ServerImpl>();
    BindingUid binding_uid = next_uid_++;

    auto binding = std::make_unique<Binding>(
        dispatcher, std::move(server_end), std::move(impl),
        [actual_close_handler = std::forward<CloseHandler>(close_handler), binding_uid, this](
            ServerImpl* impl, UnbindInfo info) mutable {
          this->OnBindingClose(binding_uid, std::move(impl), info, actual_close_handler);
        });

    bindings_.insert(
        std::pair<BindingUid, std::unique_ptr<Binding>>(binding_uid, std::move(binding)));
  }

  template <typename Dispatcher, typename ServerImpl, typename CloseHandler>
  fidl::ProtocolHandler<FidlProtocol> CreateHandler(ServerImpl* impl, Dispatcher* dispatcher,
                                                    CloseHandler&& close_handler) {
    ProtocolMatchesImplRequirement<ServerImpl>();
    return [this, impl, dispatcher, close_handler = std::forward<CloseHandler>(close_handler)](
               fidl::internal::ServerEndType<FidlProtocol> server_end) {
      AddBinding(dispatcher, std::move(server_end), impl, close_handler);
    };
  }

  void ForEachBinding(fit::function<void(const Binding&)> visitor) {
    for (const auto& binding : bindings_) {
      visitor(*binding.second);
    }
  }

  template <class ServerImpl>
  bool RemoveBindings(const ServerImpl* impl) {
    ProtocolMatchesImplRequirement<ServerImpl>();

    if (ExtractMatchedBindings(impl).empty()) {
      return false;
    }

    MaybeEmpty();
    return true;
  }

  bool RemoveAll() {
    if (bindings_.empty()) {
      return false;
    }

    bindings_.clear();
    MaybeEmpty();
    return true;
  }

  template <class ServerImpl>
  bool CloseBindings(const ServerImpl* impl, zx_status_t epitaph_value) {
    ProtocolMatchesImplRequirement<ServerImpl>();

    auto matching_bindings = ExtractMatchedBindings(impl);
    if (matching_bindings.empty()) {
      return false;
    }

    // Kick off teardown for each binding, then put all matched bindings in the special store for
    // bindings that have been removed but are waiting to be successfully torn down.
    for (auto& binding : matching_bindings) {
      binding.second->Close(epitaph_value);
      tearing_down_.insert(std::move(binding));
    }
    return true;
  }

  bool CloseAll(zx_status_t epitaph_value) {
    bool had_bindings = !bindings_.empty();

    // Kick off teardown for each binding, then put all matched bindings in the special store for
    // bindings that have been removed but are waiting to be successfully torn down.
    for (auto& binding : bindings_) {
      binding.second->Close(epitaph_value);
      tearing_down_.insert(std::move(binding));
    }

    bindings_.clear();
    return had_bindings;
  }

  size_t size() const { return bindings_.size(); }

  void set_empty_set_handler(fit::closure empty_set_handler) {
    empty_handler_ = std::move(empty_set_handler);
  }

 private:
  template <typename ServerImpl>
  static constexpr void ProtocolMatchesImplRequirement() {
    internal::ServerImplToMessageDispatcher<FidlProtocol, ServerImpl>(nullptr);
  }

  // Removes all bindings matching a specified |impl*| instance from the main |bindings_| storage,
  // and transfers ownership of them to the caller to do what it pleases with. The caller may choose
  // to then immediately drop them (thereby "removing" the bindings), or to call `Close()` on each
  // one and store them in the |tearing_down_| storage until their respective teardowns can be
  // completed.
  template <typename ServerImpl>
  StorageType ExtractMatchedBindings(const ServerImpl* impl) {
    ProtocolMatchesImplRequirement<ServerImpl>();

    // Do one pass to build up the |extracted| list. We don't |.erase()| moved entries during this
    // pass to avoid mutating the list while walking over it.
    StorageType extracted;
    for (auto& binding : bindings_) {
      ZX_ASSERT(binding.second != nullptr);
      binding.second.get()->template AsImpl<ServerImpl>([&](const ServerImpl* i) {
        if (impl == i) {
          extracted.insert(std::move(binding));
        }
      });
    }

    // Now do a second pass, erasing all entries in |bindings_| that have been moved to |extracted|.
    for (const auto& binding : extracted) {
      bindings_.erase(binding.first);
    }

    return extracted;
  }

  // Removes a single binding matching a specified |BindingUid| from the main |bindings_| storage,
  // and transfers ownership of it to the caller to do what it pleases with. The caller may choose
  // to then immediately drop it (thereby "removing" the binding), or to call `Close()` on it and
  // store it in the |tearing_down_| storage until its teardown is completed.
  std::unique_ptr<Binding> ExtractMatchedBinding(BindingUid uid) {
    auto it = bindings_.find(uid);
    if (it == bindings_.end()) {
      return nullptr;
    }

    std::unique_ptr<Binding> extracted = std::move(it->second);
    bindings_.erase(uid);

    return extracted;
  }

  // Take a binding in any stage (either active or "tearing down") and immediately remove it from
  // all storage and pass it back to the caller, who now owns it.
  std::unique_ptr<Binding> ReleaseBinding(BindingUid uid) {
    auto matched_binding = ExtractMatchedBinding(uid);
    if (matched_binding != nullptr) {
      return matched_binding;
    }
    auto it = tearing_down_.find(uid);
    if (it == tearing_down_.end()) {
      return nullptr;
    }

    std::unique_ptr<Binding> deleted = std::move(it->second);
    tearing_down_.erase(uid);

    return deleted;
  }

  // There are three ways in which this function may be called:
  //
  //   1. The binding itself has encountered an error, and needs to tear down.
  //   2. The implementation calls |completer.Close|.
  //   3. The owner of this |ServerBindingGroup| has manually closed this |ServerBinding| by calling
  //      the |CloseBinding| method on it.
  template <typename CloseHandler, typename ServerImpl>
  void OnBindingClose(BindingUid uid, ServerImpl* impl, UnbindInfo info,
                      CloseHandler&& actual_close_handler) {
    ProtocolMatchesImplRequirement<ServerImpl>();

    // Assign this binding to a locally scoped variable to ensure that it does not get dropped
    // before the |actual_close_handler| returns. If the binding has already been removed manually
    // via a |Remove*| call, this will return a |nullptr|, indicating that we should not proceed
    // with firing the |empty_handler_|.
    auto released_binding = this->ReleaseBinding(uid);

    // Execute the user-supplied |CloseHandler|.
    if constexpr (std::is_convertible_v<CloseHandler, ::fidl::internal::IgnoreBindingClosureType>) {
      // The implementer has explicitly chosen to drop errors, so do nothing.
      //
      // Suppress warnings of unused |CloseHandler|.
      (void)actual_close_handler;
    } else if constexpr (std::is_convertible_v<CloseHandler, fidl::internal::SimpleCloseHandler>) {
      actual_close_handler(info);
    } else {
      actual_close_handler(impl, info);
    }

    if (released_binding != nullptr) {
      this->MaybeEmpty();
    }
  }

  // Make sure to clean up after ourselves if we're the last ones here! Only fires the
  // |empty_handler_| once all active bindings have been removed, and all "closed" bindings have
  // finished their respective teardown routines.
  void MaybeEmpty() {
    if (this->empty_handler_ && this->bindings_.empty() && this->tearing_down_.empty()) {
      this->empty_handler_();
    }
  }

  BindingUid next_uid_ = 0;
  StorageType bindings_;

  // Store for bindings that are being torn down and may no longer be interacted with through public
  // methods (iterated over, closed, removed, etc).
  StorageType tearing_down_;
  fit::closure empty_handler_;
};

}  // namespace internal

}  // namespace fidl

#endif  // LIB_FIDL_CPP_WIRE_SERVER_H_
