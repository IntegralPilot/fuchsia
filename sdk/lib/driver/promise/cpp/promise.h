// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_PROMISE_CPP_PROMISE_H_
#define LIB_DRIVER_PROMISE_CPP_PROMISE_H_

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/fpromise/promise.h>
#include <zircon/availability.h>

namespace fdf {

namespace internal {

// Connects to the given `protocol_name` in `ns`, and returns a fpromise::result containing a
// fidl::WireSharedClient on success.
template <typename Protocol>
fpromise::result<fidl::WireSharedClient<Protocol>, zx_status_t> ConnectWithResult(
    const fdf::Namespace& ns, async_dispatcher_t* dispatcher, const char* protocol_name) {
  auto result = ns.Connect<Protocol>(protocol_name);
  if (result.is_error()) {
    return fpromise::error(result.status_value());
  }
  fidl::WireSharedClient<Protocol> client(std::move(*result), dispatcher);
  return fpromise::ok(std::move(client));
}

// Opens the given `path` in `ns`, and returns a fpromise::result containing a
// fidl::WireSharedClient on success.
fpromise::result<fidl::WireSharedClient<fuchsia_io::File>, zx_status_t> OpenWithResult(
    const fdf::Namespace& ns, async_dispatcher_t* dispatcher, const char* path,
    fuchsia_io::Flags flags);

}  // namespace internal

// Connects to the given `protocol_name` in `ns`, and returns a fpromise::promise containing a
// fidl::WireSharedClient on success.
template <typename Protocol>
fpromise::promise<fidl::WireSharedClient<Protocol>, zx_status_t> Connect(
    const fdf::Namespace& ns, async_dispatcher_t* dispatcher,
    const char* protocol_name = fidl::DiscoverableProtocolName<Protocol>) {
  return fpromise::make_result_promise(
      internal::ConnectWithResult<Protocol>(ns, dispatcher, protocol_name));
}

// Opens the given `path` in `ns`, and returns a fpromise::promise containing a
// fidl::WireSharedClient on success.
inline fpromise::promise<fidl::WireSharedClient<fuchsia_io::File>, zx_status_t> Open(
    const fdf::Namespace& ns, async_dispatcher_t* dispatcher, const char* path,
    fuchsia_io::Flags flags) {
  return fpromise::make_result_promise(internal::OpenWithResult(ns, dispatcher, path, flags));
}

// Adds a child to `client`, using `args`. `controller` must be provided, but
// `node` is optional.
fpromise::promise<void, fuchsia_driver_framework::wire::NodeError> AddChild(
    fidl::WireSharedClient<fuchsia_driver_framework::Node>& client,
    fuchsia_driver_framework::wire::NodeAddArgs args,
    fidl::ServerEnd<fuchsia_driver_framework::NodeController> controller,
    fidl::ServerEnd<fuchsia_driver_framework::Node> node);

}  // namespace fdf

#endif  // LIB_DRIVER_PROMISE_CPP_PROMISE_H_
