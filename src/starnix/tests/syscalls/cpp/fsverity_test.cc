// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/ioctl.h>
#include <sys/uio.h>
#include <sys/vfs.h>

#include <cerrno>
#include <cstdint>

#include <gtest/gtest.h>
#include <linux/fs.h>
#include <linux/fsverity.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

constexpr uint32_t kMinfs = 0x6e694d21;

#if !defined(__arm__)
// fidl doesn't compile to arch32. Check that the previous constant has the
// correct value
#include "fidl/fuchsia.fs/cpp/common_types.h"
#include "fidl/fuchsia.fs/cpp/wire.h"

static_assert(kMinfs == static_cast<uint32_t>(fuchsia_fs::VfsType::kMinfs));
#endif

#ifndef FS_VERITY_METADATA_TYPE_MERKLE_TREE

// Assume we are targeting an older sysroot without these symbols
// and define them ourselves.

struct fsverity_descriptor {
  uint8_t version;          /* must be 1 */
  uint8_t hash_algorithm;   /* Merkle tree hash algorithm */
  uint8_t log_blocksize;    /* log2 of size of data and tree blocks */
  uint8_t salt_size;        /* size of salt in bytes; 0 if none */
  uint32_t __reserved_0x04; /* must be 0 */
  uint64_t data_size;       /* size of file the Merkle tree is built over */
  uint8_t root_hash[64];    /* Merkle tree root hash */
  uint8_t salt[32];         /* salt prepended to each hashed block */
  uint8_t __reserved[144];  /* must be 0's */
};

#define FS_VERITY_METADATA_TYPE_MERKLE_TREE 1
#define FS_VERITY_METADATA_TYPE_DESCRIPTOR 2
#define FS_VERITY_METADATA_TYPE_SIGNATURE 3

struct fsverity_read_metadata_arg {
  uint64_t metadata_type;
  uint64_t offset;
  uint64_t length;
  uint64_t buf_ptr;
  uint64_t __reserved;
};

#define FS_IOC_ENABLE_VERITY _IOW('f', 133, struct fsverity_enable_arg)
#define FS_IOC_MEASURE_VERITY _IOWR('f', 134, struct fsverity_digest)
#define FS_IOC_READ_VERITY_METADATA _IOWR('f', 135, struct fsverity_read_metadata_arg)

#endif  // FS_VERITY_METADATA_TYPE_MERKLE_TREE

namespace {

class FsverityTest : public ::testing::Test {
 public:
  void SetUp() override {
    if (!test_helper::IsStarnix()) {
      // TODO(https://fxbug.dev/302596745): Find a way to support this.
      GTEST_SKIP()
          << "This test does not generally work on Linux as it requires a kernel with fsverity.";
    }

    const char *tmpdir = getenv("MUTABLE_STORAGE");
    test_filename_ = std::string(tmpdir) + "/fsverity";

    int fd = open(test_filename_.c_str(), O_CREAT | O_RDWR, S_IRUSR | S_IWUSR);
    ASSERT_GT(fd, 0) << fd << " errno:" << errno;
    write(fd, "foo", 3);
    close(fd);
  }

  void TearDown() override { unlink(test_filename_.c_str()); }

 protected:
  const char *fname() const { return test_filename_.c_str(); }

 private:
  std::string test_filename_;
};

TEST(FsverityUnitTest, OriginalFileHandle) {
  if (!test_helper::IsStarnix()) {
    // TODO(https://fxbug.dev/302596745): Find a way to support this.
    GTEST_SKIP()
        << "This test does not generally work on Linux as it requires a kernel with fsverity.";
  }

  const char *tmpdir = getenv("MUTABLE_STORAGE");
  std::string filename = std::string(tmpdir) + "/fsverity";
  int fd = open(filename.c_str(), O_CREAT | O_RDWR);
  ASSERT_GT(fd, 0) << fd << " errno:" << errno;
  write(fd, "foo", 3);

  // Can't enable fsverity using original writable file handle.
  {
    fsverity_enable_arg arg = {
        .version = 1, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 4096};
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1);
    ASSERT_EQ(errno, ETXTBSY);
  }

  // Can't enable if we still have the a writable file handle open.
  {
    int fd2 = open(filename.c_str(), O_RDONLY);
    ASSERT_GT(fd2, 0) << fd2;
    fsverity_enable_arg arg = {
        .version = 1, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 4096};
    ASSERT_EQ(ioctl(fd2, FS_IOC_ENABLE_VERITY, &arg), -1);
    ASSERT_EQ(errno, ETXTBSY);
    close(fd2);
  }
  close(fd);
}

TEST_F(FsverityTest, Version) {
  int fd = open(fname(), O_RDONLY);
  ASSERT_GT(fd, 0);
  fsverity_enable_arg arg = {
      .version = 2, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 4096};
  ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1);
  ASSERT_EQ(errno, EINVAL);
  close(fd);
}

