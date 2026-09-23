// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/socket.h>

#include <map>
#include <string>
#include <string_view>
#include <vector>

#include <gtest/gtest.h>
#include <linux/netlink.h>

#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

const int kBufferSize = 16 * 1024 * 1024;

fbl::unique_fd GetUdevSocket() {
  fbl::unique_fd fd(socket(PF_NETLINK, SOCK_DGRAM, NETLINK_KOBJECT_UEVENT));

  if (!fd.is_valid()) {
    return fd;
  }

  setsockopt(fd.get(), SOL_SOCKET, SO_RCVBUFFORCE, &kBufferSize, sizeof(kBufferSize));

  struct sockaddr_nl address;
  memset(&address, 0x00, sizeof(struct sockaddr_nl));
  address.nl_family = AF_NETLINK;
  address.nl_pid = getpid();
  address.nl_groups = -1;
  int result = bind(fd.get(), reinterpret_cast<struct sockaddr*>(&address), sizeof(address));

  if (result < 0) {
    fd.reset();
  }
  return fd;
}

std::pair<std::string, std::string> ParseUeventParam(std::string_view line) {
  size_t equal_index = line.find('=');
  if (equal_index == std::string_view::npos) {
    return {};
  }
  return {std::string(line.substr(0, equal_index)), std::string(line.substr(equal_index + 1))};
}

::testing::AssertionResult read_next_uevent(int fd, std::string* command,
                                            std::map<std::string, std::string>* parameters) {
  char buffer[4096];
  ssize_t bytes = recv(fd, buffer, sizeof(buffer), MSG_DONTWAIT);
  if (bytes == -1) {
    return ::testing::AssertionFailure() << "Unable to read from socket";
  }
  auto lines = fxl::SplitString(std::string_view(buffer, bytes), std::string_view("\0", 1),
                                fxl::kKeepWhitespace, fxl::kSplitWantNonEmpty);
  if (lines.empty()) {
    return ::testing::AssertionFailure() << "Empty message";
  }
  *command = std::string(lines[0]);
  lines.erase(lines.begin());
  parameters->clear();
  for (const auto& line : lines) {
    auto param = ParseUeventParam(line);
    if (!param.first.empty()) {
      parameters->insert(param);
    }
  }
  return ::testing::AssertionSuccess();
}

TEST(UdevTest, Connect) {
  // Assume starnix always has udevsocket.
  ASSERT_TRUE(!test_helper::IsStarnix() || GetUdevSocket().is_valid());
}

TEST(UdevTest, AddDevMapper) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (getuid() != 0) {
    GTEST_SKIP() << "Can only be run as root.";
  }
  auto fd = GetUdevSocket();
  ASSERT_TRUE(fd.is_valid());

  fbl::unique_fd write_fd(open("/sys/devices/virtual/misc/device-mapper/uevent", O_WRONLY));
  ASSERT_TRUE(write_fd.is_valid());
  ASSERT_EQ(write(write_fd.get(), "add\n", 4), 4);

  std::string command;
  std::map<std::string, std::string> parameters;
  ASSERT_TRUE(read_next_uevent(fd.get(), &command, &parameters));
  ASSERT_EQ(command, "add@/devices/virtual/misc/device-mapper");
  ASSERT_EQ(parameters["ACTION"], "add");
  ASSERT_EQ(parameters["DEVPATH"], "/devices/virtual/misc/device-mapper");
  ASSERT_EQ(parameters["SUBSYSTEM"], "misc");
  ASSERT_EQ(parameters["SYNTH_UUID"], "0");
  ASSERT_EQ(parameters["MAJOR"], "10");
  ASSERT_EQ(parameters["MINOR"], "236");
  ASSERT_EQ(parameters["DEVNAME"], "mapper/control");
  ASSERT_FALSE(parameters["SEQNUM"].empty());
}

TEST(UdevTest, RemoveDevMapper) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (getuid() != 0) {
    GTEST_SKIP() << "Can only be run as root.";
  }
  auto fd = GetUdevSocket();
  ASSERT_TRUE(fd.is_valid());

  fbl::unique_fd write_fd(open("/sys/devices/virtual/misc/device-mapper/uevent", O_WRONLY));
  ASSERT_TRUE(write_fd.is_valid());
  ASSERT_EQ(write(write_fd.get(), "remove\n", 7), 7);

  std::string command;
  std::map<std::string, std::string> parameters;
  ASSERT_TRUE(read_next_uevent(fd.get(), &command, &parameters));
  ASSERT_EQ(command, "remove@/devices/virtual/misc/device-mapper");
  ASSERT_EQ(parameters["ACTION"], "remove");
  ASSERT_EQ(parameters["DEVPATH"], "/devices/virtual/misc/device-mapper");
  ASSERT_EQ(parameters["SUBSYSTEM"], "misc");
  ASSERT_EQ(parameters["SYNTH_UUID"], "0");
  ASSERT_EQ(parameters["MAJOR"], "10");
  ASSERT_EQ(parameters["MINOR"], "236");
  ASSERT_EQ(parameters["DEVNAME"], "mapper/control");
  ASSERT_FALSE(parameters["SEQNUM"].empty());
}

TEST(UdevTest, AddInput) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (getuid() != 0) {
    GTEST_SKIP() << "Can only be run as root.";
  }
  auto fd = GetUdevSocket();
  ASSERT_TRUE(fd.is_valid());

  // This path is based on values in `ueventd.rc`.
  fbl::unique_fd write_fd(open("/sys/devices/virtual/input/event0/uevent", O_WRONLY));
  ASSERT_TRUE(write_fd.is_valid());
  ASSERT_EQ(write(write_fd.get(), "add\n", 4), 4);

  std::string command;
  std::map<std::string, std::string> parameters;
  ASSERT_TRUE(read_next_uevent(fd.get(), &command, &parameters));
  // These values are compatible with `ueventd`.
  ASSERT_EQ(command, "add@/devices/virtual/input/event0");
  ASSERT_EQ(parameters["ACTION"], "add");
  ASSERT_EQ(parameters["DEVPATH"], "/devices/virtual/input/event0");
  ASSERT_EQ(parameters["SUBSYSTEM"], "input");
  ASSERT_EQ(parameters["SYNTH_UUID"], "0");
  ASSERT_EQ(parameters["MAJOR"], "13");
  ASSERT_EQ(parameters["MINOR"], "0");
  ASSERT_EQ(parameters["DEVNAME"], "input/event0");
  ASSERT_FALSE(parameters["SEQNUM"].empty());

  // Also ensure that uevents read from sysfs have the same relevant properties.
  std::string content;
  ASSERT_TRUE(files::ReadFileToString("/sys/devices/virtual/input/event0/uevent", &content));
  std::map<std::string, std::string> params;
  for (const auto& line :
       fxl::SplitString(content, "\n", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty)) {
    auto param = ParseUeventParam(line);
    ASSERT_FALSE(param.first.empty()) << line;
    params.insert(param);
  }

  ASSERT_TRUE(!params.contains("ACTION"));
  ASSERT_TRUE(!params.contains("SEQNUM"));
  ASSERT_EQ(params["DEVPATH"], "/devices/virtual/input/event0");
  ASSERT_EQ(params["SUBSYSTEM"], "input");
  ASSERT_EQ(params["SYNTH_UUID"], "0");
  ASSERT_EQ(params["MAJOR"], "13");
  ASSERT_EQ(params["MINOR"], "0");
  ASSERT_EQ(params["DEVNAME"], "input/event0");
}

}  // namespace
