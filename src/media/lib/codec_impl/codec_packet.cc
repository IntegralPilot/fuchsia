// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/media/codec_impl/codec_buffer.h>
#include <lib/media/codec_impl/codec_impl.h>
#include <lib/media/codec_impl/codec_packet.h>
#include <stdint.h>

CodecPacket::CodecPacket(uint64_t buffer_lifetime_ordinal, uint32_t packet_index)
    : buffer_lifetime_ordinal_(buffer_lifetime_ordinal),
      allocated_packet_index_(packet_index),
      protocol_packet_index_(packet_index) {
  // nothing else to do here
}

CodecPacket::~CodecPacket() {
  // nothing else to do here
}

void CodecPacket::SetParent(const CodecImpl* parent) { parent_ = parent; }

uint64_t CodecPacket::buffer_lifetime_ordinal() const { return buffer_lifetime_ordinal_; }

uint32_t CodecPacket::packet_index() const {
  // Only the CodecAdapter potentially calls packet_index(), so assert if the CodecAdapter shouldn't
  // be calling packet_index().
  ZX_DEBUG_ASSERT(!parent_->is_supports_dynamic_buffers());
  return allocated_packet_index_;
}

void CodecPacket::SetBuffer(const CodecBuffer* buffer) {
  if (buffer) {
    buffer_keep_alive_ = buffer->GetKeepAlive();
    buffer_ = buffer;
  } else {
    buffer_ = nullptr;
    buffer_keep_alive_.reset();
  }
}

const CodecBuffer* CodecPacket::buffer() const { return buffer_; }

void CodecPacket::SetStartOffset(uint32_t start_offset) { start_offset_ = start_offset; }

bool CodecPacket::has_start_offset() const { return start_offset_ != kStartOffsetNotSet; }

uint32_t CodecPacket::start_offset() const { return start_offset_; }

void CodecPacket::SetValidLengthBytes(uint32_t valid_length_bytes) {
  valid_length_bytes_ = valid_length_bytes;
}

bool CodecPacket::has_valid_length_bytes() const {
  return valid_length_bytes_ != kValidLengthBytesNotSet;
}

uint32_t CodecPacket::valid_length_bytes() const { return valid_length_bytes_; }

void CodecPacket::SetTimstampIsh(uint64_t timestamp_ish) {
  has_timestamp_ish_ = true;
  timestamp_ish_ = timestamp_ish;
}

// 0 is a valid value - it's !has_timestamp_ish_ that actually matters here.
// However, set timestamp_ish_ to 0 anyway just to make it a little more obvious
// that !has_timestamp_ish_.
void CodecPacket::ClearTimestampIsh() {
  has_timestamp_ish_ = false;
  timestamp_ish_ = 0;
}

bool CodecPacket::has_timestamp_ish() const { return has_timestamp_ish_; }

uint64_t CodecPacket::timestamp_ish() const {
  ZX_DEBUG_ASSERT(has_timestamp_ish_ || !timestamp_ish_);
  return timestamp_ish_;
}

void CodecPacket::SetFree(bool is_free) {
  // We shouldn't need to be calling this method unless we're changing the
  // is_free state.
  ZX_DEBUG_ASSERT(is_free_ != is_free);
  is_free_ = is_free;
}

bool CodecPacket::is_free() const { return is_free_; }

void CodecPacket::SetIsNew(bool is_new) { is_new_ = is_new; }

bool CodecPacket::is_new() const { return is_new_; }

void CodecPacket::SetKeyFrame(bool key_frame) {
  key_frame_ = key_frame;
  key_frame_is_set_ = true;
}
void CodecPacket::ClearKeyFrame() { key_frame_is_set_ = true; }
bool CodecPacket::has_key_frame() const { return key_frame_is_set_; }
bool CodecPacket::key_frame() const { return key_frame_; }

void CodecPacket::CacheFlush() const { buffer()->CacheFlush(start_offset_, valid_length_bytes_); }

void CodecPacket::CacheFlushAndInvalidate() const {
  buffer()->CacheFlushAndInvalidate(start_offset_, valid_length_bytes_);
}

void CodecPacket::ClearStartOffset() { start_offset_ = kStartOffsetNotSet; }

void CodecPacket::ClearValidLengthBytes() { valid_length_bytes_ = kValidLengthBytesNotSet; }

void CodecPacket::SetProtocolPacketIndex(uint32_t protocol_packet_index) {
  ZX_DEBUG_ASSERT(is_free_);
  ZX_DEBUG_ASSERT(!protocol_packet_index_.has_value());
  protocol_packet_index_ = protocol_packet_index;
}
void CodecPacket::ClearProtocolPacketIndex() {
  ZX_DEBUG_ASSERT(is_free_);
  ZX_DEBUG_ASSERT(protocol_packet_index_.has_value());
  protocol_packet_index_.reset();
}
uint32_t CodecPacket::protocol_packet_index() const {
  ZX_DEBUG_ASSERT(protocol_packet_index_.has_value());
  return *protocol_packet_index_;
}

uint32_t CodecPacket::allocated_packet_index() const { return allocated_packet_index_; }
