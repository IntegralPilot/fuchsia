// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/diagnostics.h>
#include <lib/elfldltl/memory.h>
#include <lib/elfldltl/note.h>
#include <lib/elfldltl/testing/typed-test.h>

#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <type_traits>
#include <utility>

#include <gtest/gtest.h>

namespace {

// Diagnostic flags for signaling as much information as possible.
constexpr elfldltl::DiagnosticsFlags kFlags = {
    .multiple_errors = true,
    .warnings_are_errors = false,
    .extra_checking = true,
};

#if defined(__GLIBC__) || defined(__Fuchsia__)
#define HAVE_OPEN_MEMSTREAM 1
#else
#define HAVE_OPEN_MEMSTREAM 0
#endif

class StringFile {
 public:
  StringFile(const StringFile&) = delete;

  StringFile()
#if HAVE_OPEN_MEMSTREAM
      : file_(open_memstream(&buffer_, &buffer_size_))
#else
      : file_(tmpfile())
#endif
  {
    EXPECT_NE(file_, nullptr);
  }

  FILE* file() { return file_; }

  std::string contents() && {
#if HAVE_OPEN_MEMSTREAM
    EXPECT_EQ(0, fflush(file_));
    return std::string(buffer_, buffer_size_);
#endif
    EXPECT_EQ(0, fseek(file_, 0, SEEK_END)) << "fseek: " << strerror(errno);
    std::string result(ftell(file_), 'x');
    rewind(file_);
    EXPECT_FALSE(ferror(file_));
    EXPECT_EQ(1u, fread(result.data(), result.size(), 1, file_))
        << "fread of " << result.size() << ": " << strerror(errno);
    return result;
  }

  ~StringFile() {
    if (file_) {
      fclose(file_);
    }
#if defined(__GLIBC__) || defined(__Fuchsia__)
    free(buffer_);
#endif
  }

 private:
  FILE* file_ = nullptr;
#if HAVE_OPEN_MEMSTREAM
  char* buffer_ = nullptr;
  size_t buffer_size_ = 0;
#endif
};

template <auto Class, auto Data>
constexpr bool kAliasCheck = std::conjunction_v<
    std::is_same<typename elfldltl::Elf<Class, Data>::Note, elfldltl::ElfNote>,
    std::is_same<typename elfldltl::Elf<Class, Data>::NoteSegment, elfldltl::ElfNoteSegment<Data>>>;

static_assert(kAliasCheck<elfldltl::ElfClass::k32, elfldltl::ElfData::k2Msb>);
static_assert(kAliasCheck<elfldltl::ElfClass::k64, elfldltl::ElfData::k2Msb>);
static_assert(kAliasCheck<elfldltl::ElfClass::k32, elfldltl::ElfData::k2Lsb>);
static_assert(kAliasCheck<elfldltl::ElfClass::k64, elfldltl::ElfData::k2Lsb>);

TEST(ElfldltlNoteTests, Empty) {
  constexpr elfldltl::ElfNote::Bytes kData{};
  elfldltl::ElfNoteSegment notes(kData);

  for (auto note : notes) {
    // This should never be reached, but these statements ensure that the
    // intended API usages compile correctly.
    EXPECT_TRUE(note.name.empty());
    EXPECT_TRUE(note.desc.empty());
    EXPECT_EQ(note.type, uint32_t{0});
    FAIL() << "container should be empty";
  }
}

// Simple way to create an elf note in memory, it could be more ergonomic,
// but these are just for tests so I can't be bothered. It assumes 64 bit
// EI_CLASS and native EI_DATA.
template <uint32_t NameSz, uint32_t DescSz>
struct alignas(4) InMemoryNote {
  elfldltl::LayoutBase<elfldltl::ElfData::kNative>::Nhdr header;
  std::array<char, NameSz> name{};
  alignas(4) std::array<std::byte, DescSz> desc{};

  template <typename T, typename N, typename D>
  InMemoryNote(T type, N&& nameData, D&& descData)
      : header{NameSz, DescSz, static_cast<uint32_t>(type)} {
    memcpy(name.data(), std::data(nameData), std::size(nameData));
    memcpy(desc.data(), std::data(descData), std::size(descData));
  }

