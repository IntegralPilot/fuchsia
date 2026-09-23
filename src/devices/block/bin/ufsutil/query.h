// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_BIN_UFSUTIL_QUERY_H_
#define SRC_DEVICES_BLOCK_BIN_UFSUTIL_QUERY_H_

#include <fidl/fuchsia.hardware.ufs/cpp/wire.h>

#include <string>
#include <variant>

using OptionValue = std::variant<uint32_t, std::string>;

int HandleReadDescriptor(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                         const std::unordered_map<uint32_t, OptionValue>& options);

int HandleWriteDescriptor(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                          const std::unordered_map<uint32_t, OptionValue>& options);

int HandleSetFlag(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                  const std::unordered_map<uint32_t, OptionValue>& options);

int HandleReadFlag(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                   const std::unordered_map<uint32_t, OptionValue>& options);

int HandleToggleFlag(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                     const std::unordered_map<uint32_t, OptionValue>& options);

int HandleClearFlag(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                    const std::unordered_map<uint32_t, OptionValue>& options);

int HandleReadAttribute(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                        const std::unordered_map<uint32_t, OptionValue>& options);

int HandleWriteAttribute(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                         const std::unordered_map<uint32_t, OptionValue>& options);

#endif  // SRC_DEVICES_BLOCK_BIN_UFSUTIL_QUERY_H_
