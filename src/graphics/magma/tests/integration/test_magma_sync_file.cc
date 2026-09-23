// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/magma/magma.h>
#include <lib/magma/magma_common_defs.h>
#include <poll.h>
#include <sys/epoll.h>
#include <sys/ioctl.h>
#include <time.h>

#include <algorithm>
#include <limits>
#include <random>
#include <thread>
#include <vector>

#include <gtest/gtest.h>
#include <linux/sync_file.h>

#define MERGED_SYNC_FILE_NAME "test merged sync file"
#define SYNC_FILE_NAME "magma semaphore"
#define DRIVER_NAME "Magma"

class TestMagmaSyncFile : public testing::Test {
 public:
  static constexpr const char* kDeviceNameVirtioMagma = "/dev/magma0";

  static constexpr bool is_valid_handle(magma_handle_t handle) {
    return static_cast<int>(handle) >= 0;
  }

  std::string device_name() { return device_name_; }

  void SetUp() override {
    int fd = open(kDeviceNameVirtioMagma, O_RDWR);
    if (fd >= 0) {
      device_name_ = kDeviceNameVirtioMagma;
    }
    EXPECT_TRUE(fd >= 0);
    if (fd >= 0) {
      EXPECT_EQ(MAGMA_STATUS_OK, magma_device_import(fd, &device_));
    }

    if (device_) {
      magma_device_create_connection(device_, &connection_);
    }
  }

  void TearDown() override {
    if (connection_)
      magma_connection_release(connection_);
    if (device_)
      magma_device_release(device_);
    if (fd_ >= 0)
      close(fd_);
  }

  int fd() { return fd_; }

  magma_connection_t connection() { return connection_; }

  static uint64_t get_ns_monotonic() {
    struct timespec time;
    int ret = clock_gettime(CLOCK_MONOTONIC, &time);
    if (ret < 0)
      return 0;
    return static_cast<uint64_t>(time.tv_sec) * 1000000000ULL + time.tv_nsec;
  }

  static void CheckPoll(int fd, magma_semaphore_t semaphore, int expected_status) {
    struct pollfd pfd = {
        .fd = fd,
        .events = POLLIN,
        .revents = 0,
    };

    int ret = poll(&pfd, 1, /*timeout=*/0);

    if (expected_status == 1) {
      EXPECT_EQ(ret, 1);
      EXPECT_EQ(pfd.revents, pfd.events);
    } else {
      EXPECT_EQ(ret, 0);
      EXPECT_EQ(pfd.revents, 0);
    }

    if (!semaphore) {
      return;
    }

    magma_poll_item_t item = {
        .semaphore = semaphore,
        .type = MAGMA_POLL_TYPE_SEMAPHORE,
        .condition = MAGMA_POLL_CONDITION_SIGNALED,
        .result = 0,
    };

    magma_status_t status = magma_poll(&item, 1, /*timeout=*/0);

    if (expected_status == 1) {
      EXPECT_EQ(status, MAGMA_STATUS_OK);
      EXPECT_EQ(item.result, item.condition);
    } else {
      EXPECT_EQ(status, MAGMA_STATUS_TIMED_OUT);
      EXPECT_EQ(item.result, 0u);
    }
  }