  constexpr elfldltl::ElfNote::Bytes asBytes() const {
    return {(const std::byte*)this, sizeof(*this)};
  }

  bool operator==(const elfldltl::ElfNote& note) const {
    return note.type == header.type && note.name.size() == header.namesz &&
           note.name == std::string_view{name.data(), name.size()} &&
           note.desc.size() == header.descsz && !memcmp(note.desc.data(), desc.data(), desc.size());
  }
};

TEST(ElfldltlNoteTests, BuildId) {
  static const InMemoryNote<4, 8> noteData{
      elfldltl::ElfNoteType::kGnuBuildId, "GNU",
      std::array{std::byte{1}, std::byte{2}, std::byte{3}, std::byte{4}, std::byte{5}, std::byte{6},
                 std::byte{7}, std::byte{8}}};
  static_assert(sizeof(noteData) == 12 + 4 + 8);
  elfldltl::ElfNoteSegment<elfldltl::ElfData::kNative> notes(noteData.asBytes());

  size_t count = 0;
  for (auto note : notes) {
    ++count;

    EXPECT_TRUE(note.IsBuildId());

    EXPECT_EQ(16u, note.HexSize());

    std::string str;
    note.HexDump([&str](char c) { str += c; });
    EXPECT_EQ(str, "0102030405060708");

    StringFile sf;
    note.HexDump(sf.file());
    EXPECT_EQ(std::move(sf).contents(), "0102030405060708");
  }
  EXPECT_EQ(count, size_t{1});
}

// Testing all formats isn't necessary for these kinds of tests.

template <class Observer>
class ElfldltlNoteObserverTests : public ::testing::Test {
 public:
  using Elf = elfldltl::Elf64<>;
  using Phdr = Elf::Phdr;

  static constexpr auto Make = Observer::Make;
  static constexpr std::string_view kFailMessage = Observer::kFailMessage;

  template <typename T, typename Diag, typename... Observers>
  bool ObserveNotes(T& notes_data, Diag& diag, Observers&&... observers) {
    elfldltl::DirectMemory file{
        std::span<std::byte>{reinterpret_cast<std::byte*>(std::addressof(notes_data)),
                             sizeof(notes_data)},
        0};
    static constexpr Phdr phdr = {
        .offset = 0,
        .vaddr = 0,
        .filesz = sizeof(notes_data),
        .memsz = sizeof(notes_data),
    };

    return Make(Elf{}, file, observers...)
        .Observe(diag, elfldltl::PhdrTypeMatch<elfldltl::ElfPhdrType::kNote>{}, phdr);
  }
};

struct FileObserver {
  static constexpr std::string_view kName = "FileObserver";

  static constexpr auto Make = [](auto&& elf, auto&& file, auto&&... callback) {
    return elfldltl::PhdrFileNoteObserver(
        std::forward<decltype(elf)>(elf), std::forward<decltype(file)>(file),
        elfldltl::NoArrayFromFile<typename std::decay_t<decltype(elf)>::Phdr>{},
        std::forward<decltype(callback)>(callback)...);
  };

  static constexpr std::string_view kFailMessage = "failed to read note segment from file";
};

struct MemoryObserver {
  static constexpr std::string_view kName = "MemoryObserver";

  static constexpr auto Make = [](auto&&... args) {
    return elfldltl::PhdrMemoryNoteObserver(std::forward<decltype(args)>(args)...);
  };

  static constexpr std::string_view kFailMessage = "failed to read note segment from memory image";
};

using NoteObservers = ::testing::Types<FileObserver, MemoryObserver>;

TYPED_TEST_SUITE(ElfldltlNoteObserverTests, NoteObservers, elfldltl::testing::TestNames);

TYPED_TEST(ElfldltlNoteObserverTests, ObserveEmpty) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename TestFixture::Phdr;

  elfldltl::DirectMemory file;
  auto observer = TestFixture::Make(Elf{}, file, [](const elfldltl::ElfNote& note) {
    EXPECT_TRUE(false) << "callback shouldn't be called";
    return fit::failed();
  });

  std::vector<std::string> errors;
  auto diag = elfldltl::CollectStringsDiagnostics(errors, kFlags);

