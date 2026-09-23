// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_CODEC_PACKET_H_
#define SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_CODEC_PACKET_H_

#include <fuchsia/media/cpp/fidl.h>
#include <lib/media/codec_impl/codec_buffer.h>
#include <stdint.h>

#include <limits>
#include <memory>

#include <fbl/macros.h>

class CodecImpl;
class CodecPacketForTest;

// Instances of this class are 1:1 with fuchsia::media::Packet.
class CodecPacket {
 public:
  ~CodecPacket();

  uint64_t buffer_lifetime_ordinal() const;

  // This will assert in debug if is_supports_dynamic_buffers_ true.
  //
  // This is deprecated. CodecAdapter(s) should not call this, and should also
  // not need to call protocol_packet_index or allocated_packet_index (both are
  // private). The CodecPacket* itself is how CodecImpl and CodecAdapter
  // communicate regarding a CodecPacket.
  uint32_t packet_index() const;

  void SetBuffer(const CodecBuffer* buffer);
  const CodecBuffer* buffer() const;

  void SetStartOffset(uint32_t start_offset);
  bool has_start_offset() const;
  uint32_t start_offset() const;

  void SetValidLengthBytes(uint32_t valid_length_bytes);
  bool has_valid_length_bytes() const;
  uint32_t valid_length_bytes() const;

  void SetTimstampIsh(uint64_t timestamp_ish);
  // Sets timestamp_ish() to kTimestampIshNotSet, which also causes
  // has_timestamp_ish() to return false.
  void ClearTimestampIsh();
  bool has_timestamp_ish() const;
  uint64_t timestamp_ish() const;

  // from CodecImpl / protocol point of view; CodecAdapter's point of view can be different for
  // short time intervals
  void SetFree(bool is_free);
  bool is_free() const;

  void SetIsNew(bool is_new);
  bool is_new() const;

  void SetKeyFrame(bool key_frame);
  void ClearKeyFrame();
  bool has_key_frame() const;
  bool key_frame() const;

  void CacheFlush() const;
  void CacheFlushAndInvalidate() const;

  // The rest is protected for the benefit of tests. Sub-classes outside tests
  // are not supported.
 protected:
  // The public section is for the core codec to call - the private section is
  // only for CodecImpl to call.
  friend class CodecImpl;
  friend class CodecPacketForTest;

  static constexpr uint32_t kStartOffsetNotSet = std::numeric_limits<uint32_t>::max();
  static constexpr uint32_t kValidLengthBytesNotSet = std::numeric_limits<uint32_t>::max();

  // The buffer ptr is not owned.  The buffer lifetime is slightly longer than
  // the Packet lifetime.
  CodecPacket(uint64_t buffer_lifetime_ordinal, uint32_t packet_index);

  // This is separate from the constructor because some core codec tests use
  // CodecPacket but don't have a CodecImpl, and there's no compelling reason to
  // create a CodecPacketOwner interface.
  void SetParent(const CodecImpl* parent);

  void ClearStartOffset();
  void ClearValidLengthBytes();

  // This is intentionally private so that the CodecAdapter can't call this.
  //
  // CodecImpl handles the protocol packet_index and ensures that the
  // CodecAdapter can track packets by CodecPacket*, without worrying about the
  // protocol packet_index.
  //
  // This is only valid to call when is_free() true, and this will assert in
  // debug if is_free() false.
  //
  // In general this value can change as a CodecPacket is associated with
  // different protocol packet_index values over time.
  void SetProtocolPacketIndex(uint32_t protocol_packet_index);
  void ClearProtocolPacketIndex();
  // Until SetProtocolPacketIndex() is called the first time, this will be the
  // same value as allocated_packet_index().
  uint32_t protocol_packet_index() const;

  // This is intentionally private so that the CodecAdapter can't call this.
  //
  // This is the index within the vector in CodecImpl.active_packets_ of this
  // CodecPacket. In general this is not the same as protocol_packet_index().
  //
  // This value doesn't change for a given constructed CodecPacket.
  uint32_t allocated_packet_index() const;

  const CodecImpl* parent_ = nullptr;

  const uint64_t buffer_lifetime_ordinal_ = 0;

  const uint32_t allocated_packet_index_ = 0;
  std::optional<uint32_t> protocol_packet_index_ = 0;

  // From CodecImpl's point of view, the buffer_ is meaningful only while a
  // packet_index is in-flight, not while the packet_index is free from
  // CodecImpl's point of view (per is_free()).
  //
  // A CodecAdapter can optionally ensure this is nullptr when a packet becomes
  // free from the CodecAdapter's point of view (during handling of
  // CoreCodecRecycleOutputPacket, whether the handling is sync or async).
  //
  // A CodecAdapter can rely on this being nullptr at the start of the first
  // CoreCodecRecycleOutputPacket call for this packet.
  const CodecBuffer* buffer_ = nullptr;
  std::optional<CodecBuffer::KeepAlive> buffer_keep_alive_;

  uint32_t start_offset_ = kStartOffsetNotSet;
  uint32_t valid_length_bytes_ = kValidLengthBytesNotSet;

  // Allow all timestamp_ish values to be valid by carying valid bool
  // separately.
  bool has_timestamp_ish_ = false;
  uint64_t timestamp_ish_ = 0;

  // is_free_
  //
  // This is tracked by the Codec server, not provided by the Codec client.
  //
  // True means free at protocol level.  False means in-flight at protocol
  // level.  This is used to check for nonsense from the client.
  //
  // When CodecPacket doesn't exist, that corresponds to packet not allocated at
  // the protocol level.
  //
  // An input packet starts out free with the client, and and output packet
  // starts out free with the codec server.  Either way, it starts free.
  bool is_free_ = true;

  // Starts true when a packet is truly new.  In addition, a CodecAdapter may
  // set this back to true whenever the packet is logically new from the
  // CodecAdapter's point of view.  This allows for the CodecAdapter to
  // determine whether to recycle a packet to the core codec depending on
  // whether the packet is new or not, on first call to
  // CoreCodecRecycleOutputPacket().  Some core codecs potentially want an
  // internal recycle call or equivalent for new packets, while others don't
  // (such as amlogic-video).
  bool is_new_ = true;

  // Set to true if this packet is part of a key frame.
  bool key_frame_ = false;
  bool key_frame_is_set_ = false;

  CodecPacket() = delete;
  DISALLOW_COPY_ASSIGN_AND_MOVE(CodecPacket);
};

#endif  // SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_CODEC_PACKET_H_
