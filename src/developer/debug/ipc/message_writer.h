// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_IPC_MESSAGE_WRITER_H_
#define SRC_DEVELOPER_DEBUG_IPC_MESSAGE_WRITER_H_

#include <stdint.h>

#include <cstdint>
#include <vector>

#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/shared/serialization.h"

namespace debug_ipc {

// Provides a simple means to append to a dynamic buffer different types of
// data.
//
// The first 4 bytes of each message is the message size. It's assumed that
// these bytes will be explicitly written to. Normally a message will start
// with a struct which contains space for this explicitly.
class MessageWriter : public Serializer {
 public:
  // |initial_size| is a hint for the initial size of the message.
  MessageWriter(uint32_t version, size_t initial_size) : version_(version) {
    buffer_.reserve(initial_size);
  }

  size_t current_length() const { return buffer_.size(); }

  // Writes the size of the current buffer to the first 4 bytes, and
  // destructively returns the buffer.
  std::vector<char> MessageComplete();

  // Implement |Serializer|.
  uint32_t GetVersion() const override { return version_; }
  void SerializeBytes(void* data, uint32_t len) override;

 private:
  uint32_t version_;
  std::vector<char> buffer_;
};

namespace internal {

template <typename MsgType>
bool IsSupported(uint32_t version)
  requires requires(MsgType t) {
    { t.kSupportedSinceVersion };
  }
{
  return version >= MsgType::kSupportedSinceVersion;
}

// The message types that were part of the protocol before we supported different versions do not
// have the kSupportedSinceVersion field and are supported by all protocol versions.
template <typename>
bool IsSupported(...) {
  return true;
}

template <typename MsgType>
constexpr MsgHeader::Type GetTypeForMsg(const MsgType& msg) {
#define FN(msg_type)                                        \
  if constexpr (std::same_as<MsgType, msg_type##Request> || \
                std::same_as<MsgType, msg_type##Reply>) {   \
    return MsgHeader::Type::k##msg_type;                    \
  }

  FOR_EACH_REQUEST_TYPE(FN)
#undef FN

#define FN(notification_type)                               \
  if constexpr (std::same_as<MsgType, notification_type>) { \
    return MsgHeader::Type::k##notification_type;           \
  }

  FOR_EACH_NOTIFICATION_TYPE(FN)
#undef FN

  return MsgHeader::Type::kNone;
}

}  // namespace internal

template <typename MsgType>
std::vector<char> Serialize(const MsgType& request, uint32_t transaction_id, uint32_t version)
  requires IsDebugIpcMessageType<MsgType>
{
  if (!internal::IsSupported<MsgType>(version)) {
    return {};
  }

  MsgHeader header{0, internal::GetTypeForMsg(request), transaction_id};
  MessageWriter writer(version, sizeof(header) + sizeof(request));
  writer | header | const_cast<MsgType&>(request);
  return writer.MessageComplete();
}

template <typename NotificationType>
std::vector<char> Serialize(const NotificationType& notify, uint32_t version)
  requires IsDebugIpcNotificationType<NotificationType>
{
  if (!internal::IsSupported<NotificationType>(version)) {
    return {};
  }
  MsgHeader header{0, internal::GetTypeForMsg(notify), 0};
  MessageWriter writer(version, sizeof(header) + sizeof(notify));
  writer | header | const_cast<NotificationType&>(notify);
  return writer.MessageComplete();
}

}  // namespace debug_ipc

#endif  // SRC_DEVELOPER_DEBUG_IPC_MESSAGE_WRITER_H_
