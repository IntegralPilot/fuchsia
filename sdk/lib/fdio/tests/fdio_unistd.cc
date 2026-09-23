// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fcntl.h>
#include <lib/fit/defer.h>
#include <lib/fit/result.h>
#include <limits.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <zxtest/zxtest.h>

#include "src/lib/fxl/strings/concatenate.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace {

// An invalid file descriptor that is not equal to AT_FDCWD.
constexpr int kInvalidFD = -1;
static_assert(kInvalidFD != AT_FDCWD);

std::string TestTempDirPath() {
  const char* tmp = getenv("TEST_TMPDIR");
  return std::string(tmp == nullptr ? "/tmp" : tmp);
}

enum class Existing {
  kNone,
  kFile,
  KDirectory,
};

class OpenCreate : public zxtest::TestWithParam<std::tuple<Existing, bool, bool>> {
 public:
  struct Param {
    explicit Param(ParamType info)
        : existing(std::get<0>(info)),
          o_directory_flag(std::get<1>(info)),
          trailing_slash(std::get<2>(info)) {}
    const Existing existing;
    const bool o_directory_flag;
    const bool trailing_slash;
  };
};

// TDOD(b/443987944): Test fails on debian 12 Linux. Re-enable once bug is fixed.
TEST_P(OpenCreate, DISABLED_DirectoryUndefinedBehavior) {
  const Param param(GetParam());
  const std::string base_filename = TestTempDirPath() + "/open-create";

  // Create precondition.
  {
    fbl::unique_fd fd;
    switch (param.existing) {
      case Existing::kNone: {
        struct stat s;
        ASSERT_EQ(-1, stat(base_filename.c_str(), &s), "%o", s.st_mode);
        ASSERT_EQ(ENOENT, errno, "%s", strerror(errno));
        break;
      }
      case Existing::kFile:
        ASSERT_GE(0, fd.reset(open(base_filename.c_str(), O_CREAT | O_EXCL, 0666)), "%s",
                  strerror(errno));
        break;
      case Existing::KDirectory:
        ASSERT_GE(0, fd.reset(mkdir(base_filename.c_str(), 0666)), "%s", strerror(errno));
        break;
    }
  }
  // Deferred precondition cleanup.
  auto cleanup = fit::defer([existing = param.existing, base_filename]() {
    switch (existing) {
      case Existing::kNone:
        break;
      case Existing::kFile:
        ASSERT_EQ(0, unlink(base_filename.c_str()), "%s", strerror(errno));
        break;
      case Existing::KDirectory:
        ASSERT_EQ(0, rmdir(base_filename.c_str()), "%s", strerror(errno));
        break;
    }
  });
  std::string filename = base_filename;
  if (param.trailing_slash) {
    filename.append("/");
  }
  int flags = O_CREAT;
  if (param.o_directory_flag) {
    flags |= O_DIRECTORY;
  }
  // +-----------+-------------+---+-------------------+-------------------+
  // | Existing  | O_DIRECTORY | / |       Linux       |      Fuchsia      |
  // |           |             |   | Errno   | Created | Errno   | Created |
  // +-----------+-------------+---+---------+---------+---------+---------+
  // | None      | N           | N | SUCCESS | Y       | SUCCESS | Y       |
  // | None      | N           | Y | EISDIR  | N       | EISDIR  | N       |
  // | None      | Y           | N | ENOTDIR | Y       | EINVAL  | N       |
  // | None      | Y           | Y | EISDIR  | N       | EINVAL  | N       |
  // +-----------+-------------+---+---------+---------+---------+---------+
  // | File      | N           | N | SUCCESS | N/A     | SUCCESS | N/A     |
  // | File      | N           | Y | EISDIR  | N/A     | EISDIR  | N/A     |
  // | File      | Y           | N | ENOTDIR | N/A     | EINVAL  | N/A     |
  // | File      | Y           | Y | EISDIR  | N/A     | EINVAL  | N/A     |
  // +-----------+-------------+---+---------+---------+---------+---------+
  // | DIRECTORY | N           | N | EISDIR  | N/A     | EISDIR  | N/A     |
  // | DIRECTORY | N           | Y | EISDIR  | N/A     | EISDIR  | N/A     |
  // | DIRECTORY | Y           | N | EISDIR  | N/A     | EINVAL  | N/A     |
  // | DIRECTORY | Y           | Y | EISDIR  | N/A     | EINVAL  | N/A     |
  // +-----------+-------------+---+---------+---------+---------+---------+
  const std::optional open_errno = [&]() -> std::optional<int> {
    const int result = open(filename.c_str(), flags, 0666);
    if (result < 0) {
      return errno;
    }
    EXPECT_EQ(0, close(result), "%s", strerror(errno));
    return {};
  }();
  switch (param.existing) {
    case Existing::kNone: {
      const std::optional unlink_errno = [&]() -> std::optional<int> {
        const int result = unlink(filename.c_str());
        if (result < 0) {
          return errno;
        }
        return {};
      }();
#if defined(__linux__)
      if (!param.trailing_slash) {
#else
      if (!param.o_directory_flag && !param.trailing_slash) {
#endif
        EXPECT_FALSE(unlink_errno.has_value(), "%s", strerror(unlink_errno.value()));
      } else {
        ASSERT_TRUE(unlink_errno.has_value());
        EXPECT_EQ(ENOENT, unlink_errno.value(), "%s", strerror(unlink_errno.value()));
      }
      [[fallthrough]];
    }
    case Existing::kFile:
#if defined(__linux__)
      if (param.trailing_slash) {
        ASSERT_TRUE(open_errno.has_value());
        EXPECT_EQ(EISDIR, open_errno.value(), "%s", strerror(open_errno.value()));
      } else if (param.o_directory_flag) {
        ASSERT_TRUE(open_errno.has_value());
        EXPECT_EQ(ENOTDIR, open_errno.value(), "%s", strerror(open_errno.value()));
      } else {
        EXPECT_FALSE(open_errno.has_value(), "%s", strerror(open_errno.value()));
      }
      break;
#else
      if (!param.o_directory_flag && !param.trailing_slash) {
        EXPECT_FALSE(open_errno.has_value(), "%s", strerror(open_errno.value()));
        break;
      }
      [[fallthrough]];
#endif
    case Existing::KDirectory:
#if defined(__linux__)
      ASSERT_TRUE(open_errno.has_value());
      EXPECT_EQ(EISDIR, open_errno.value(), "%s", strerror(open_errno.value()));
#else
      if (param.o_directory_flag) {
        ASSERT_TRUE(open_errno.has_value());
        EXPECT_EQ(EINVAL, open_errno.value(), "%s", strerror(open_errno.value()));
      } else {
        ASSERT_TRUE(open_errno.has_value());
        EXPECT_EQ(EISDIR, open_errno.value(), "%s", strerror(open_errno.value()));
      }
#endif
      break;
  }
}

INSTANTIATE_TEST_SUITE_P(UnistdTest, OpenCreate,
                         zxtest::Combine(zxtest::Values(Existing::kNone, Existing::kFile,
                                                        Existing::KDirectory),
                                         zxtest::Bool(), zxtest::Bool()),
                         [](const zxtest::TestParamInfo<OpenCreate::ParamType>& info) {
                           const OpenCreate::Param param(info.param);
                           return fxl::StringPrintf(
                               "With%s/%s/%s",
                               [existing = param.existing]() {
                                 switch (existing) {
                                   case Existing::kNone:
                                     return "None";
                                   case Existing::kFile:
                                     return "File";
                                   case Existing::KDirectory:
                                     return "Directory";
                                 }
                               }(),
                               param.o_directory_flag ? "O_DIRECTORY" : "{}",
                               param.trailing_slash ? "WithSlash" : "WithoutSlash");
                         });

TEST(UnistdTest, TruncateWithNegativeLength) {
  std::string filename = TestTempDirPath() + "/truncate-with-negative-length-test";
  fbl::unique_fd fd(open(filename.c_str(), O_CREAT | O_RDWR, 0666));
  ASSERT_TRUE(fd, "open failed with error %s (%d)", strerror(errno), errno);
  EXPECT_EQ(-1, ftruncate(fd.get(), -1));
  EXPECT_EQ(EINVAL, errno);
  EXPECT_EQ(-1, ftruncate(fd.get(), std::numeric_limits<off_t>::min()));
  EXPECT_EQ(EINVAL, errno);

  EXPECT_EQ(-1, truncate(filename.c_str(), -1));
  EXPECT_EQ(EINVAL, errno);
  EXPECT_EQ(-1, truncate(filename.c_str(), std::numeric_limits<off_t>::min()));
  EXPECT_EQ(EINVAL, errno);
}

TEST(UnistdTest, LinkAt) {
  // Create a temporary directory, store its absolute path and chdir to it.
  std::string root_abs = TestTempDirPath() + "/fdio-linkat.XXXXXX";
  ASSERT_NOT_NULL(mkdtemp(root_abs.data()), "%s", strerror(errno));
  auto cleanup_root =
      fit::defer([&root_abs]() { EXPECT_EQ(0, rmdir(root_abs.c_str()), "%s", strerror(errno)); });
  char prev_cwd[PATH_MAX];
  ASSERT_NOT_NULL(getcwd(prev_cwd, sizeof(prev_cwd)));
  ASSERT_EQ(0, chdir(root_abs.c_str()), "%s", strerror(errno));
  auto restore_cwd =
      fit::defer([&prev_cwd]() { EXPECT_EQ(0, chdir(prev_cwd), "%s", strerror(errno)); });

  // Create a subdirectory with a file in it.
  constexpr char dir_name[] = "dir", foo_name[] = "foo", foo_rel[] = "dir/foo";
  ASSERT_EQ(0, mkdir(dir_name, 0777), "%s", strerror(errno));
  auto cleanup_dir =
      fit::defer([&dir_name]() { EXPECT_EQ(0, rmdir(dir_name), "%s", strerror(errno)); });
  ASSERT_TRUE(fbl::unique_fd(creat(foo_rel, 0666)), "%s", strerror(errno));
  auto cleanup_foo =
      fit::defer([&foo_rel]() { EXPECT_EQ(0, unlink(foo_rel), "%s", strerror(errno)); });

  fbl::unique_fd dir_fd(open(dir_name, O_RDONLY | O_DIRECTORY, 0644));
  ASSERT_TRUE(dir_fd, "%s", strerror(errno));

  // Create link using relative paths.
  constexpr char bar_name[] = "bar", bar_rel[] = "dir/bar";
  ASSERT_EQ(0, linkat(dir_fd.get(), foo_name, AT_FDCWD, bar_rel, 0), "%s", strerror(errno));
  ASSERT_EQ(0, unlink(bar_rel), "%s", strerror(errno));
  ASSERT_EQ(0, linkat(AT_FDCWD, foo_rel, dir_fd.get(), bar_name, 0), "%s", strerror(errno));
  ASSERT_EQ(0, unlink(bar_rel), "%s", strerror(errno));

  // Create link using an absolute oldpath (newpath), verifying that olddirfd (newdirfd) is
  // ignored. We also test that an invalid file descriptor is accepted if the corresponding path
  // is absolute.
  const std::string foo_abs = fxl::Concatenate({root_abs, "/", foo_rel});
  const std::string bar_abs = fxl::Concatenate({root_abs, "/", bar_rel});
  for (int olddirfd : {dir_fd.get(), AT_FDCWD, kInvalidFD}) {
    ASSERT_EQ(0, linkat(olddirfd, foo_abs.c_str(), AT_FDCWD, bar_rel, 0), "%s", strerror(errno));
    ASSERT_EQ(0, unlink(bar_rel), "%s", strerror(errno));
  }
  for (int newdirfd : {dir_fd.get(), AT_FDCWD, kInvalidFD}) {
    ASSERT_EQ(0, linkat(AT_FDCWD, foo_rel, newdirfd, bar_abs.c_str(), 0), "%s", strerror(errno));
    ASSERT_EQ(0, unlink(bar_rel), "%s", strerror(errno));
  }

  // Test errors: an invalid file descriptor is not accepted if the corresponding path is
  // relative.
  constexpr char baz_name[] = "baz";
  ASSERT_EQ(-1, linkat(kInvalidFD, foo_rel, AT_FDCWD, baz_name, 0));
  EXPECT_EQ(EBADF, errno, "%s", strerror(errno));
  ASSERT_EQ(-1, linkat(AT_FDCWD, foo_rel, kInvalidFD, baz_name, 0));
  EXPECT_EQ(EBADF, errno, "%s", strerror(errno));
}

TEST(UnistdTest, LinkAtFollow) {
  std::string root_abs = TestTempDirPath() + "/fdio-linkat-follow.XXXXXX";
  ASSERT_NOT_NULL(mkdtemp(root_abs.data()), "%s", strerror(errno));
  auto cleanup_root =
      fit::defer([&root_abs]() { EXPECT_EQ(0, rmdir(root_abs.c_str()), "%s", strerror(errno)); });

  const std::string file_abs = fxl::Concatenate({root_abs, "/", "file"});
  ASSERT_TRUE(fbl::unique_fd(creat(file_abs.c_str(), 0666)), "%s", strerror(errno));
  auto cleanup_file = fit::defer([&file_abs]() {
    // This must be an ASSERT_* so that errors can be caught by ASSERT_NO_FATAL_FAILURE below.
    ASSERT_EQ(0, unlink(file_abs.c_str()), "%s", strerror(errno));
  });

  // Verify that we can create a hard link to a regular file even if AT_SYMLINK_FOLLOW is set.
  const std::string hard_abs = fxl::Concatenate({root_abs, "/", "hard"});
  ASSERT_EQ(0, linkat(AT_FDCWD, file_abs.c_str(), AT_FDCWD, hard_abs.c_str(), AT_SYMLINK_FOLLOW),
            "%s", strerror(errno));
  ASSERT_EQ(0, unlink(hard_abs.c_str()), "%s", strerror(errno));

  // Create a symlink and test AT_SYMLINK_FOLLOW on it.
  const std::string sym_abs = fxl::Concatenate({root_abs, "/", "sym"});
#ifndef __Fuchsia__
  ASSERT_EQ(0, symlink(file_abs.c_str(), sym_abs.c_str()), "%s", strerror(errno));
  auto cleanup_sym =
      fit::defer([&sym_abs]() { EXPECT_EQ(0, unlink(sym_abs.c_str()), "%s", strerror(errno)); });

  auto expect_file_type_and_unlink = [](const char* path, int expected_file_type) {
    auto unlink_path = fit::defer([path]() { ASSERT_EQ(0, unlink(path), "%s", strerror(errno)); });
    struct stat st;
    ASSERT_EQ(0, lstat(path, &st), "%s", strerror(errno));
    EXPECT_EQ(expected_file_type, st.st_mode & S_IFMT);
  };

  ASSERT_EQ(0, linkat(AT_FDCWD, sym_abs.c_str(), AT_FDCWD, hard_abs.c_str(), 0), "%s",
            strerror(errno));
  ASSERT_NO_FATAL_FAILURE(expect_file_type_and_unlink(hard_abs.c_str(), S_IFLNK));

  ASSERT_EQ(0, linkat(AT_FDCWD, sym_abs.c_str(), AT_FDCWD, hard_abs.c_str(), AT_SYMLINK_FOLLOW),
            "%s", strerror(errno));
  ASSERT_NO_FATAL_FAILURE(expect_file_type_and_unlink(hard_abs.c_str(), S_IFREG));

  // Make our symlink dangling by removing its target.
  ASSERT_NO_FATAL_FAILURE(cleanup_file.call());

  ASSERT_EQ(0, linkat(AT_FDCWD, sym_abs.c_str(), AT_FDCWD, hard_abs.c_str(), 0), "%s",
            strerror(errno));
  ASSERT_NO_FATAL_FAILURE(expect_file_type_and_unlink(hard_abs.c_str(), S_IFLNK));

  ASSERT_EQ(-1, linkat(AT_FDCWD, sym_abs.c_str(), AT_FDCWD, hard_abs.c_str(), AT_SYMLINK_FOLLOW));
  EXPECT_EQ(ENOENT, errno, "%s", strerror(errno));
#else
  // Assert that Fuchsia does not support symlinks yet.
  ASSERT_EQ(-1, symlink(file_abs.c_str(), sym_abs.c_str()));
  ASSERT_EQ(ENOSYS, errno, "%s", strerror(errno));
#endif
}

TEST(UnistdTest, ReadAndWriteWithNegativeOffsets) {
  std::string filename = TestTempDirPath() + "/read-write-with-negative-offsets-test";
  fbl::unique_fd fd(open(filename.c_str(), O_CREAT | O_RDWR, 0666));
  ASSERT_TRUE(fd);
  ASSERT_EQ(-1, pwrite(fd.get(), "hello", 5, -1));
  ASSERT_EQ(EINVAL, errno, "%s", strerror(errno));
  char buf[5];
  ASSERT_EQ(-1, pwrite(fd.get(), buf, 5, -1));
  ASSERT_EQ(EINVAL, errno, "%s", strerror(errno));
}

TEST(UnistdTest, PreadvIovecMaxSize) {
  std::string filename = TestTempDirPath() + "/preadv-iovec-max-size-test";
  fbl::unique_fd fd(open(filename.c_str(), O_CREAT | O_RDWR, 0666));
  ASSERT_TRUE(fd, "open %d: %s", errno, strerror(errno));
  std::vector<struct iovec> iovec(IOV_MAX);
  char buf[1];
  for (auto& i : iovec) {
    i.iov_base = buf;
    i.iov_len = 1;
  }
  ASSERT_EQ(0, ftruncate(fd.get(), IOV_MAX), "ftruncate %d: %s", errno, strerror(errno));
  ASSERT_LE(0, preadv(fd.get(), iovec.data(), static_cast<int>(iovec.size()), 0), "preadv %d: %s",
            errno, strerror(errno));
}

TEST(UnistdTest, PreadvIovecTooLarge) {
  std::string filename = TestTempDirPath() + "/preadv-iovec-too-large-test";
  fbl::unique_fd fd(open(filename.c_str(), O_CREAT | O_RDWR, 0666));
  ASSERT_TRUE(fd, "open %d: %s", errno, strerror(errno));
  std::vector<struct iovec> iovec(IOV_MAX + 1);
  char buf[1];
  for (auto& i : iovec) {
    i.iov_base = buf;
    i.iov_len = 1;
  }
  ASSERT_EQ(-1, preadv(fd.get(), iovec.data(), static_cast<int>(iovec.size()), 0));
  ASSERT_EQ(EINVAL, errno, "%s", strerror(errno));
}

TEST(UnistdTest, PwritevIovecMaxSize) {
  std::string filename = TestTempDirPath() + "/pwritev-iovec-max-size-test";
  fbl::unique_fd fd(open(filename.c_str(), O_CREAT | O_RDWR, 0666));
  ASSERT_TRUE(fd, "open %d: %s", errno, strerror(errno));
  std::vector<struct iovec> iovec(IOV_MAX);
  char buf[1] = {'a'};
  for (auto& i : iovec) {
    i.iov_base = buf;
    i.iov_len = 1;
  }

  ASSERT_LE(0, pwritev(fd.get(), iovec.data(), static_cast<int>(iovec.size()), 0), "preadv %d: %s",
            errno, strerror(errno));
}

TEST(UnistdTest, PwritevIovecTooLarge) {
  std::string filename = TestTempDirPath() + "/pwritev-iovec-too-large-test";
  fbl::unique_fd fd(open(filename.c_str(), O_CREAT | O_RDWR, 0666));
  ASSERT_TRUE(fd, "open %d: %s", errno, strerror(errno));
  std::vector<struct iovec> iovec(IOV_MAX + 1);
  char buf[1] = {'a'};
  for (auto& i : iovec) {
    i.iov_base = buf;
    i.iov_len = 1;
  }
  ASSERT_EQ(-1, pwritev(fd.get(), iovec.data(), static_cast<int>(iovec.size()), 0));
  ASSERT_EQ(EINVAL, errno, "%s", strerror(errno));
}

TEST(UnistdTest, OpenMaxPath) {
  std::string max_path = "/";
  max_path.append(PATH_MAX - 1, 'a');
  EXPECT_EQ(-1, open(max_path.c_str(), O_RDONLY));
  EXPECT_EQ(errno, ENAMETOOLONG, "%s", strerror(errno));
}

TEST(UnistdTest, DupAndRewinddir) {
  // Create at least one file to iterate over.
  std::string test_file_name = TestTempDirPath() + "/fdio-dup-and-rewind";
  {
    fbl::unique_fd fd(open(test_file_name.c_str(), O_CREAT));
    ASSERT_TRUE(fd.is_valid());
  }

  auto cleanup_test_file =
      fit::defer([&] { ASSERT_EQ(unlink(test_file_name.c_str()), 0, "%s", strerror(errno)); });

  fbl::unique_fd orig_fd(open(TestTempDirPath().c_str(), O_RDONLY));
  ASSERT_TRUE(orig_fd.is_valid());

  auto count_dirents = [](fbl::unique_fd dir_fd, size_t& out_count) {
    ASSERT_TRUE(dir_fd.is_valid());

    DIR* dir = fdopendir(dir_fd.release());
    ASSERT_NE(dir, nullptr, "%s", strerror(errno));
    auto cleanup_dir = fit::defer([&] { ASSERT_EQ(closedir(dir), 0, "%s", strerror(errno)); });

    rewinddir(dir);

    out_count = 0;
    while (dirent* ent = readdir(dir)) {
      std::string filename = ent->d_name;
      if (filename != "." && filename != "..") {
        ++out_count;
      }
    }
  };

  size_t initial_count;
  count_dirents(orig_fd.duplicate(), initial_count);

  // Ensure that continuously duping and rewinding the directory stream always counts the same
  // number of files.
  constexpr size_t kNumRecountIterations = 5;
  for (size_t i = 0; i < kNumRecountIterations; i++) {
    size_t recount;
    count_dirents(orig_fd.duplicate(), recount);
    EXPECT_EQ(recount, initial_count);
  }
}

}  // namespace
