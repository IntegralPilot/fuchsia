// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_WIRE_OUTGOING_MESSAGE_H_
#define LIB_FIDL_CPP_WIRE_OUTGOING_MESSAGE_H_

#include <lib/fidl/cpp/wire/internal/transport.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fidl/cpp/wire/wire_coding_traits.h>
#include <lib/fidl/cpp/wire_format_metadata.h>
#include <lib/stdcompat/version.h>
#include <zircon/fidl.h>
#include <zircon/types.h>

#include <cstdint>
#include <utility>
#include <vector>
#if defined(__cpp_lib_ranges) && __cpp_lib_ranges >= 201811L
#include <ranges>
#endif

namespace fidl {

namespace internal {

template <typename>
class UnownedEncodedMessageBase;

}  // namespace internal

// |OutgoingMessage| represents a FIDL message on the write path.
//
// This class does not allocate its own memory storage. Instead, users need to
// pass in encoding buffers of sufficient size, which an |OutgoingMessage| will
// borrow until its destruction.
//
// This class takes ownership of handles in the message.
//
// For efficiency, errors are stored inside this object. |Write| operations are
// no-op and return the contained error if the message is in an error state.
class OutgoingMessage : public ::fidl::Status {
 public:
  // Copy and move is disabled for the sake of avoiding double handle close.
  // It is possible to implement the move operations with correct semantics if they are
  // ever needed.
  OutgoingMessage(const OutgoingMessage&) = delete;
  OutgoingMessage(OutgoingMessage&&) = delete;
  OutgoingMessage& operator=(const OutgoingMessage&) = delete;
  OutgoingMessage& operator=(OutgoingMessage&&) = delete;
  OutgoingMessage() = delete;
  ~OutgoingMessage();

  struct InternalIovecConstructorArgs {
    const internal::TransportVTable* transport_vtable;
    zx_channel_iovec_t* iovecs;
    uint32_t num_iovecs;
    uint32_t iovec_capacity;
    fidl_handle_t* handles;
    fidl_handle_metadata_t* handle_metadata;
    uint32_t num_handles;
    uint32_t handle_capacity;
    uint8_t* backing_buffer;
    uint32_t backing_buffer_capacity;
    bool is_transactional;
  };

  // Creates an object which can manage a FIDL message.
  //
  // |args.iovecs|, |args.handles|, and |args.handle_metadata| should contain encoded data up to
  // |args.num_iovecs| and |args.num_handles|.
  //
  // Internal-only function that should not be called outside of the FIDL library.
  static OutgoingMessage Create_InternalMayBreak(InternalIovecConstructorArgs args) {
    return OutgoingMessage(args);
  }

  // Creates an empty outgoing message representing an error.
  //
  // |failure| must contain an error result.
  explicit OutgoingMessage(const ::fidl::Status& failure);

  // Set the txid in the message header.
  //
  // Requires that the message is encoded, and is a transactional message.
  // Requires that there are sufficient bytes to store the header in the buffer.
  void set_txid(zx_txid_t txid) {
    if (!ok()) {
      return;
    }
    ZX_ASSERT(message_.is_transactional);
    ZX_ASSERT(iovec_actual() >= 1 && iovecs()[0].capacity >= sizeof(fidl_message_header_t));
    // The byte buffer is const because the kernel only reads the bytes.
    // const_cast is needed to populate it here.
    static_cast<fidl_message_header_t*>(const_cast<void*>(iovecs()[0].buffer))->txid = txid;
  }

  zx_channel_iovec_t* iovecs() const { return message_.iovecs; }
  uint32_t iovec_actual() const { return message_.num_iovecs; }
  fidl_handle_t* handles() const { return message_.handles; }
  internal::fidl_transport_type transport_type() const { return message_.transport_vtable->type; }
  uint32_t handle_actual() const { return message_.num_handles; }

  template <typename Transport>
  typename Transport::HandleMetadata* handle_metadata() const {
    ZX_ASSERT(Transport::VTable.type == message_.transport_vtable->type);
    return reinterpret_cast<typename Transport::HandleMetadata*>(message_.handle_metadata);
  }

