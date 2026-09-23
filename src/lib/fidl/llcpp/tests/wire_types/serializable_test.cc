// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.types/cpp/wire.h>

#include <gtest/gtest.h>

// Verify that serializable FIDL types define `kSerializableName` correctly.
TEST(Serializable, SerializableName) {
  ASSERT_STREQ(test_types::wire::SerializableTable::kSerializableName,
               "test.types.SerializableTable");
  ASSERT_STREQ(test_types::wire::SerializableStruct::kSerializableName,
               "test.types.SerializableStruct");
  ASSERT_STREQ(test_types::wire::SerializableUnion::kSerializableName,
               "test.types.SerializableUnion");
}
