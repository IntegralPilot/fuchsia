// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <initializer_list>
#include <string_view>
#include <vector>

#include <fbl/ref_ptr.h>
#include <fbl/string.h>
#include <fbl/vector.h>
#include <gtest/gtest.h>

#include "src/storage/lib/vfs/cpp/pseudo_file.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace {

zx_status_t DummyReader(fbl::String* output) { return ZX_OK; }

zx_status_t DummyWriter(std::string_view input) { return ZX_OK; }

class VectorReader {
 public:
  VectorReader(std::initializer_list<fbl::String> strings) : strings_(strings) {}

  fs::PseudoFile::ReadHandler GetHandler() {
    return [this](fbl::String* output) {
      if (index_ >= strings_.size())
        return ZX_ERR_IO;
      *output = strings_[index_++];
      return ZX_OK;
    };
  }

  const fbl::Vector<fbl::String>& strings() const { return strings_; }

 private:
  fbl::Vector<fbl::String> strings_;
  size_t index_ = 0u;
};

class VectorWriter {
 public:
  explicit VectorWriter(size_t max_strings) : max_strings_(max_strings) {}

  fs::PseudoFile::WriteHandler GetHandler() {
    return [this](std::string_view input) {
      if (strings_.size() >= max_strings_)
        return ZX_ERR_IO;
      strings_.push_back(fbl::String(input));
      return ZX_OK;
    };
  }

  const fbl::Vector<fbl::String>& strings() const { return strings_; }

 private:
  const size_t max_strings_;
  fbl::Vector<fbl::String> strings_;
};

void CheckRead(const fbl::RefPtr<fs::Vnode>& file, zx_status_t status, size_t length, size_t offset,
               std::string_view expected) {
  std::vector<uint8_t> buf(length, '!');
  size_t actual = 0u;
  EXPECT_EQ(status, file->Read(buf.data(), length, offset, &actual));
  EXPECT_EQ(expected.size(), actual);
  EXPECT_EQ(memcmp(reinterpret_cast<const uint8_t*>(expected.data()), buf.data(), expected.size()),
            0);
}

void CheckWrite(const fbl::RefPtr<fs::Vnode>& file, zx_status_t status, size_t offset,
                std::string_view content, size_t expected_actual) {
  size_t actual = 0u;
  EXPECT_EQ(status, file->Write(content.data(), content.size(), offset, &actual));
  EXPECT_EQ(expected_actual, actual);
}

void CheckAppend(const fbl::RefPtr<fs::Vnode>& file, zx_status_t status, std::string_view content,
                 size_t expected_end, size_t expected_actual) {
  size_t end = 0u;
  size_t actual = 0u;
  EXPECT_EQ(status, file->Append(content.data(), content.size(), &end, &actual));
  EXPECT_EQ(expected_end, end);
  EXPECT_EQ(expected_actual, actual);
}
TEST(PseudoFile, Protocols) {
  auto file = fbl::MakeRefCounted<fs::BufferedPseudoFile>();
  ASSERT_EQ(file->GetProtocols(), fuchsia_io::NodeProtocolKinds::kFile);
}

TEST(PseudoFile, OpenValidationBuffered) {
  // no read handler, no write handler
  {
    auto file = fbl::MakeRefCounted<fs::BufferedPseudoFile>();
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRwStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRxStarDir));
  }

  // read handler, no write handler
  {
    auto file = fbl::MakeRefCounted<fs::BufferedPseudoFile>(&DummyReader);
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRwStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRxStarDir));

    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_TRUE(redirect);
  }

  // no read handler, write handler
  {
    auto file = fbl::MakeRefCounted<fs::BufferedPseudoFile>(nullptr, &DummyWriter);
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRwStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRxStarDir));

    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_TRUE(redirect);
  }

  // read handler, write handler
  {
    auto file = fbl::MakeRefCounted<fs::BufferedPseudoFile>(&DummyReader, &DummyWriter);
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRxStarDir));
    {
      fbl::RefPtr<fs::Vnode> redirect;
      ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
      EXPECT_EQ(ZX_OK, file->Open(&redirect));
      EXPECT_TRUE(redirect);
    }
    {
      fbl::RefPtr<fs::Vnode> redirect;
      ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRwStarDir));
      EXPECT_EQ(ZX_OK, file->Open(&redirect));
      EXPECT_TRUE(redirect);
    }
    {
      fbl::RefPtr<fs::Vnode> redirect;
      ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
      EXPECT_EQ(ZX_OK, file->Open(&redirect));
      EXPECT_TRUE(redirect);
    }
  }
}

