// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fcntl.h>
#include <sys/ioctl.h>

#include <gtest/gtest.h>

#include "src/starnix/lib/linux_uapi/stub/kgsl/msm_kgsl.h"

namespace {

struct ErrStr {
  [[maybe_unused]] friend std::ostream& operator<<(std::ostream& out, const ErrStr&) {
    int err = errno;
    out << err << " (" << strerror(err) << ")";
    return out;
  }
};

class KgslUnitTest : public ::testing::Test {
 protected:
  KgslUnitTest() = default;

  ~KgslUnitTest() override {}

  void SetUp() override {
    constexpr auto kDevicePath = "/dev/kgsl-3d0";
    fd_ = open(kDevicePath, O_RDWR);
    ASSERT_GE(fd_, 0) << "Failed to open " << kDevicePath << ": " << ErrStr();
  }

  void TearDown() override {
    if (fd_ >= 0) {
      int result = close(fd_);
      EXPECT_EQ(result, 0);
    }
  }

  // NOLINTBEGIN
  int fd_ = -1;
  // NOLINTEND
};

// Test querying for simple property type.
TEST_F(KgslUnitTest, GetDeviceBitness) {
  uint32_t device_bitness = 0;
  kgsl_device_getproperty args{.type = KGSL_PROP_DEVICE_BITNESS,
                               .value = &device_bitness,
                               .sizebytes = sizeof(device_bitness)};
  int result = ioctl(fd_, IOCTL_KGSL_DEVICE_GETPROPERTY, &args);
  EXPECT_EQ(result, 0) << ErrStr();
  EXPECT_NE(device_bitness, 0u);
  std::printf("Device Bitness: %u\n", device_bitness);
}

// Test querying for composite property type.
TEST_F(KgslUnitTest, GetDeviceInfo) {
  kgsl_devinfo devinfo{};
  kgsl_device_getproperty args{
      .type = KGSL_PROP_DEVICE_INFO, .value = &devinfo, .sizebytes = sizeof(devinfo)};
  int result = ioctl(fd_, IOCTL_KGSL_DEVICE_GETPROPERTY, &args);
  EXPECT_EQ(result, 0) << ErrStr();
  EXPECT_NE(devinfo.chip_id, 0u);
  std::printf("Chip Id: %u\n", devinfo.chip_id);
}

// Test querying for array property type.
TEST_F(KgslUnitTest, GetGpuModel) {
  constexpr size_t kGpuModelBufferSize = 64;
  char gpu_model[kGpuModelBufferSize]{};
  kgsl_device_getproperty args{
      .type = KGSL_PROP_GPU_MODEL, .value = gpu_model, .sizebytes = sizeof(gpu_model)};
  int result = ioctl(fd_, IOCTL_KGSL_DEVICE_GETPROPERTY, &args);
  EXPECT_EQ(result, 0) << ErrStr();
  EXPECT_NE(gpu_model[0], 0);
  ASSERT_EQ(gpu_model[kGpuModelBufferSize - 1], 0);
  std::printf("GPU Model: %s\n", gpu_model);
}

}  // namespace