TEST_F(FsverityTest, BlockSize) {
  int fd = open(fname(), O_RDONLY);
  ASSERT_GT(fd, 0);
  {
    fsverity_enable_arg arg = {
        .version = 1, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 512};
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1);
    ASSERT_EQ(errno, EINVAL);
  }
  {
    fsverity_enable_arg arg = {
        .version = 1, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 1025};
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1);
    ASSERT_EQ(errno, EINVAL);
  }
  {
    fsverity_enable_arg arg = {
        .version = 1, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 16384};
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1);
    ASSERT_EQ(errno, EINVAL);
  }
  close(fd);
}

TEST_F(FsverityTest, HashAlgorithm) {
  int fd = open(fname(), O_RDONLY);
  ASSERT_GT(fd, 0);
  // Unsupported/invalid algorithm.
  {
    fsverity_enable_arg arg = {.version = 1, .hash_algorithm = 9, .block_size = 4096};
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1);
    ASSERT_EQ(errno, ENOTSUP);
  }
  close(fd);
}
TEST_F(FsverityTest, Salt) {
  int fd = open(fname(), O_RDONLY);
  ASSERT_GT(fd, 0);
  // Very long salt is not supported.
  {
    char salt[64] = "1234";
    fsverity_enable_arg arg = {.version = 1,
                               .hash_algorithm = FS_VERITY_HASH_ALG_SHA256,
                               .block_size = 4096,
                               .salt_size = 48,
                               .salt_ptr = reinterpret_cast<uint64_t>(&salt[0])};
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1);
    ASSERT_EQ(errno, EINVAL);
  }
  close(fd);
}

TEST_F(FsverityTest, Signatures) {
  int fd = open(fname(), O_RDONLY);
  ASSERT_GT(fd, 0);
  // Signatures not yet supported.
  {
    fsverity_enable_arg arg = {.version = 1,
                               .hash_algorithm = FS_VERITY_HASH_ALG_SHA256,
                               .block_size = 4096,
                               .sig_size = 1};
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1);
    ASSERT_EQ(errno, ENOTSUP);
  }
  close(fd);
}

TEST_F(FsverityTest, MeasureVerityWhenNotVerity) {
  int fd = open(fname(), O_RDONLY);
  ASSERT_GT(fd, 0);
  // We should get ENODATA if we ask for the digest.
  char buf[64];
  struct fsverity_digest *arg = reinterpret_cast<struct fsverity_digest *>(&buf);
  arg->digest_size = 32;
  ASSERT_EQ(ioctl(fd, FS_IOC_MEASURE_VERITY, arg), -1);
  ASSERT_EQ(errno, ENODATA);
  close(fd);
}

TEST_F(FsverityTest, MeasureVerityOverflowDigestSize) {
  int fd = open(fname(), O_RDONLY);
  ASSERT_GT(fd, 0);
  bool is_minfs;
  {
    struct statfs64 fs;
    SAFE_SYSCALL(statfs64(fname(), &fs));
    is_minfs = fs.f_type == kMinfs;
  }

  // Valid enable request, no salt, no sig.
  {
    fsverity_enable_arg arg = {
        .version = 1, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 4096};
    if (is_minfs) {
      ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1) << errno;
      ASSERT_EQ(errno, EOPNOTSUPP);
      close(fd);
      return;
    }
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), 0) << errno;
  }

  {
    char buf[64];
    struct fsverity_digest *arg = reinterpret_cast<struct fsverity_digest *>(&buf);
    arg->digest_size = 31;
    // Ugly polling wait for fsverity to build.
    for (int i = 0; i < 10000; i++) {
      if (ioctl(fd, FS_IOC_MEASURE_VERITY, arg) == -1 && errno == EOVERFLOW) {
        return;
      }
      ASSERT_EQ(errno, ENODATA);
      usleep(10000);
    }
    ASSERT_TRUE(false) << "fsverity did not finish building after 10000 loop iterations";
  }
}

