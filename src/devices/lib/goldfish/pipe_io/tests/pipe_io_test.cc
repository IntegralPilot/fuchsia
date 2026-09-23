// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/lib/goldfish/pipe_io/pipe_io.h"

#include <fidl/fuchsia.hardware.goldfish.pipe/cpp/wire.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/loop.h>

#include <future>
#include <string_view>
#include <type_traits>

#include <ddktl/device.h>
#include <gtest/gtest.h>

#include "src/devices/lib/goldfish/pipe_io/pipe_auto_reader.h"
#include "src/devices/testing/goldfish/fake_pipe/fake_pipe.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace goldfish::sensor {
namespace {

const size_t kPageSize = zx_system_get_page_size();

class PipeIoTest : public ::testing::Test {
 public:
  PipeIoTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) {}

  void SetUp() override {
    loop_.StartThread("pipe-io-server");

    auto endpoints = fidl::Endpoints<fuchsia_hardware_goldfish_pipe::Bus>::Create();

    binding_ = fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), &pipe_);
    EXPECT_TRUE(binding_.has_value());

    pipe_bus_client_ = fidl::WireSyncClient(std::move(endpoints.client));
    io_ = std::make_unique<PipeIo>(std::move(pipe_bus_client_), "pipe");
  }

  void TearDown() override {
    // The PipeIo object must be released before the shutdown of the loop,
    // because PipeIo's destructor makes a FIDL call that relies on the server
    // that is running in the loop.
    io_.reset();
    loop_.Shutdown();
  }

 protected:
  testing::FakePipe pipe_;
  fidl::WireSyncClient<fuchsia_hardware_goldfish_pipe::Bus> pipe_bus_client_;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_goldfish_pipe::Bus>> binding_;
  std::unique_ptr<PipeIo> io_;
  async::Loop loop_;
};

TEST_F(PipeIoTest, BlockingWrite) {
  auto old_size = pipe_.io_buffer_contents().size();

  const char* kSrcString1 = "hello world";
  io_->Write(kSrcString1, true);
  ASSERT_EQ(pipe_.io_buffer_contents().size(), old_size + 1);
  EXPECT_EQ(memcmp(pipe_.io_buffer_contents().back().data(), kSrcString1, strlen(kSrcString1)), 0);

  const char* kDstString1 = "000bhello world";
  io_->WriteWithHeader(kSrcString1);
  ASSERT_EQ(pipe_.io_buffer_contents().size(), old_size + 2);
  EXPECT_EQ(memcmp(pipe_.io_buffer_contents().back().data(), kDstString1, strlen(kDstString1)), 0);
}

TEST_F(PipeIoTest, BlockingWrite_Vmo) {
  auto old_size = pipe_.io_buffer_contents().size();

  // Allocate VMO
  zx::vmo vmo;
  ASSERT_EQ(ZX_OK, zx::vmo::create(kPageSize, 0u, &vmo));
  auto pinned_vmo = io_->PinVmo(vmo, ZX_BTI_PERM_READ | ZX_BTI_CONTIGUOUS);

  constexpr std::string_view kVmoStr = "vmo1";
  vmo.write(kVmoStr.data(), 0, kVmoStr.length());

  PipeIo::WriteSrc sources[] = {
      {.data = PipeIo::WriteSrc::PinnedVmo{&pinned_vmo, 0u, kVmoStr.length()}},
  };
  io_->Write(sources, true);

  ASSERT_EQ(pipe_.io_buffer_contents().size(), old_size + 1);
  EXPECT_EQ(memcmp(pipe_.io_buffer_contents()[pipe_.io_buffer_contents().size() - 1].data(),
                   kVmoStr.data(), kVmoStr.length()),
            0);
}

TEST_F(PipeIoTest, BlockingWrite_VmoRange) {
  auto old_size = pipe_.io_buffer_contents().size();

  // Allocate VMO, but only pin the second page.
  zx::vmo vmo;
  ASSERT_EQ(ZX_OK, zx::vmo::create(3 * kPageSize, 0u, &vmo));
  auto pinned_vmo = io_->PinVmo(vmo, ZX_BTI_PERM_READ | ZX_BTI_CONTIGUOUS, kPageSize, kPageSize);

  constexpr std::string_view kVmoStr = "vmo2";
  vmo.write(kVmoStr.data(), kPageSize, kVmoStr.length());

  PipeIo::WriteSrc sources[] = {
      {.data = PipeIo::WriteSrc::PinnedVmo{&pinned_vmo, 0u, kVmoStr.length()}},
  };
  io_->Write(sources, true);

  ASSERT_EQ(pipe_.io_buffer_contents().size(), old_size + 1);
  EXPECT_EQ(memcmp(pipe_.io_buffer_contents()[pipe_.io_buffer_contents().size() - 1].data(),
                   kVmoStr.data(), kVmoStr.length()),
            0);
}