TEST(PseudoFile, OpenValidationUnbuffered) {
  // no read handler, no write handler
  {
    auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>();
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRwStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRxStarDir));
  }

  // read handler, no write handler
  {
    auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(&DummyReader);
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRwStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRxStarDir));
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_TRUE(redirect);
  }

  // no read handler, write handler
  {
    auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(nullptr, &DummyWriter);
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRwStarDir));
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRxStarDir));
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_TRUE(redirect);
  }

  // read handler, write handler
  {
    auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(&DummyReader, &DummyWriter);
    EXPECT_FALSE(file->ValidateRights(fuchsia_io::kRxStarDir));
    {
      fbl::RefPtr<fs::Vnode> redirect;
      ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
      EXPECT_EQ(ZX_OK, file->Open(&redirect));
      EXPECT_TRUE(redirect);
    }
    {
      fbl::RefPtr<fs::Vnode> redirect;
      ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRwStarDir));
      EXPECT_EQ(ZX_OK, file->Open(&redirect));
      EXPECT_TRUE(redirect);
    }
    {
      fbl::RefPtr<fs::Vnode> redirect;
      ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
      EXPECT_EQ(ZX_OK, file->Open(&redirect));
      EXPECT_TRUE(redirect);
    }
  }
}

TEST(PseudoFile, GetattrBuffered) {
  const fbl::String kHello = "hello";

  auto reader = [kHello](fbl::String* out) {
    *out = kHello;
    return ZX_OK;
  };

  // no read handler, no write handler
  {
    auto file = fbl::MakeRefCounted<fs::BufferedPseudoFile>();
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, file->GetProtocols());
    zx::result attr = file->GetAttributes();
    ASSERT_TRUE(attr.is_ok());

    fbl::RefPtr<fs::Vnode> redirect;
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, redirect->GetProtocols());

    zx::result open_attr = redirect->GetAttributes();
    ASSERT_TRUE(open_attr.is_ok());
    attr->content_size = 0;
    EXPECT_EQ(attr, open_attr);
  }

  // read handler, no write handler
  {
    auto file = fbl::MakeRefCounted<fs::BufferedPseudoFile>(reader);
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, file->GetProtocols());
    zx::result attr = file->GetAttributes();
    ASSERT_TRUE(attr.is_ok());

    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, redirect->GetProtocols());
    zx::result open_attr = redirect->GetAttributes();
    ASSERT_TRUE(open_attr.is_ok());
    attr->content_size = kHello.size();
    EXPECT_EQ(attr, open_attr);
  }

  // no read handler, write handler
  {
    auto file = fbl::MakeRefCounted<fs::BufferedPseudoFile>(nullptr, &DummyWriter);
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, file->GetProtocols());
    zx::result attr = file->GetAttributes();
    ASSERT_TRUE(attr.is_ok());

    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, redirect->GetProtocols());
    zx::result open_attr = redirect->GetAttributes();
    ASSERT_TRUE(open_attr.is_ok());
    attr->content_size = 0;
    EXPECT_EQ(attr, open_attr);
  }

  // read handler, write handler
  {
    auto file = fbl::MakeRefCounted<fs::BufferedPseudoFile>(reader, &DummyWriter);
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, file->GetProtocols());
    zx::result attr = file->GetAttributes();
    ASSERT_TRUE(attr.is_ok());

    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRwStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, redirect->GetProtocols());

    zx::result open_attr = redirect->GetAttributes();
    ASSERT_TRUE(open_attr.is_ok());
    attr->content_size = kHello.size();
    EXPECT_EQ(attr, open_attr);
  }
}

