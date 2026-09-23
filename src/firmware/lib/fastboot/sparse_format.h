// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_LIB_FASTBOOT_SPARSE_FORMAT_H_
#define SRC_FIRMWARE_LIB_FASTBOOT_SPARSE_FORMAT_H_

#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <optional>

namespace fastboot {

/// Returns the unsparsed size of `buffer` if this is an Android sparse image, otherwise returns
/// std::nullopt. `buffer` must have the correct alignment.
std::optional<uint64_t> GetUnsparsedSize(const void* buffer, size_t size);

/// Returns the unsparsed size of `buffer` if this is an Android sparse image, otherwise returns
/// std::nullopt. `buffer` must have the correct alignment.
std::optional<uint64_t> GetUnsparsedSize(const fzl::OwnedVmoMapper& buffer);

/// Returns true if `buffer` is in the Android sparse format, false otherwise.
bool IsSparseFormat(const fzl::OwnedVmoMapper& buffer);

/// Type of callback used to log errors when unsparsing fails.
using UnsparseErrorLogger = int(const char*, ...);

/// Unsparses the sparse file from `src` directly into `dst`. `dst` must be large enough to
/// accommodate the unsparsed payload (see `GetUnsparsedSize`). `fill_buffer` is used to optimize
/// fill chunks and should have a size which is a multiple of 4 bytes.
zx::result<> Unsparse(fzl::OwnedVmoMapper& src, zx::vmo& dst, fzl::OwnedVmoMapper& fill_buffer,
                      UnsparseErrorLogger logger);

}  // namespace fastboot

#endif  // SRC_FIRMWARE_LIB_FASTBOOT_SPARSE_FORMAT_H_
