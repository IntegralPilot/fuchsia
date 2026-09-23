// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_VERSIONS_H_
#define SRC_SYSMEM_SERVER_VERSIONS_H_

#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2.internal/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <inttypes.h>

enum VersionIndex : size_t {
  kVersionIndexV1 = 0,
  kVersionIndexV2 = 1,
  kVersionIndexCombinedV1AndV2 = 2,
};

// Since CollectionServerEnd and GroupServerEnd never have a combined server end, we use an empty
// protocol server end as a stand-in to avoid complicating the templates below.
using EmptyCombinedServerEnd = fidl::ServerEnd<fuchsia_sysmem2_internal::EmptyCombinedServerEnd>;

using TokenServerEndV1 = fidl::ServerEnd<fuchsia_sysmem::BufferCollectionToken>;
using TokenServerEndV2 = fidl::ServerEnd<fuchsia_sysmem2::BufferCollectionToken>;
using TokenServerEndCombinedV1AndV2 =
    fidl::ServerEnd<fuchsia_sysmem2_internal::CombinedBufferCollectionToken>;
using TokenServerEnd =
    std::variant<TokenServerEndV1, TokenServerEndV2, TokenServerEndCombinedV1AndV2>;

using CollectionServerEndV1 = fidl::ServerEnd<fuchsia_sysmem::BufferCollection>;
using CollectionServerEndV2 = fidl::ServerEnd<fuchsia_sysmem2::BufferCollection>;
using CollectionServerEnd =
    std::variant<CollectionServerEndV1, CollectionServerEndV2, EmptyCombinedServerEnd>;

using GroupServerEndV1 = fidl::ServerEnd<fuchsia_sysmem::BufferCollectionTokenGroup>;
using GroupServerEndV2 = fidl::ServerEnd<fuchsia_sysmem2::BufferCollectionTokenGroup>;
using GroupServerEnd = std::variant<GroupServerEndV1, GroupServerEndV2, EmptyCombinedServerEnd>;

// Use a more specific type above in most places.  In a few places we use this more generic type to
// avoid templates that likely would generate more code.
using NodeServerEnd = std::variant<zx::channel, zx::channel, zx::channel>;

