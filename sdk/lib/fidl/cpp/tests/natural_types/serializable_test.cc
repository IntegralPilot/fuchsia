// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.types/cpp/natural_types.h>

#include <gtest/gtest.h>

// Verify that serializable FIDL types define `kSerializableName` correctly.
TEST(Serializable, SerializableName) {
  ASSERT_STREQ(test_types::SerializableTable::kSerializableName, "test.types.SerializableTable");
  ASSERT_STREQ(test_types::SerializableStruct::kSerializableName, "test.types.SerializableStruct");
  ASSERT_STREQ(test_types::SerializableUnion::kSerializableName, "test.types.SerializableUnion");
}
