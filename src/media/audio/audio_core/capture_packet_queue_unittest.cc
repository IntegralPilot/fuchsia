// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/capture_packet_queue.h"

#include <lib/syslog/cpp/macros.h>

#include <atomic>
#include <thread>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

using ASF = fuchsia::media::AudioSampleFormat;
using StreamPacket = fuchsia::media::StreamPacket;

namespace media::audio {

namespace {
static constexpr auto kFrameRate = 48000;
static const auto kFormat = Format::Create<ASF::SIGNED_16>(1, kFrameRate).value();
static const auto kBytesPerFrame = kFormat.bytes_per_frame();
}  // namespace

class CapturePacketQueueTest : public ::testing::Test {
 public:
  void CreateMapper(size_t frames) {
    auto status =
        payload_buffer_.CreateAndMap(frames * kBytesPerFrame, /*flags=*/0, nullptr, &payload_vmo_);
    FX_CHECK(status == ZX_OK) << status;
    payload_start_ = static_cast<char*>(payload_buffer_.start());
  }

  using Packet = CapturePacketQueue::Packet;
  void ExpectPacket(fbl::RefPtr<Packet> got, StreamPacket want) {
    EXPECT_EQ(got->stream_packet().payload_buffer_id, want.payload_buffer_id);
    EXPECT_EQ(got->stream_packet().payload_offset, want.payload_offset);
    EXPECT_EQ(got->stream_packet().payload_size, want.payload_size);
  }

  void PopAndExpectPacketAtOffset(CapturePacketQueue* pq, size_t want_offset_bytes,
                                  size_t want_size_bytes) {
    auto mix_state = pq->NextMixerJob();
    ASSERT_TRUE(mix_state.has_value());
    EXPECT_EQ(mix_state->target, payload_start_ + want_offset_bytes);
    EXPECT_EQ(mix_state->frames, want_size_bytes / kBytesPerFrame);
    ASSERT_EQ(CapturePacketQueue::PacketMixStatus::Done, pq->FinishMixerJob(*mix_state));
    ASSERT_EQ(pq->ReadySize(), 1u);
    auto p = pq->PopReady();
    ExpectPacket(p, {
                        .payload_buffer_id = 0,
                        .payload_offset = want_offset_bytes,
                        .payload_size = want_size_bytes,
                    });
    if (p->callback()) {
      p->callback()(p->stream_packet());
    }
  }