namespace sysmem_service {

namespace internal {

template <typename T, typename Enable = void>
struct GetUnownedChannelImpl {
  // intentionally no get_unowned_channel() here
};
template <typename T>
struct GetUnownedChannelImpl<T, std::enable_if_t<std::is_same_v<TokenServerEnd, T> ||
                                                 std::is_same_v<CollectionServerEnd, T> ||
                                                 std::is_same_v<GroupServerEnd, T>>> {
  static zx::unowned_channel get_unowned_channel(const T& server_end) {
    switch (server_end.index()) {
      case kVersionIndexV1:
        return zx::unowned_channel(std::get<kVersionIndexV1>(server_end).channel());
      case kVersionIndexV2:
        return zx::unowned_channel(std::get<kVersionIndexV2>(server_end).channel());
      case kVersionIndexCombinedV1AndV2:
        return zx::unowned_channel(std::get<kVersionIndexCombinedV1AndV2>(server_end).channel());
    }
    ZX_PANIC("unreachable");
  }
};
template <typename T>
struct GetUnownedChannelImpl<
    T, std::enable_if_t<
           std::is_same_v<TokenServerEndV1, T> || std::is_same_v<TokenServerEndV2, T> ||
           std::is_same_v<TokenServerEndCombinedV1AndV2, T> ||
           std::is_same_v<CollectionServerEndV1, T> || std::is_same_v<CollectionServerEndV2, T> ||
           std::is_same_v<GroupServerEndV1, T> || std::is_same_v<GroupServerEndV2, T>>> {
  static zx::unowned_channel get_unowned_channel(const T& server_end) {
    return zx::unowned_channel(server_end.channel());
  }
};
template <typename T>
struct GetUnownedChannelImpl<T, std::enable_if_t<std::is_same_v<NodeServerEnd, T>>> {
  static zx::unowned_channel get_unowned_channel(const T& server_end) {
    switch (server_end.index()) {
      case kVersionIndexV1:
        return zx::unowned_channel(std::get<kVersionIndexV1>(server_end));
      case kVersionIndexV2:
        return zx::unowned_channel(std::get<kVersionIndexV2>(server_end));
      case kVersionIndexCombinedV1AndV2:
        return zx::unowned_channel(std::get<kVersionIndexCombinedV1AndV2>(server_end));
    }
    ZX_PANIC("unreachable");
  }
};

template <typename T, typename Enable = void>
struct TakeChannelImpl {
  // intentionally no get_unowned_channel() here
};
template <typename T>
struct TakeChannelImpl<T, std::enable_if_t<std::is_same_v<TokenServerEnd, T> ||
                                           std::is_same_v<CollectionServerEnd, T> ||
                                           std::is_same_v<GroupServerEnd, T>>> {
  static zx::channel take_channel(const T& server_end) {
    switch (server_end.index()) {
      case kVersionIndexV1:
        return std::move(std::get<kVersionIndexV1>(server_end).TakeChannel());
      case kVersionIndexV2:
        return std::move(std::get<kVersionIndexV2>(server_end).TakeChannel());
      case kVersionIndexCombinedV1AndV2:
        return std::move(std::get<kVersionIndexCombinedV1AndV2>(server_end).TakeChannel());
    }
    ZX_PANIC("unreachable");
  }
};
template <typename T>
struct TakeChannelImpl<
    T, std::enable_if_t<
           std::is_same_v<TokenServerEndV1, T> || std::is_same_v<TokenServerEndV2, T> ||
           std::is_same_v<TokenServerEndCombinedV1AndV2, T> ||
           std::is_same_v<CollectionServerEndV1, T> || std::is_same_v<CollectionServerEndV2, T> ||
           std::is_same_v<GroupServerEndV1, T> || std::is_same_v<GroupServerEndV2, T>>> {
  static zx::channel take_channel(T server_end) { return server_end.TakeChannel(); }
};
template <typename T>
struct TakeChannelImpl<T, std::enable_if_t<std::is_same_v<NodeServerEnd, T>>> {
  static zx::channel take_channel(T server_end) {
    switch (server_end.index()) {
      case kVersionIndexV1:
        return std::move(std::get<kVersionIndexV1>(server_end));
      case kVersionIndexV2:
        return std::move(std::get<kVersionIndexV2>(server_end));
      case kVersionIndexCombinedV1AndV2:
        return std::move(std::get<kVersionIndexCombinedV1AndV2>(server_end));
    }
    ZX_PANIC("unreachable");
  }
};

template <typename T, typename Enable = void>
struct TakeNodeServerEndImpl {
  // no take_nod_server_end() here
};
template <typename T>
struct TakeNodeServerEndImpl<T, typename std::enable_if_t<std::is_same_v<TokenServerEnd, T> ||
                                                          std::is_same_v<CollectionServerEnd, T> ||
                                                          std::is_same_v<GroupServerEnd, T>>> {
  static NodeServerEnd take_node_server_end(T server_end) {
    std::optional<NodeServerEnd> node_server_end;
    switch (server_end.index()) {
      case kVersionIndexV1:
        node_server_end.emplace(std::in_place_index<kVersionIndexV1>,
                                std::move(std::get<kVersionIndexV1>(server_end).TakeChannel()));
        break;
      case kVersionIndexV2:
        node_server_end.emplace(std::in_place_index<kVersionIndexV2>,
                                std::move(std::get<kVersionIndexV2>(server_end).TakeChannel()));
        break;
      case kVersionIndexCombinedV1AndV2:
        node_server_end.emplace(
            std::in_place_index<kVersionIndexCombinedV1AndV2>,
            std::move(std::get<kVersionIndexCombinedV1AndV2>(server_end).TakeChannel()));
        break;
    }
    return std::move(node_server_end.value());
  }
};

}  // namespace internal

template <typename T>
zx::unowned_channel GetUnownedChannel(const T& server_end) {
  return internal::GetUnownedChannelImpl<T>::get_unowned_channel(server_end);
}

template <typename T>
zx::channel TakeChannel(T server_end) {
  return internal::GetUnownedChannelImpl<T>::take_channel(std::move(server_end));
}

template <typename T>
NodeServerEnd TakeNodeServerEnd(T server_end) {
  return internal::TakeNodeServerEndImpl<T>::take_node_server_end(std::move(server_end));
}

}  // namespace sysmem_service

#endif  // SRC_SYSMEM_SERVER_VERSIONS_H_
