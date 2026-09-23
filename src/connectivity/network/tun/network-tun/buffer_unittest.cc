// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "buffer.h"

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace network {
namespace tun {
namespace testing {

const uint64_t kVmoSize = zx_system_get_page_size();
constexpr uint8_t kVmoId = 0x06;

class BufferTest : public ::testing::Test {
  void SetUp() override {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
    ASSERT_OK(vmos_.RegisterVmo(kVmoId, std::move(vmo)));
  }

 public:
  void MintVmo(size_t offset, size_t len) {
    uint8_t val = 0;
    while (len--) {
      ASSERT_OK(vmos_.Write(kVmoId, offset, 1, &val));
      offset++;
      val++;
    }
  }

  void MintVmo(const fuchsia_hardware_network_driver::wire::BufferRegion& region) {
    MintVmo(region.offset, region.length);
  }

  std::vector<uint8_t> ReadVmo(const fuchsia_hardware_network_driver::wire::BufferRegion& region) {
    std::vector<uint8_t> ret;
    ret.reserve(region.length);
    EXPECT_EQ(region.vmo, kVmoId);
    EXPECT_OK(vmos_.Read(kVmoId, region.offset, region.length, std::back_inserter(ret)));
    return ret;
  }

 protected:
  VmoStore vmos_;
};

TEST_F(BufferTest, TestBufferBuildTx) {
  fuchsia_hardware_network_driver::wire::BufferRegion regions[] = {
      {.vmo = kVmoId, .offset = 10, .length = 5},
      {.vmo = kVmoId, .offset = 100, .length = 3},
  };
  for (const fuchsia_hardware_network_driver::wire::BufferRegion& region : regions) {
    MintVmo(region);
  }
  fuchsia_hardware_network_driver::wire::TxBuffer tx = {
      .id = 1,
      .data = fidl::VectorView<fuchsia_hardware_network_driver::wire::BufferRegion>::FromExternal(
          regions),
      .meta =
          {
              .flags = static_cast<uint32_t>(
                  fuchsia_hardware_network::wire::TxFlags::kComputeGenericChecksum),
              .frame_type = fuchsia_hardware_network::wire::FrameType::kEthernet,
          },
  };
  TxBuffer b = vmos_.MakeTxBuffer(tx, true);
  EXPECT_EQ(b.id(), tx.id);
  EXPECT_EQ(b.frame_type(), fuchsia_hardware_network::wire::FrameType::kEthernet);
  auto meta = b.TakeMetadata();
  EXPECT_EQ(meta->flags, static_cast<uint32_t>(
                             fuchsia_hardware_network::wire::TxFlags::kComputeGenericChecksum));
  std::vector<uint8_t> data;
  ASSERT_OK(b.Read(data));
  EXPECT_EQ(data, std::vector<uint8_t>({0x00, 0x01, 0x02, 0x03, 0x04, 0x00, 0x01, 0x02}));
}

TEST_F(BufferTest, TestBufferBuildRx) {
  const fuchsia_hardware_network_driver::wire::RxSpaceBuffer space_1 = {
      .id = 1,
      .region =
          {
              .vmo = kVmoId,
              .offset = 10,
              .length = 5,
          },
  };
  const fuchsia_hardware_network_driver::wire::RxSpaceBuffer space_2 = {
      .id = 2,
      .region =
          {
              .vmo = kVmoId,
              .offset = 100,
              .length = 3,
          },
  };
  RxBuffer b = vmos_.MakeRxSpaceBuffer(space_1);
  b.PushRxSpace(space_2);
  std::vector<uint8_t> wr_data({0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x00, 0x01, 0x02});
  ASSERT_OK(b.Write(wr_data));
  EXPECT_EQ(ReadVmo(space_1.region), std::vector<uint8_t>({0xAA, 0xBB, 0xCC, 0xDD, 0xEE}));
  EXPECT_EQ(ReadVmo(space_2.region), std::vector<uint8_t>({0x00, 0x01, 0x02}));
}

TEST_F(BufferTest, CopyBuffer) {
  fuchsia_hardware_network_driver::wire::BufferRegion tx_parts[3] = {
      {.vmo = kVmoId, .offset = 0, .length = 5},
      {.vmo = kVmoId, .offset = 10, .length = 3},
      {.vmo = kVmoId, .offset = 20, .length = 2},
  };
  for (const fuchsia_hardware_network_driver::wire::BufferRegion& region : tx_parts) {
    MintVmo(region);
  }
  fuchsia_hardware_network_driver::wire::TxBuffer tx = {
      .id = 1,
      .data = fidl::VectorView<fuchsia_hardware_network_driver::wire::BufferRegion>::FromExternal(
          tx_parts),
  };

  TxBuffer b_tx = vmos_.MakeTxBuffer(tx, false);

  fuchsia_hardware_network_driver::wire::RxSpaceBuffer rx_space[3] = {
      {.id = 2, .region = {.vmo = kVmoId, .offset = 100, .length = 3}},
      {.id = 3, .region = {.vmo = kVmoId, .offset = 110, .length = 5}},
      {.id = 4, .region = {.vmo = kVmoId, .offset = 120, .length = 100}},
  };

  RxBuffer b_rx = vmos_.MakeEmptyRxBuffer();
  for (const fuchsia_hardware_network_driver::wire::RxSpaceBuffer& space : rx_space) {
    b_rx.PushRxSpace(space);
  }

  zx::result status = b_rx.CopyFrom(b_tx);
  ASSERT_OK(status.status_value());
  EXPECT_EQ(status.value(), 10ul);

  EXPECT_EQ(ReadVmo(rx_space[0].region), std::vector<uint8_t>({0x00, 0x01, 0x02}));
  EXPECT_EQ(ReadVmo(rx_space[1].region), std::vector<uint8_t>({0x03, 0x04, 0x00, 0x01, 0x02}));
  EXPECT_EQ(ReadVmo(fuchsia_hardware_network_driver::wire::BufferRegion{
                .vmo = kVmoId,
                .offset = rx_space[2].region.offset,
                .length = 2,
            }),
            std::vector<uint8_t>({0x00, 0x01}));
}

TEST_F(BufferTest, WriteFailure) {
  {
    // Write more than buffer's length is invalid.
    RxBuffer b = vmos_.MakeRxSpaceBuffer(fuchsia_hardware_network_driver::wire::RxSpaceBuffer{
        .id = 1,
        .region =
            {
                .vmo = kVmoId,
                .offset = 10,
                .length = 3,
            },
    });
    ASSERT_EQ(b.Write({0x01, 0x02, 0x03, 0x04}), ZX_ERR_OUT_OF_RANGE);
  }
  {
    // A buffer that doesn't fit its VMO is invalid.
    RxBuffer b = vmos_.MakeRxSpaceBuffer(fuchsia_hardware_network_driver::wire::RxSpaceBuffer{
        .id = 1,
        .region =
            {
                .vmo = kVmoId,
                .offset = kVmoSize,
                .length = 3,
            },
    });
    ASSERT_EQ(b.Write({0x01}), ZX_ERR_OUT_OF_RANGE);
  }
  {
    // A buffer whose offset + length wraps around is invalid.
    RxBuffer b = vmos_.MakeRxSpaceBuffer(fuchsia_hardware_network_driver::wire::RxSpaceBuffer{
        .id = 1,
        .region =
            {
                .vmo = kVmoId,
                .offset = std::numeric_limits<uint64_t>::max() - 1,
                .length = 3,
            },
    });
    ASSERT_EQ(b.Write({0x01, 0x02, 0x03}), ZX_ERR_OUT_OF_RANGE);
  }
  {
    // A buffer with an invalid vmo_id is invalid.
    RxBuffer b = vmos_.MakeRxSpaceBuffer(fuchsia_hardware_network_driver::wire::RxSpaceBuffer{
        .id = 1,
        .region =
            {
                .vmo = kVmoId + 1,
                .offset = 10,
                .length = 3,
            },
    });
    ASSERT_EQ(b.Write({0x01}), ZX_ERR_NOT_FOUND);
  }
}

TEST_F(BufferTest, ReadFailure) {
  std::vector<uint8_t> data;
  {
    // A buffer that doesn't fit its VMO is invalid.
    fuchsia_hardware_network_driver::wire::BufferRegion part = {
        .vmo = kVmoId,
        .offset = kVmoSize,
        .length = 10,
    };
    TxBuffer b = vmos_.MakeTxBuffer(
        fuchsia_hardware_network_driver::wire::TxBuffer{
            .id = 1,
            .data =
                fidl::VectorView<fuchsia_hardware_network_driver::wire::BufferRegion>::FromExternal(
                    &part, 1),
        },
        false);
    ASSERT_EQ(b.Read(data), ZX_ERR_OUT_OF_RANGE);
  }
  {
    // A buffer whose offset + length wraps around is invalid.
    fuchsia_hardware_network_driver::wire::BufferRegion part = {
        .vmo = kVmoId,
        .offset = std::numeric_limits<uint64_t>::max() - 1,
        .length = 10,
    };
    TxBuffer b = vmos_.MakeTxBuffer(
        fuchsia_hardware_network_driver::wire::TxBuffer{
            .id = 1,
            .data =
                fidl::VectorView<fuchsia_hardware_network_driver::wire::BufferRegion>::FromExternal(
                    &part, 1),
        },
        false);
    ASSERT_EQ(b.Read(data), ZX_ERR_OUT_OF_RANGE);
  }
  {
    // A buffer with an invalid vmo_id is invalid.
    fuchsia_hardware_network_driver::wire::BufferRegion part = {.vmo = kVmoId + 1, .length = 10};
    TxBuffer b = vmos_.MakeTxBuffer(
        fuchsia_hardware_network_driver::wire::TxBuffer{
            .id = 1,
            .data =
                fidl::VectorView<fuchsia_hardware_network_driver::wire::BufferRegion>::FromExternal(
                    &part, 1),
        },
        false);
    ASSERT_EQ(b.Read(data), ZX_ERR_NOT_FOUND);
  }
}

TEST_F(BufferTest, CopyFailure) {
  // Source region is out of range.
  ASSERT_EQ(VmoStore::Copy(vmos_, kVmoId, kVmoSize, vmos_, kVmoId, 0, 10), ZX_ERR_OUT_OF_RANGE);
  // Source region offset + length wraps around.
  ASSERT_EQ(
      VmoStore::Copy(vmos_, kVmoId, std::numeric_limits<size_t>::max() - 1, vmos_, kVmoId, 0, 10),
      ZX_ERR_OUT_OF_RANGE);
  // Destination region is out of range.
  ASSERT_EQ(VmoStore::Copy(vmos_, kVmoId, 0, vmos_, kVmoId, kVmoSize, 10), ZX_ERR_OUT_OF_RANGE);
  // Destination region offset + length wraps around.
  ASSERT_EQ(
      VmoStore::Copy(vmos_, kVmoId, 0, vmos_, kVmoId, std::numeric_limits<size_t>::max() - 1, 10),
      ZX_ERR_OUT_OF_RANGE);
  // Source region is has bad id.
  ASSERT_EQ(VmoStore::Copy(vmos_, kVmoId + 1, 0, vmos_, kVmoId, 0, 10), ZX_ERR_NOT_FOUND);
  // Destination region is has bad id.
  ASSERT_EQ(VmoStore::Copy(vmos_, kVmoId, 0, vmos_, kVmoId + 1, 0, 10), ZX_ERR_NOT_FOUND);
}

}  // namespace testing
}  // namespace tun
}  // namespace network
