// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/tests/test_library.h"

#define BORINGSSL_NO_CXX

#include <gtest/gtest.h>
#include <openssl/sha.h>
#include <re2/re2.h>

namespace fidlc {
namespace {

uint64_t FakeMethodHasher(std::string_view selector) {
  if (selector.find("HashesToZero") != std::string_view::npos)
    return 0;
  if (selector.find("Clash") != std::string_view::npos)
    return 456789;
  return Sha256MethodHasher(selector);
}

TEST(OrdinalsTests, BadOrdinalCannotBeZero) {
  TestLibrary library(R"FIDL(
library example;

protocol Special {
    ThisOneHashesToZero() -> (struct { i int64; });
};
)FIDL");
  library.method_hasher() = FakeMethodHasher;
  library.ExpectFail(ErrGeneratedZeroValueOrdinal);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(OrdinalsTests, BadClashingOrdinalValues) {
  TestLibrary library(R"FIDL(
library example;

using zx;

protocol Special {
    ClashOne(struct { s string; b bool; }) -> (struct { i int32; });
    ClashTwo(struct { s string; }) -> (resource struct { r zx.Handle:CHANNEL; });
};
)FIDL");
  library.method_hasher() = FakeMethodHasher;
  library.UseLibraryZx();
  library.ExpectFail(ErrDuplicateMethodOrdinal, "example.fidl:7:5");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(OrdinalsTests, BadClashingOrdinalValuesWithAttribute) {
  TestLibrary library(R"FIDL(
library example;

using zx;

protocol Special {
    @selector("ClashOne")
    foo(struct { s string; b bool; }) -> (struct { i int32; });
    @selector("ClashTwo")
    bar(struct { s string; }) -> (resource struct { r zx.Handle:CHANNEL; });
};
)FIDL");
  library.method_hasher() = FakeMethodHasher;
  library.UseLibraryZx();
  library.ExpectFail(ErrDuplicateMethodOrdinal, "example.fidl:8:5");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(OrdinalsTests, BadClashingOrdinalBadSelector) {
  TestLibrary library;
  library.AddFile("bad/fi-0081.test.fidl");
  library.ExpectFail(ErrDuplicateMethodOrdinal, "bad/fi-0081.test.fidl:7:5");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(OrdinalsTests, GoodAttributeResolvesClashes) {
  TestLibrary library(R"FIDL(
library example;

using zx;

protocol Special {
    @selector("SomethingElse")
    ClashOne(struct { s string; b bool; }) -> (struct { i int32; });
    ClashTwo(struct { s string; }) -> (resource struct { r zx.Handle:CHANNEL; });
};
)FIDL");
  library.method_hasher() = FakeMethodHasher;
  library.UseLibraryZx();
  ASSERT_COMPILED(library);
}

TEST(OrdinalsTests, GoodOrdinalValueIsSha256) {
  TestLibrary library(R"FIDL(
library a.b.c;

protocol protocol {
    selector(struct {
        s string;
        b bool;
    }) -> (struct {
        i int32;
    });
};
)FIDL");
  ASSERT_COMPILED(library);

  const char hash_name64[] = "a.b.c/protocol.selector";
  uint8_t digest64[SHA256_DIGEST_LENGTH];
  SHA256(reinterpret_cast<const uint8_t*>(hash_name64), strlen(hash_name64), digest64);
  uint64_t expected_hash64 = *(reinterpret_cast<uint64_t*>(digest64)) & 0x7fffffffffffffff;

  const Protocol* iface = library.LookupProtocol("protocol");
  uint64_t actual_hash64 = iface->methods[0].ordinal;
  ASSERT_EQ(actual_hash64, expected_hash64) << "Expected 64bits hash is not correct";
}

TEST(OrdinalsTests, GoodSelectorWithFullPath) {
  TestLibrary library(R"FIDL(
library not.important;

protocol at {
    @selector("a.b.c/protocol.selector")
    all();
};
)FIDL");
  ASSERT_COMPILED(library);

  const char hash_name64[] = "a.b.c/protocol.selector";
  uint8_t digest64[SHA256_DIGEST_LENGTH];
  SHA256(reinterpret_cast<const uint8_t*>(hash_name64), strlen(hash_name64), digest64);
  uint64_t expected_hash64 = *(reinterpret_cast<uint64_t*>(digest64)) & 0x7fffffffffffffff;

  const Protocol* iface = library.LookupProtocol("at");
  uint64_t actual_hash64 = iface->methods[0].ordinal;
  ASSERT_EQ(actual_hash64, expected_hash64) << "Expected 64bits hash is not correct";
}

TEST(OrdinalsTests, BadSelectorValueWrongFormat) {
  TestLibrary library;
  library.AddFile("bad/fi-0082.test.fidl");

  library.ExpectFail(ErrInvalidSelectorValue);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(OrdinalsTests, BadSelectorValueNotString) {
  TestLibrary library(R"FIDL(
library not.important;

protocol at {
    // should be a string
    @selector(true)
    all();
};
)FIDL");
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "true", "bool", "string");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(OrdinalsTests, GoodSelectorValueReferencesConst) {
  TestLibrary library(R"FIDL(
library not.important;

protocol at {
    @selector(SEL)
    all();
};

const SEL string = "a.b.c/protocol.selector";
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(OrdinalsTests, BadSelectorValueReferencesNonexistent) {
  TestLibrary library(R"FIDL(
library not.important;

protocol at {
    @selector(nonexistent)
    all();
};
)FIDL");
  library.ExpectFail(ErrNameNotFound, "nonexistent", "library 'not.important'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(OrdinalsTests, GoodOrdinalValueIsFirst64BitsOfSha256) {
  TestLibrary library(R"FIDL(
library a.b.c;

protocol protocol {
    s0();
    s1();
    s2();
    s3();
    s4();
    s5();
    s6();
    s7();
    s8();
    s9();
    s10();
    s11();
    s12();
    s13();
    s14();
    s15();
    s16();
    s17();
    s18();
    s19();
    s20();
    s21();
    s22();
    s23();
    s24();
    s25();
    s26();
    s27();
    s28();
    s29();
    s30();
    s31();
};
)FIDL");
  ASSERT_COMPILED(library);

  const Protocol* iface = library.LookupProtocol("protocol");

  // The expected ordinals were generated by the following Python code:
  //
  //     import hashlib
  //     for i in range(32):
  //         fqn = f"a.b.c/protocol.s{i}"
  //         hash = hashlib.sha256(fqn.encode()).digest()
  //         print(hash[7::-1].hex())
  //
  EXPECT_EQ(iface->methods[0].ordinal, 0x3b1625372e15f1aeu);
  EXPECT_EQ(iface->methods[1].ordinal, 0x4199e504fa71b5a4u);
  EXPECT_EQ(iface->methods[2].ordinal, 0x247ca8a890628135u);
  EXPECT_EQ(iface->methods[3].ordinal, 0x64f7c02cfffb7846u);
  EXPECT_EQ(iface->methods[4].ordinal, 0x20d3f06c598f0cc3u);
  EXPECT_EQ(iface->methods[5].ordinal, 0x1ce13806085dac7au);
  EXPECT_EQ(iface->methods[6].ordinal, 0x09e1d4b200770defu);
  EXPECT_EQ(iface->methods[7].ordinal, 0x53df65d26411d8eeu);
  EXPECT_EQ(iface->methods[8].ordinal, 0x690c3617405590c7u);
  EXPECT_EQ(iface->methods[9].ordinal, 0x4ff9ef5fb170f550u);
  EXPECT_EQ(iface->methods[10].ordinal, 0x1542d4c21d8a6c00u);
  EXPECT_EQ(iface->methods[11].ordinal, 0x564e9e47f7418e0fu);
  EXPECT_EQ(iface->methods[12].ordinal, 0x29681e66f3506231u);
  EXPECT_EQ(iface->methods[13].ordinal, 0x5ee63b26268f7760u);
  EXPECT_EQ(iface->methods[14].ordinal, 0x256950edf00aac63u);
  EXPECT_EQ(iface->methods[15].ordinal, 0x6b21c0ff1aa02896u);
  EXPECT_EQ(iface->methods[16].ordinal, 0x5a54f3dca00089e9u);
  EXPECT_EQ(iface->methods[17].ordinal, 0x772476706fa4be0eu);
  EXPECT_EQ(iface->methods[18].ordinal, 0x294e338bf71a773bu);
  EXPECT_EQ(iface->methods[19].ordinal, 0x5a6aa228cfb68d16u);
  EXPECT_EQ(iface->methods[20].ordinal, 0x55a09c6b033f3f98u);
  EXPECT_EQ(iface->methods[21].ordinal, 0x1192d5b856d22cd8u);
  EXPECT_EQ(iface->methods[22].ordinal, 0x2e68bdea28f9ce7bu);
  EXPECT_EQ(iface->methods[23].ordinal, 0x4c8ebf26900e4451u);
  EXPECT_EQ(iface->methods[24].ordinal, 0x3df0dbe9378c4fd3u);
  EXPECT_EQ(iface->methods[25].ordinal, 0x087268657bb0cad1u);
  EXPECT_EQ(iface->methods[26].ordinal, 0x0aee6ad161a90ae1u);
  EXPECT_EQ(iface->methods[27].ordinal, 0x44e6f2282baf727au);
  EXPECT_EQ(iface->methods[28].ordinal, 0x3e8984f57ab5830du);
  EXPECT_EQ(iface->methods[29].ordinal, 0x696f9f73a5cabd21u);
  EXPECT_EQ(iface->methods[30].ordinal, 0x327d7b0d2389e054u);
  EXPECT_EQ(iface->methods[31].ordinal, 0x54fd307bb5bfab2du);
}

TEST(OrdinalsTests, GoodHackToRenameFuchsiaIoToFuchsiaIoOneNoSelector) {
  TestLibrary library;
  library.AddFile("bad/fi-0083.test.fidl");
  library.ExpectFail(ErrFuchsiaIoExplicitOrdinals);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(OrdinalsTests, GoodHackToRenameFuchsiaIoToFuchsiaIoOneHasSelector) {
  TestLibrary library(R"FIDL(
library fuchsia.io;

protocol SomeProtocol {
    @selector("fuchsia.io1/Node.Open")
    SomeMethod();
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(OrdinalsTests, WrongComposedMethodDoesNotGetGeneratedOrdinal) {
  TestLibrary library(R"FIDL(
library example;

protocol Node {
    SomeMethod(struct { id Id; });
};

protocol Directory {
    compose Node;
    Unlink();
};

protocol DirectoryAdmin {
    compose Directory;
};
)FIDL");
  library.ExpectFail(ErrNameNotFound, "Id", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
