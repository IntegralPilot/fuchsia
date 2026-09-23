// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/lib/fidl/device_server.h"

#include <lib/async/cpp/task.h>
#include <lib/ddk/device.h>
#include <lib/fidl/cpp/wire/status.h>
#include <zircon/errors.h>

#include <ddktl/fidl.h>

namespace devfs_fidl {

DeviceServer::DeviceServer(DeviceInterface& device, async_dispatcher_t* dispatcher)
    : controller_(device), dispatcher_(dispatcher) {}

void DeviceServer::CloseAllConnections(fit::callback<void()> callback) {
  async::PostTask(dispatcher_, [this, callback = std::move(callback)]() mutable {
    if (bindings_.empty()) {
      if (callback != nullptr) {
        callback();
      }
      return;
    }
    if (callback != nullptr) {
      ZX_ASSERT(!std::exchange(callback_, std::move(callback)).has_value());
    }
    for (auto& [handle, binding] : bindings_) {
      binding.Unbind();
    }
  });
}

void DeviceServer::ServeDeviceFidl(zx::channel channel) {
  fidl::ServerEnd<TypeErasedProtocol> server_end(std::move(channel));
  async::PostTask(
      dispatcher_, [this, server_end = std::move(server_end), impl = &device_]() mutable {
        const zx_handle_t key = server_end.channel().get();
        const auto binding =
            fidl::ServerBindingRef<TypeErasedProtocol>{fidl::internal::BindServerTypeErased(
                dispatcher_, fidl::internal::MakeAnyTransport(server_end.TakeHandle()), impl,
                fidl::internal::ThreadingPolicy::kCreateAndTeardownFromDispatcherThread,
                [this, key](fidl::internal::IncomingMessageDispatcher*, fidl::UnbindInfo,
                            fidl::internal::AnyTransport) {
                  DeviceServer& self = *this;
                  size_t erased = self.bindings_.erase(key);
                  ZX_ASSERT_MSG(erased == 1, "erased=%zu", erased);
                  if (self.bindings_.empty()) {
                    if (std::optional callback = std::exchange(self.callback_, {});
                        callback.has_value()) {
                      callback.value()();
                    }
                  }
                })};

        auto [it, inserted] = bindings_.try_emplace(key, binding);
        ZX_ASSERT_MSG(inserted, "handle=%d", key);
      });
}

DeviceServer::MessageDispatcher::MessageDispatcher(DeviceServer& parent) : parent_(parent) {}

namespace {

// TODO(https://fxbug.dev/42166376): This target uses uses internal FIDL machinery to ad-hoc compose
// protocols. Ad-hoc composition of protocols (try to match a method against protocol A, then B,
// etc.) is not supported by FIDL. We should move to public supported APIs.
template <typename FidlProtocol>
fidl::DispatchResult TryDispatch(fidl::WireServer<FidlProtocol>* impl,
                                 fidl::IncomingHeaderAndMessage& msg, fidl::Transaction* txn) {
  return fidl::internal::WireServerDispatcher<FidlProtocol>::TryDispatch(impl, msg, nullptr, txn);
}

class Transaction final : public fidl::Transaction {
 public:
  explicit Transaction(fidl::Transaction* inner) : inner_(inner) {}
  ~Transaction() final = default;

  const std::optional<std::tuple<fidl::UnbindInfo, fidl::ErrorOrigin>>& internal_error() const {
    return internal_error_;
  }

 private:
  std::unique_ptr<fidl::Transaction> TakeOwnership() final { return inner_->TakeOwnership(); }

  zx_status_t Reply(fidl::OutgoingMessage* message, fidl::WriteOptions write_options) final {
    return inner_->Reply(message, std::move(write_options));
  }

  void Close(zx_status_t epitaph) final { return inner_->Close(epitaph); }

  void InternalError(fidl::UnbindInfo error, fidl::ErrorOrigin origin) final {
    internal_error_.emplace(error, origin);
    return inner_->InternalError(error, origin);
  }

  void EnableNextDispatch() final { return inner_->EnableNextDispatch(); }

  bool DidOrGoingToUnbind() final { return inner_->DidOrGoingToUnbind(); }

  fidl::Transaction* const inner_;
  std::optional<std::tuple<fidl::UnbindInfo, fidl::ErrorOrigin>> internal_error_;
};

}  // namespace

void DeviceServer::MessageDispatcher::dispatch_message(
    fidl::IncomingHeaderAndMessage&& msg, fidl::Transaction* txn,
    fidl::internal::MessageStorageViewBase* storage_view) {
  // If the device is unbound it shouldn't receive messages so close the channel.
  if (parent_.controller_.IsUnbound()) {
    txn->Close(ZX_ERR_IO_NOT_PRESENT);
    return;
  }

  if (!msg.ok()) {
    // Mimic fidl::internal::TryDispatch.
    txn->InternalError(fidl::UnbindInfo{msg}, fidl::ErrorOrigin::kReceive);
    return;
  }

  uint64_t ordinal = msg.header()->ordinal;

  // Use shadowing lambda captures to ensure consumed values aren't used.
  [&, txn = Transaction(txn)]() mutable {
    if (!parent_.controller_.MessageOp(std::move(msg), ddk::IntoDeviceFIDLTransaction(&txn))) {
      // The device doesn't implement zx_protocol_device::message.
      static_cast<fidl::Transaction*>(&txn)->InternalError(fidl::UnbindInfo::UnknownOrdinal(),
                                                           fidl::ErrorOrigin::kReceive);
    }
    std::optional internal_error = txn.internal_error();
    if (!internal_error.has_value()) {
      return;
    }
    auto& [error, origin] = internal_error.value();
    if (error.reason() == fidl::Reason::kUnexpectedMessage) {
      std::string message;
      message.append("Failed to send message with ordinal=");
      message.append(std::to_string(ordinal));
      message.append(" to device: ");
      message.append(error.FormatDescription());
      message.append(
          "\n"
          "It is possible that this message assumed the wrong fidl protocol.");
      parent_.controller_.LogError(message.c_str());
    }
  }();
}

}  // namespace devfs_fidl