TEST(PseudoFile, GetattrUnbuffered) {
  const fbl::String kHello = "hello";

  auto reader = [kHello](fbl::String* out) {
    *out = kHello;
    return ZX_OK;
  };

  // no read handler, no write handler
  {
    auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>();
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, file->GetProtocols());
    zx::result attr = file->GetAttributes();
    ASSERT_TRUE(attr.is_ok());
    fbl::RefPtr<fs::Vnode> redirect;
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, redirect->GetProtocols());

    zx::result open_attr = redirect->GetAttributes();
    ASSERT_TRUE(open_attr.is_ok());
    EXPECT_EQ(attr, open_attr);
  }

  // read handler, no write handler
  {
    auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(reader);
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, file->GetProtocols());
    zx::result attr = file->GetAttributes();
    ASSERT_TRUE(attr.is_ok());

    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, redirect->GetProtocols());
    zx::result open_attr = redirect->GetAttributes();
    ASSERT_TRUE(open_attr.is_ok());
    EXPECT_EQ(attr, open_attr);
  }

  // no read handler, write handler
  {
    auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(nullptr, &DummyWriter);
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, file->GetProtocols());
    zx::result attr = file->GetAttributes();
    ASSERT_TRUE(attr.is_ok());

    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, redirect->GetProtocols());
    zx::result open_attr = redirect->GetAttributes();
    ASSERT_TRUE(open_attr.is_ok());
    EXPECT_EQ(attr, open_attr);
  }

  // read handler, write handler
  {
    auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(reader, &DummyWriter);
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, file->GetProtocols());
    zx::result attr = file->GetAttributes();
    ASSERT_TRUE(attr.is_ok());

    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRwStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(fuchsia_io::NodeProtocolKinds::kFile, redirect->GetProtocols());
    zx::result open_attr = redirect->GetAttributes();
    ASSERT_TRUE(open_attr.is_ok());
    EXPECT_EQ(attr, open_attr);
  }
}

TEST(PseudoFile, ReadBuffered) {
  VectorReader reader{"first", "second", "", fbl::String(std::string_view("null\0null", 9u))};
  auto file = fbl::MakeRefCounted<fs::BufferedPseudoFile>(reader.GetHandler());

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckRead(redirect, ZX_OK, 0u, 0u, "");
    CheckRead(redirect, ZX_OK, 4u, 0u, "firs");
    CheckRead(redirect, ZX_OK, 4u, 2u, "rst");
    CheckRead(redirect, ZX_OK, 5u, 0u, "first");
    CheckRead(redirect, ZX_OK, 8u, 0u, "first");
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckRead(redirect, ZX_OK, 4u, 2u, "cond");
    CheckRead(redirect, ZX_OK, 6u, 0u, "second");
    CheckRead(redirect, ZX_OK, 8u, 0u, "second");
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckRead(redirect, ZX_OK, 4u, 0u, "");
    CheckRead(redirect, ZX_OK, 4u, 2u, "");
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckRead(redirect, ZX_OK, 0u, 0u, "");
    CheckRead(redirect, ZX_OK, 4u, 0u, "null");
    CheckRead(redirect, ZX_OK, 4u, 2u, std::string_view("ll\0n", 4u));
    CheckRead(redirect, ZX_OK, 9u, 0u, std::string_view("null\0null", 9u));
    CheckRead(redirect, ZX_OK, 12u, 0u, std::string_view("null\0null", 9u));
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_EQ(ZX_ERR_IO, file->Open(&redirect));
  }
}

TEST(PseudoFile, ReadUnbuffered) {
  VectorReader reader{"first",
                      "second",
                      "third",
                      "fourth",
                      "fifth",
                      "",
                      fbl::String(std::string_view("null\0null", 9u))};
  auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(reader.GetHandler());

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckRead(redirect, ZX_OK, 0u, 0u, "");
    CheckRead(redirect, ZX_OK, 4u, 0u, "seco");
    CheckRead(redirect, ZX_OK, 4u, 2u, "");
    CheckRead(redirect, ZX_OK, 3u, 0u, "thi");
    CheckRead(redirect, ZX_OK, 6u, 0u, "fourth");
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kRStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckRead(redirect, ZX_OK, 8u, 0u, "fifth");
    CheckRead(redirect, ZX_OK, 4u, 0u, "");
    CheckRead(redirect, ZX_OK, 12u, 0u, std::string_view("null\0null", 9u));
    CheckRead(redirect, ZX_ERR_IO, 0u, 0u, "");
    EXPECT_EQ(ZX_OK, redirect->Close());
  }
}