  static constexpr Phdr phdr = {.filesz = 0, .memsz = 0};
  ASSERT_TRUE(
      observer.Observe(diag, elfldltl::PhdrTypeMatch<elfldltl::ElfPhdrType::kNote>{}, phdr));
  EXPECT_EQ(diag.warnings() + diag.errors(), 0u);
}

TYPED_TEST(ElfldltlNoteObserverTests, ObserveBadFile) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename TestFixture::Phdr;

  elfldltl::DirectMemory file;
  auto observer = TestFixture::Make(Elf{}, file, [](const elfldltl::ElfNote& note) {
    EXPECT_TRUE(false) << "callback shouldn't be called";
    return fit::failed();
  });
  std::vector<std::string> errors;
  auto diag = elfldltl::CollectStringsDiagnostics(errors, kFlags);

  static constexpr Phdr phdr = {.filesz = 1, .memsz = 1};
  observer.Observe(diag, elfldltl::PhdrTypeMatch<elfldltl::ElfPhdrType::kNote>{}, phdr);
  EXPECT_EQ(diag.warnings(), 0u);
  EXPECT_EQ(diag.errors(), 1u);
  EXPECT_EQ(errors[0], TestFixture::kFailMessage);
}

TYPED_TEST(ElfldltlNoteObserverTests, ObserveOneBuildID) {
  static InMemoryNote<4, 4> note_data{elfldltl::ElfNoteType::kGnuBuildId, "GNU", "123"};

  std::vector<std::string> errors;
  auto diag = elfldltl::CollectStringsDiagnostics(errors, kFlags);

  {
    std::optional<elfldltl::ElfNote> note;
    EXPECT_FALSE(this->ObserveNotes(note_data, diag, ObserveBuildIdNote(note, false)));
    ASSERT_TRUE(note.has_value());
    EXPECT_EQ(note_data, *note);
    EXPECT_EQ(diag.warnings() + diag.errors(), 0u);
  }

  {
    std::optional<elfldltl::ElfNote> note;
    EXPECT_TRUE(this->ObserveNotes(note_data, diag, ObserveBuildIdNote(note, true)));
    ASSERT_TRUE(note.has_value());
    EXPECT_EQ(note_data, *note);
    EXPECT_EQ(diag.warnings() + diag.errors(), 0u);
  }
}

TYPED_TEST(ElfldltlNoteObserverTests, ObserveBuildIDFirst) {
  static struct {
    InMemoryNote<4, 4> build_id{elfldltl::ElfNoteType::kGnuBuildId, "GNU", "abc"};
    InMemoryNote<4, 2> version{1, "GNU", "1"};
  } note_data;

  std::vector<std::string> errors;
  auto diag = elfldltl::CollectStringsDiagnostics(errors, kFlags);

  {
    std::optional<elfldltl::ElfNote> note;
    EXPECT_FALSE(this->ObserveNotes(note_data, diag, ObserveBuildIdNote(note, false)));
    ASSERT_TRUE(note.has_value());
    EXPECT_EQ(note_data.build_id, *note);
    EXPECT_EQ(diag.warnings() + diag.errors(), 0u);
  }

  {
    std::optional<elfldltl::ElfNote> note;
    EXPECT_TRUE(this->ObserveNotes(note_data, diag, ObserveBuildIdNote(note, true)));
    ASSERT_TRUE(note.has_value());
    EXPECT_EQ(note_data.build_id, *note);
    EXPECT_EQ(diag.warnings() + diag.errors(), 0u);
  }
}

TYPED_TEST(ElfldltlNoteObserverTests, ObserveBuildIDLast) {
  static struct {
    InMemoryNote<4, 8> version{1, "GNU", "123"};
    InMemoryNote<4, 4> build_id{elfldltl::ElfNoteType::kGnuBuildId, "GNU", "abc"};
  } note_data;

  std::vector<std::string> errors;
  auto diag = elfldltl::CollectStringsDiagnostics(errors, kFlags);

  {
    std::optional<elfldltl::ElfNote> note;
    EXPECT_FALSE(this->ObserveNotes(note_data, diag, ObserveBuildIdNote(note, false)));
    ASSERT_TRUE(note.has_value());
    EXPECT_EQ(note_data.build_id, *note);
    EXPECT_EQ(diag.warnings() + diag.errors(), 0u);
  }

  {
    std::optional<elfldltl::ElfNote> note;
    EXPECT_TRUE(this->ObserveNotes(note_data, diag, ObserveBuildIdNote(note, true)));
    ASSERT_TRUE(note.has_value());
    EXPECT_EQ(note_data.build_id, *note);
    EXPECT_EQ(diag.warnings() + diag.errors(), 0u);
  }
}

