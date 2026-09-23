// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma_service/mock/mock_msd.h>
#include <lib/magma_service/sys_driver/magma_system_connection.h>
#include <lib/magma_service/sys_driver/magma_system_device.h>

#include <gtest/gtest.h>

namespace msd {
class MsdMockBufferManager_Create : public MsdMockBufferManager {
 public:
  MsdMockBufferManager_Create() : has_created_buffer_(false), has_destroyed_buffer_(false) {}

  std::unique_ptr<MsdMockBuffer> CreateBuffer(zx::vmo handle, uint64_t client_id) {
    has_created_buffer_ = true;
    return MsdMockBufferManager::CreateBuffer(std::move(handle), client_id);
  }

  void DestroyBuffer(MsdMockBuffer* buf) {
    has_destroyed_buffer_ = true;
    MsdMockBufferManager::DestroyBuffer(buf);
  }

  bool has_created_buffer() { return has_created_buffer_; }
  bool has_destroyed_buffer() { return has_destroyed_buffer_; }

 private:
  bool has_created_buffer_;
  bool has_destroyed_buffer_;
};

TEST(MagmaSystemBuffer, Create) {
  auto scoped_bufmgr = MsdMockBufferManager::ScopedMockBufferManager(
      std::unique_ptr<MsdMockBufferManager_Create>(new MsdMockBufferManager_Create()));

  auto bufmgr = static_cast<MsdMockBufferManager_Create*>(scoped_bufmgr.get());

  auto msd_drv = std::make_unique<MsdMockDriver>();
  auto msd_dev = msd_drv->MsdCreateDevice(nullptr);
  auto msd_dev_ptr = msd_dev.get();
  auto dev = MagmaSystemDevice::Create(msd_drv.get(), std::move(msd_dev));
  auto msd_connection = msd_dev_ptr->MsdOpen(0);
  ASSERT_NE(msd_connection, nullptr);
  auto connection = std::unique_ptr<MagmaSystemConnection>(
      new MagmaSystemConnection(dev.get(), std::move(msd_connection)));
  ASSERT_NE(connection, nullptr);

  EXPECT_FALSE(bufmgr->has_created_buffer());
  EXPECT_FALSE(bufmgr->has_destroyed_buffer());

  {
    auto buf = magma::PlatformBuffer::Create(256, "test");

    zx::handle duplicate_handle;
    ASSERT_TRUE(buf->duplicate_handle(&duplicate_handle));

    EXPECT_TRUE(connection->ImportBuffer(std::move(duplicate_handle), buf->id()));
    EXPECT_TRUE(bufmgr->has_created_buffer());
    EXPECT_FALSE(bufmgr->has_destroyed_buffer());

    EXPECT_TRUE(connection->ReleaseBuffer(buf->id()));
  }
  EXPECT_TRUE(bufmgr->has_created_buffer());
  EXPECT_TRUE(bufmgr->has_destroyed_buffer());

  msd_drv.reset();
}

}  // namespace msd