  // Retrieve the sync file info and check status.
  // If a magma semaphore is provided, use it to signal and reset the sync file while checking info.
  void Test(magma_semaphore_t semaphore, const char* expected_name, int fd,
            std::vector<int>& initial_fence_state, bool one_shot = false) {
    ASSERT_TRUE(is_valid_handle(fd));

    // Signaled fences get deleted.
    uint32_t merged_fence_count =
        std::max(1u, static_cast<uint32_t>(
                         std::count(initial_fence_state.begin(), initial_fence_state.end(), 0)));

    int expected_status = 1;
    for (int status : initial_fence_state) {
      if (status != 1) {
        expected_status = 0;
      }
    }

    // Get the number of fences
    struct sync_file_info info = {};
    ASSERT_EQ(0, ioctl(fd, SYNC_IOC_FILE_INFO, &info));
    ASSERT_EQ(info.num_fences, merged_fence_count);

    // Allocate info storage
    std::vector<sync_fence_info> fence_info(info.num_fences);
    info = {
        .num_fences = static_cast<uint32_t>(fence_info.size()),
        .sync_fence_info = reinterpret_cast<uintptr_t>(fence_info.data()),
    };
    ASSERT_EQ(0, ioctl(fd, SYNC_IOC_FILE_INFO, &info));

    // For testing convenience, we assume null terminated strings
    ASSERT_EQ(info.name[31], 0);
    EXPECT_STREQ(info.name, expected_name);
    EXPECT_EQ(info.status, expected_status);
    EXPECT_EQ(info.flags, 0u);
    EXPECT_EQ(info.num_fences, merged_fence_count);

    for (uint32_t i = 0; i < info.num_fences; i++) {
      // For testing convenience, we assume null terminated strings
      ASSERT_EQ(fence_info[i].obj_name[31], 0);
      ASSERT_EQ(fence_info[i].driver_name[31], 0);
      EXPECT_STREQ(fence_info[i].obj_name, "");
      EXPECT_STREQ(fence_info[i].driver_name, DRIVER_NAME);
      EXPECT_EQ(fence_info[i].flags, 0u);
      if (expected_status) {
        EXPECT_EQ(fence_info[i].status, 1);
        EXPECT_NE(fence_info[i].timestamp_ns, 0u);
      } else {
        EXPECT_EQ(fence_info[i].status, 0);
        EXPECT_EQ(fence_info[i].timestamp_ns, 0u);
      }
    }

    ASSERT_NO_FATAL_FAILURE(CheckPoll(fd, semaphore, expected_status));

    if (!semaphore) {
      return;
    }

    // Signal the magma semaphore and check the sync file.
    // Note: magma signaling updates the timestamp even if already signaled.
    uint64_t low_signal_time, high_signal_time;
    {
      low_signal_time = get_ns_monotonic();
      magma_semaphore_signal(semaphore);
      high_signal_time = get_ns_monotonic();
    }

    std::vector<sync_fence_info> signal_fence_info(info.num_fences);
    info = {
        .num_fences = static_cast<uint32_t>(signal_fence_info.size()),
        .sync_fence_info = reinterpret_cast<uintptr_t>(signal_fence_info.data()),
    };
    ASSERT_EQ(0, ioctl(fd, SYNC_IOC_FILE_INFO, &info));

    // For testing convenience, we assume null terminated strings
    ASSERT_EQ(info.name[31], 0);
    EXPECT_STREQ(info.name, expected_name);
    EXPECT_EQ(info.status, 1);
    EXPECT_EQ(info.flags, 0u);
    EXPECT_EQ(info.num_fences, merged_fence_count);
    for (uint32_t i = 0; i < info.num_fences; i++) {
      // For testing convenience, we assume null terminated strings
      ASSERT_EQ(fence_info[i].obj_name[31], 0);
      ASSERT_EQ(fence_info[i].driver_name[31], 0);
      EXPECT_STREQ(signal_fence_info[i].obj_name, "");
      EXPECT_STREQ(signal_fence_info[i].driver_name, DRIVER_NAME);
      EXPECT_EQ(signal_fence_info[i].flags, 0u);
      EXPECT_EQ(signal_fence_info[i].status, 1);
      EXPECT_GT(signal_fence_info[i].timestamp_ns, low_signal_time);
      EXPECT_LT(signal_fence_info[i].timestamp_ns, high_signal_time);
    }

    ASSERT_NO_FATAL_FAILURE(CheckPoll(fd, semaphore, /*expected_status=*/1));

    // Reset the magma semaphore and check the sync file.
    magma_semaphore_reset(semaphore);

    std::vector<sync_fence_info> reset_fence_info(info.num_fences);
    info = {
        .num_fences = static_cast<uint32_t>(reset_fence_info.size()),
        .sync_fence_info = reinterpret_cast<uintptr_t>(reset_fence_info.data()),
    };
    ASSERT_EQ(0, ioctl(fd, SYNC_IOC_FILE_INFO, &info));

    // For testing convenience, we assume null terminated strings
    ASSERT_EQ(info.name[31], 0);
    EXPECT_STREQ(info.name, expected_name);
    EXPECT_EQ(info.status, one_shot ? 1 : 0);
    EXPECT_EQ(info.flags, 0u);
    EXPECT_EQ(info.num_fences, merged_fence_count);
    for (uint32_t i = 0; i < info.num_fences; i++) {
      // For testing convenience, we assume null terminated strings
      ASSERT_EQ(fence_info[i].obj_name[31], 0);
      ASSERT_EQ(fence_info[i].driver_name[31], 0);
      EXPECT_STREQ(reset_fence_info[i].obj_name, "");
      EXPECT_STREQ(reset_fence_info[i].driver_name, DRIVER_NAME);
      EXPECT_EQ(reset_fence_info[i].flags, 0u);
      EXPECT_EQ(reset_fence_info[i].status, one_shot ? 1 : 0);
      if (one_shot) {
        EXPECT_EQ(reset_fence_info[i].timestamp_ns, signal_fence_info[i].timestamp_ns);
      } else {
        EXPECT_EQ(reset_fence_info[i].timestamp_ns, 0u);
      }
    }

    expected_status = one_shot ? 1 : 0;
    ASSERT_NO_FATAL_FAILURE(CheckPoll(fd, semaphore, expected_status));
  }