  zx::vmo payload_vmo_;
  fzl::VmoMapper payload_buffer_;
  char* payload_start_ = nullptr;
};

TEST_F(CapturePacketQueueTest, PreallocatedFramesFitPerfectly) {
  CreateMapper(40);
  auto result = CapturePacketQueue::CreatePreallocated(payload_buffer_, kFormat, 10);
  ASSERT_TRUE(result.is_ok()) << result.error();

  const auto kBytesPerPacket = 10 * kBytesPerFrame;
  auto pq = result.take_value();

  {
    SCOPED_TRACE("pop #1");
    ASSERT_EQ(pq->PendingSize(), 4u);
    PopAndExpectPacketAtOffset(pq.get(), 0 * kBytesPerPacket, kBytesPerPacket);
  }

  {
    SCOPED_TRACE("pop #2");
    ASSERT_EQ(pq->PendingSize(), 3u);
    PopAndExpectPacketAtOffset(pq.get(), 1 * kBytesPerPacket, kBytesPerPacket);
  }

  {
    SCOPED_TRACE("pop #3");
    ASSERT_EQ(pq->PendingSize(), 2u);
    PopAndExpectPacketAtOffset(pq.get(), 2 * kBytesPerPacket, kBytesPerPacket);
  }

  {
    SCOPED_TRACE("pop #4");
    ASSERT_EQ(pq->PendingSize(), 1u);
    PopAndExpectPacketAtOffset(pq.get(), 3 * kBytesPerPacket, kBytesPerPacket);
  }

  ASSERT_TRUE(pq->empty());
  ASSERT_EQ(pq->PendingSize(), 0u);
  ASSERT_EQ(pq->ReadySize(), 0u);
}

TEST_F(CapturePacketQueueTest, PreallocatedFramesLeftover) {
  CreateMapper(40);
  auto result = CapturePacketQueue::CreatePreallocated(payload_buffer_, kFormat, 15);
  ASSERT_TRUE(result.is_ok()) << result.error();

  const auto kBytesPerPacket = 15 * kBytesPerFrame;
  auto pq = result.take_value();

  // 40 frames in the payload, 15 frames per packet, so the packets have
  // frames [0,14] and [15,29].
  {
    SCOPED_TRACE("pop #1");
    ASSERT_EQ(pq->PendingSize(), 2u);
    PopAndExpectPacketAtOffset(pq.get(), 0 * kBytesPerPacket, kBytesPerPacket);
  }

  {
    SCOPED_TRACE("pop #2");
    ASSERT_EQ(pq->PendingSize(), 1u);
    PopAndExpectPacketAtOffset(pq.get(), 1 * kBytesPerPacket, kBytesPerPacket);
  }

  ASSERT_TRUE(pq->empty());
  ASSERT_EQ(pq->PendingSize(), 0u);
  ASSERT_EQ(pq->ReadySize(), 0u);
}

TEST_F(CapturePacketQueueTest, PreallocatedMixStatePreserved) {
  CreateMapper(20);
  auto result = CapturePacketQueue::CreatePreallocated(payload_buffer_, kFormat, 10);
  ASSERT_TRUE(result.is_ok()) << result.error();

  const auto kBytesPerPacket = 10 * kBytesPerFrame;
  auto pq = result.take_value();

  ASSERT_EQ(pq->PendingSize(), 2u);
  auto mix_state = pq->NextMixerJob().value();
  mix_state.capture_timestamp = 99;
  mix_state.flags = 1;
  ASSERT_EQ(CapturePacketQueue::PacketMixStatus::Done, pq->FinishMixerJob(mix_state));

  ASSERT_EQ(pq->ReadySize(), 1u);
  ExpectPacket(pq->PopReady(), {
                                   .pts = 99,
                                   .payload_buffer_id = 0,
                                   .payload_offset = 0,
                                   .payload_size = kBytesPerPacket,
                                   .flags = 1,
                               });
}

TEST_F(CapturePacketQueueTest, PreallocatedPartialMix) {
  CreateMapper(20);
  auto result = CapturePacketQueue::CreatePreallocated(payload_buffer_, kFormat, 10);
  ASSERT_TRUE(result.is_ok()) << result.error();

  auto pq = result.take_value();

  {
    SCOPED_TRACE("partial mix");
    ASSERT_EQ(pq->PendingSize(), 2u);
    auto mix_state = pq->NextMixerJob().value();
    EXPECT_EQ(mix_state.target, payload_start_ + 0);
    EXPECT_EQ(mix_state.frames, 10u);
    mix_state.capture_timestamp = 99;
    mix_state.flags = 1;
    mix_state.frames = 6;
    ASSERT_EQ(CapturePacketQueue::PacketMixStatus::Partial, pq->FinishMixerJob(mix_state));
    ASSERT_EQ(pq->ReadySize(), 0u);
  }

  {
    SCOPED_TRACE("finish mix");
    ASSERT_EQ(pq->PendingSize(), 2u);
    auto mix_state = pq->NextMixerJob().value();
    EXPECT_EQ(mix_state.capture_timestamp, 99);
    EXPECT_EQ(mix_state.flags, 1u);
    EXPECT_EQ(mix_state.target, payload_start_ + 6 * kBytesPerFrame);
    EXPECT_EQ(mix_state.frames, 4u);
    ASSERT_EQ(CapturePacketQueue::PacketMixStatus::Done, pq->FinishMixerJob(mix_state));
    ASSERT_EQ(pq->ReadySize(), 1u);
  }
}

TEST_F(CapturePacketQueueTest, PreallocatedDiscardedMix) {
  CreateMapper(20);
  auto result = CapturePacketQueue::CreatePreallocated(payload_buffer_, kFormat, 10);
  ASSERT_TRUE(result.is_ok()) << result.error();

  const auto kBytesPerPacket = 10 * kBytesPerFrame;
  auto pq = result.take_value();

  ASSERT_EQ(pq->PendingSize(), 2u);
  auto mix_state = pq->NextMixerJob().value();
  EXPECT_EQ(mix_state.target, payload_start_ + 0);
  EXPECT_EQ(mix_state.frames, 10u);

  // Before completing this mix, discard all pending packets.
  pq->DiscardPendingPackets();
  ASSERT_EQ(pq->ReadySize(), 2u);
  ExpectPacket(pq->PopReady(), {
                                   .payload_buffer_id = 0,
                                   .payload_offset = 0 * kBytesPerPacket,
                                   .payload_size = 0,
                               });
  ExpectPacket(pq->PopReady(), {
                                   .payload_buffer_id = 0,
                                   .payload_offset = 1 * kBytesPerPacket,
                                   .payload_size = 0,
                               });

  EXPECT_EQ(pq->ReadySize(), 0u);
  EXPECT_EQ(CapturePacketQueue::PacketMixStatus::Discarded, pq->FinishMixerJob(mix_state));
}

TEST_F(CapturePacketQueueTest, PreallocatedDiscardedAfterPartialMix) {
  CreateMapper(20);
  auto result = CapturePacketQueue::CreatePreallocated(payload_buffer_, kFormat, 10);
  ASSERT_TRUE(result.is_ok()) << result.error();

  const auto kBytesPerPacket = 10 * kBytesPerFrame;
  auto pq = result.take_value();

  // Partial mix.
  ASSERT_EQ(pq->PendingSize(), 2u);
  auto mix_state = pq->NextMixerJob().value();
  EXPECT_EQ(mix_state.target, payload_start_ + 0);
  EXPECT_EQ(mix_state.frames, 10u);
  mix_state.frames = 6;
  EXPECT_EQ(CapturePacketQueue::PacketMixStatus::Partial, pq->FinishMixerJob(mix_state));

  // Second mix.
  mix_state = pq->NextMixerJob().value();
  EXPECT_EQ(mix_state.target, payload_start_ + 6 * kBytesPerFrame);
  EXPECT_EQ(mix_state.frames, 4u);

  // Before completing this mix, discard all pending packets.
  EXPECT_EQ(pq->PendingSize(), 2u);
  ASSERT_EQ(pq->ReadySize(), 0u);
  pq->DiscardPendingPackets();
  EXPECT_EQ(pq->PendingSize(), 0u);
  ASSERT_EQ(pq->ReadySize(), 2u);
  ExpectPacket(pq->PopReady(), {
                                   .payload_buffer_id = 0,
                                   .payload_offset = 0 * kBytesPerPacket,
                                   .payload_size = 6 * kBytesPerFrame,  // this was partially mixed
                               });
  ExpectPacket(pq->PopReady(), {
                                   .payload_buffer_id = 0,
                                   .payload_offset = 1 * kBytesPerPacket,
                                   .payload_size = 0,
                               });

  EXPECT_EQ(pq->ReadySize(), 0u);
  EXPECT_EQ(CapturePacketQueue::PacketMixStatus::Discarded, pq->FinishMixerJob(mix_state));
}

TEST_F(CapturePacketQueueTest, PreallocatedRecycle) {
  CreateMapper(20);
  auto result = CapturePacketQueue::CreatePreallocated(payload_buffer_, kFormat, 10);
  ASSERT_TRUE(result.is_ok()) << result.error();

  const auto kBytesPerPacket = 10 * kBytesPerFrame;
  auto pq = result.take_value();

  {
    SCOPED_TRACE("pop and recycle #1");
    ASSERT_EQ(pq->PendingSize(), 2u);
    auto mix_state = pq->NextMixerJob().value();
    ASSERT_EQ(CapturePacketQueue::PacketMixStatus::Done, pq->FinishMixerJob(mix_state));

    ASSERT_EQ(pq->ReadySize(), 1u);
    auto p = pq->PopReady();
    ExpectPacket(p, {
                        .payload_buffer_id = 0,
                        .payload_offset = 0,
                        .payload_size = kBytesPerPacket,
                    });

    ASSERT_EQ(pq->PendingSize(), 1u);
    auto result = pq->Recycle(p->stream_packet());
    ASSERT_TRUE(result.is_ok()) << result.error();
  }

  {
    SCOPED_TRACE("pop #2");
    ASSERT_EQ(pq->PendingSize(), 2u);
    PopAndExpectPacketAtOffset(pq.get(), 1 * kBytesPerPacket, kBytesPerPacket);
  }

  {
    SCOPED_TRACE("pop #1 again");
    ASSERT_EQ(pq->PendingSize(), 1u);
    PopAndExpectPacketAtOffset(pq.get(), 0 * kBytesPerPacket, kBytesPerPacket);
  }
}

TEST_F(CapturePacketQueueTest, PreallocatedRecycleErrors) {
  CreateMapper(20);
  auto result = CapturePacketQueue::CreatePreallocated(payload_buffer_, kFormat, 10);
  ASSERT_TRUE(result.is_ok()) << result.error();

  const auto kBytesPerPacket = 10 * kBytesPerFrame;
  auto pq = result.take_value();

  // Pop the first packet.
  ASSERT_EQ(CapturePacketQueue::PacketMixStatus::Done,
            pq->FinishMixerJob(pq->NextMixerJob().value()));
  ASSERT_EQ(pq->ReadySize(), 1u);
  auto p1 = pq->PopReady();

  // Offset not found.
  auto recycle_result = pq->Recycle({
      .payload_buffer_id = 0,
      .payload_offset = 100,
      .payload_size = kBytesPerPacket,
  });
  ASSERT_TRUE(recycle_result.is_error());

  // Wrong buffer ID.
  recycle_result = pq->Recycle({
      .payload_buffer_id = 1,
      .payload_offset = 0,
      .payload_size = kBytesPerPacket,
  });
  ASSERT_TRUE(recycle_result.is_error());

  // Wrong size.
  recycle_result = pq->Recycle({
      .payload_buffer_id = 0,
      .payload_offset = 0,
      .payload_size = kBytesPerPacket - 1,
  });
  ASSERT_TRUE(recycle_result.is_error());

  // Double recycle fails.
  StreamPacket sp = fidl::Clone(p1->stream_packet());
  recycle_result = pq->Recycle(sp);
  ASSERT_TRUE(recycle_result.is_ok()) << recycle_result.error();

  recycle_result = pq->Recycle(sp);
  ASSERT_TRUE(recycle_result.is_error());
}

TEST_F(CapturePacketQueueTest, DynamicallyAllocated) {
  CreateMapper(50);
  auto pq = CapturePacketQueue::CreateDynamicallyAllocated(payload_buffer_, kFormat);
  ASSERT_TRUE(pq->empty());
  ASSERT_EQ(pq->PendingSize(), 0u);

  bool got_p1_callback = false;
  auto push_result =
      pq->PushPending(0, 10, [&got_p1_callback](StreamPacket p) { got_p1_callback = true; });
  ASSERT_TRUE(push_result.is_ok()) << push_result.error();
  ASSERT_EQ(pq->PendingSize(), 1u);

  bool got_p2_callback = false;
  push_result =
      pq->PushPending(15, 20, [&got_p2_callback](StreamPacket p) { got_p2_callback = true; });
  ASSERT_TRUE(push_result.is_ok()) << push_result.error();

  {
    SCOPED_TRACE("pop #1");
    ASSERT_EQ(pq->PendingSize(), 2u);
    PopAndExpectPacketAtOffset(pq.get(), 0 * kBytesPerFrame, 10 * kBytesPerFrame);
    EXPECT_TRUE(got_p1_callback);
    EXPECT_FALSE(got_p2_callback);
  }

  {
    SCOPED_TRACE("pop #2");
    ASSERT_EQ(pq->PendingSize(), 1u);
    PopAndExpectPacketAtOffset(pq.get(), 15 * kBytesPerFrame, 20 * kBytesPerFrame);
    EXPECT_TRUE(got_p2_callback);
  }

  ASSERT_TRUE(pq->empty());
  ASSERT_EQ(pq->PendingSize(), 0u);
}

TEST_F(CapturePacketQueueTest, DynamicallyAllocatedPushErrors) {
  CreateMapper(50);
  auto pq = CapturePacketQueue::CreateDynamicallyAllocated(payload_buffer_, kFormat);

  // num_frames == 0
  auto push_result = pq->PushPending(0, 0, nullptr);
  ASSERT_TRUE(push_result.is_error());

  // Payload goes past end of buffer.
  push_result = pq->PushPending(40, 11, nullptr);
  ASSERT_TRUE(push_result.is_error());
}

// Verify that WaitForPendingPacket returns immediately with false when the queue is already shut
// down (exercising the non-blocking shutdown path where NextMixerJob returns nullopt).
TEST_F(CapturePacketQueueTest, WaitForPendingPacketWhenAlreadyShutdown) {
  CreateMapper(50);
  auto pq = CapturePacketQueue::CreateDynamicallyAllocated(payload_buffer_, kFormat);
  pq->Shutdown();

  EXPECT_FALSE(pq->WaitForPendingPacket());
  EXPECT_EQ(pq->NextMixerJob(), std::nullopt);
}

// Verify that WaitForPendingPacket returns true immediately without blocking if pending packets
// are already available in the queue.
TEST_F(CapturePacketQueueTest, WaitForPendingPacketReturnsTrueWhenPacketAvailable) {
  CreateMapper(50);
  auto pq = CapturePacketQueue::CreateDynamicallyAllocated(payload_buffer_, kFormat);
  auto push_result = pq->PushPending(0, 10, nullptr);
  ASSERT_TRUE(push_result.is_ok());

  EXPECT_TRUE(pq->WaitForPendingPacket());
  auto mix_state = pq->NextMixerJob();
  ASSERT_TRUE(mix_state.has_value());
  EXPECT_EQ(mix_state->frames, 10u);
}

// Verify that WaitForPendingPacket blocks across threads when the queue is empty, and returns true
// once a new packet is dynamically pushed via PushPending.
TEST_F(CapturePacketQueueTest, WaitForPendingPacketReturnsTrueWhenPacketPushed) {
  CreateMapper(50);
  auto pq = CapturePacketQueue::CreateDynamicallyAllocated(payload_buffer_, kFormat);

  std::atomic<bool> worker_started = false;
  std::atomic<bool> wait_finished = false;
  std::atomic<bool> wait_result = false;
  std::thread worker([&pq, &worker_started, &wait_finished, &wait_result]() {
    worker_started = true;
    wait_result = pq->WaitForPendingPacket();
    wait_finished = true;
  });

  while (!worker_started.load()) {
    std::this_thread::yield();
  }
  std::this_thread::sleep_for(std::chrono::milliseconds(10));

  EXPECT_FALSE(wait_finished.load());
  auto push_result = pq->PushPending(0, 10, nullptr);
  ASSERT_TRUE(push_result.is_ok());

  worker.join();
  EXPECT_TRUE(wait_finished.load());
  EXPECT_TRUE(wait_result.load());

  auto mix_state = pq->NextMixerJob();
  ASSERT_TRUE(mix_state.has_value());
  EXPECT_EQ(mix_state->frames, 10u);
}

// Verify that WaitForPendingPacket blocks in preallocated mode when all packets are in flight,
// and unblocks returning true once a packet is returned via Recycle (exercising async capture
// overflow recovery).
TEST_F(CapturePacketQueueTest, WaitForPendingPacketReturnsTrueWhenPacketRecycled) {
  CreateMapper(20);
  auto result = CapturePacketQueue::CreatePreallocated(payload_buffer_, kFormat, 10);
  ASSERT_TRUE(result.is_ok()) << result.error();
  auto pq = result.take_value();

  // Pop both packets so the pending queue is exhausted.
  auto mix_state1 = pq->NextMixerJob().value();
  ASSERT_EQ(CapturePacketQueue::PacketMixStatus::Done, pq->FinishMixerJob(mix_state1));
  auto p1 = pq->PopReady();

  auto mix_state2 = pq->NextMixerJob().value();
  ASSERT_EQ(CapturePacketQueue::PacketMixStatus::Done, pq->FinishMixerJob(mix_state2));
  auto p2 = pq->PopReady();

  ASSERT_EQ(pq->PendingSize(), 0u);

  std::atomic<bool> worker_started = false;
  std::atomic<bool> wait_finished = false;
  std::atomic<bool> wait_result = false;
  std::thread worker([&pq, &worker_started, &wait_finished, &wait_result]() {
    worker_started = true;
    wait_result = pq->WaitForPendingPacket();
    wait_finished = true;
  });

  while (!worker_started.load()) {
    std::this_thread::yield();
  }
  std::this_thread::sleep_for(std::chrono::milliseconds(10));

  EXPECT_FALSE(wait_finished.load());
  auto recycle_result = pq->Recycle(p1->stream_packet());
  ASSERT_TRUE(recycle_result.is_ok()) << recycle_result.error();

  worker.join();
  EXPECT_TRUE(wait_finished.load());
  EXPECT_TRUE(wait_result.load());

  auto mix_state = pq->NextMixerJob();
  ASSERT_TRUE(mix_state.has_value());
  EXPECT_EQ(mix_state->frames, 10u);
}

// Verify that a thread blocked in WaitForPendingPacket is unblocked and returns false when
// Shutdown is called concurrently (exercising cancellation/shutdown during packet starvation).
TEST_F(CapturePacketQueueTest, WaitForPendingPacketReturnsFalseOnShutdown) {
  CreateMapper(50);
  auto pq = CapturePacketQueue::CreateDynamicallyAllocated(payload_buffer_, kFormat);

  std::atomic<bool> worker_started = false;
  std::atomic<bool> wait_finished = false;
  std::atomic<bool> wait_result = true;
  std::thread worker([&pq, &worker_started, &wait_finished, &wait_result]() {
    worker_started = true;
    wait_result = pq->WaitForPendingPacket();
    wait_finished = true;
  });

  while (!worker_started.load()) {
    std::this_thread::yield();
  }
  std::this_thread::sleep_for(std::chrono::milliseconds(10));
  EXPECT_FALSE(wait_finished.load());

  pq->Shutdown();
  worker.join();

  EXPECT_TRUE(wait_finished.load());
  EXPECT_FALSE(wait_result.load());
  EXPECT_EQ(pq->NextMixerJob(), std::nullopt);
}

}  // namespace media::audio