TEST(PseudoFile, WriteBuffered) {
  VectorWriter writer(6u);
  auto file = fbl::MakeRefCounted<fs::BufferedPseudoFile>(nullptr, writer.GetHandler(), 10u);

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckWrite(redirect, ZX_OK, 0u, "fixx", 4u);
    CheckWrite(redirect, ZX_OK, 0u, "", 0u);
    CheckWrite(redirect, ZX_OK, 2u, "rst", 3u);
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckWrite(redirect, ZX_OK, 0u, "second", 6u);
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckAppend(redirect, ZX_OK, "thxrxxx", 7u, 7u);
    CheckWrite(redirect, ZX_OK, 2u, "i", 1u);
    EXPECT_EQ(ZX_OK, redirect->Truncate(4u));
    CheckAppend(redirect, ZX_OK, "d", 5u, 1u);
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckWrite(redirect, ZX_OK, 0u, "null", 4u);
    EXPECT_EQ(ZX_OK, redirect->Truncate(5u));
    CheckAppend(redirect, ZX_OK, "null", 9u, 4u);
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(ZX_ERR_NO_SPACE, redirect->Truncate(11u));
    CheckAppend(redirect, ZX_OK, "too-long", 8u, 8u);
    CheckAppend(redirect, ZX_OK, "-off-the-end", 10u, 2u);
    CheckAppend(redirect, ZX_ERR_NO_SPACE, "-overflow", 0u, 0u);
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(ZX_ERR_IO, redirect->Close());
  }

  EXPECT_EQ(6u, writer.strings().size());
  EXPECT_EQ(writer.strings()[0], fbl::String("first"));
  EXPECT_EQ(writer.strings()[1], fbl::String("second"));
  EXPECT_EQ(writer.strings()[2], fbl::String(""));
  EXPECT_EQ(writer.strings()[3], fbl::String("third"));
  EXPECT_EQ(writer.strings()[4], fbl::String("null\0null", 9u));
  EXPECT_EQ(writer.strings()[5], fbl::String("too-long-o"));
}

TEST(PseudoFile, WriteBufferedGapZeroInitialized) {
  VectorWriter writer(2u);
  auto file = fbl::MakeRefCounted<fs::BufferedPseudoFile>(nullptr, writer.GetHandler(), 32u);

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckWrite(redirect, ZX_OK, 4u, "test", 4u);
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckWrite(redirect, ZX_OK, 0u, "head", 4u);
    CheckWrite(redirect, ZX_OK, 8u, "tail", 4u);
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  EXPECT_EQ(2u, writer.strings().size());
  EXPECT_EQ(writer.strings()[0], fbl::String(std::string_view("\0\0\0\0test", 8u)));
  EXPECT_EQ(writer.strings()[1], fbl::String(std::string_view("head\0\0\0\0tail", 12u)));
}

TEST(PseudoFile, WriteUnbuffered) {
  VectorWriter writer(10u);
  auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(nullptr, writer.GetHandler());

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckWrite(redirect, ZX_OK, 0u, "first", 5u);
    CheckWrite(redirect, ZX_ERR_NO_SPACE, 2u, "xxx", 0u);
    CheckWrite(redirect, ZX_OK, 0u, "second", 6u);
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckWrite(redirect, ZX_OK, 0u, "", 0u);
    CheckAppend(redirect, ZX_OK, "third", 5u, 5u);
    CheckAppend(redirect, ZX_OK, std::string_view("null\0null", 9u), 9u, 9u);
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(ZX_OK, redirect->Truncate(0u));
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckAppend(redirect, ZX_OK, "fourth", 6u, 6u);
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckAppend(redirect, ZX_OK, "fifth", 5u, 5u);
    EXPECT_EQ(ZX_ERR_INVALID_ARGS, redirect->Truncate(10u));
    EXPECT_EQ(ZX_OK, redirect->Truncate(0u));
    EXPECT_EQ(ZX_OK, redirect->Close());
  }

  {
    fbl::RefPtr<fs::Vnode> redirect;
    ASSERT_TRUE(file->ValidateRights(fuchsia_io::kWStarDir));
    EXPECT_EQ(ZX_OK, file->Open(&redirect));
    CheckWrite(redirect, ZX_OK, 0u, "a long string", 13u);
    EXPECT_EQ(ZX_OK, redirect->Truncate(0u));
    EXPECT_EQ(ZX_ERR_IO, redirect->Close());
  }

  EXPECT_EQ(10u, writer.strings().size());
  EXPECT_EQ(writer.strings()[0], fbl::String("first"));
  EXPECT_EQ(writer.strings()[1], fbl::String("second"));
  EXPECT_EQ(writer.strings()[2], fbl::String(""));
  EXPECT_EQ(writer.strings()[3], fbl::String("third"));
  EXPECT_EQ(writer.strings()[4], fbl::String("null\0null", 9u));
  EXPECT_EQ(writer.strings()[5], fbl::String(""));
  EXPECT_EQ(writer.strings()[6], fbl::String("fourth"));
  EXPECT_EQ(writer.strings()[7], fbl::String("fifth"));
  EXPECT_EQ(writer.strings()[8], fbl::String(""));
  EXPECT_EQ(writer.strings()[9], fbl::String("a long string"));
}

}  // namespace