TEST_F(FsverityTest, MeasureVeritySetDigestAlgorithm) {
  int fd = open(fname(), O_RDONLY);
  ASSERT_GT(fd, 0);
  bool is_minfs;
  {
    struct statfs64 fs;
    ASSERT_EQ(statfs64(fname(), &fs), 0);
    is_minfs = fs.f_type == kMinfs;
  }

  // Valid enable request, no salt, no sig.
  {
    fsverity_enable_arg arg = {
        .version = 1, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 4096};
    if (is_minfs) {
      ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1) << errno;
      ASSERT_EQ(errno, EOPNOTSUPP);
      close(fd);
      return;
    }
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), 0) << errno;
  }

  // Now we should get back a digest for the data once fsverity has finished building.
  {
    char buf[64];
    struct fsverity_digest *arg = reinterpret_cast<struct fsverity_digest *>(&buf);
    arg->digest_size = 32;
    // 3 is an invalid digest algorithm. Test that FS_IOC_MEASURE_VERITY ignores values set
    // on an output field.
    arg->digest_algorithm = 3;
    // Ugly polling wait for fsverity to build.
    for (int i = 0; i < 10000; i++) {
      if (ioctl(fd, FS_IOC_MEASURE_VERITY, arg) == 0) {
        break;
      }
      ASSERT_EQ(errno, ENODATA);
      usleep(10000);
    }
    ASSERT_EQ(ioctl(fd, FS_IOC_MEASURE_VERITY, arg), 0) << errno;
    ASSERT_EQ(arg->digest_algorithm, FS_VERITY_HASH_ALG_SHA256);
    ASSERT_EQ(arg->digest_size, 32);

    // Obtained via:
    // ```
    // $ echo -ne "foo" > /tmp/foo.txt
    // $ fsverity digest /tmp/foo.txt
    // sha256:84c7384b3239274691380d7042dc3d8c13f9e606ef546544fe9e348afb0e8af5 /tmp/foo.txt
    // ```
    uint8_t expected_digest[32] = {0x84, 0xc7, 0x38, 0x4b, 0x32, 0x39, 0x27, 0x46, 0x91, 0x38, 0x0d,
                                   0x70, 0x42, 0xdc, 0x3d, 0x8c, 0x13, 0xf9, 0xe6, 0x06, 0xef, 0x54,
                                   0x65, 0x44, 0xfe, 0x9e, 0x34, 0x8a, 0xfb, 0x0e, 0x8a, 0xf5

    };
    ASSERT_TRUE(memcmp(&expected_digest[0], arg->digest, 32) == 0);
  }
}