TEST_F(PipeIoTest, BlockingWrite_MultipleTargets) {
  auto old_size = pipe_.io_buffer_contents().size();

  const char* kSrcString1 = "String";
  const std::vector<uint8_t> kUintVector{'V', 'e', 'c', 't', 'o', 'r'};

  // Allocate VMO
  zx::vmo vmo;
  ASSERT_EQ(ZX_OK, zx::vmo::create(kPageSize, 0u, &vmo));
  auto pinned_vmo = io_->PinVmo(vmo, ZX_BTI_PERM_READ);

  constexpr std::string_view kVmoStr = "Vmo";
  vmo.write(kVmoStr.data(), 0, kVmoStr.length());

  PipeIo::WriteSrc sources[] = {
      {.data = kSrcString1},
      {.data = kUintVector},
      {.data = PipeIo::WriteSrc::PinnedVmo{&pinned_vmo, 0u, kVmoStr.length()}},
  };
  io_->Write(sources, true);

  ASSERT_EQ(pipe_.io_buffer_contents().size(), old_size + 3);
  EXPECT_EQ(memcmp(pipe_.io_buffer_contents()[pipe_.io_buffer_contents().size() - 3].data(),
                   kSrcString1, strlen(kSrcString1)),
            0);
  EXPECT_EQ(memcmp(pipe_.io_buffer_contents()[pipe_.io_buffer_contents().size() - 2].data(),
                   kUintVector.data(), kUintVector.size()),
            0);
  EXPECT_EQ(memcmp(pipe_.io_buffer_contents()[pipe_.io_buffer_contents().size() - 1].data(),
                   kVmoStr.data(), kVmoStr.length()),
            0);
}

TEST_F(PipeIoTest, NonBlockingRead) {
  constexpr std::string_view kSrcString1 = "hello world";

  // If the pipe has some bytes available for reading, we'll be able to read it
  // directly.
  {
    std::vector<uint8_t> bytes_to_read(kSrcString1.begin(), kSrcString1.end());
    pipe_.EnqueueBytesToRead(bytes_to_read);

    auto read_result = io_->Read<char>(kSrcString1.length(), false);
    ASSERT_TRUE(read_result.is_ok());
    static_assert(
        std::is_same_v<std::remove_reference_t<decltype(read_result.value())>, std::string>);

    EXPECT_EQ(memcmp(read_result.value().data(), bytes_to_read.data(), bytes_to_read.size()), 0);
    EXPECT_EQ(read_result.value().size(), bytes_to_read.size());
  }

  // Test reading strings with frame headers.
  constexpr std::string_view kHeader = "000b";
  {
    std::vector<uint8_t> header(kHeader.begin(), kHeader.end());
    pipe_.EnqueueBytesToRead(header);

    std::vector<uint8_t> bytes_to_read(kSrcString1.begin(), kSrcString1.end());
    pipe_.EnqueueBytesToRead(bytes_to_read);

    auto read_result = io_->ReadWithHeader(false);
    ASSERT_TRUE(read_result.is_ok());
    EXPECT_EQ(memcmp(read_result.value().data(), bytes_to_read.data(), bytes_to_read.size()), 0);

    EXPECT_EQ(read_result.value().size(), bytes_to_read.size());
  }

  // If the pipe doesn't have anything to read, it will return a
  // PIPE_ERROR_AGAIN error.
  {
    auto read_result = io_->Read<char>(kSrcString1.length(), false);
    ASSERT_TRUE(read_result.is_error());
    EXPECT_EQ(read_result.error_value(), ZX_ERR_SHOULD_WAIT);
  }

  // If the pipe has fewer bytes than requested, it will return a
  // PIPE_ERROR_AGAIN error as well.
  {
    std::vector<uint8_t> bytes_to_read(kSrcString1.begin(), kSrcString1.end());
    pipe_.EnqueueBytesToRead(bytes_to_read);

    auto read_result = io_->Read<char>(kSrcString1.length() * 2, false);
    ASSERT_TRUE(read_result.is_error());
    EXPECT_EQ(read_result.error_value(), ZX_ERR_SHOULD_WAIT);
  }
}

