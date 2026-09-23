// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/fd.h>
#include <lib/elfldltl/file.h>
#include <lib/elfldltl/memory.h>
#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/elfldltl/testing/typed-test.h>
#include <lib/elfldltl/unique-fd.h>
#include <stdio.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#ifdef __Fuchsia__
#include <lib/elfldltl/vmo.h>
#include <lib/zx/vmo.h>
#endif

namespace {

using ::testing::ElementsAreArray;
using ::testing::Eq;
using ::testing::Optional;

class TestFdFile : public ::testing::Test {
 public:
  static constexpr std::string_view kName = "TestFdFile";

 protected:
  static constexpr bool kDestroysHandle = false;

  static constexpr elfldltl::PosixError kInvalidFdError{EBADF};

  template <typename Diagnostics>
  using FileT = elfldltl::FdFile<Diagnostics>;

  TestFdFile() {
    f_ = tmpfile();
    EXPECT_NE(f_, nullptr);
  }

  ~TestFdFile() {
    if (f_) {
      fclose(f_);
    }
  }

  void Write(const void* p, size_t size) {
    EXPECT_EQ(fwrite(p, 1, size, f_), size);
    EXPECT_EQ(fflush(f_), 0);
  }

  int GetHandle() const { return fileno(f_); }

 private:
  FILE* f_;
};

class TestUniqueFdFile : public TestFdFile {
 public:
  static constexpr std::string_view kName = "TestUniqueFdFile";

 protected:
  static constexpr bool kDestroysHandle = true;

  template <typename Diagnostics>
  using FileT = elfldltl::UniqueFdFile<Diagnostics>;

  fbl::unique_fd GetHandle() { return fbl::unique_fd{TestFdFile::GetHandle()}; }
};

#ifdef __Fuchsia__

class TestVmoFile : public ::testing::Test {
 public:
  static constexpr std::string_view kName = "TestVmoFile";

 protected:
  static constexpr bool kDestroysHandle = true;

  static constexpr elfldltl::ZirconError kInvalidFdError{ZX_ERR_BAD_HANDLE};

  template <typename Diagnostics>
  using FileT = elfldltl::VmoFile<Diagnostics>;

  // The fixture starts with an empty VMO so that reads on GetHandle() will
  // fail with ZX_ERR_OUT_OF_RANGE rather than ZX_ERR_BAD_HANDLE.
  TestVmoFile() { EXPECT_EQ(zx::vmo::create(0, 0, &vmo_), ZX_OK); }

  void Write(const void* p, size_t size) {
    // This replaces the empty VMO.
    ASSERT_EQ(zx::vmo::create(size, 0, &vmo_), ZX_OK);
    ASSERT_EQ(vmo_.write(p, 0, size), ZX_OK);
  }

  zx::vmo GetHandle() { return std::move(vmo_); }

  zx::unowned_vmo Borrow() { return vmo_.borrow(); }

 private:
  zx::vmo vmo_;
};

class TestUnownedVmoFile : public TestVmoFile {
 public:
  static constexpr std::string_view kName = "TestUnownedVmoFile";

 protected:
  static constexpr bool kDestroysHandle = false;

  template <typename Diagnostics>
  using FileT = elfldltl::UnownedVmoFile<Diagnostics>;

  zx::unowned_vmo GetHandle() { return Borrow(); }
};

#endif  // __Fuchsia__

using FileTypes = ::testing::Types<
#ifdef __Fuchsia__
    TestVmoFile, TestUnownedVmoFile,
#endif
    TestFdFile, TestUniqueFdFile>;

using ExpectOkDiagnosticsType = decltype(elfldltl::testing::ExpectOkDiagnostics());

template <class TestFile>
class ElfldltlFileTests : public TestFile {
 public:
  using OkFileT = typename TestFile::template FileT<ExpectOkDiagnosticsType>;
  using offset_type = typename OkFileT::offset_type;
  using unsigned_offset_type = std::make_unsigned_t<offset_type>;

