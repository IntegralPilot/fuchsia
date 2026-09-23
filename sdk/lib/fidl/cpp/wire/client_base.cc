// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fidl/cpp/wire/client_base.h>
#include <lib/fidl/txn_header.h>
#include <lib/fit/function.h>

namespace fidl {
namespace internal {

// TODO(madhaviyengar): Move this constant to zircon/fidl.h
constexpr uint32_t kUserspaceTxidMask = 0x7FFFFFFF;

ResponseContext::~ResponseContext() {
  if (client_base_ != nullptr) {
    client_base_->ForgetAsyncTxn(txid_, this);
  }
}

ClientBase::~ClientBase() {
  // If transactions race with teardown, there may still be lingering
  // |ResponseContext|s. Clear them again to avoid triggering debug asserts.
  ReleaseResponseContexts(UnbindInfo::Unbind());
}

std::shared_ptr<ClientBase> ClientBase::Create(
    AnyTransport&& transport, async_dispatcher_t* dispatcher,
    AnyIncomingEventDispatcher&& event_dispatcher, AsyncEventHandler* error_handler,
    fidl::AnyTeardownObserver&& teardown_observer, ThreadingPolicy threading_policy,
    std::weak_ptr<ClientControlBlock> client_object_lifetime) {
  std::shared_ptr client_base = std::make_shared<ClientBase>();
  client_base->Bind(std::move(transport), dispatcher, std::move(event_dispatcher), error_handler,
                    std::move(teardown_observer), threading_policy,
                    std::move(client_object_lifetime));
  return client_base;
}

void ClientBase::Bind(AnyTransport&& transport, async_dispatcher_t* dispatcher,
                      AnyIncomingEventDispatcher&& event_dispatcher,
                      AsyncEventHandler* error_handler, AnyTeardownObserver&& teardown_observer,
                      ThreadingPolicy threading_policy,
                      std::weak_ptr<ClientControlBlock> client_object_lifetime) {
  ZX_DEBUG_ASSERT(!binding_.lock());
  auto binding = AsyncClientBinding::Create(
      dispatcher, std::make_shared<fidl::internal::AnyTransport>(std::move(transport)),
      shared_from_this(), error_handler, std::move(teardown_observer), threading_policy);
  binding_ = WeakBindingRef<AsyncClientBinding>{binding, binding->shared_unbind_info()};
  client_object_lifetime_ = std::move(client_object_lifetime);
  dispatcher_ = dispatcher;
  event_dispatcher_ = std::move(event_dispatcher);
  binding->BeginFirstWait();
}

ClientBase::AsyncTeardownResult ClientBase::AsyncTeardown() {
  auto matchers = MatchVariant{
      [this](std::shared_ptr<AsyncClientBinding>&& binding) -> AsyncTeardownResult {
        fit::result teardown_result = binding->StartTeardown(std::move(binding));
        if (teardown_result.is_error()) {
          // Already torn down or raced with in-progress teardown.
          return fit::error(fidl::Status::Unbound());
        }

        // If there are pending two-way calls, do not allow the user to extract
        // the client endpoint.
        std::scoped_lock lock(lock_);
        if (!contexts_.is_empty()) {
          return fit::error(fidl::Status::PendingTwoWayCallPreventsUnbind());
        }

        return teardown_result.take_value();
      },
      [](fidl::UnbindInfo info) -> AsyncTeardownResult { return fit::error(info.ToError()); },
      [](WeakClientBindingRef::Invalid) -> AsyncTeardownResult {
        // Should not reach here if we check for a valid client before
        // user initiated teardown.
        __builtin_abort();
      },
  };
  return std::visit(matchers, binding_.lock_or_error());
}

zx_txid_t ClientBase::PrepareAsyncTxn(ResponseContext* context) {
  std::scoped_lock lock(lock_);

  // Generate the next txid. Verify that it doesn't overlap with any outstanding txids.
  do {
    do {
      context->txid_ = ++txid_base_ & kUserspaceTxidMask;  // txid must be within mask.
    } while (unlikely(!context->txid_));  // txid must be non-zero.
  } while (unlikely(!contexts_.insert_or_find(context)));

  context->client_base_ = this;
  list_add_tail(&delete_list_, context);

  return context->txid_;
}

bool ClientBase::ForgetAsyncTxn(zx_txid_t txid, ResponseContext* context) {
  std::scoped_lock lock(lock_);

  auto it = contexts_.find(txid);
  // We make sure that the returned iterator matches the context before removing
  // from the list. If the txid of the transaction is reused then the context
  // may not match (txids are currently only reused on wraparound, which is very
  // unlikely).
  if (it != contexts_.end() && &*it == context) {
    contexts_.erase(it);
    list_delete(static_cast<list_node_t*>(context));
    context->client_base_ = nullptr;
    return true;
  }
  return false;
}

void ClientBase::ReleaseResponseContexts(fidl::UnbindInfo info) {
  // Release ownership on any outstanding |ResponseContext|s outside of locks.
  list_node_t delete_list;
  {
    std::scoped_lock lock(lock_);
    contexts_.clear();
    list_move(&delete_list_, &delete_list);
  }

  list_node_t* node = nullptr;
  list_node_t* temp_node = nullptr;
  list_for_every_safe(&delete_list, node, temp_node) {
    list_delete(node);
    auto* context = static_cast<ResponseContext*>(node);
    context->client_base_ = nullptr;
    // |kClose| is never used on the client side.
    ZX_DEBUG_ASSERT(info.reason() != fidl::Reason::kClose);
    context->OnError(ErrorFromUnbindInfo(info));
  }
}

void ClientBase::SendTwoWay(fidl::OutgoingMessage& message, ResponseContext* context,
                            fidl::WriteOptions write_options) {
  auto matchers = MatchVariant{
      [&](std::shared_ptr<AnyTransport>&& transport) {
        zx_txid_t txid = 0;
        {
          std::shared_ptr scoped_transport = std::move(transport);
          txid = PrepareAsyncTxn(context);
          message.set_txid(txid);
          message.Write(*scoped_transport, std::move(write_options));
        }
        // Handle error without holding onto the transport.
        if (!message.ok()) {
          if (ForgetAsyncTxn(txid, context)) {
            TryAsyncDeliverError(message.error(), context);
          }
          HandleSendError(message.error());
        }
      },
      [&](fidl::UnbindInfo info) { TryAsyncDeliverError(ErrorFromUnbindInfo(info), context); },
  };
  std::visit(matchers, GetTransportOrError());
}

fidl::OneWayStatus ClientBase::SendOneWay(fidl::OutgoingMessage& message,
                                          fidl::WriteOptions write_options) {
  auto matchers = MatchVariant{
      [&](std::shared_ptr<AnyTransport>&& transport) {
        {
          std::shared_ptr scoped_transport = std::move(transport);
          message.set_txid(0);
          message.Write(*scoped_transport, std::move(write_options));
        }
        // Handle error without holding onto the transport.
        if (!message.ok()) {
          HandleSendError(message.error());
          return fidl::OneWayStatus{message.error()};
        }
        return fidl::OneWayStatus{fidl::Status::Ok()};
      },
      [](fidl::UnbindInfo info) { return OneWayErrorFromUnbindInfo(info); },
  };
  return std::visit(matchers, GetTransportOrError());
}

void ClientBase::HandleSendError(fidl::Status error) {
  if (auto binding = binding_.lock()) {
    binding->HandleError(std::move(binding), {UnbindInfo{error}, ErrorOrigin::kSend});
  }
}

void ClientBase::TryAsyncDeliverError(::fidl::Status error, ResponseContext* context) {
  zx_status_t status = context->TryAsyncDeliverError(error, dispatcher_);
  if (status != ZX_OK) {
    context->OnError(error);
  }
}

ClientBase::TransportOrError ClientBase::GetTransportOrError() {
  auto matchers = MatchVariant{
      [](const std::shared_ptr<AsyncClientBinding>& binding) -> TransportOrError {
        if (auto transport = binding->GetTransport(); transport) {
          return transport;
        }
        // If the transport is gone, that means the user explicitly requested unbinding.
        return fidl::UnbindInfo::Unbind();
      },
      [](fidl::UnbindInfo info) -> TransportOrError { return info; },
      [](WeakClientBindingRef::Invalid) -> TransportOrError {
        // Should not reach here if we check for a valid client before
        // making calls.
        __builtin_abort();
      },
  };
  return std::visit(matchers, binding_.lock_or_error());
}

std::optional<UnbindInfo> ClientBase::Dispatch(fidl::IncomingHeaderAndMessage& msg,
                                               internal::MessageStorageViewBase* storage_view) {
  if (fidl_epitaph_t* epitaph = msg.maybe_epitaph(); unlikely(epitaph)) {
    return UnbindInfo::PeerClosed(epitaph->error);
  }

  auto* hdr = msg.header();
  if (hdr->txid == 0) {
    // Dispatch events (received messages with no txid).
    // Dispatch will always consume the message even if it is not recognized
    // (unknown interaction), so it is important to not reference the message or
    // header again after calling this.
    fidl::Status status = event_dispatcher_->DispatchEvent(msg, storage_view);
    if (status.ok()) {
      return std::nullopt;
    }
    return fidl::UnbindInfo{status};
  }

  // If this is a response, look up the corresponding ResponseContext based on the txid.
  ResponseContext* context = nullptr;
  {
    std::scoped_lock lock(lock_);
    context = contexts_.erase(hdr->txid);
    if (likely(context != nullptr)) {
      list_delete(static_cast<list_node_t*>(context));
      context->client_base_ = nullptr;
    } else {
      // Received unknown txid.
      return UnbindInfo{
          Status::UnexpectedMessage(ZX_ERR_NOT_FOUND, fidl::internal::kErrorUnknownTxId)};
    }
  }
  return context->OnRawResult(std::move(msg), storage_view);
}

void ClientController::Bind(AnyTransport client_end, async_dispatcher_t* dispatcher,
                            AnyIncomingEventDispatcher&& event_dispatcher,
                            AsyncEventHandler* error_handler,
                            AnyTeardownObserver&& teardown_observer,
                            ThreadingPolicy threading_policy) {
  ZX_ASSERT(!client_impl_);
  // This three step dance is to setup a circular reference where |ClientBase|
  // weakly references |ClientControlBlock| and |ClientControlBlock| strongly
  // references |ClientBase|.
  control_ = std::make_shared<ClientControlBlock>(nullptr);
  client_impl_ = ClientBase::Create(std::move(client_end), dispatcher, std::move(event_dispatcher),
                                    error_handler, std::move(teardown_observer), threading_policy,
                                    control_->weak_from_this());
  // Actually fill in the |client_impl_|.
  *control_ = ClientControlBlock{client_impl_};
}

void ClientController::Unbind() { (void)UnbindMaybeGetEndpoint(); }

fit::result<fidl::Error, fidl::internal::AnyTransport> ClientController::UnbindMaybeGetEndpoint() {
  ZX_ASSERT(client_impl_);
  // Destroy |control| after |AsyncTeardown|.
  std::shared_ptr control = std::move(control_);
  return client_impl_->ClientBase::AsyncTeardown();
}

}  // namespace internal
}  // namespace fidl