template <typename T>
void AssertBlocked(const std::future<T>& fut) {
  // Give an asynchronous blocking operation some time to reach the blocking state. Clocks
  // sometimes jump in infrastructure, which may cause a single wait to trip sooner than expected,
  // without the asynchronous task getting a meaningful shot at running. We protect against that by
  // splitting the wait into multiple calls as an attempt to guarantee that clock jumps do not
  // impact the duration of a wait.
  for (int i = 0; i < 50; i++) {
    ASSERT_EQ(fut.wait_for(std::chrono::milliseconds(1)), std::future_status::timeout);
  }
}

TEST_F(PipeIoTest, BlockingRead) {
  constexpr std::string_view kSegment1 = "hello";
  constexpr std::string_view kSegment2 = "world!";
  constexpr std::string_view kConcat = "helloworld!";

  const auto fut = std::async(std::launch::async, [this, kConcat]() {
    zx::result result = io_->Read<char>(kConcat.length(), true);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
    ASSERT_EQ(result.value(), kConcat);
  });

  ASSERT_NO_FATAL_FAILURE(AssertBlocked(fut));

  pipe_.EnqueueBytesToRead({kSegment1.begin(), kSegment1.end()});

  ASSERT_NO_FATAL_FAILURE(AssertBlocked(fut));

  ASSERT_EQ(pipe_.pipe_event().signal(0u, fuchsia_hardware_goldfish::wire::kSignalReadable), ZX_OK);

  ASSERT_NO_FATAL_FAILURE(AssertBlocked(fut));

  pipe_.EnqueueBytesToRead({kSegment2.begin(), kSegment2.end()});

  ASSERT_NO_FATAL_FAILURE(AssertBlocked(fut));

  ASSERT_EQ(pipe_.pipe_event().signal(0u, fuchsia_hardware_goldfish::wire::kSignalReadable), ZX_OK);

  fut.wait();
}

TEST_F(PipeIoTest, BlockingCall) {
  auto old_size = pipe_.io_buffer_contents().size();

  // Prepare contents to write to pipe.
  constexpr std::string_view kWriteString = "WriteString";
  // Allocate VMO
  zx::vmo vmo;
  ASSERT_EQ(ZX_OK, zx::vmo::create(kPageSize, 0u, &vmo));
  auto pinned_vmo = io_->PinVmo(vmo, ZX_BTI_PERM_READ);
  constexpr std::string_view kWriteVmoStr = "WriteVmo";
  vmo.write(kWriteVmoStr.data(), 0, kWriteVmoStr.length());

  // Prepare contents to read from pipe.
  constexpr std::string_view kReadString = "ReadString";
  auto bytes_to_read = std::vector<uint8_t>(kReadString.begin(), kReadString.end());
  pipe_.EnqueueBytesToRead(bytes_to_read);
  pipe_.pipe_event().signal(0u, fuchsia_hardware_goldfish::wire::kSignalReadable);

  PipeIo::WriteSrc sources[] = {
      {.data = kWriteString},
      {.data = PipeIo::WriteSrc::PinnedVmo{&pinned_vmo, 0u, kWriteVmoStr.length()}},
  };
  auto result = io_->Call<char>(sources, kReadString.length(), true);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(memcmp(result.value().data(), kReadString.data(), kReadString.length()), 0);

  ASSERT_EQ(pipe_.io_buffer_contents().size(), old_size + 2);
  EXPECT_EQ(memcmp(pipe_.io_buffer_contents()[pipe_.io_buffer_contents().size() - 2].data(),
                   kWriteString.data(), kWriteString.length()),
            0);
  EXPECT_EQ(memcmp(pipe_.io_buffer_contents()[pipe_.io_buffer_contents().size() - 1].data(),
                   kWriteVmoStr.data(), kWriteVmoStr.length()),
            0);
}