TYPED_TEST(ElfldltlNoteObserverTests, Observe2BuildIDs) {
  static struct {
    InMemoryNote<4, 4> build_id{elfldltl::ElfNoteType::kGnuBuildId, "GNU", "123"};
    InMemoryNote<4, 5> build_id2{elfldltl::ElfNoteType::kGnuBuildId, "GNU", "abcd"};
  } note_data;

  std::vector<std::string> errors;
  auto diag = elfldltl::CollectStringsDiagnostics(errors, kFlags);

  // These check that ObserveBuildIdNote will yield the first found and not later ones.
  {
    std::optional<elfldltl::ElfNote> note;
    EXPECT_FALSE(this->ObserveNotes(note_data, diag, ObserveBuildIdNote(note, false)));
    ASSERT_TRUE(note.has_value());
    EXPECT_EQ(note_data.build_id, *note);
    EXPECT_EQ(diag.warnings() + diag.errors(), 0u);
  }

  {
    std::optional<elfldltl::ElfNote> note;
    EXPECT_TRUE(this->ObserveNotes(note_data, diag, ObserveBuildIdNote(note, true)));
    ASSERT_TRUE(note.has_value());
    EXPECT_EQ(note_data.build_id, *note);
    EXPECT_EQ(diag.warnings() + diag.errors(), 0u);
  }
}

TYPED_TEST(ElfldltlNoteObserverTests, ObserveNoBuildID) {
  static struct {
    InMemoryNote<4, 4> version{1, "GNU", "123"};
    InMemoryNote<4, 5> version2{1, "GNU", "abcd"};
  } note_data;

  std::vector<std::string> errors;
  auto diag = elfldltl::CollectStringsDiagnostics(errors, kFlags);

  std::optional<elfldltl::ElfNote> note;
  EXPECT_TRUE(this->ObserveNotes(note_data, diag, ObserveBuildIdNote(note)));
  EXPECT_FALSE(note.has_value());
}

TYPED_TEST(ElfldltlNoteObserverTests, ObserveMultipleObservers) {
  static InMemoryNote<4, 4> note_data{elfldltl::ElfNoteType::kGnuBuildId, "GNU", "123"};

  std::vector<std::string> errors;
  auto diag = elfldltl::CollectStringsDiagnostics(errors, kFlags);

  std::optional<elfldltl::ElfNote> note, note2;
  EXPECT_TRUE(
      this->ObserveNotes(note_data, diag, ObserveBuildIdNote(note), ObserveBuildIdNote(note2)));
  ASSERT_TRUE(note.has_value());
  EXPECT_EQ(note_data, *note);
  ASSERT_TRUE(note2.has_value());
  EXPECT_EQ(note_data, *note2);
  EXPECT_EQ(diag.warnings() + diag.errors(), 0u);
}

TYPED_TEST(ElfldltlNoteObserverTests, ObserveMultipleStopsEarly) {
  static InMemoryNote<4, 4> note_data{elfldltl::ElfNoteType::kGnuBuildId, "GNU", "123"};

  std::vector<std::string> errors;
  auto diag = elfldltl::CollectStringsDiagnostics(errors, kFlags);

  std::optional<elfldltl::ElfNote> note, note2;
  EXPECT_FALSE(this->ObserveNotes(note_data, diag, ObserveBuildIdNote(note, false),
                                  ObserveBuildIdNote(note2, true)));
  ASSERT_TRUE(note.has_value());
  EXPECT_EQ(note_data, *note);
  EXPECT_FALSE(note2.has_value());
  EXPECT_EQ(diag.warnings() + diag.errors(), 0u);
}

}  // namespace
