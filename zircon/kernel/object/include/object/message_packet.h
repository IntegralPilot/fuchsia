// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_MESSAGE_PACKET_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_MESSAGE_PACKET_H_

#include <lib/user_copy/user_ptr.h>
#include <stdint.h>
#include <string.h>
#include <zircon/types.h>

#include <cstdint>

#include <fbl/intrusive_double_list.h>
#include <ktl/span.h>
#include <ktl/unique_ptr.h>
#include <object/handle.h>

constexpr uint32_t kMaxMessageSize = 65536u;
constexpr uint32_t kMaxMessageHandles = 64u;
constexpr uint32_t kMaxIovecsCount = 8192u;

// ensure public constants are aligned
static_assert(ZX_CHANNEL_MAX_MSG_BYTES == kMaxMessageSize, "");
static_assert(ZX_CHANNEL_MAX_MSG_HANDLES == kMaxMessageHandles, "");
static_assert(ZX_CHANNEL_MAX_MSG_IOVECS == kMaxIovecsCount, "");

class Handle;
class MessagePacket;
namespace internal {
struct MessagePacketDeleter;
}  // namespace internal

// Definition of a MessagePacket's specific pointer type.  Message packets must
// be managed using this specific type of pointer, because MessagePackets have a
// specific custom deletion requirement.
using MessagePacketPtr = ktl::unique_ptr<MessagePacket, internal::MessagePacketDeleter>;

extern "C" {
zx_status_t rust_message_packet_create_user(uintptr_t data, size_t data_size, size_t num_handles,
                                            MessagePacket** out);
zx_status_t rust_message_packet_create_iovecs(uintptr_t iovecs, size_t num_iovecs,
                                              size_t num_handles, MessagePacket** out);
zx_status_t rust_message_packet_create_kernel(const uint8_t* data, size_t data_size,
                                              size_t num_handles, MessagePacket** out);
void rust_message_packet_delete(MessagePacket* packet);
zx_status_t rust_message_packet_copy_data_to(const MessagePacket* packet, uintptr_t buf);
size_t rust_message_packet_get_data_size(const MessagePacket* packet);
size_t rust_message_packet_get_num_handles(const MessagePacket* packet);
Handle* const* rust_message_packet_get_handles(const MessagePacket* packet);
Handle** rust_message_packet_get_mutable_handles(MessagePacket* packet);
void rust_message_packet_set_owns_handles(MessagePacket* packet, bool owns_handles);
zx_txid_t rust_message_packet_get_txid(const MessagePacket* packet);
void rust_message_packet_set_txid(MessagePacket* packet, zx_txid_t txid);
void rust_message_packet_get_start_of_payload(const MessagePacket* packet, const uint8_t** out_ptr,
                                              size_t* out_len);
}  // extern "C"

class MessagePacket final : public fbl::DoublyLinkedListable<MessagePacketPtr> {
 public:
  static constexpr uint32_t kIovecChunkSize = 16;

  // Creates a message packet containing the provided data and space for
  // |num_handles| handles. The handles array is uninitialized and must
  // be completely overwritten by clients.
  static zx_status_t Create(user_in_ptr<const char> data, uint32_t data_size, uint32_t num_handles,
                            MessagePacketPtr* msg);
  static zx_status_t Create(user_in_ptr<const zx_channel_iovec_t> iovecs, uint32_t num_iovecs,
                            uint32_t num_handles, MessagePacketPtr* msg);
  static zx_status_t Create(const char* data, uint32_t data_size, uint32_t num_handles,
                            MessagePacketPtr* msg);

  uint32_t data_size() const {
    return static_cast<uint32_t>(rust_message_packet_get_data_size(this));
  }

  // Copies the packet's |data_size()| bytes to |buf|.
  // Returns an error if |buf| points to a bad user address.
  zx_status_t CopyDataTo(user_out_ptr<char> buf) const {
    return rust_message_packet_copy_data_to(this, reinterpret_cast<uintptr_t>(buf.get()));
  }

  uint32_t num_handles() const {
    return static_cast<uint32_t>(rust_message_packet_get_num_handles(this));
  }
  Handle* const* handles() const { return rust_message_packet_get_handles(this); }
  Handle** mutable_handles() { return rust_message_packet_get_mutable_handles(this); }

  void set_owns_handles(bool own_handles) {
    rust_message_packet_set_owns_handles(this, own_handles);
  }

  // zx_channel_call treats the leading bytes of the payload as
  // a transaction id of type zx_txid_t.
  zx_txid_t get_txid() const { return rust_message_packet_get_txid(this); }

  void set_txid(zx_txid_t txid) { rust_message_packet_set_txid(this, txid); }

  struct FidlHeader {
    zx_txid_t txid{};
    uint8_t flags[3]{0, 0, 0};
    uint8_t magic{0};
    uint64_t ordinal{0};
  };
  static_assert(sizeof(FidlHeader) == 2 * sizeof(uint64_t));

  FidlHeader fidl_header() const {
    const ktl::span<const uint8_t> payload = start_of_payload();
    if (payload.size() >= sizeof(FidlHeader)) {
      FidlHeader header;
      memcpy(&header, payload.data(), sizeof(header));
      return header;
    }
    return FidlHeader{};
  }

  // The first chunk of payload.
  // Eventually we'd want to actually get the whole message out.
  ktl::span<const uint8_t> start_of_payload() const {
    const uint8_t* ptr = nullptr;
    size_t len = 0;
    rust_message_packet_get_start_of_payload(this, &ptr, &len);
    return ktl::span<const uint8_t>(ptr, len);
  }

 private:
  MessagePacket() = default;
  ~MessagePacket() = default;

  friend struct internal::MessagePacketDeleter;
  static void recycle(MessagePacket* packet) { rust_message_packet_delete(packet); }
};

namespace internal {
struct MessagePacketDeleter {
  void operator()(MessagePacket* packet) const noexcept { MessagePacket::recycle(packet); }
};
}  // namespace internal

inline zx_status_t MessagePacket::Create(user_in_ptr<const char> data, uint32_t data_size,
                                         uint32_t num_handles, MessagePacketPtr* msg) {
  MessagePacket* raw = nullptr;
  zx_status_t status = rust_message_packet_create_user(reinterpret_cast<uintptr_t>(data.get()),
                                                       data_size, num_handles, &raw);
  if (status != ZX_OK) {
    return status;
  }
  msg->reset(raw);
  return ZX_OK;
}

inline zx_status_t MessagePacket::Create(user_in_ptr<const zx_channel_iovec_t> iovecs,
                                         uint32_t num_iovecs, uint32_t num_handles,
                                         MessagePacketPtr* msg) {
  MessagePacket* raw = nullptr;
  zx_status_t status = rust_message_packet_create_iovecs(reinterpret_cast<uintptr_t>(iovecs.get()),
                                                         num_iovecs, num_handles, &raw);
  if (status != ZX_OK) {
    return status;
  }
  msg->reset(raw);
  return ZX_OK;
}

inline zx_status_t MessagePacket::Create(const char* data, uint32_t data_size, uint32_t num_handles,
                                         MessagePacketPtr* msg) {
  MessagePacket* raw = nullptr;
  zx_status_t status = rust_message_packet_create_kernel(reinterpret_cast<const uint8_t*>(data),
                                                         data_size, num_handles, &raw);
  if (status != ZX_OK) {
    return status;
  }
  msg->reset(raw);
  return ZX_OK;
}

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_MESSAGE_PACKET_H_
