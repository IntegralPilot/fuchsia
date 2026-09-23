// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_FIDL_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_FIDL_PROVIDER_H_

#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/client.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fit/function.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>

#include <algorithm>
#include <functional>
#include <memory>

#include "src/developer/forensics/feedback/annotations/provider.h"
#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/lib/backoff/backoff.h"
#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/lib/fxl/strings/substitute.h"

namespace forensics::feedback {

namespace internal {

// Defines how a provider should behave in the event of a connection error with the server.
struct DisconnectResponse {
  static DisconnectResponse BuildFrom(const zx_status_t status,
                                      const std::string_view interface_name) {
    if (status == ZX_ERR_NOT_FOUND) {
      return DisconnectResponse{
          .log_message = fxl::Substitute("$0 unavailable, will not retry", interface_name),
          .error = Error::kNotAvailableInProduct,
          .should_reconnect = false,
      };
    }

    return DisconnectResponse{
        .log_message = fxl::Substitute("Lost connection to $0", interface_name),
        .error = Error::kConnectionError,
        .should_reconnect = true,
    };
  }

  std::string log_message;
  Error error;
  bool should_reconnect;
};

}  // namespace internal

// Static async annotation provider that handles calling a single FIDL method and
// returning the result of the call as Annotations when the method completes.
//
// |Protocol| is the FIDL protocol being interacted with.
// |Method| is a callable that invokes the FIDL method on fidl::Client<Protocol>.
// |Convert| is a function object type for converting the results of the method call to Annotations.
template <typename Protocol, auto Method, typename Convert>
class StaticSingleFidlMethodAnnotationProvider : public StaticAsyncAnnotationProvider,
                                                 public fidl::AsyncEventHandler<Protocol> {
 public:
  StaticSingleFidlMethodAnnotationProvider(async_dispatcher_t* dispatcher,
                                           std::shared_ptr<sys::ServiceDirectory> services,
                                           std::unique_ptr<backoff::Backoff> backoff)
      : dispatcher_(dispatcher), services_(std::move(services)), backoff_(std::move(backoff)) {}

  void on_fidl_error(fidl::UnbindInfo info) override {
    const internal::DisconnectResponse disconnect = internal::DisconnectResponse::BuildFrom(
        info.status(), fidl::DiscoverableProtocolName<Protocol>);

    FX_PLOGS(WARNING, info.status()) << disconnect.log_message;
    client_ = fidl::Client<Protocol>();

    if (!disconnect.should_reconnect) {
      if (callback_ != nullptr) {
        callback_(convert_(disconnect.error));
      }
      return;
    }

    async::PostDelayedTask(
        dispatcher_,
        [self = ptr_factory_.GetWeakPtr()] {
          if (self) {
            self->Call();
          }
        },
        backoff_->GetNext());
  }

  void GetOnce(::fit::callback<void(Annotations)> callback) override {
    callback_ = std::move(callback);
    Call();
  }

 private:
  bool Connect() {
    if (client_.is_valid()) {
      return true;
    }

    zx::result endpoints = fidl::CreateEndpoints<Protocol>();
    if (endpoints.is_error()) {
      FX_LOGS(ERROR) << "Failed to create endpoints: " << endpoints.status_string();
      return false;
    }

    services_->Connect(fidl::DiscoverableProtocolName<Protocol>, endpoints->server.TakeChannel());
    client_ = fidl::Client<Protocol>(std::move(endpoints->client), dispatcher_, this);
    return true;
  }

  void Call() {
    if (!Connect()) {
      if (callback_ != nullptr) {
        callback_(convert_(Error::kConnectionError));
      }
      return;
    }

    std::invoke(Method, client_).Then([this](auto& result) {
      if (result.is_ok()) {
        backoff_->Reset();

        if (callback_ != nullptr) {
          callback_(convert_(result.value()));
        }

        // Should only be called once; no need to stay connected.
        client_ = fidl::Client<Protocol>();
      } else {
        using ErrorType = std::decay_t<decltype(result.error_value())>;

        // FIDL methods without a custom error type won't have is_domain_error defined. If the error
        // is a framework error, on_fidl_error will be called and will attempt to reconnect.
        if constexpr (requires(ErrorType e) { e.is_domain_error(); }) {
          if (result.error_value().is_domain_error()) {
            if (callback_ != nullptr) {
              callback_(convert_(result.error_value()));
            }

            client_ = fidl::Client<Protocol>();
          }
        }
      }
    });
  }

  async_dispatcher_t* dispatcher_;
  std::shared_ptr<sys::ServiceDirectory> services_;
  std::unique_ptr<backoff::Backoff> backoff_;
  Convert convert_;

  fidl::Client<Protocol> client_;
  ::fit::callback<void(Annotations)> callback_;
  fxl::WeakPtrFactory<StaticSingleFidlMethodAnnotationProvider> ptr_factory_{this};
};

// Dynamic async annotation provider that handles calling a single FIDL method and
// returning the result of the call as Annotations each time Get is invoked.
//
// |Protocol| is the FIDL protocol being interacted with.
// |Method| is a callable that invokes the FIDL method on fidl::Client<Protocol>.
// |Convert| is a function object type for converting the results of the method call to Annotations.
template <typename Protocol, auto Method, typename Convert>
class DynamicSingleFidlMethodAnnotationProvider : public DynamicAsyncAnnotationProvider,
                                                  public fidl::AsyncEventHandler<Protocol> {
 public:
  DynamicSingleFidlMethodAnnotationProvider(async_dispatcher_t* dispatcher,
                                            std::shared_ptr<sys::ServiceDirectory> services,
                                            std::unique_ptr<backoff::Backoff> backoff)
      : dispatcher_(dispatcher), services_(std::move(services)), backoff_(std::move(backoff)) {
    Connect();
  }

  void on_fidl_error(fidl::UnbindInfo error) override {
    const internal::DisconnectResponse disconnect = internal::DisconnectResponse::BuildFrom(
        error.status(), fidl::DiscoverableProtocolName<Protocol>);

    if (!disconnect.should_reconnect) {
      // Invalidate the client so that future requests aren't made.
      client_ = fidl::Client<Protocol>();
      FX_LOGS(ERROR) << fidl::DiscoverableProtocolName<
                            Protocol> << " not found, will not attempt to reconnect";
      return;
    }

    FX_PLOGS(WARNING, error.status()) << disconnect.log_message;
    reconnect_task_.PostDelayed(dispatcher_, backoff_->GetNext());
  }

  void Get(::fit::callback<void(Annotations)> callback) override {
    if (!client_.is_valid()) {
      callback(convert_(Error::kNotAvailableInProduct));
      return;
    }

    std::invoke(Method, client_).Then([this, callback = std::move(callback)](auto& result) mutable {
      if (result.is_error()) {
        FX_LOGS(ERROR) << "Call to " << fidl::DiscoverableProtocolName<Protocol> << " failed: "
                       << result.error_value();
        const Error error = FidlErrorToForensicsError(result.error_value());
        callback(convert_(error));
        return;
      }

      backoff_->Reset();
      callback(convert_(result.value()));
    });
  }

 private:
  void Connect() {
    zx::result endpoints = fidl::CreateEndpoints<Protocol>();
    if (endpoints.is_error()) {
      FX_LOGS(ERROR) << "Failed to create endpoints: " << endpoints.status_string();
      return;
    }

    services_->Connect(fidl::DiscoverableProtocolName<Protocol>, endpoints->server.TakeChannel());
    client_ = fidl::Client<Protocol>(std::move(endpoints->client), dispatcher_, this);
  }

  async_dispatcher_t* dispatcher_;
  std::shared_ptr<sys::ServiceDirectory> services_;
  std::unique_ptr<backoff::Backoff> backoff_;
  Convert convert_;

  fidl::Client<Protocol> client_;
  async::TaskClosureMethod<DynamicSingleFidlMethodAnnotationProvider,
                           &DynamicSingleFidlMethodAnnotationProvider::Connect>
      reconnect_task_{this};
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_FIDL_PROVIDER_H_
