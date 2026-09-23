// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_IPC_MESSAGE_READER_H_
#define SRC_DEVELOPER_DEBUG_IPC_MESSAGE_READER_H_

#include <stdint.h>

#include <cstdint>
#include <string>
#include <vector>

#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/shared/serialization.h"

namespace debug_ipc {

class MessageReader : public Serializer {
 public:
  MessageReader(std::vector<char> message, uint32_t version)
      : message_(std::move(message)), version_(version) {}

  bool has_error() const { return has_error_; }

  // Returns the number of bytes available still to read.
  size_t remaining() const { return message_.size() - offset_; }
  size_t message_size() const { return message_.size(); }

  // Implement |Serializer|.
  uint32_t GetVersion() const override { return version_; }
  // Although it's called "SerializeBytes", it's actually "DeserializeBytes".
  void SerializeBytes(void* data, uint32_t len) override;

 private:
  const std::vector<char> message_;

  uint32_t version_ = 0;

  size_t offset_ = 0;  // Current read offset.

  bool has_error_ = false;
};

// MsgType can be either Request or Reply types.
template <typename MsgType>
bool Deserialize(std::vector<char> data, MsgType* msg, uint32_t* transaction_id, uint32_t version)
  requires IsDebugIpcMessageType<MsgType>
{
  MessageReader reader(std::move(data), version);
  MsgHeader header;
  reader | header | *msg;
  *transaction_id = header.transaction_id;
  return !reader.has_error();
}

template <typename NotificationType>
bool Deserialize(std::vector<char> data, NotificationType* notify, uint32_t version)
  requires IsDebugIpcNotificationType<NotificationType>
{
  MessageReader reader(std::move(data), version);
  MsgHeader header;
  reader | header | *notify;
  return !reader.has_error();
}

}  // namespace debug_ipc

#endif  // SRC_DEVELOPER_DEBUG_IPC_MESSAGE_READER_H_