  enum class TestMergeType {
    NONE_SIGNALED,
    FIRST_SIGNALED,
    SECOND_SIGNALED,
    MERGE_SELF,
    MERGE_PAIR_OF_PAIRS,
  };

  int CreateMergedSyncFile(TestMergeType merge_type,
                           std::vector<magma_semaphore_t>* semaphores_out = nullptr) {
    int fd[2] = {-1};

    for (uint32_t i = 0; i < 2; i++) {
      if (merge_type == TestMergeType::MERGE_PAIR_OF_PAIRS) {
        fd[i] = CreateMergedSyncFile(TestMergeType::NONE_SIGNALED, semaphores_out);
        continue;
      }
      if (fd[0] >= 0 && merge_type == TestMergeType::MERGE_SELF) {
        fd[i] = fd[0];
        continue;
      }

      magma_semaphore_t semaphore = 0;
      magma_semaphore_id_t semaphore_id = 0;
      EXPECT_EQ(MAGMA_STATUS_OK,
                magma_connection_create_semaphore(connection(), &semaphore, &semaphore_id));

      magma_handle_t handle = 0;
      EXPECT_EQ(MAGMA_STATUS_OK, magma_semaphore_export(semaphore, &handle));

      switch (merge_type) {
        case TestMergeType::FIRST_SIGNALED:
          if (i == 0) {
            magma_semaphore_signal(semaphore);
          }
          break;
        case TestMergeType::SECOND_SIGNALED:
          if (i == 1) {
            magma_semaphore_signal(semaphore);
          }
          break;
        default:
          break;
      }

      fd[i] = static_cast<int>(handle);

      if (semaphores_out) {
        semaphores_out->push_back(semaphore);
      } else {
        magma_connection_release_semaphore(connection(), semaphore);
      }
    }

    struct sync_merge_data merge_data = {
        .name = MERGED_SYNC_FILE_NAME,
        .fd2 = fd[1],
        .fence = 0,  // returned fd
        .flags = 0,
    };
    EXPECT_EQ(0, ioctl(fd[0], SYNC_IOC_MERGE, &merge_data));

    for (uint32_t i = 0; i < 2; i++) {
      close(fd[i]);
    }

    int new_fd = merge_data.fence;
    EXPECT_GE(new_fd, 0);

    return new_fd;
  }

  void TestMerge(TestMergeType merge_type, bool one_shot = false) {
    int merged_fd = CreateMergedSyncFile(merge_type);
    ASSERT_TRUE(is_valid_handle(merged_fd));

    std::vector<int> initial_fence_state;
    switch (merge_type) {
      case TestMergeType::FIRST_SIGNALED:
        initial_fence_state = {1, 0};
        break;
      case TestMergeType::SECOND_SIGNALED:
        initial_fence_state = {0, 1};
        break;
      case TestMergeType::MERGE_SELF:
        initial_fence_state = {0};
        break;
      case TestMergeType::MERGE_PAIR_OF_PAIRS:
        initial_fence_state = {0, 0, 0, 0};
        break;
      default:
        initial_fence_state = {0, 0};
    }

    Test(/*magma_semaphore_t=*/0, MERGED_SYNC_FILE_NAME, merged_fd, initial_fence_state, one_shot);

    close(merged_fd);
  }

  void TestMultiPoll(uint32_t count) {
    struct SyncFile {
      int fd = -1;
      magma_semaphore_t semaphore;

      void release(magma_connection_t connection) {
        close(fd);
        magma_connection_release_semaphore(connection, semaphore);
      }
    };

    std::vector<SyncFile> sync_files;

    for (uint32_t i = 0; i < count; i++) {
      SyncFile sync_file = {};
      magma_semaphore_id_t semaphore_id = 0;
      ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_semaphore(
                                     connection(), &sync_file.semaphore, &semaphore_id));

      magma_handle_t handle = 0;
      ASSERT_EQ(MAGMA_STATUS_OK, magma_semaphore_export(sync_file.semaphore, &handle));

      sync_file.fd = static_cast<int>(handle);
      sync_files.push_back(std::move(sync_file));
    }

