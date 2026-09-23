// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_WIRE_MESSAGE_H_
#define LIB_FIDL_CPP_WIRE_MESSAGE_H_

#include <lib/fidl/cpp/features.h>
#include <lib/fidl/cpp/transaction_header.h>
#include <lib/fidl/cpp/wire/decoded_value.h>
#include <lib/fidl/cpp/wire/incoming_message.h>
#include <lib/fidl/cpp/wire/internal/transport.h>
#include <lib/fidl/cpp/wire/message_storage.h>
#include <lib/fidl/cpp/wire/outgoing_message.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fidl/cpp/wire/traits.h>
#include <lib/fidl/cpp/wire/wire_coding_traits.h>
#include <lib/fidl/cpp/wire/wire_types.h>
#include <lib/fidl/cpp/wire_format_metadata.h>
#include <lib/fidl/txn_header.h>
#include <lib/fit/nullable.h>
#include <lib/fit/result.h>
#include <zircon/assert.h>
#include <zircon/fidl.h>

#include <array>
#include <memory>
#include <string>
#include <type_traits>
#include <variant>
#include <vector>

#if __FIDL_SUPPORT_HANDLES
#include <lib/fidl/cpp/wire/internal/endpoints.h>
#include <lib/zx/channel.h>
#endif  // __FIDL_SUPPORT_HANDLES

namespace fidl {

// Reads a transactional message from |transport| using the |storage| as needed.
//
// |storage| must be a subclass of |fidl::internal::MessageStorageViewBase|, and
// is specific to the transport. For example, the Zircon channel transport uses
// |fidl::ChannelMessageStorageView| which points to bytes and handles:
//
//     fidl::IncomingHeaderAndMessage message = fidl::MessageRead(
//         zx::unowned_channel(...),
//         fidl::ChannelMessageStorageView{...});
//
// Error information is embedded in the returned |IncomingHeaderAndMessage| in case of
// failures.
template <typename TransportObject>
IncomingHeaderAndMessage MessageRead(
    TransportObject&& transport,
    typename internal::AssociatedTransport<TransportObject>::MessageStorageView storage,
    const ReadOptions& options) {
  auto type_erased_transport =
      internal::MakeAnyUnownedTransport(std::forward<TransportObject>(transport));
  uint8_t* result_bytes;
  fidl_handle_t* result_handles;
  fidl_handle_metadata_t* result_handle_metadata;
  uint32_t actual_num_bytes = 0u;
  uint32_t actual_num_handles = 0u;
  zx_status_t status =
      type_erased_transport.read(options, internal::ReadArgs{
                                              .storage_view = &storage,
                                              .out_data = reinterpret_cast<void**>(&result_bytes),
                                              .out_handles = &result_handles,
                                              .out_handle_metadata = &result_handle_metadata,
                                              .out_data_actual_count = &actual_num_bytes,
                                              .out_handles_actual_count = &actual_num_handles,
                                          });
  if (status != ZX_OK) {
    return IncomingHeaderAndMessage::Create(fidl::Status::TransportError(status));
  }
  return IncomingHeaderAndMessage(type_erased_transport.vtable(), result_bytes, actual_num_bytes,
                                  result_handles, result_handle_metadata, actual_num_handles);
}

// Overload of |MessageRead| with default options. See other |MessageRead|.
template <typename TransportObject>
IncomingHeaderAndMessage MessageRead(
    TransportObject&& transport,
    typename internal::AssociatedTransport<TransportObject>::MessageStorageView storage) {
  return MessageRead(std::forward<TransportObject>(transport), storage, {});
}

namespace internal {

// This class owns a message of |FidlType| and encodes the message automatically upon construction
// into a byte buffer.
template <typename FidlType, typename Transport = internal::ChannelTransport>
class OwnedEncodedMessage final {
 public:
  explicit OwnedEncodedMessage(FidlType* response)
      : message_(1u, backing_buffer_.data(), static_cast<uint32_t>(backing_buffer_.size()),
                 response) {}
  explicit OwnedEncodedMessage(fidl::internal::WireFormatVersion wire_format_version,
                               FidlType* response)
      : message_(wire_format_version, 1u, backing_buffer_.data(),
                 static_cast<uint32_t>(backing_buffer_.size()), response) {}
  // Internal constructor.
  explicit OwnedEncodedMessage(::fidl::internal::AllowUnownedInputRef allow_unowned,
                               FidlType* response)
      : message_(Transport::kNumIovecs, backing_buffer_.data(),
                 static_cast<uint32_t>(backing_buffer_.size()), response) {}
  explicit OwnedEncodedMessage(::fidl::internal::AllowUnownedInputRef allow_unowned,
                               fidl::internal::WireFormatVersion wire_format_version,
                               FidlType* response)
      : message_(wire_format_version, Transport::kNumIovecs, backing_buffer_.data(),
                 static_cast<uint32_t>(backing_buffer_.size()), response) {}
  OwnedEncodedMessage(const OwnedEncodedMessage&) = delete;
  OwnedEncodedMessage(OwnedEncodedMessage&&) = delete;
  OwnedEncodedMessage* operator=(const OwnedEncodedMessage&) = delete;
  OwnedEncodedMessage* operator=(OwnedEncodedMessage&&) = delete;

