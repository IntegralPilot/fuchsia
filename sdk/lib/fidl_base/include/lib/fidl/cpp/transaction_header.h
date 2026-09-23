// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_TRANSACTION_HEADER_H_
#define LIB_FIDL_CPP_TRANSACTION_HEADER_H_

#include <lib/fidl/txn_header.h>

#include <type_traits>

namespace fidl {

// Defines the possible flag values for the FIDL transaction header
// dynamic_flags field.
struct MessageDynamicFlags {
  uint8_t value;

  static const MessageDynamicFlags kStrictMethod;
  static const MessageDynamicFlags kFlexibleMethod;
  // TODO(https://fxbug.dev/42065546): this feature is not yet enabled for C++ bindings - do not set this flag
  // outside of testing contexts!
  static const MessageDynamicFlags kByteOverflow;
};

inline constexpr MessageDynamicFlags MessageDynamicFlags::kStrictMethod = {
    FIDL_MESSAGE_HEADER_DYNAMIC_FLAGS_STRICT_METHOD};
inline constexpr MessageDynamicFlags MessageDynamicFlags::kFlexibleMethod = {
    FIDL_MESSAGE_HEADER_DYNAMIC_FLAGS_FLEXIBLE_METHOD};
inline constexpr MessageDynamicFlags MessageDynamicFlags::kByteOverflow = {
    FIDL_MESSAGE_HEADER_DYNAMIC_FLAGS_BYTE_OVERFLOW};

inline MessageDynamicFlags operator|(MessageDynamicFlags lhs, MessageDynamicFlags rhs) {
  return {static_cast<uint8_t>(lhs.value | rhs.value)};
}

inline void InitTxnHeader(fidl_message_header_t* out_hdr, zx_txid_t txid, uint64_t ordinal,
                          MessageDynamicFlags dynamic_flags) {
  fidl_init_txn_header(out_hdr, txid, ordinal, dynamic_flags.value);
}

// Returns true if the transaction header is for a flexible interaction.
inline bool IsFlexibleInteraction(const fidl_message_header_t* hdr) {
  return (hdr->dynamic_flags & FIDL_MESSAGE_HEADER_DYNAMIC_FLAGS_FLEXIBLE_METHOD) != 0;
}

// Returns true if the transaction header is for a large message.
inline bool IsByteOverflow(const fidl_message_header_t* hdr) {
  return (hdr->dynamic_flags & FIDL_MESSAGE_HEADER_DYNAMIC_FLAGS_BYTE_OVERFLOW) != 0;
}

}  // namespace fidl

#endif  // LIB_FIDL_CPP_TRANSACTION_HEADER_H_
