// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_COMPONENT_INCOMING_CPP_CLONE_H_
#define LIB_COMPONENT_INCOMING_CPP_CLONE_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.unknown/cpp/wire.h>
#include <lib/component/incoming/cpp/internal.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <utility>

namespace component {

// Typed channel wrapper around |fuchsia.unknown/Cloneable.Clone|.
//
// Given an unowned client end |client|, returns an owned clone as a new connection using protocol
// request pipelining.
//
// |client| must be a channel that supports |fuchsia.unknown/Cloneable|, for example:
//
// ```
//   // |client| could be |fidl::ClientEnd| or |fidl::UnownedClientEnd|.
//   auto clone = component::Clone(client);
// ```
//
// By default, this function will verify that the protocol type supports cloning
// (i.e. satisfies the protocol requirement above).
template <typename Protocol>
zx::result<fidl::ClientEnd<Protocol>> Clone(fidl::UnownedClientEnd<Protocol> client) {
  static_assert(internal::is_complete_v<Protocol>,
                "|Protocol| must be defined to use |component::Clone|");
  static_assert(internal::has_fidl_method_fuchsia_unknown_clone_v<Protocol>,
                "|Protocol| must compose |fuchsia.unknown/Cloneable|.");
  zx::result<zx::channel> result =
      internal::CloneRaw(fidl::UnownedClientEnd<fuchsia_unknown::Cloneable>(client.channel()));
  if (!result.is_ok()) {
    return result.take_error();
  }
  return zx::ok(fidl::ClientEnd<Protocol>(std::move(*result)));
}

// Overload of |component::Clone| to emulate implicit conversion from a
// |const fidl::ClientEnd&| into |fidl::UnownedClientEnd|. C++ cannot consider
// actual implicit conversions when performing template argument deduction.
template <typename Protocol>
zx::result<fidl::ClientEnd<Protocol>> Clone(const fidl::ClientEnd<Protocol>& client) {
  return Clone(client.borrow());
}

// Unchecked wrapper around |component::Clone|. Prefer using |component::Clone| over this function.
//
// Unlike |component::Clone|, this function ignores transport errors on |client|, and returns an
// invalid client end on failure.
template <typename Protocol>
fidl::ClientEnd<Protocol> MaybeClone(fidl::UnownedClientEnd<Protocol> client) {
  auto result = Clone(client);
  if (!result.is_ok()) {
    return {};
  }
  return std::move(*result);
}

// Overload of |component::MaybeClone| to emulate implicit conversion from a
// |const fidl::ClientEnd&| into |fidl::UnownedClientEnd|. C++ cannot consider
// actual implicit conversions when performing template argument deduction.
template <typename Protocol>
fidl::ClientEnd<Protocol> MaybeClone(const fidl::ClientEnd<Protocol>& client) {
  return MaybeClone(client.borrow());
}

}  // namespace component

#endif  // LIB_COMPONENT_INCOMING_CPP_CLONE_H_
