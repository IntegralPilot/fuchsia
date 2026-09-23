// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/zxtest.h>

#include "src/devices/misc/drivers/virtio-socket/socket.h"

class ConnectionTest : public zxtest::Test {
  void SetUp() override {
    zx::socket rx;
    ASSERT_OK(zx::socket::create(ZX_SOCKET_DATAGRAM, &rx, &tx_));

    connection_.emplace(
        key_, std::move(rx),
        [](zx_status_t, const zx_packet_signal_t*, fbl::RefPtr<virtio::SocketDevice::Connection>) {
        },
        0, lock_);
  }

 public:
  fuchsia_hardware_vsock::wire::Addr addr_;
  virtio::SocketDevice::ConnectionKey key_{addr_};
  std::mutex lock_;
  zx::socket tx_;
  std::optional<virtio::SocketDevice::Connection> connection_;
};

TEST_F(ConnectionTest, EmptyCredits) {
  virtio::SocketDevice::CreditInfo info = connection_->GetCreditInfo();
  ASSERT_EQ(info.fwd_count, 0);
}

TEST_F(ConnectionTest, WaitingCredits) {
  uint8_t data[] = {1, 2, 3};
  ASSERT_TRUE(connection_->Rx(data, sizeof(data)));

  virtio::SocketDevice::CreditInfo info = connection_->GetCreditInfo();
  ASSERT_EQ(info.fwd_count, 0);
}

TEST_F(ConnectionTest, ReadCredits) {
  uint8_t data[] = {1, 2, 3};
  ASSERT_TRUE(connection_->Rx(data, sizeof(data)));
  ASSERT_OK(tx_.read(0, data, sizeof(data), nullptr));

  virtio::SocketDevice::CreditInfo info = connection_->GetCreditInfo();
  ASSERT_EQ(info.fwd_count, sizeof(data));
}