  // Returns the number of bytes in the message.
  uint32_t CountBytes() const;

  // Returns true iff the bytes in this message are identical to the bytes in the argument.
  bool BytesMatch(const OutgoingMessage& other) const;

  // Holds a heap-allocated contiguous copy of the bytes in this message.
  //
  // This owns the allocated buffer and frees it when the object goes out of scope.
  // To create a |CopiedBytes|, use |CopyBytes|.
  class CopiedBytes {
   public:
    CopiedBytes() = default;
    CopiedBytes(CopiedBytes&&) = default;
    CopiedBytes& operator=(CopiedBytes&&) = default;
    CopiedBytes(const CopiedBytes&) = delete;
    CopiedBytes& operator=(const CopiedBytes&) = delete;

    uint8_t* data() { return bytes_.data(); }
    size_t size() const { return bytes_.size(); }
    std::vector<uint8_t>::const_iterator begin() const { return bytes_.begin(); }
    std::vector<uint8_t>::const_iterator end() const { return bytes_.end(); }
    std::vector<uint8_t>::iterator begin() { return bytes_.begin(); }
    std::vector<uint8_t>::iterator end() { return bytes_.end(); }

   private:
    explicit CopiedBytes(const OutgoingMessage& msg);

    std::vector<uint8_t> bytes_;

    friend class OutgoingMessage;
  };

#if defined(__cpp_lib_ranges) && __cpp_lib_ranges >= 201811L
  // Require that CopiedBytes& be usable as an input argument to the std::span
  // range constructor.
  static_assert(std::ranges::contiguous_range<CopiedBytes&>);
#endif

  // Create a heap-allocated contiguous copy of the bytes in this message.
  CopiedBytes CopyBytes() const { return CopiedBytes(*this); }

  // Release the handles to prevent them to be closed by CloseHandles. This method is only useful
  // when interfacing with low-level channel operations which consume the handles.
  void ReleaseHandles() { message_.num_handles = 0; }

  // Writes the message to the |transport|.
  void Write(internal::AnyUnownedTransport transport, WriteOptions options = {});

  // Writes the message to the |transport|. This overload takes a concrete
  // transport endpoint, such as a |zx::unowned_channel|.
  template <typename TransportObject>
  void Write(TransportObject&& transport, WriteOptions options = {}) {
    Write(internal::MakeAnyUnownedTransport(std::forward<TransportObject>(transport)),
          std::move(options));
  }

  // Makes a call and returns the response read from the transport, without
  // decoding.
  template <typename TransportObject>
  auto Call(TransportObject&& transport,
            typename internal::AssociatedTransport<TransportObject>::MessageStorageView storage,
            CallOptions options = {}) {
    return CallImpl(internal::MakeAnyUnownedTransport(std::forward<TransportObject>(transport)),
                    static_cast<internal::MessageStorageViewBase&>(storage), std::move(options));
  }

  bool is_transactional() const { return message_.is_transactional; }

 private:
  void EncodeImpl(fidl::internal::WireFormatVersion wire_format_version, void* data,
                  size_t inline_size, fidl::internal::TopLevelEncodeFn encode_fn);

  uint32_t iovec_capacity() const { return message_.iovec_capacity; }
  uint32_t handle_capacity() const { return message_.handle_capacity; }
  uint32_t backing_buffer_capacity() const { return message_.backing_buffer_capacity; }
  uint8_t* backing_buffer() const { return message_.backing_buffer; }

  explicit OutgoingMessage(InternalIovecConstructorArgs args);

  fidl::IncomingHeaderAndMessage CallImpl(internal::AnyUnownedTransport transport,
                                          internal::MessageStorageViewBase& storage,
                                          CallOptions options);

  using Status::SetStatus;

  InternalIovecConstructorArgs message_;

  template <typename>
  friend class internal::UnownedEncodedMessageBase;
};

}  // namespace fidl

#endif  // LIB_FIDL_CPP_WIRE_OUTGOING_MESSAGE_H_
