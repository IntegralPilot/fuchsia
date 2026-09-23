// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_FIDL_SERVER_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_FIDL_SERVER_H_

#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/unified_messaging_declarations.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/transaction.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/vfs/cpp/service.h>
#include <zircon/errors.h>

#include <memory>
#include <optional>
#include <string>

namespace forensics::stubs {

template <typename Protocol, ::fidl::internal::Openness Openness = Protocol::kOpenness>
class FidlServer;

// Specialize for closed protocols.
template <typename Protocol>
class FidlServer<Protocol, ::fidl::internal::Openness::kClosed>
    : public fidl::testing::TestBase<Protocol> {
 public:
  virtual ~FidlServer() = default;

  virtual std::unique_ptr<vfs::Service> GetService(async_dispatcher_t* dispatcher) {
    FX_NOTIMPLEMENTED() << "GetService is not implemented";
    return nullptr;
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    FX_NOTIMPLEMENTED() << name << " is not implemented";
  }
};

// Specialize for open and ajar protocols.
template <typename Protocol, ::fidl::internal::Openness Openness>
class FidlServer : public fidl::testing::TestBase<Protocol> {
 public:
  virtual ~FidlServer() = default;

  virtual std::unique_ptr<vfs::Service> GetService(async_dispatcher_t* dispatcher) {
    FX_NOTIMPLEMENTED() << "GetService is not implemented";
    return nullptr;
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    FX_NOTIMPLEMENTED() << name << " is not implemented";
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<Protocol> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_NOTIMPLEMENTED() << "Method ordinal '" << metadata.method_ordinal << "' is not implemented";
  }
};

template <typename Protocol>
class SingleBindingFidlServer : public FidlServer<Protocol> {
 public:
  void Bind(fidl::ServerEnd<Protocol> server_end, async_dispatcher_t* dispatcher) {
    binding_.emplace(dispatcher, std::move(server_end), this,
                     [this](fidl::UnbindInfo info) { is_bound_ = false; });
    is_bound_ = true;
  }

  void CloseConnection(const zx_status_t status) {
    if (binding_.has_value()) {
      FX_CHECK(is_bound_);
      binding_->Close(status);
      is_bound_ = false;
    }
  }

  bool IsBound() const { return is_bound_; }

  std::unique_ptr<vfs::Service> GetService(async_dispatcher_t* dispatcher) override {
    return std::make_unique<vfs::Service>(
        [this, dispatcher](zx::channel channel, async_dispatcher_t* /*unused*/) {
          binding_.emplace(dispatcher, fidl::ServerEnd<Protocol>(std::move(channel)), this,
                           [this](fidl::UnbindInfo info) { is_bound_ = false; });

          is_bound_ = true;
        });
  }

 protected:
  std::optional<fidl::ServerBinding<Protocol>>& binding() { return binding_; }

 private:
  std::optional<fidl::ServerBinding<Protocol>> binding_;
  bool is_bound_ = false;
};

}  // namespace forensics::stubs

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_FIDL_SERVER_H_