TEST_F(FsverityTest, EnableVerity) {
  int fd = open(fname(), O_RDONLY);
  ASSERT_GT(fd, 0);
  // TODO(https://fxbug.dev/300003181): Replace this when we switch to native support.
  // We are currently storing these ioctls in extended attributes which minfs
  // does not support. Minfs here acts as a test of the "ENOTSUP" case.
  bool is_minfs;
  {
    struct statfs64 fs;
    SAFE_SYSCALL(statfs64(fname(), &fs));
    is_minfs = fs.f_type == kMinfs;
  }

  // Enabling when there is an open write handle should fail with ETXTBSY
  {
    int fd = open(fname(), O_RDWR);
    fsverity_enable_arg arg = {
        .version = 1, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 4096};
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1) << errno;
    ASSERT_EQ(errno, ETXTBSY);
    close(fd);
  }

  // Valid enable request, no salt, no sig.
  {
    fsverity_enable_arg arg = {
        .version = 1, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 4096};
    if (is_minfs) {
      ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1) << errno;
      ASSERT_EQ(errno, EOPNOTSUPP);
      close(fd);
      return;
    }
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), 0) << errno;
  }
  // A second attempt should return EBUSY (if building) or EEXIST (if done).
  {
    fsverity_enable_arg arg = {
        .version = 1, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 4096};
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1);
    ASSERT_TRUE(errno == EBUSY || errno == EEXIST);
  }

  // Now we should get back a digest for the data once fsverity has finished building.
  {
    char buf[64];
    struct fsverity_digest *arg = reinterpret_cast<struct fsverity_digest *>(&buf);
    arg->digest_size = 32;
    // Ugly polling wait for fsverity to build.
    for (int i = 0; i < 10000; i++) {
      if (ioctl(fd, FS_IOC_MEASURE_VERITY, arg) == 0) {
        break;
      }
      ASSERT_EQ(errno, ENODATA);
      usleep(10000);
    }
    ASSERT_EQ(ioctl(fd, FS_IOC_MEASURE_VERITY, arg), 0) << errno;
    ASSERT_EQ(arg->digest_algorithm, FS_VERITY_HASH_ALG_SHA256);
    ASSERT_EQ(arg->digest_size, 32);

    // Obtained via:
    // ```
    // $ echo -ne "foo" > /tmp/foo.txt
    // $ fsverity digest /tmp/foo.txt
    // sha256:84c7384b3239274691380d7042dc3d8c13f9e606ef546544fe9e348afb0e8af5 /tmp/foo.txt
    // ```
    uint8_t expected_digest[32] = {0x84, 0xc7, 0x38, 0x4b, 0x32, 0x39, 0x27, 0x46, 0x91, 0x38, 0x0d,
                                   0x70, 0x42, 0xdc, 0x3d, 0x8c, 0x13, 0xf9, 0xe6, 0x06, 0xef, 0x54,
                                   0x65, 0x44, 0xfe, 0x9e, 0x34, 0x8a, 0xfb, 0x0e, 0x8a, 0xf5

    };
    ASSERT_TRUE(memcmp(&expected_digest[0], arg->digest, 32) == 0);
  }
  // Enabling now should return EEXIST
  {
    fsverity_enable_arg arg = {
        .version = 1, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 4096};
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1);
    ASSERT_EQ(errno, EEXIST);
  }

  close(fd);

  // The file is now using fsverity. Check that it persists across a fresh file handle.
  fd = open(fname(), O_RDONLY);
  ASSERT_GT(fd, 0);

  {
    fsverity_enable_arg arg = {
        .version = 1, .hash_algorithm = FS_VERITY_HASH_ALG_SHA256, .block_size = 4096};
    ASSERT_EQ(ioctl(fd, FS_IOC_ENABLE_VERITY, &arg), -1);
    ASSERT_EQ(errno, EEXIST);
  }
  // TODO(https://fxbug.dev/300003181): Test FS_IOC_READ_VERITY_METADATA -- Merkle Tree (not
  // supported)
  {
    uint8_t buf[64];
    fsverity_read_metadata_arg arg = {
        .metadata_type = FS_VERITY_METADATA_TYPE_MERKLE_TREE,
        .offset = 0,
        .length = sizeof(buf),
        .buf_ptr = reinterpret_cast<__u64>(&buf),
    };
    ASSERT_EQ(ioctl(fd, FS_IOC_READ_VERITY_METADATA, &arg), -1);
    ASSERT_EQ(errno, ENOTSUP);
  }
  // Test FS_IOC_READ_VERITY_METADATA -- Descriptor
  {
    fsverity_descriptor descriptor = {};
    fsverity_read_metadata_arg arg = {
        .metadata_type = FS_VERITY_METADATA_TYPE_DESCRIPTOR,
        .offset = 0,
        .length = sizeof(descriptor),
        .buf_ptr = reinterpret_cast<__u64>(&descriptor),
    };
    ASSERT_EQ(ioctl(fd, FS_IOC_READ_VERITY_METADATA, &arg), 0);
    // Obtained via:
    // ```
    // $ echo -ne "foo" > /tmp/foo.txt
    // $ fsverity digest /tmp/foo.txt --out-descriptor=/tmp/descr
    // $ hexdump /tmp/descr -e "16/1 \"0x%02x,\" \"\n\"" -v
    uint8_t expected_descriptor[] = {
        0x01, 0x01, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0xdf, 0xfd, 0xd9, 0x7c, 0xfb, 0xf2, 0x88, 0xa7, 0x29, 0xf6,
        0xaf, 0x66, 0xf1, 0x2a, 0xc8, 0x88, 0x4f, 0xd7, 0x8d, 0xf3, 0xf1, 0x87, 0x6d,
        0xcc, 0xc5, 0x8b, 0x5a, 0xb2, 0x36, 0x83, 0x9b, 0x49, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00};
    ASSERT_TRUE(memcmp(&expected_descriptor[0], &descriptor, sizeof(expected_descriptor)) == 0);
  }
  // Test FS_IOC_READ_VERITY_METADATA -- Signature (not supported)
  {
    char buf[1024];
    fsverity_read_metadata_arg arg = {
        .metadata_type = FS_VERITY_METADATA_TYPE_SIGNATURE,
        .offset = 0,
        .length = sizeof(buf),
        .buf_ptr = reinterpret_cast<__u64>(&buf),
    };
    ASSERT_EQ(ioctl(fd, FS_IOC_READ_VERITY_METADATA, &arg), -1);
    ASSERT_EQ(errno, ENOTSUP);
  }

  // Test FS_IOC_GETFLAGS for FS_VERITY_FL.
  {
    // Regression test for https://fxbug.dev/421907931
    // FS_IOC_GETFLAGS should retrieve a 32 bit value in both 32 and 64 bit modes.
    // Pass a pointer to the middle of an array to verify that this is true.
    uint32_t flags[] = {0xabababab, 0xcdcdcdcd, 0xdededede};
    SAFE_SYSCALL(ioctl(fd, FS_IOC_GETFLAGS, &flags[1]));
    EXPECT_EQ(flags[0], 0xabababab);
    ASSERT_TRUE(flags[1] | FS_VERITY_FL);
    EXPECT_EQ(flags[2], 0xdededede);
  }
  close(fd);

  // The file is now using fsverity. We can't open it in write mode.
  {
    int fd = open(fname(), O_RDWR);
    ASSERT_EQ(fd, -1);
    ASSERT_EQ(errno, EACCES);
  }
}
//
// TODO(https://fxbug.dev/302604990): Test statx.

}  // namespace