    {
      std::vector<struct pollfd> pfds;
      for (auto& sf : sync_files) {
        pfds.push_back(pollfd{
            .fd = sf.fd,
            .events = POLLIN,
            .revents = 0,
        });
      }
      EXPECT_EQ(0, poll(pfds.data(), pfds.size(), /*timeout=*/0));
    }

    // Signal in batches
    uint32_t batch_size = std::max(1u, static_cast<uint32_t>(sync_files.size()) / 2);
    uint32_t semaphore_index = 0;

    for (int ret = 0; ret < static_cast<int>(sync_files.size());) {
      int count = 0;
      for (uint32_t i = 0; i < batch_size; i++) {
        if (semaphore_index + count < sync_files.size()) {
          magma_semaphore_signal(sync_files[semaphore_index + count].semaphore);
          count += 1;
        }
      }

      {
        std::vector<struct pollfd> pfds;
        for (auto& sf : sync_files) {
          pfds.push_back(pollfd{
              .fd = sf.fd,
              .events = POLLIN,
              .revents = 0,
          });
        }
        ret = poll(pfds.data(), pfds.size(), /*timeout=*/0);
        EXPECT_GE(ret, count);
        semaphore_index += count;
      }
    }

    for (auto& sf : sync_files) {
      sf.release(connection());
    }
  }

  void TestMultiEpollWait(uint32_t count) {
    struct SyncFile {
      int fd = -1;
      magma_semaphore_t semaphore;

      void release(magma_connection_t connection) {
        close(fd);
        magma_connection_release_semaphore(connection, semaphore);
      }
    };

    std::vector<SyncFile> sync_files;

    for (uint32_t i = 0; i < count; i++) {
      SyncFile sync_file = {};
      magma_semaphore_id_t semaphore_id = 0;
      ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_semaphore(
                                     connection(), &sync_file.semaphore, &semaphore_id));

      magma_handle_t handle = 0;
      ASSERT_EQ(MAGMA_STATUS_OK, magma_semaphore_export(sync_file.semaphore, &handle));

      sync_file.fd = static_cast<int>(handle);
      sync_files.push_back(std::move(sync_file));
    }

    int epollfd = epoll_create1(/*flags=*/0);
    ASSERT_GE(epollfd, 0);

    // Add all to epoll instance
    for (auto& sf : sync_files) {
      struct epoll_event event = {
          .events = EPOLLIN,
          .data = {.fd = sf.fd},
      };
      ASSERT_EQ(0, epoll_ctl(epollfd, EPOLL_CTL_ADD, sf.fd, &event));
    }

    {
      constexpr int kMaxEvents = 10;
      struct epoll_event events[kMaxEvents];
      EXPECT_EQ(0, epoll_wait(epollfd, events, kMaxEvents, 0));
    }

    // Signal in batches
    uint32_t batch_size = std::max(1u, static_cast<uint32_t>(sync_files.size()) / 2);
    uint32_t semaphore_index = 0;

    for (int ret = 0; ret < static_cast<int>(sync_files.size());) {
      int count = 0;
      for (uint32_t i = 0; i < batch_size; i++) {
        if (semaphore_index + count < sync_files.size()) {
          magma_semaphore_signal(sync_files[semaphore_index + count].semaphore);
          count += 1;
        }
      }

      {
        constexpr int kMaxEvents = 10;
        struct epoll_event events[kMaxEvents];
        ret = epoll_wait(epollfd, events, kMaxEvents, -1);
        EXPECT_GE(ret, count);
        semaphore_index += count;
      }
    }

    close(epollfd);

    for (auto& sf : sync_files) {
      sf.release(connection());
    }
  }

  void TestMultiMergedEpollCancel(uint32_t count) {
    struct SyncFile {
      int fd = -1;
      std::vector<magma_semaphore_t> semaphores;

      void release(magma_connection_t connection) {
        close(fd);
        for (auto& semaphore : semaphores) {
          magma_connection_release_semaphore(connection, semaphore);
        }
      }
    };

    std::vector<SyncFile> sync_files;

    // +1 to ensure we can exclude the last and still have one to signal
    for (uint32_t i = 0; i < count + 1; i++) {
      SyncFile sync_file = {};

      sync_file.fd = CreateMergedSyncFile(TestMergeType::NONE_SIGNALED, &sync_file.semaphores);
      ASSERT_GE(sync_file.fd, 0);

      sync_files.push_back(std::move(sync_file));
    }

    int epollfd = epoll_create1(/*flags=*/0);
    ASSERT_GE(epollfd, 0);

    // Add all
    for (auto& sf : sync_files) {
      struct epoll_event event = {
          .events = EPOLLIN,
          .data = {.fd = sf.fd},
      };
      ASSERT_EQ(0, epoll_ctl(epollfd, EPOLL_CTL_ADD, sf.fd, &event));
    }

    {
      // Check for no events pending.
      constexpr int kMaxEvents = 10;
      struct epoll_event events[kMaxEvents];
      EXPECT_EQ(0, epoll_wait(epollfd, events, kMaxEvents, /*timeout=*/0));
    }

    // Remove from epoll instance all but the last
    for (auto& sf : sync_files) {
      if (sf.fd == sync_files.back().fd)
        continue;

      struct epoll_event event = {
          .events = EPOLLIN,
          .data = {.fd = sf.fd},
      };
      ASSERT_EQ(0, epoll_ctl(epollfd, EPOLL_CTL_DEL, sf.fd, &event));
    }

    // Partial signal
    for (auto& sf : sync_files) {
      for (auto& semaphore : sf.semaphores) {
        if (semaphore == sf.semaphores.back())
          continue;
        magma_semaphore_signal(semaphore);
      }
    }

    {
      // Check for no events pending.
      constexpr int kMaxEvents = 10;
      struct epoll_event events[kMaxEvents];
      EXPECT_EQ(0, epoll_wait(epollfd, events, kMaxEvents, /*timeout=*/0));
    }

    // Signal all
    for (auto& sf : sync_files) {
      for (auto& semaphore : sf.semaphores) {
        magma_semaphore_signal(semaphore);
      }
    }

    {
      // Check for only the last event
      constexpr int kMaxEvents = 10;
      struct epoll_event events[kMaxEvents];
      EXPECT_EQ(1, epoll_wait(epollfd, events, kMaxEvents, /*timeout=*/0));
      EXPECT_EQ(events[0].data.fd, sync_files.back().fd);
    }

    close(epollfd);

    for (auto& sf : sync_files) {
      sf.release(connection());
    }
  }

  // Tests import of a merged sync file into a (parent) magma semaphore.
  void TestImportMergedMultiPoll(uint32_t count) {
    struct SyncFile {
      int fd = -1;
      std::vector<magma_semaphore_t> child_semaphores;
      magma_semaphore_t parent_semaphore = 0;

      void release(magma_connection_t connection) {
        close(fd);
        magma_connection_release_semaphore(connection, parent_semaphore);
        for (auto& semaphore : child_semaphores) {
          magma_connection_release_semaphore(connection, semaphore);
        }
      }
    };

    std::vector<SyncFile> sync_files(count);

    for (auto& sf : sync_files) {
      sf.fd = CreateMergedSyncFile(TestMergeType::NONE_SIGNALED, &sf.child_semaphores);
      ASSERT_TRUE(is_valid_handle(sf.fd));

      int import_fd = dup(sf.fd);
      ASSERT_TRUE(is_valid_handle(import_fd));

      magma_semaphore_id_t semaphore_id = 0;
      magma_handle_t handle = import_fd;
      uint64_t flags = 0;
      ASSERT_EQ(MAGMA_STATUS_OK,
                magma_connection_import_semaphore2(connection(), handle, flags,
                                                   &sf.parent_semaphore, &semaphore_id));
    }

    std::vector<struct pollfd> pfds;
    std::vector<magma_poll_item_t> pitems;
    std::vector<int> indices;

    {
      int index = 0;

      for (auto& sf : sync_files) {
        pfds.push_back(pollfd{
            .fd = sf.fd,
            .events = POLLIN,
            .revents = 0,
        });

        pitems.push_back(magma_poll_item_t{
            .semaphore = sf.parent_semaphore,
            .type = MAGMA_POLL_TYPE_SEMAPHORE,
            .condition = MAGMA_POLL_CONDITION_SIGNALED,
            .result = 0,
        });

        indices.push_back(index);
        index++;
      }
    }

    std::shuffle(indices.begin(), indices.end(), std::default_random_engine());

    {
      int signaled_count = 0;

      // Check the sync file status with poll
      for (int index : indices) {
        magma_semaphore_signal(sync_files[index].child_semaphores[0]);

        ASSERT_EQ(signaled_count, poll(pfds.data(), pfds.size(), /*timeout=*/0));
        ASSERT_EQ(pfds[index].revents, 0);

        magma_semaphore_signal(sync_files[index].child_semaphores[1]);

        signaled_count += 1;

        ASSERT_EQ(signaled_count, poll(pfds.data(), pfds.size(), /*timeout=*/0));
        ASSERT_EQ(pfds[index].revents, pfds[index].events);
      }
    }

    // Reset the sync file semaphores
    for (auto& sf : sync_files) {
      magma_semaphore_reset(sf.parent_semaphore);
    }

    {
      magma_status_t status = MAGMA_STATUS_TIMED_OUT;

      // Check the parent semaphore status with magma_poll
      for (int index : indices) {
        magma_semaphore_signal(sync_files[index].child_semaphores[0]);

        ASSERT_EQ(status, magma_poll(pitems.data(), count, /*timeout=*/0));
        ASSERT_EQ(pitems[index].condition, MAGMA_POLL_CONDITION_SIGNALED);
        ASSERT_EQ(pitems[index].result, 0u);

        magma_semaphore_signal(sync_files[index].child_semaphores[1]);

        status = MAGMA_STATUS_OK;

        ASSERT_EQ(status, magma_poll(pitems.data(), count, /*timeout=*/0));
        ASSERT_EQ(pitems[index].condition, MAGMA_POLL_CONDITION_SIGNALED);
        ASSERT_EQ(pitems[index].result, MAGMA_POLL_CONDITION_SIGNALED);
      }
    }

    for (auto& sf : sync_files) {
      sf.release(connection());
    }
  }

 private:
  std::string device_name_;
  int fd_ = -1;
  magma_device_t device_ = 0;
  magma_connection_t connection_ = 0;
};

