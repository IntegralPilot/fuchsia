// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fidl/cpp/features.h>
#include <lib/fidl/cpp/wire/incoming_message.h>
#include <lib/fidl/txn_header.h>

#if __FIDL_SUPPORT_HANDLES
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#else
#include <lib/fidl/cpp/wire/internal/transport_channel_host.h>
#endif  // __FIDL_SUPPORT_HANDLES

namespace fidl {

namespace {

struct fidl_incoming_msg_impl {
  cpp20::span<uint8_t> bytes;
  fidl_handle_t* handles;
  fidl_handle_metadata_t* handle_metadata;
  uint32_t num_handles;
};
static_assert(sizeof(fidl_incoming_msg_impl) == sizeof(fidl_incoming_msg_t));
static_assert(alignof(fidl_incoming_msg_impl) == alignof(fidl_incoming_msg_t));

}  // namespace

EncodedMessage EncodedMessage::Create(cpp20::span<uint8_t> bytes) {
  return EncodedMessage(nullptr, bytes, nullptr, nullptr, 0);
}

EncodedMessage EncodedMessage::Create(cpp20::span<uint8_t> bytes, zx_handle_t* handles,
                                      fidl_channel_handle_metadata_t* handle_metadata,
                                      uint32_t handle_actual) {
  return EncodedMessage(&internal::ChannelTransport::VTable, bytes, handles,
                        reinterpret_cast<fidl_handle_metadata_t*>(handle_metadata), handle_actual);
}

std::pair<cpp20::span<uint8_t>, cpp20::span<fidl_handle_t>> EncodedMessage::Release() && {
  ZX_ASSERT(transport_vtable_->type == internal::fidl_transport_type::kChannel);
  cpp20::span bytes = this->bytes();
  cpp20::span handles{handles_, num_handles_};
  std::move(*this).ReleaseHandles();
  return {bytes, handles};
}

EncodedMessage::~EncodedMessage() { std::move(*this).CloseHandles(); }

void EncodedMessage::CloseHandles() && {
  if (transport_vtable_) {
    transport_vtable_->encoding_configuration->close_many(handles(), num_handles());
  }
  std::move(*this).ReleaseHandles();
}

EncodedMessage::EncodedMessage(const internal::TransportVTable* transport_vtable,
                               cpp20::span<uint8_t> bytes, fidl_handle_t* handles,
                               fidl_handle_metadata_t* handle_metadata, uint32_t handle_actual)
    : transport_vtable_(transport_vtable),
      bytes_(bytes),
      handles_(handles),
      num_handles_(handle_actual),
      handle_metadata_(handle_metadata) {
  ZX_DEBUG_ASSERT(bytes.size() < std::numeric_limits<uint32_t>::max());
}

IncomingHeaderAndMessage IncomingHeaderAndMessage::FromEncodedCMessage(
    const fidl_incoming_msg_t& c_msg) {
  const auto& msg = reinterpret_cast<const fidl_incoming_msg_impl&>(c_msg);
  ZX_DEBUG_ASSERT(msg.bytes.size() >= sizeof(fidl_message_header_t));
  return IncomingHeaderAndMessage(&internal::ChannelTransport::VTable, msg.bytes.data(),
                                  static_cast<uint32_t>(msg.bytes.size()), msg.handles,
                                  msg.handle_metadata, msg.num_handles);
}

IncomingHeaderAndMessage::~IncomingHeaderAndMessage() = default;

fidl_epitaph_t* IncomingHeaderAndMessage::maybe_epitaph() const {
  ZX_DEBUG_ASSERT(ok());
  if (unlikely(header()->ordinal == kFidlOrdinalEpitaph)) {
    return reinterpret_cast<fidl_epitaph_t*>(bytes());
  }
  return nullptr;
}

fidl_incoming_msg_t IncomingHeaderAndMessage::ReleaseToEncodedCMessage() && {
  ZX_DEBUG_ASSERT_MSG(status() == ZX_OK, "%s", status_string());
  fidl_handle_t* handles = body_.handles();
  uint32_t num_handles = body_.num_handles();
  fidl_handle_metadata_t* handle_metadata = body_.raw_handle_metadata();
  std::move(body_).ReleaseHandles();
  fidl_incoming_msg_t msg;
  reinterpret_cast<fidl_incoming_msg_impl&>(msg) = {
      .bytes = bytes_,
      .handles = handles,
      .handle_metadata = handle_metadata,
      .num_handles = num_handles,
  };
  return msg;
}

void IncomingHeaderAndMessage::CloseHandles() && { std::move(body_).CloseHandles(); }

EncodedMessage IncomingHeaderAndMessage::SkipTransactionHeader() && { return std::move(body_); }

IncomingHeaderAndMessage::IncomingHeaderAndMessage(const fidl::Status& failure)
    : fidl::Status(failure), body_(EncodedMessage::Create({})) {
  ZX_DEBUG_ASSERT(failure.status() != ZX_OK);
}

IncomingHeaderAndMessage::IncomingHeaderAndMessage(
    const internal::TransportVTable* transport_vtable, uint8_t* bytes, uint32_t byte_actual,
    fidl_handle_t* handles, fidl_handle_metadata_t* handle_metadata, uint32_t handle_actual)
    : fidl::Status(fidl::Status::Ok()),
      bytes_(cpp20::span{bytes, byte_actual}),
      body_(bytes_.size() >= sizeof(fidl_message_header_t)
                ? EncodedMessage(transport_vtable, bytes_.subspan(sizeof(fidl_message_header_t)),
                                 handles, handle_metadata, handle_actual)
                : EncodedMessage::Create({})) {
  ValidateHeader();
}

void IncomingHeaderAndMessage::ValidateHeader() {
  if (byte_actual() < sizeof(fidl_message_header_t)) {
    return SetStatus(fidl::Status::UnexpectedMessage(ZX_ERR_INVALID_ARGS,
                                                     ::fidl::internal::kErrorInvalidHeader));
  }

  auto* hdr = header();
  zx_status_t status = fidl_validate_txn_header(hdr);
  if (status != ZX_OK) {
    return SetStatus(
        fidl::Status::UnexpectedMessage(status, ::fidl::internal::kErrorInvalidHeader));
  }

  // See
  // https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0053_epitaphs?hl=en#wire_format
  if (unlikely(maybe_epitaph())) {
    if (hdr->txid != 0) {
      return SetStatus(fidl::Status::UnexpectedMessage(ZX_ERR_INVALID_ARGS,
                                                       ::fidl::internal::kErrorInvalidHeader));
    }
  }
}

}  // namespace fidl
