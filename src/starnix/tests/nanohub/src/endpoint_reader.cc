// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <poll.h>
#include <sys/epoll.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <string>
#include <vector>

#include "src/starnix/tests/nanohub/src/gpio.h"
namespace fs = std::filesystem;
namespace {
bool check_sysfs_file(const char* path, const char* expected_value) {
  auto fd = open(path, 'r');
  if (!fd) {
    printf("Path did not open successfully: %s\n", path);
    return false;
  }

  // Max sysfs return size: 4096
  char buffer[4096];

  // Read the main contents of the buffer
  auto bytes_read = read(fd, buffer, 4096);
  if (bytes_read < 0) {
    printf("Error: Failed to read file contents from %s\n", path);
    return false;
  }
  if (bytes_read == 0) {
    printf("Error: read returned zero bytes from %s\n", path);
    return false;
  }

  std::string content(buffer, bytes_read);
  printf("Bytes read: %zd  \nContents: %s\n", bytes_read, content.c_str());

  // Read again, to check for EOF
  printf("Reading again to await EOF\n");

  // Flushing here as a failure case will cause a hang
  fflush(stdout);
  bytes_read = read(fd, buffer, 4096);

  if (bytes_read < 0) {
    printf("Error: Read failed when expecting EOF from %s\n", path);
    return false;
  }
  if (bytes_read > 0) {
    // Unexpected buffer contents
    std::string unexpected_content(buffer, bytes_read);
    printf("Error: Bytes read: %zd  \nUnexpected content: %s from %s", bytes_read,
           unexpected_content.c_str(), path);
    return false;
  }

  // To reach here, bytes_read was 0, correctly marking EOF
  printf("Successfully read EOF from %s\n", path);

  // Make sure we received the correct string...
  if (content != expected_value) {
    printf("Error: Received incorrect string from %s\n", path);
    return false;
  }

  return true;
}

// Checks is path1 is a symlink to path2.
// Returns false if the file is not a symlink or links to a different path.
bool check_symlink_file(const char* path1, const char* path2) {
  if (fs::is_symlink(path1)) {
    fs::path res = fs::read_symlink(path1);
    printf("%s is a symlink to %s", path1, res.c_str());
    // File is a symlink, but does not link to the correct path.
    if (res != path2) {
      return false;
    }
  } else {
    printf("%s is not a symlink", path1);
    return false;
  }

  return true;
}

// Opens the given device path.
// Returns a file descriptor on success, or -1 on failure.
int OpenDevice(const char* path) {
  while (1) {
    int fd = open(path, O_RDWR);
    if (fd == -1) {
      fprintf(stdout, "Warning: [%s] when attempting to open path: %s. Retrying...\n",
              strerror(errno), path);
      sleep(1);
      continue;
    }
    return fd;
  }
}

// Reads the contents of the given file descriptor into `out`.
// Returns true on success, false on failure.
bool ReadContents(int fd, std::string& out) {
  // Max sysfs return size: 4096
  char buffer[4096];

  ssize_t bytes_read = read(fd, buffer, sizeof(buffer));
  if (bytes_read <= 0) {
    fprintf(stderr, "Error: read() failed.\n");
    return false;
  }

  out.assign(buffer, bytes_read);
  return true;
}

// Writes the given data to the file descriptor.
// Returns true on success, false on failure.
bool WriteContents(int fd, const std::string& data) {
  ssize_t bytes_written = write(fd, data.c_str(), data.length());
  if (bytes_written < 0) {
    fprintf(stderr, "Error: write() failed.\n");
    return false;
  }
  return true;
}

// Polls the given file descriptor for a specific event.
// Returns true on success, false on failure or timeout.
bool PollForEvent(int fd, short poll_event) {
  struct pollfd fds = {.fd = fd, .events = poll_event, .revents = 0};
  int rv = poll(&fds, 1, 500);  // 500ms timeout

  if (rv < 0) {
    fprintf(stderr, "Error: poll() failed.\n");
    return false;
  }
  if (rv == 0) {
    fprintf(stderr, "Error: poll() timed out.\n");
    return false;
  }
  if (fds.revents & poll_event) {
    return true;
  }

  fprintf(stderr, "Error: poll() returned with unexpected events: %d\n", fds.revents);
  return false;
}

bool TestDevice(const char* path, const char* expected_result) {
  int fd = OpenDevice(path);
  if (fd == -1) {
    return false;
  }

  // Before writing, the device should be ready for writing (POLLOUT).
  if (!PollForEvent(fd, POLLOUT)) {
    fprintf(stderr, "Error: Device should be ready for writing.\n");
    close(fd);
    return false;
  }

  // Before writing, the device should not have any data to read (POLLIN).
  if (PollForEvent(fd, POLLIN)) {
    fprintf(stderr, "Error: Device should not have data to read yet.\n");
    close(fd);
    return false;
  }

  // Write the test data to the device.
  if (!WriteContents(fd, expected_result)) {
    fprintf(stderr, "Error: Failed to write to device.\n");
    close(fd);
    return false;
  }

  // After writing, the device should have data available for reading (POLLIN).
  if (!PollForEvent(fd, POLLIN)) {
    fprintf(stderr, "Error: Device should have data to read.\n");
    close(fd);
    return false;
  }

  // Read content back from the device.
  std::string content;
  if (!ReadContents(fd, content)) {
    fprintf(stderr, "Error: Failed to read from device.\n");
    close(fd);
    return false;
  }

  // Verify that the content read matches the content written.
  if (content != expected_result) {
    fprintf(stderr, "Error: Read content does not match written content.\n");
    close(fd);
    return false;
  }

  close(fd);
  return true;
}

int test_datachannel() {
  struct TestConfig {
    const char* path;
    const char* expected_result;
  };

  std::vector<TestConfig> tests = {
      {.path = "/dev/test_endpoint1", .expected_result = "test_endpoint1_data"},
      {.path = "/dev/test_endpoint2", .expected_result = "test_endpoint2_data"},
  };

  for (const auto& test : tests) {
    if (!TestDevice(test.path, test.expected_result)) {
      fprintf(stderr, "Test failed for %s\n", test.path);
      return 1;
    }
  }

  return 0;
}

int test_sysfs() {
  if (!check_sysfs_file("/sys/class/nanohub/nanohub_comms/firmware_name", "test_firmware_name")) {
    return 1;
  }
  if (!check_sysfs_file("/sys/class/display/display_comms/display_state", "4\n")) {
    return 1;
  }
  if (!check_sysfs_file("/sys/class/display/display_comms/display_info",
                        "display_mode: 4\npanel_mode: 1\nnbm_brightness: 2\naod_brightness: 3\n")) {
    return 1;
  }
  if (!check_sysfs_file("/sys/class/display/display_comms/display_select", "0\n")) {
    return 1;
  }
  if (!check_sysfs_file("/sys/firmware/devicetree/base/mcu/board_type", "starnix")) {
    return 1;
  }

  return 0;
}

int test_procfs() {
  if (!check_symlink_file("/proc/device-tree", "/sys/firmware/devicetree/base")) {
    return 1;
  }

  return 0;
}

int test_gpiod_api() {
  constexpr char gpio_chip_path[] = "/dev/gpiochipwake0";
  constexpr char gpio_chip_consumer_name[] = "gpio_chip_test";

  static constexpr uint32_t kGpioOffset = 19;

  // Open base GPIO Chip
  auto chip_fd = open(gpio_chip_path, O_RDWR);
  if (chip_fd < 0) {
    fprintf(stderr, "Error: Failed to open test gpiochip(%s) .\n", strerror(errno));
    return 1;
  }

  // Test IOCTL surface area
  struct mock_gpio_v2_line_request req;
  memset(&req, 0, sizeof(req));
  req.num_lines = 1;
  req.offsets[0] = kGpioOffset;
  req.config.flags = (MOCK_GPIO_V2_LINE_FLAG_INPUT | MOCK_GPIO_V2_LINE_FLAG_EDGE_RISING |
                      MOCK_GPIO_V2_LINE_FLAG_BIAS_PULL_DOWN);
  strncpy(req.consumer, gpio_chip_consumer_name, sizeof(req.consumer) - 1);

  if (ioctl(chip_fd, MOCK_GPIO_V2_GET_LINE_IOCTL, &req) < 0) {
    fprintf(stderr, "Failed to request GPIO line: %s\n", strerror(errno));

    return 1;
  }

  // Test One-shot Poll
  struct pollfd fds = {.fd = req.fd, .events = POLLIN, .revents = 0};

  int rv = poll(&fds, 1, 100);  // 500ms timeout

  if (rv < 0) {
    fprintf(stderr, "Error: poll() failed.\n");
    return 1;
  }
  if (fds.revents & POLLIN) {
    return 1;
  }
  if (rv == 0) {
    fprintf(stdout, "poll() correctly timed out.\n");
  }

  // Test epoll
  int epoll_fd = epoll_create1(/*flags*/ 0);
  epoll_event event;
  event.data.fd = req.fd;
  event.events = POLLIN;

  auto epoll_ctl_result = epoll_ctl(epoll_fd, EPOLL_CTL_ADD, req.fd, &event);

  if (epoll_ctl_result != 0) {
    fprintf(stderr, "Error: epoll_ctl() returned status: %d (errno: %s).\n", epoll_ctl_result,
            strerror(errno));
    return 1;
  }

  return 0;
}
}  // namespace
int main(int argc, char** argv) {
  int test_sysfs_result = test_sysfs();
  int test_procfs_result = test_procfs();
  int test_datachannel_result = test_datachannel();
  int test_gpiod_result = test_gpiod_api();

  return test_sysfs_result | test_procfs_result | test_datachannel_result | test_gpiod_result;
}