TEST_F(PipeIoTest, NonBlockingCall) {
  auto old_size = pipe_.io_buffer_contents().size();

  // Case 1: Non-blocking call succeeds.

  // Prepare contents to write to pipe.
  constexpr std::string_view kWriteString = "WriteString";

  // Prepare contents to read from pipe.
  constexpr std::string_view kReadString = "ReadString";
  auto bytes_to_read = std::vector<uint8_t>(kReadString.begin(), kReadString.end());
  pipe_.EnqueueBytesToRead(bytes_to_read);
  pipe_.pipe_event().signal(0u, fuchsia_hardware_goldfish::wire::kSignalReadable);

  PipeIo::WriteSrc sources[] = {
      {.data = kWriteString},
  };
  auto result = io_->Call<char>(sources, kReadString.length(), false);
  ASSERT_TRUE(result.is_ok());
  EXPECT_TRUE(result.value() == kReadString);

  ASSERT_EQ(pipe_.io_buffer_contents().size(), old_size + 1);
  EXPECT_EQ(
      memcmp(pipe_.io_buffer_contents().back().data(), kWriteString.data(), kWriteString.length()),
      0);

  // Case 2: Non-blocking call fails due to back pressure.

  // Prepare contents to read from pipe.
  pipe_.EnqueueBytesToRead(bytes_to_read);
  pipe_.pipe_event().signal(0u, fuchsia_hardware_goldfish::wire::kSignalReadable);

  result = io_->Call<char>(sources, kReadString.length() * 2, false);
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error_value(), ZX_ERR_SHOULD_WAIT);
}

// TODO(https://fxbug.dev/42053213): Use a single test loop and async FIDL client.
class PipeAutoReaderTest : public gtest::TestLoopFixture {
 public:
  PipeAutoReaderTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) {}

  void SetUp() override {
    TestLoopFixture::SetUp();

    auto endpoints = fidl::Endpoints<fuchsia_hardware_goldfish_pipe::Bus>::Create();

    binding_ = fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), &pipe_);
    EXPECT_TRUE(binding_.has_value());

    pipe_bus_client_ = fidl::WireSyncClient(std::move(endpoints.client));

    loop_.StartThread("pipe-io-server");
  }

  void TearDown() override {
    loop_.Shutdown();
    TestLoopFixture::TearDown();
  }

 protected:
  testing::FakePipe pipe_;
  fidl::WireSyncClient<fuchsia_hardware_goldfish_pipe::Bus> pipe_bus_client_;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_goldfish_pipe::Bus>> binding_;
  async::Loop loop_;
};

TEST_F(PipeAutoReaderTest, AutoRead) {
  int sum = 0;
  std::unique_ptr<PipeAutoReader> reader = std::make_unique<PipeAutoReader>(
      std::move(pipe_bus_client_), "pipe", dispatcher(), [&sum](PipeIo::ReadResult<char> result) {
        ASSERT_TRUE(result.is_ok());
        int val = 0;
        sscanf(result.value().data(), "%d", &val);
        sum += val;
      });
  reader->BeginRead();
  EXPECT_TRUE(RunLoopUntilIdle());

  {
    constexpr std::string_view kHeader = "0003";
    constexpr std::string_view kNum = "123";
    std::vector<uint8_t> header(kHeader.begin(), kHeader.end());
    std::vector<uint8_t> bytes_to_read(kNum.begin(), kNum.end());
    pipe_.EnqueueBytesToRead(header);
    pipe_.EnqueueBytesToRead(bytes_to_read);
    pipe_.pipe_event().signal(0u, fuchsia_hardware_goldfish::wire::kSignalReadable);
  }
  EXPECT_TRUE(RunLoopUntilIdle());
  EXPECT_EQ(sum, 123);

  {
    constexpr std::string_view kHeader = "0004";
    constexpr std::string_view kNum = "4567";
    std::vector<uint8_t> header(kHeader.begin(), kHeader.end());
    std::vector<uint8_t> bytes_to_read(kNum.begin(), kNum.end());
    pipe_.EnqueueBytesToRead(header);
    pipe_.EnqueueBytesToRead(bytes_to_read);
    pipe_.pipe_event().signal(0u, fuchsia_hardware_goldfish::wire::kSignalReadable);
  }
  EXPECT_TRUE(RunLoopUntilIdle());
  EXPECT_EQ(sum, 123 + 4567);

  {
    constexpr std::string_view kHeader = "0005";
    constexpr std::string_view kNum = "89012";
    std::vector<uint8_t> header(kHeader.begin(), kHeader.end());
    std::vector<uint8_t> bytes_to_read(kNum.begin(), kNum.end());
    pipe_.EnqueueBytesToRead(header);
    pipe_.EnqueueBytesToRead(bytes_to_read);
    pipe_.pipe_event().signal(0u, fuchsia_hardware_goldfish::wire::kSignalReadable);
  }
  EXPECT_TRUE(RunLoopUntilIdle());
  EXPECT_EQ(sum, 123 + 4567 + 89012);

  reader->StopRead();
}

}  // namespace
}  // namespace goldfish::sensor