TEST_F(TestMagmaSyncFile, Info) {
  magma_semaphore_t semaphore = 0;
  magma_semaphore_id_t semaphore_id = 0;
  ASSERT_EQ(MAGMA_STATUS_OK,
            magma_connection_create_semaphore(connection(), &semaphore, &semaphore_id));

  magma_handle_t handle = 0;
  ASSERT_EQ(MAGMA_STATUS_OK, magma_semaphore_export(semaphore, &handle));

  int fd = static_cast<int>(handle);

  std::vector<int> initial_fence_state{0};
  Test(semaphore, SYNC_FILE_NAME, fd, initial_fence_state);

  close(fd);

  magma_connection_release_semaphore(connection(), semaphore);
}

TEST_F(TestMagmaSyncFile, InfoMergeFirstSignaled) { TestMerge(TestMergeType::FIRST_SIGNALED); }

TEST_F(TestMagmaSyncFile, InfoMergeFirstSignaledOneShot) {
  TestMerge(TestMergeType::FIRST_SIGNALED, /*one_shot=*/true);
}

TEST_F(TestMagmaSyncFile, InfoMergeSecondSignaled) { TestMerge(TestMergeType::SECOND_SIGNALED); }

TEST_F(TestMagmaSyncFile, InfoMergeSecondSignaledOneShot) {
  TestMerge(TestMergeType::SECOND_SIGNALED, /*one_shot=*/true);
}