  static constexpr unsigned_offset_type kZeroOffset = 0;
  static constexpr elfldltl::FileOffset kZeroFileOffset{kZeroOffset};

  static constexpr auto MakeExpectedInvalidFd() {
    return elfldltl::testing::ExpectedSingleError{
        "cannot read ", sizeof(int), " bytes", kZeroFileOffset, ": ", TestFile::kInvalidFdError,
    };
  }

  static constexpr auto MakeExpectedEof() {
    return elfldltl::testing::ExpectedSingleError{
        "cannot read ", sizeof(int), " bytes", kZeroFileOffset, ": ", "reached end of file",
    };
  }

  using InvalidFdDiagnostics = decltype(MakeExpectedInvalidFd());
  using InvalidFdFileT = typename TestFile::template FileT<InvalidFdDiagnostics>;

  using EofDiagnostics = decltype(MakeExpectedEof());
  using EofFileT = typename TestFile::template FileT<EofDiagnostics>;
};

TYPED_TEST_SUITE(ElfldltlFileTests, FileTypes, elfldltl::testing::TestNames);

TYPED_TEST(ElfldltlFileTests, InvalidFd) {
  auto expected = TestFixture::MakeExpectedInvalidFd();
  typename TestFixture::InvalidFdFileT file{expected};

  std::optional<int> got = file.template ReadFromFile<int>(0);
  EXPECT_FALSE(got);
}

TYPED_TEST(ElfldltlFileTests, Eof) {
  auto expected = TestFixture::MakeExpectedEof();
  typename TestFixture::EofFileT file{this->GetHandle(), expected};

  std::optional<int> got = file.template ReadFromFile<int>(0);
  EXPECT_EQ(got, std::nullopt);
}

TYPED_TEST(ElfldltlFileTests, ReadFromFile) {
  int i = 123;

  this->Write(&i, sizeof(i));

  auto diag = elfldltl::testing::ExpectOkDiagnostics();
  typename TestFixture::OkFileT file{this->GetHandle(), diag};

  std::optional<int> got = file.template ReadFromFile<int>(0);
  EXPECT_THAT(got, Optional(Eq(i)));
}

TYPED_TEST(ElfldltlFileTests, ReadArrayFromFile) {
  constexpr int kData[] = {123, 456, 789};

  this->Write(kData, sizeof(kData));

  auto diag = elfldltl::testing::ExpectOkDiagnostics();
  typename TestFixture::OkFileT file{this->GetHandle(), diag};

  elfldltl::FixedArrayFromFile<int, 3> allocator;
  auto got = file.template ReadArrayFromFile<int>(0, allocator, 3);

  ASSERT_TRUE(got);
  std::span<const int> data = *got;
  EXPECT_THAT(data, ElementsAreArray(kData));
}

TYPED_TEST(ElfldltlFileTests, Assignment) {
  auto test_assignment = [this](auto&& diag) {
    using Diagnostics = std::decay_t<decltype(diag)>;
    using FileT = typename TestFixture::template FileT<Diagnostics>;

    int i = 123;

    this->Write(&i, sizeof(i));

    FileT file{this->GetHandle(), diag};
    EXPECT_THAT(file.template ReadFromFile<int>(0), Optional(Eq(i)));
    if constexpr (std::is_copy_assignable_v<decltype(file)>) {
      auto other = file;
      EXPECT_THAT(other.template ReadFromFile<int>(0), Optional(Eq(i)));
    }
    EXPECT_THAT(file.template ReadFromFile<int>(0), Optional(Eq(i)));
    {
      auto other = std::move(file);
      EXPECT_THAT(other.template ReadFromFile<int>(0), Optional(Eq(i)));
    }
    if constexpr (TestFixture::kDestroysHandle) {
      EXPECT_FALSE(file.template ReadFromFile<int>(0).has_value());
    }
  };

  if constexpr (TestFixture::kDestroysHandle) {
    test_assignment(TestFixture::MakeExpectedInvalidFd());
  } else {
    test_assignment(elfldltl::testing::ExpectOkDiagnostics());
  }
}

}  // anonymous namespace
