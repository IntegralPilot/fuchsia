// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/errors.h"

#include <lib/fidl/cpp/wire/status.h>

#include <gtest/gtest.h>

namespace forensics {
namespace {

TEST(ErrorsTest, ValidUtf8String) {
  const ErrorOrString value("value1");

  ASSERT_TRUE(value.HasValue());
  EXPECT_EQ(value.Value(), "value1");
}

TEST(ErrorsTest, InvalidUtf8String) {
  const ErrorOrString value("\xC0\x80");

  ASSERT_FALSE(value.HasValue());
  EXPECT_EQ(value.Error(), Error::kInvalidFormat);
}

TEST(FidlErrorToForensicsErrorTest, NotFound) {
  const ::fidl::Error fidl_error = ::fidl::Status::TransportError(ZX_ERR_NOT_FOUND);
  EXPECT_EQ(FidlErrorToForensicsError(fidl_error), Error::kNotAvailableInProduct);
}

TEST(FidlErrorToForensicsErrorTest, Timeout) {
  const ::fidl::Error fidl_error = ::fidl::Status::TransportError(ZX_ERR_TIMED_OUT);
  EXPECT_EQ(FidlErrorToForensicsError(fidl_error), Error::kTimeout);
}

TEST(FidlErrorToForensicsErrorTest, Other) {
  const ::fidl::Error fidl_error = ::fidl::Status::TransportError(ZX_ERR_INTERNAL);
  EXPECT_EQ(FidlErrorToForensicsError(fidl_error), Error::kConnectionError);
}

}  // namespace
}  // namespace forensics
