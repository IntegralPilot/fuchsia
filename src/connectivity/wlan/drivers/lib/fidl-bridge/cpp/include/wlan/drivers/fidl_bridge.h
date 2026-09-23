// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_FIDL_BRIDGE_CPP_INCLUDE_WLAN_DRIVERS_FIDL_BRIDGE_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_FIDL_BRIDGE_CPP_INCLUDE_WLAN_DRIVERS_FIDL_BRIDGE_H_

#include <lib/fdf/cpp/arena.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fidl_driver/cpp/transport.h>
#include <lib/stdcompat/source_location.h>

#include <wlan/drivers/log.h>

namespace wlan::drivers::fidl_bridge {

// Retrieve the underlying zx_status_t from a FIDL error, which may be either a domain or framework
// error. This assumes that the type of the domain_error is a zx_status_t, not a domain-specific
// error enum.
template <typename ErrorType>
zx_status_t FidlErrorToStatus(const ErrorType& e) {
  return e.is_domain_error() ? e.domain_error() : e.framework_error().status();
}

// Returns a lambda that can be used as the callback passed to |Then| or |ThenExactlyOnce| on the
// return value from making an async FIDL request over |bridge_client_|.
// The returned lambda will forward the result to the provided |completer|.
//
// Framework errors that result in channel closure are propagated by closing the completer with
// an epitaph, which results in closing the underlying transport.
// Framework errors that do not result in channel closure are either:
// - Returned as a domain error if |FidlMethod| has one.
// - Logged then ignored if |FidlMethod| does not have a domain error. If |FidlMethod| returns a
//   response to the caller, then a default-initialized response is returned.
template <typename FidlMethod, typename AsyncCompleter>
auto ForwardResult(AsyncCompleter completer,
                   cpp20::source_location loc = cpp20::source_location::current()) {
  return [completer = std::move(completer), loc](auto& result) mutable {
    ltrace(0, nullptr, "Forwarding result for %s", loc.function_name());
    if (result.is_error()) {
      lerror("Result not ok for %s: %s", loc.function_name(),
             result.error_value().FormatDescription().c_str());
    }

    if constexpr (FidlMethod::kHasDomainError) {
      // If we get a framework error that results in channel closure, then we close the channel.
      if (result.is_error() && result.error_value().is_framework_error()) {
        fidl::Status& framework_error = result.error_value().framework_error();
        if (framework_error.is_peer_closed()) {
          completer.Close(framework_error.status());
          return;
        }
      }

      // Framework errors that do not result in channel closure get sent up as a domain error
      // instead.
      completer.Reply(result.map_error([](auto error) { return FidlErrorToStatus(error); }));
    } else {
      // If we get an error that results in channel closure, then we close the channel.
      if (result.is_error() && result.error_value().is_peer_closed()) {
        completer.Close(result.error_value().status());
        return;
      }

      if constexpr (FidlMethod::kHasNonEmptyUserFacingResponse) {
        if (result.is_error()) {
          // If the error did not result in channel closure, then reply with some
          // default-initialized value.
          // TODO(https://fxbug.dev/341756361) Remove this when API is cleaned up.
          completer.Reply({});
        } else {
          completer.Reply(result.value());
        }
      } else {
        // completer.Reply() has no parameters only if FidlMethod does not return a value or error.
        completer.Reply();
      }
    }
  };
}

template <typename FidlMethod, typename AsyncCompleter>
auto ForwardWireResult(AsyncCompleter completer, fdf::Arena arena,
                       cpp20::source_location loc = cpp20::source_location::current()) {
  return [completer = std::move(completer), arena = std::move(arena), loc](auto& result) mutable {
    ltrace(0, nullptr, "Forwarding result for %s", loc.function_name());
    if (result.is_error()) {
      lerror("Result not ok for %s: %s", loc.function_name(),
             result.error_value().FormatDescription().c_str());
    }

    if constexpr (FidlMethod::kHasDomainError) {
      // If we get a framework error that results in channel closure, then we close the channel.
      if (result.is_error() && result.error_value().is_framework_error()) {
        fidl::Status& framework_error = result.error_value().framework_error();
        if (framework_error.is_peer_closed()) {
          completer.Close(framework_error.status());
          return;
        }
      }

      // Framework errors that do not result in channel closure get sent up as a domain error
      // instead.
      completer.buffer(arena).Reply(
          result.map_error([](auto error) { return FidlErrorToStatus(error); }));
    } else {
      // If we get an error that results in channel closure, then we close the channel.
      if (result.is_error() && result.error_value().is_peer_closed()) {
        completer.Close(result.error_value().status());
        return;
      }

      if constexpr (FidlMethod::kHasNonEmptyUserFacingResponse) {
        if (result.is_error()) {
          // If the error did not result in channel closure, then reply with some
          // default-initialized value.
          // TODO(https://fxbug.dev/341756361) Remove this when API is cleaned up.
          completer.buffer(arena).Reply({});
        } else {
          completer.buffer(arena).Reply(result.value());
        }
      } else {
        // completer.Reply() has no parameters only if FidlMethod does not return a value or error.
        completer.buffer(arena).Reply();
      }
    }
  };
}

}  // namespace wlan::drivers::fidl_bridge

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_FIDL_BRIDGE_CPP_INCLUDE_WLAN_DRIVERS_FIDL_BRIDGE_H_