  zx_status_t status() const { return message_.status(); }
  const char* status_string() const { return message_.status_string(); }
  bool ok() const { return message_.ok(); }
  std::string FormatDescription() const { return message_.FormatDescription(); }
  const char* lossy_description() const { return message_.lossy_description(); }
  const ::fidl::Status& error() const { return message_.error(); }

  ::fidl::OutgoingMessage& GetOutgoingMessage() { return message_.GetOutgoingMessage(); }

  template <typename TransportObject>
  void Write(TransportObject&& client, WriteOptions options = {}) {
    message_.Write(std::forward<TransportObject>(client), std::move(options));
  }

  ::fidl::WireFormatMetadata wire_format_metadata() const {
    return message_.wire_format_metadata();
  }

 private:
  ::fidl::internal::OutgoingMessageBuffer<FidlType> backing_buffer_;
  ::fidl::internal::UnownedEncodedMessage<FidlType, Transport> message_;
};

}  // namespace internal

// Holds the result of converting an outgoing message to an encoded message.
//
// |fidl::Encode| defers handle rights and type validation to the transport.
// This converter completes the encoding by performing those validation, without
// necessarily writing the message to a transport.
//
// |OutgoingToEncodedMessage| objects own the bytes and handles resulting from
// conversion.
class OutgoingToEncodedMessage {
 public:
  // Converts an outgoing message to an encoded message.
  //
  // The provided |OutgoingMessage| must use the Zircon channel transport.
  // It also must be a non-transactional outgoing message (i.e. from standalone
  // encoding and not from writing a request/response).
  //
  // In doing so, this function will make syscalls to fetch rights and type
  // information of any provided handles. The caller is responsible for ensuring
  // that returned handle rights and object types are checked appropriately.
  //
  // The constructed |OutgoingToEncodedMessage| will take ownership over
  // handles from the input |OutgoingMessage|.
  explicit OutgoingToEncodedMessage(OutgoingMessage& input);

  ~OutgoingToEncodedMessage() = default;

  fidl::EncodedMessage& message() & {
    ZX_DEBUG_ASSERT(ok());
    return encoded_message_;
  }

  [[nodiscard]] fidl::Error error() const {
    ZX_DEBUG_ASSERT(!ok());
    return status_;
  }
  [[nodiscard]] zx_status_t status() const { return status_.status(); }
  [[nodiscard]] bool ok() const { return status_.ok(); }
  [[nodiscard]] std::string FormatDescription() const;

 private:
  static fidl::EncodedMessage ConversionImpl(
      OutgoingMessage& input, OutgoingMessage::CopiedBytes& buf_bytes,
      std::unique_ptr<zx_handle_t[]>& buf_handles,
      std::unique_ptr<fidl_channel_handle_metadata_t[]>& buf_handle_metadata,
      fidl::Status& out_status);

  fidl::Status status_;
  OutgoingMessage::CopiedBytes buf_bytes_;
  std::unique_ptr<zx_handle_t[]> buf_handles_ = {};
  std::unique_ptr<fidl_channel_handle_metadata_t[]> buf_handle_metadata_ = {};
  fidl::EncodedMessage encoded_message_;
};

}  // namespace fidl

#endif  // LIB_FIDL_CPP_WIRE_MESSAGE_H_
