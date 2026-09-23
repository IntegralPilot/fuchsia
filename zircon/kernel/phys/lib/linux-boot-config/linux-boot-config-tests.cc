// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/result.h>
#include <lib/linux-boot-config/linux-boot-config.h>
#include <lib/stdcompat/bit.h>

#include <array>
#include <memory>
#include <optional>
#include <span>
#include <string_view>
#include <vector>

#include <fbl/algorithm.h>
#include <fbl/intrusive_double_list.h>
#include <gtest/gtest.h>

namespace {

using linux_boot_config::Key;
using linux_boot_config::LinuxBootConfig;
using linux_boot_config::Trailer;
using linux_boot_config::Value;
using CompareResult = linux_boot_config::Key::CompareResult;
using namespace std::string_view_literals;

struct KeyHolder {
  explicit KeyHolder(std::span<std::string_view> args) {
    key_parts = {args.begin(), args.end()};
    for (const auto& key_part : key_parts) {
      auto part = std::make_unique<linux_boot_config::KeyPart>(
          linux_boot_config::KeyPart{.name = key_part});
      parts.push_back(std::move(part));
      key.push_back(parts.back().get());
    }
  }

  ~KeyHolder() { key.clear(); }

  std::vector<std::string> key_parts;
  std::vector<std::unique_ptr<linux_boot_config::KeyPart>> parts;
  Key key;
};

TEST(KeyTest, Compare) {
  auto kCollections = std::to_array<std::vector<std::string_view>>({
      {"foo", "bar", "baz"},
      {"foo.bar", "baz"},
      {"foo", "bar.baz"},
      {"foo.bar.baz"},
  });

  static constexpr std::string_view kChildOf = "foo.bar";
  static constexpr std::string_view kChildOf2 = "foo";
  static constexpr std::string_view kParentOf = "foo.bar.baz.boo";

  static constexpr std::string_view kMatch = "foo.bar.baz";
  static constexpr std::string_view kNoMatch1 = "foz.bar.b";
  static constexpr std::string_view kNoMatch2 = "foo.bar.b";
  static constexpr std::string_view kNoMatch3 = "foo.bar.ba";
  static constexpr std::string_view kNoMatch4 = "foo.bar.baz1";

  for (std::span<std::string_view> collection : kCollections) {
    SCOPED_TRACE(collection[0]);
    KeyHolder kh(collection);
    EXPECT_EQ(kh.key, kMatch);
    EXPECT_NE(kh.key, kNoMatch1);
    EXPECT_NE(kh.key, kNoMatch2);
    EXPECT_NE(kh.key, kNoMatch3);
    EXPECT_NE(kh.key, kNoMatch4);

    EXPECT_EQ(kh.key.Compare(kChildOf), CompareResult::kChild);
    EXPECT_EQ(kh.key.Compare(kChildOf2), CompareResult::kChild);
    EXPECT_EQ(kh.key.Compare(kParentOf), CompareResult::kParent);

    EXPECT_FALSE(kh.key.start_of(kChildOf));
    EXPECT_FALSE(kh.key.start_of(kChildOf2));
    EXPECT_TRUE(kh.key.start_of(kParentOf));

    EXPECT_TRUE(kh.key.starts_with(kChildOf));
    EXPECT_TRUE(kh.key.starts_with(kChildOf2));
    EXPECT_FALSE(kh.key.starts_with(kParentOf));
  }
}

std::vector<std::byte> MakeRamdisk(size_t ramdisk_size, std::string_view linux_boot_config_contents,
                                   std::optional<Trailer> trailer_override = std::nullopt,
                                   bool calculate_checksum = true) {
  std::vector<std::byte> initrd;
  uint32_t linux_boot_config_size =
      static_cast<uint32_t>(fbl::round_up(linux_boot_config_contents.size(), 4u) + sizeof(Trailer));
  size_t total_size = linux_boot_config_size + ramdisk_size;
  // padding bytes filled.
  initrd.resize(total_size, static_cast<std::byte>('\0'));

  auto linux_boot_config_view =
      std::span(initrd).subspan(initrd.size() - linux_boot_config_size, linux_boot_config_size);
  if (!linux_boot_config_contents.empty()) {
    memcpy(linux_boot_config_view.data(), linux_boot_config_contents.data(),
           linux_boot_config_contents.size());
  }

  Trailer trailer = {
      .size = static_cast<uint32_t>(linux_boot_config_size - sizeof(Trailer)),
      .checksum = 0xBADBEEF,
      .magic = Trailer::kMagic,
  };

  auto final_trailer = trailer_override.value_or(trailer);
  if (calculate_checksum) {
    final_trailer.checksum = linux_boot_config::Checksum(
        linux_boot_config_view.subspan(0, linux_boot_config_size - sizeof(Trailer)));
  }

  auto trailer_view = linux_boot_config_view.subspan(
      linux_boot_config_view.size_bytes() - sizeof(Trailer), sizeof(Trailer));
  final_trailer.Write(trailer_view);

  return initrd;
}

TEST(LinuxBootConfigTest, CreateWithoutTrailer) {
  // No LinuxBootConfig present, equivalent to no magic.
  std::vector initrd = MakeRamdisk(128, "", Trailer{.size = 4096, .checksum = 1234, .magic = {}});
  auto linux_boot_config = LinuxBootConfig::Create(initrd);
  ASSERT_TRUE(linux_boot_config.is_ok());
  EXPECT_EQ(linux_boot_config->size_bytes(), 0u);
}

TEST(LinuxBootConfigTest, CreateWithTrailerAndZeroSize) {
  // Zero sized linux_boot_config.
  std::vector initrd =
      MakeRamdisk(128, "", Trailer{.size = 0, .checksum = 1234, .magic = Trailer::kMagic});
  auto linux_boot_config = LinuxBootConfig::Create(initrd);
  ASSERT_TRUE(linux_boot_config.is_ok());
  EXPECT_EQ(linux_boot_config->size_bytes(), 0u);
}

TEST(LinuxBootConfigTest, CreateWithTrailerAndSizeBiggerThanFile) {
  // Larger than file, 128 + 0 + 12 + 1 (extra) aligned to 4 = 144.
  std::vector initrd =
      MakeRamdisk(128, "", Trailer{.size = 144, .checksum = 1234, .magic = Trailer::kMagic});
  auto linux_boot_config = LinuxBootConfig::Create(initrd);
  EXPECT_TRUE(linux_boot_config.is_error());
}

TEST(LinuxBootConfigTest, CreateWithTrailerAndUnalignedSize) {
  // Size must be aligned to 4.
  std::vector initrd =
      MakeRamdisk(128, "a=b1\n", Trailer{.size = 5, .checksum = 1234, .magic = Trailer::kMagic});
  auto linux_boot_config = LinuxBootConfig::Create(initrd);
  EXPECT_TRUE(linux_boot_config.is_error());
}

TEST(LinuxBootConfigTest, CreateWithPayloadTooSmall) {
  // Small payload, cannot contain a trailer, so we emite a non present linux_boot_config.
  std::vector<std::byte> initrd;
  initrd.resize(sizeof(Trailer) - 1);

  auto linux_boot_config = LinuxBootConfig::Create(initrd);
  ASSERT_TRUE(linux_boot_config.is_ok());
  EXPECT_EQ(linux_boot_config->size_bytes(), 0u);
}

TEST(LinuxBootConfigTest, CreateWithValidLinuxBootConfigPayload) {
  // All valid.
  constexpr std::string_view kContents =
      "foo=bar\nbar=baz #This is not a game\nbar.boo = { foo = bar; bar=baz}\n";
  std::vector initrd = MakeRamdisk(128, kContents);
  auto linux_boot_config = LinuxBootConfig::Create(initrd);
  ASSERT_TRUE(linux_boot_config.is_ok()) << linux_boot_config.error_value().description;

  size_t content_size = fbl::round_up(kContents.size(), 4u);
  EXPECT_EQ(linux_boot_config->size_bytes(), content_size);

  // We can use the embedded size bytes to calculate the size of the ramdisk in initrd.
  auto ramdisk = std::span(initrd).subspan(
      0, initrd.size() - linux_boot_config->size_bytes() - sizeof(Trailer));
  EXPECT_EQ(ramdisk.size_bytes(), 128u);
}

TEST(LinuxBootConfigTest, ParseSingleEntryDefine) {
  constexpr auto kBootEntryFiles = std::to_array<std::string_view>({
      {"foo.bar=123"},
      {"foo.bar=\"123\""},
      {"foo.bar='123'"},
      {"foo.bar='123';"},
      {"foo.bar=\"123\"\n"},
      {"foo.bar='123'\n"},
      {"foo.bar=123\n"},
      {"foo.bar = \"123\""},
      {"foo.bar = '123'"},
      {"foo.bar = \"123\" \n"},
      {"foo.bar = '123' \n"},
      {"foo.bar = 123\n"},

      // Comments are ignored.
      {"foo.bar=123 # Foo Bar comment ignored;{},#:\"'"},
      {"foo.bar=123 # Foo Bar comment ignored;{},#:\"'\n"},
      {"# Foo Bar comment ignored;{},#:\"'\nfoo.bar=123 "},
  });

  constexpr std::string_view kKey = "foo.bar";
  constexpr std::string_view kValue = "123";
  constexpr Value::Action kAction = Value::Action::kDefine;

  for (const auto& file : kBootEntryFiles) {
    SCOPED_TRACE(file);
    size_t visit_count = 0;
    auto linux_boot_config = LinuxBootConfig(file);
    auto parse_result = linux_boot_config.Parse([&](const Key& key, const Value& value) {
      visit_count++;
      EXPECT_EQ(key.Compare(kKey), CompareResult::kMatch);
      EXPECT_EQ(value.action, kAction);
      EXPECT_EQ(value.value, kValue);
      return;
    });
    EXPECT_TRUE(parse_result.is_ok());
    EXPECT_EQ(visit_count, 1u);
  }
}

TEST(LinuxBootConfigTest, ParseSingleEntryOverride) {
  constexpr auto kBootEntryFiles = std::to_array<std::string_view>({
      {"foo.bar:=123"},
      {"foo.bar:=\"123\""},
      {"foo.bar:='123'"},
      {"foo.bar:='123';"},
      {"foo.bar:=\"123\"\n"},
      {"foo.bar:='123'\n"},
      {"foo.bar:=123\n"},
      {"foo.bar := \"123\""},
      {"foo.bar := '123'"},
      {"foo.bar := \"123\" \n"},
      {"foo.bar := '123' \n"},
      {"foo.bar := 123\n"},

      // Comments are ignored.
      {"foo.bar:=123 # Foo Bar comment ignored;{},#:\"'"},
      {"foo.bar:=123 # Foo Bar comment ignored;{},#:\"'\n"},
      {"# Foo Bar comment ignored;{},#:\"'\nfoo.bar:=123 "},
  });

  constexpr std::string_view kKey = "foo.bar";
  constexpr std::string_view kValue = "123";
  constexpr Value::Action kAction = Value::Action::kOverride;

  for (const auto& file : kBootEntryFiles) {
    SCOPED_TRACE(file);
    size_t visit_count = 0;
    auto linux_boot_config = LinuxBootConfig(file);
    auto parse_result = linux_boot_config.Parse([&](const Key& key, const Value& value) {
      visit_count++;
      EXPECT_EQ(key.Compare(kKey), CompareResult::kMatch);
      EXPECT_EQ(value.action, kAction);
      EXPECT_EQ(value.value, kValue);
      return;
    });
    EXPECT_TRUE(parse_result.is_ok());
    EXPECT_EQ(visit_count, 1u);
  }
}

TEST(LinuxBootConfigTest, ParseSingleEntryAppend) {
  constexpr auto kBootEntryFiles = std::to_array<std::string_view>({
      {"foo.bar+=123"},
      {"foo.bar+=\"123\""},
      {"foo.bar+='123'"},
      {"foo.bar+='123';"},
      {"foo.bar+=\"123\"\n"},
      {"foo.bar+='123'\n"},
      {"foo.bar+=123\n"},
      {"foo.bar += \"123\""},
      {"foo.bar += '123'"},
      {"foo.bar += \"123\" \n"},
      {"foo.bar += '123' \n"},
      {"foo.bar += 123\n"},

      // Comments are ignored.
      {"foo.bar+=123 # Foo Bar comment ignored;{},#:\"'"},
      {"foo.bar+=123 # Foo Bar comment ignored;{},#:\"'\n"},
      {"# Foo Bar comment ignored;{},#:\"'\nfoo.bar+=123 "},
  });

  constexpr std::string_view kKey = "foo.bar";
  constexpr std::string_view kValue = "123";
  constexpr Value::Action kAction = Value::Action::kAppend;

  for (const auto& file : kBootEntryFiles) {
    SCOPED_TRACE(file);
    size_t visit_count = 0;
    auto linux_boot_config = LinuxBootConfig(file);
    auto parse_result = linux_boot_config.Parse([&](const Key& key, const Value& value) {
      visit_count++;
      EXPECT_EQ(key.Compare(kKey), CompareResult::kMatch);
      EXPECT_EQ(value.action, kAction);
      EXPECT_EQ(value.value, kValue);
      return;
    });
    EXPECT_TRUE(parse_result.is_ok());
    EXPECT_EQ(visit_count, 1u);
  }
}

TEST(LinuxBootConfigTest, ParseMultipleEntries) {
  constexpr auto kBootEntryFiles = std::to_array<std::string_view>({
      {"foo.bar=123\nbar.foo=1234\nfiz.bar"},
      {"foo.bar=123\nbar.foo=1234\nfiz.bar;"},
      {"foo.bar = 123 \n bar.foo = 1234 \n fiz.bar"},
      {"foo.bar = 123 \n bar.foo = 1234 \n fiz.bar;"},
      {"foo.bar = 123 ; bar.foo = 1234 ; fiz.bar;"},
      {"# Comment 1 \nfoo.bar = 123  # Comment 2\n bar.foo = 1234 # Commnet 3 \n fiz.bar # Comment 4"},
      {"# Comment 1 \nfoo.bar = 123  # Comment 2\n bar.foo = 1234 # Commnet 3 \n fiz.bar ;# Comment 4"},
      {"# Comment 1 \nfoo.bar = 123  # Comment 2\n bar.foo = 1234 # Commnet 3 \n fiz.bar ;# Comment 4 \n # Comment 5"},
  });

  constexpr auto kKeys = std::to_array<std::string_view>({"foo.bar", "bar.foo", "fiz.bar"});
  constexpr auto kValues = std::to_array<std::string_view>({"123", "1234", ""});
  constexpr auto kActions = std::to_array({
      Value::Action::kDefine,
      Value::Action::kDefine,
      Value::Action::kDefine,
  });

  for (const auto& file : kBootEntryFiles) {
    SCOPED_TRACE(file);
    size_t visit_count = 0;
    auto linux_boot_config = LinuxBootConfig(file);
    auto parse_result = linux_boot_config.Parse([&](const Key& key, const Value& value) {
      const auto expected_key = kKeys[visit_count];
      const auto expected_value = kValues[visit_count];
      const auto expected_action = kActions[visit_count];
      visit_count++;
      EXPECT_EQ(key.Compare(expected_key), CompareResult::kMatch);
      EXPECT_EQ(value.action, expected_action);
      EXPECT_EQ(value.value, expected_value);
      return;
    });
    EXPECT_TRUE(parse_result.is_ok());
    EXPECT_EQ(visit_count, 3u);
  }
}

TEST(LinuxBootConfigTest, ParseNestedEntries) {
  constexpr auto kBootEntryFiles = std::to_array<std::string_view>({
      {"foo{bar=123\nbar.foo=1234\nfiz.bar}"},
      {"foo { bar=123\n bar.foo=1234\n fiz.bar\n}"},
      {"foo { bar=123\n bar.foo=1234\n fiz.bar\n}\n"},
      {"foo {bar=123;bar.foo=1234;fiz.bar}\n"},
      {"foo{\nbar=123\nbar {\nfoo=1234\n}\nfiz.bar\n}"},
      {"foo{\nbar=123\nbar {\nfoo=1234\n}\nfiz.bar\n}\n"},
      {"foo{bar=123;bar{foo=1234}fiz.bar}"},
      {"#Comment 0\nfoo{# Comment 1\nbar=123 # Comment 2\nbar.foo=1234 # Comment 3\nfiz.bar #Comment 4\n} # Comment 6"},
  });

  constexpr auto kKeys = std::to_array<std::string_view>({"foo.bar", "foo.bar.foo", "foo.fiz.bar"});
  constexpr auto kValues = std::to_array<std::string_view>({"123", "1234", ""});
  constexpr auto kActions = std::to_array({
      Value::Action::kDefine,
      Value::Action::kDefine,
      Value::Action::kDefine,
  });

  for (const auto& file : kBootEntryFiles) {
    SCOPED_TRACE(file);
    size_t visit_count = 0;
    auto linux_boot_config = LinuxBootConfig(file);
    auto parse_result = linux_boot_config.Parse([&](const Key& key, const Value& value) {
      const auto expected_key = kKeys[visit_count];
      const auto expected_value = kValues[visit_count];
      const auto expected_action = kActions[visit_count];
      visit_count++;
      EXPECT_EQ(key.Compare(expected_key), CompareResult::kMatch);
      EXPECT_EQ(value.action, expected_action);
      EXPECT_EQ(value.value, expected_value);
      return;
    });
    EXPECT_TRUE(parse_result.is_ok());
    EXPECT_EQ(visit_count, 3u);
  }
}

TEST(LinuxBootConfigTest, ParseSingleEntryArray) {
  constexpr auto kBootEntryFiles = std::to_array<std::string_view>({
      {"foo=1,2,3,4,5"},
      {"foo=1,2,3,4,5\n"},
      {"foo=1,\n2,\n3,\n4,\n5\n"},
      {"foo=1,# Comment 123\n2, #Comment 12345\n3, #Comment 123456\n4, #Comment \n5#Comment 456\n"},
  });

  constexpr std::string_view kKey = "foo";
  constexpr auto kValues = std::to_array<std::string_view>({"1", "2", "3", "4", "5"});
  constexpr auto kActions = std::to_array({
      Value::Action::kDefine,
      Value::Action::kAppend,
      Value::Action::kAppend,
      Value::Action::kAppend,
      Value::Action::kAppend,
  });

  for (const auto& file : kBootEntryFiles) {
    SCOPED_TRACE(file);
    size_t visit_count = 0;
    auto linux_boot_config = LinuxBootConfig(file);
    auto parse_result = linux_boot_config.Parse([&](const Key& key, const Value& value) {
      const auto expected_value = kValues[visit_count];
      const auto expected_action = kActions[visit_count];
      visit_count++;
      EXPECT_EQ(key.Compare(kKey), CompareResult::kMatch);
      EXPECT_EQ(value.action, expected_action);
      EXPECT_EQ(value.value, expected_value);
    });
    EXPECT_TRUE(parse_result.is_ok());
    EXPECT_EQ(visit_count, kValues.size());
  }
}

TEST(LinuxBootConfigTest, ParseMultipleEntriesNested) {
  constexpr auto kBootEntryFiles = std::to_array<std::string_view>({
      {"foo{bar=1,2,3\nbar.foo=1234\nfiz.bar}"},
      {"foo { bar=\"1\",'2',3\n bar.foo=1234\n fiz.bar\n}"},
      {"foo { bar=\"1\"\n bar+=2,3; bar.foo=1234\n fiz.bar\n}"},
      {"foo { bar=\"1\"\n bar+=2; bar+=3; bar.foo=1234\n fiz.bar\n}"},
      {"foo { bar=1, #Comment foo\n2, #Comment Bar\n 3 #Comment Dar\n bar.foo=1234\n fiz.bar\n}\n"},
      {"#Comment 0\nfoo{# Comment 1\nbar=1,2,3 # Comment 2\nbar.foo=1234 # Comment 3\nfiz.bar #Comment 4\n} # Comment 6"},
  });

  constexpr auto kKeys = std::to_array<std::string_view>(
      {"foo.bar", "foo.bar", "foo.bar", "foo.bar.foo", "foo.fiz.bar"});
  constexpr auto kValues = std::to_array<std::string_view>({"1", "2", "3", "1234", ""});
  constexpr auto kActions = std::to_array({
      Value::Action::kDefine,
      Value::Action::kAppend,
      Value::Action::kAppend,
      Value::Action::kDefine,
      Value::Action::kDefine,
  });

  for (const auto& file : kBootEntryFiles) {
    SCOPED_TRACE(file);
    size_t visit_count = 0;
    auto linux_boot_config = LinuxBootConfig(file);
    auto parse_result = linux_boot_config.Parse([&](const Key& key, const Value& value) {
      const auto expected_key = kKeys[visit_count];
      const auto expected_value = kValues[visit_count];
      const auto expected_action = kActions[visit_count];
      visit_count++;
      EXPECT_EQ(key.Compare(expected_key), CompareResult::kMatch);
      EXPECT_EQ(value.action, expected_action);
      EXPECT_EQ(value.value, expected_value);
    });
    EXPECT_TRUE(parse_result.is_ok());
    EXPECT_EQ(visit_count, kValues.size());
  }
}

TEST(LinuxBootConfigTest, ParseInvalid) {
  // All of these files contain "invalid" sequences, that can be detected without
  // unbounded memory. Things like := or += after a = require knowledge if the key
  // has already been set.
  constexpr auto kBootEntryFiles = std::to_array<std::string_view>({
      // Unterminated scope.
      {"foo {"},
      // Comment before ,
      {"foo = 1 #12345\n,2"},
      // Invalid key character
      {"foo$ = 1"},
      // Invalid value character
      {"foo = \t1234"},
      // Unterminated quote in value
      {"foo = '"},
      {"foo = \""},
      {"foo = "},
  });

  for (const auto& file : kBootEntryFiles) {
    SCOPED_TRACE(file);
    auto linux_boot_config = LinuxBootConfig(file);
    auto parse_result = linux_boot_config.Parse([](const Key& key, const Value& value) {});

    ASSERT_TRUE(parse_result.is_error());
  }
}

TEST(LinuxBootConfigTest, ParseEmpty) {
  // Files that define no key values, but should be ok.
  constexpr auto kBootEntryFiles = std::to_array<std::string_view>({
      {""},
      {"  "},
      {";"},
      {"foo { }"},
      {"foo { ;}"},
      {"# Just comment"},
  });

  for (const auto& file : kBootEntryFiles) {
    SCOPED_TRACE(file);
    auto linux_boot_config = LinuxBootConfig(file);
    size_t visit_count = 0;
    auto parse_result =
        linux_boot_config.Parse([&](const Key& key, const Value& value) { visit_count++; });

    ASSERT_TRUE(parse_result.is_ok());
    ASSERT_EQ(visit_count, 0u);
  }
}

TEST(LinuxBootConfigTest, ParseWithTrailingNullPadding) {
  constexpr std::string_view kContents = "foo=bar\n\0\0\0"sv;
  std::vector initrd = MakeRamdisk(0, kContents);
  auto linux_boot_config = LinuxBootConfig::Create(initrd);
  ASSERT_TRUE(linux_boot_config.is_ok());

  // contents() and size_bytes() docs explicitly include padding so they should include both our
  // padding here as well as any implicit 4-alignment padding added by `MakeRamdisk()`.
  std::string_view expected = "foo=bar\n\0\0\0\0"sv;  // 11 bytes kContents + 1 to 4-align.
  EXPECT_EQ(linux_boot_config->size_bytes(), expected.size());
  EXPECT_EQ(linux_boot_config->contents(), expected);

  // `Parse()` should omit the trailing data.
  size_t visit_count = 0;
  auto parse_result = linux_boot_config->Parse([&](const Key& key, const Value& value) {
    visit_count++;
    EXPECT_EQ(key, "foo");
    EXPECT_EQ(value.value, "bar");
  });
  EXPECT_TRUE(parse_result.is_ok());
  EXPECT_EQ(visit_count, 1u);
}

// Accommodate non-compliant bootloaders that may not fully zero out the padding.
TEST(LinuxBootConfigTest, ParseWithTrailingNullPaddingAndGarbage) {
  constexpr std::string_view kContents = "foo=bar\n\0abc\n"sv;
  std::vector initrd = MakeRamdisk(0, kContents);
  auto linux_boot_config = LinuxBootConfig::Create(initrd);
  ASSERT_TRUE(linux_boot_config.is_ok());

  // contents() and size_bytes() docs explicitly include padding so they should include both our
  // padding here as well as any implicit 4-alignment padding added by `MakeRamdisk()`.
  std::string_view expected = "foo=bar\n\0abc\n\0\0\0"sv;  // 13 bytes kContents + 3 to 4-align.
  EXPECT_EQ(linux_boot_config->size_bytes(), expected.size());
  EXPECT_EQ(linux_boot_config->contents(), expected);

  size_t visit_count = 0;
  auto parse_result = linux_boot_config->Parse([&](const Key& key, const Value& value) {
    visit_count++;
    EXPECT_EQ(key, "foo");
    EXPECT_EQ(value.value, "bar");
  });
  EXPECT_TRUE(parse_result.is_ok());
  EXPECT_EQ(visit_count, 1u);
}

}  // namespace