TEST_F(TestMagmaSyncFile, InfoMergeSelf) { TestMerge(TestMergeType::MERGE_SELF); }

TEST_F(TestMagmaSyncFile, InfoMergePairOfPairs) { TestMerge(TestMergeType::MERGE_PAIR_OF_PAIRS); }

class TestMagmaSyncFileWithCount : public TestMagmaSyncFile,
                                   public testing::WithParamInterface<uint32_t> {};

TEST_P(TestMagmaSyncFileWithCount, MultiPoll) {
  uint32_t count = GetParam();
  TestMultiPoll(count);
}

TEST_P(TestMagmaSyncFileWithCount, MultiEpollWait) {
  uint32_t count = GetParam();
  TestMultiEpollWait(count);
}

TEST_P(TestMagmaSyncFileWithCount, MultiMergedEpollCancel) {
  uint32_t count = GetParam();
  TestMultiMergedEpollCancel(count);
}

TEST_P(TestMagmaSyncFileWithCount, ImportMergedMultiPoll) {
  uint32_t count = GetParam();
  TestImportMergedMultiPoll(count);
}

INSTANTIATE_TEST_SUITE_P(TestMagmaSyncFileWithCount, TestMagmaSyncFileWithCount,
                         testing::Values(1, 2, 3, 5, 10),
                         [](const testing::TestParamInfo<uint32_t>& info) {
                           return std::to_string(info.param);
                         });
