// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/operation/unbuffered_operations_builder.h"

#include <lib/zx/vmo.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace storage {
namespace {

using ::testing::_;
using ::testing::ElementsAre;
using ::testing::Field;

constexpr size_t kVmoSize = 8192;

TEST(UnbufferedOperationsBuilderTest, NoRequest) {
  UnbufferedOperationsBuilder builder;
  EXPECT_EQ(builder.BlockCount(), 0ul);

  auto requests = builder.TakeOperations();

  EXPECT_TRUE(requests.empty());
  EXPECT_EQ(builder.BlockCount(), 0ul);
}

TEST(UnbufferedOperationsBuilderTest, EmptyRequest) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operation;
  operation.vmo = zx::unowned_vmo(vmo.get());
  operation.op.type = OperationType::kWrite;
  operation.op.vmo_offset = 0;
  operation.op.dev_offset = 0;
  operation.op.length = 0;
  builder.Add(operation);
  EXPECT_EQ(builder.BlockCount(), 0ul);

  auto requests = builder.TakeOperations();
  EXPECT_EQ(BlockCount(requests), 0ul);
  EXPECT_TRUE(requests.empty());
}

TEST(UnbufferedOperationsBuilderTest, OneRequest) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operation;
  operation.vmo = zx::unowned_vmo(vmo.get());
  operation.op.type = OperationType::kWrite;
  operation.op.vmo_offset = 0;
  operation.op.dev_offset = 0;
  operation.op.length = 1;
  builder.Add(operation);
  ASSERT_EQ(builder.BlockCount(), 1ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(BlockCount(requests), 1ul);
  ASSERT_EQ(requests.size(), 1ul);
  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, operation.op.vmo_offset);
  EXPECT_EQ(requests[0].op.dev_offset, operation.op.dev_offset);
  EXPECT_EQ(requests[0].op.length, operation.op.length);
  EXPECT_EQ(builder.BlockCount(), 0ul);
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsDifferentVmos) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmos[2];
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmos[0]), ZX_OK);
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmos[1]), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmos[0].get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 0;
  operations[0].op.dev_offset = 0;
  operations[0].op.length = 1;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmos[1].get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 1;
  operations[1].op.dev_offset = 1;
  operations[1].op.length = 2;
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);
  auto requests = builder.TakeOperations();
  EXPECT_EQ(BlockCount(requests), 3ul);
  ASSERT_EQ(requests.size(), 2ul);

  for (size_t i = 0; i < 2; i++) {
    EXPECT_EQ(requests[i].vmo->get(), vmos[i].get());
    EXPECT_EQ(requests[i].op.vmo_offset, operations[i].op.vmo_offset);
    EXPECT_EQ(requests[i].op.dev_offset, operations[i].op.dev_offset);
    EXPECT_EQ(requests[i].op.length, operations[i].op.length);
  }
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsSameVmoUnalignedVmoOffset) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 0;
  operations[0].op.dev_offset = 0;
  operations[0].op.length = 1;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 2;
  operations[1].op.dev_offset = 1;
  operations[1].op.length = 2;
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);
  auto requests = builder.TakeOperations();
  EXPECT_EQ(BlockCount(requests), 3ul);
  ASSERT_EQ(requests.size(), 2ul);

  for (size_t i = 0; i < 2; i++) {
    EXPECT_EQ(requests[i].vmo->get(), vmo.get());
    EXPECT_EQ(requests[i].op.vmo_offset, operations[i].op.vmo_offset);
    EXPECT_EQ(requests[i].op.dev_offset, operations[i].op.dev_offset);
    EXPECT_EQ(requests[i].op.length, operations[i].op.length);
  }
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsSameVmoUnalignedVmoOffsetReverseOrder) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 2;
  operations[0].op.dev_offset = 1;
  operations[0].op.length = 2;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 0;
  operations[1].op.dev_offset = 0;
  operations[1].op.length = 1;
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 2ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, operations[1].op.vmo_offset);
  EXPECT_EQ(requests[0].op.dev_offset, operations[1].op.dev_offset);
  EXPECT_EQ(requests[0].op.length, operations[1].op.length);

  EXPECT_EQ(requests[1].vmo->get(), vmo.get());
  EXPECT_EQ(requests[1].op.vmo_offset, operations[0].op.vmo_offset);
  EXPECT_EQ(requests[1].op.dev_offset, operations[0].op.dev_offset);
  EXPECT_EQ(requests[1].op.length, operations[0].op.length);
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsSameVmoUnalignedDevOffset) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 0;
  operations[0].op.dev_offset = 0;
  operations[0].op.length = 1;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 1;
  operations[1].op.dev_offset = 2;
  operations[1].op.length = 2;
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 2ul);

  for (size_t i = 0; i < 2; i++) {
    EXPECT_EQ(requests[i].vmo->get(), vmo.get());
    EXPECT_EQ(requests[i].op.vmo_offset, operations[i].op.vmo_offset);
    EXPECT_EQ(requests[i].op.dev_offset, operations[i].op.dev_offset);
    EXPECT_EQ(requests[i].op.length, operations[i].op.length);
  }
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsSameVmoUnalignedDevOffsetReverseOrder) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 1;
  operations[0].op.dev_offset = 2;
  operations[0].op.length = 2;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 0;
  operations[1].op.dev_offset = 0;
  operations[1].op.length = 1;
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 2ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, operations[1].op.vmo_offset);
  EXPECT_EQ(requests[0].op.dev_offset, operations[1].op.dev_offset);
  EXPECT_EQ(requests[0].op.length, operations[1].op.length);

  EXPECT_EQ(requests[1].vmo->get(), vmo.get());
  EXPECT_EQ(requests[1].op.vmo_offset, operations[0].op.vmo_offset);
  EXPECT_EQ(requests[1].op.dev_offset, operations[0].op.dev_offset);
  EXPECT_EQ(requests[1].op.length, operations[0].op.length);
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsSameVmoDifferentTypes) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 0;
  operations[0].op.dev_offset = 0;
  operations[0].op.length = 1;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kRead;
  operations[1].op.vmo_offset = 1;
  operations[1].op.dev_offset = 1;
  operations[1].op.length = 2;
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 2ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.type, operations[0].op.type);
  EXPECT_EQ(requests[0].op.vmo_offset, operations[0].op.vmo_offset);
  EXPECT_EQ(requests[0].op.dev_offset, operations[0].op.dev_offset);
  EXPECT_EQ(requests[0].op.length, operations[0].op.length);

  EXPECT_EQ(requests[1].vmo->get(), vmo.get());
  EXPECT_EQ(requests[1].op.type, operations[1].op.type);
  EXPECT_EQ(requests[1].op.vmo_offset, operations[1].op.vmo_offset);
  EXPECT_EQ(requests[1].op.dev_offset, operations[1].op.dev_offset);
  EXPECT_EQ(requests[1].op.length, operations[1].op.length);
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsSameVmoDifferentStartCoalesced) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 0;
  operations[0].op.dev_offset = 0;
  operations[0].op.length = 1;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 1;
  operations[1].op.dev_offset = 1;
  operations[1].op.length = 2;
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 1ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, operations[0].op.vmo_offset);
  EXPECT_EQ(requests[0].op.dev_offset, operations[0].op.dev_offset);
  EXPECT_EQ(requests[0].op.length, operations[0].op.length + operations[1].op.length);
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsSameVmoDifferentStartCoalescedReverseOrder) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 1;
  operations[0].op.dev_offset = 1;
  operations[0].op.length = 2;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 0;
  operations[1].op.dev_offset = 0;
  operations[1].op.length = 1;
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 1ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, operations[1].op.vmo_offset);
  EXPECT_EQ(requests[0].op.dev_offset, operations[1].op.dev_offset);
  EXPECT_EQ(requests[0].op.length, operations[0].op.length + operations[1].op.length);
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsSameVmoDifferentStartPartialCoalesced) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 0;
  operations[0].op.dev_offset = 0;
  operations[0].op.length = 2;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 1;
  operations[1].op.dev_offset = 1;
  operations[1].op.length = 2;
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 1ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, operations[0].op.vmo_offset);
  EXPECT_EQ(requests[0].op.dev_offset, operations[0].op.dev_offset);
  EXPECT_EQ(requests[0].op.length, 3ul);
}

TEST(UnbufferedOperationsBuilderTest,
     TwoRequestsSameVmoDifferentStartPartialCoalescedReverseOrder) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 1;
  operations[0].op.dev_offset = 1;
  operations[0].op.length = 2;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 0;
  operations[1].op.dev_offset = 0;
  operations[1].op.length = 2;
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 1ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, operations[1].op.vmo_offset);
  EXPECT_EQ(requests[0].op.dev_offset, operations[1].op.dev_offset);
  EXPECT_EQ(requests[0].op.length, 3ul);
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsSameVmoSameStartCoalesced) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 0;
  operations[0].op.dev_offset = 0;
  operations[0].op.length = 1;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 0;
  operations[1].op.dev_offset = 0;
  operations[1].op.length = 2;
  builder.Add(operations[1]);
  ASSERT_EQ(builder.BlockCount(), 2ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 1ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, operations[0].op.vmo_offset);
  EXPECT_EQ(requests[0].op.dev_offset, operations[0].op.dev_offset);
  EXPECT_EQ(requests[0].op.length, operations[1].op.length);
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsSameVmoSameStartCoalescedReverseOrder) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 0;
  operations[0].op.dev_offset = 0;
  operations[0].op.length = 2;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 0;
  operations[1].op.dev_offset = 0;
  operations[1].op.length = 1;
  builder.Add(operations[1]);
  ASSERT_EQ(builder.BlockCount(), 2ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 1ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, operations[0].op.vmo_offset);
  EXPECT_EQ(requests[0].op.dev_offset, operations[0].op.dev_offset);
  EXPECT_EQ(requests[0].op.length, operations[0].op.length);
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsSameVmoSubsumeRequest) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 1;
  operations[0].op.dev_offset = 1;
  operations[0].op.length = 1;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 0;
  operations[1].op.dev_offset = 0;
  operations[1].op.length = 3;
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 1ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, operations[1].op.vmo_offset);
  EXPECT_EQ(requests[0].op.dev_offset, operations[1].op.dev_offset);
  EXPECT_EQ(requests[0].op.length, operations[1].op.length);
}

TEST(UnbufferedOperationsBuilderTest, TwoRequestsSameVmoSubsumeRequestReverse) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[2];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 0;
  operations[0].op.dev_offset = 0;
  operations[0].op.length = 3;
  builder.Add(operations[0]);
  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 1;
  operations[1].op.dev_offset = 1;
  operations[1].op.length = 1;
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 1ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, operations[0].op.vmo_offset);
  EXPECT_EQ(requests[0].op.dev_offset, operations[0].op.dev_offset);
  EXPECT_EQ(requests[0].op.length, operations[0].op.length);
}

TEST(UnbufferedOperationsBuilderTest, RequestsBridgeAndCoalesceAllMergableRequests) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation operations[3];
  operations[0].vmo = zx::unowned_vmo(vmo.get());
  operations[0].op.type = OperationType::kWrite;
  operations[0].op.vmo_offset = 0;
  operations[0].op.dev_offset = 0;
  operations[0].op.length = 3;

  operations[1].vmo = zx::unowned_vmo(vmo.get());
  operations[1].op.type = OperationType::kWrite;
  operations[1].op.vmo_offset = 5;
  operations[1].op.dev_offset = 5;
  operations[1].op.length = 3;

  // operation two has range that overlaps with operation[0] and operation[1].
  operations[2].vmo = zx::unowned_vmo(vmo.get());
  operations[2].op.type = OperationType::kWrite;
  operations[2].op.vmo_offset = 2;
  operations[2].op.dev_offset = 2;
  operations[2].op.length = 4;

  builder.Add(operations[0]);
  EXPECT_EQ(builder.BlockCount(), 3ul);
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 6ul);
  builder.Add(operations[2]);
  EXPECT_EQ(builder.BlockCount(), 8ul);

  // operation[2] bridges operation[0] and operation[1], coalescing all three into one.
  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 1ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, 0ul);
  EXPECT_EQ(requests[0].op.dev_offset, 0ul);
  EXPECT_EQ(requests[0].op.length, 8ul);

  // Flip the order of Add. It should still coalesce all three into one.
  builder.Add(operations[1]);
  EXPECT_EQ(builder.BlockCount(), 3ul);
  builder.Add(operations[0]);
  EXPECT_EQ(builder.BlockCount(), 6ul);
  builder.Add(operations[2]);
  EXPECT_EQ(builder.BlockCount(), 8ul);

  requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 1ul);

  EXPECT_EQ(requests[0].vmo->get(), vmo.get());
  EXPECT_EQ(requests[0].op.vmo_offset, 0ul);
  EXPECT_EQ(requests[0].op.dev_offset, 0ul);
  EXPECT_EQ(requests[0].op.length, 8ul);
}

TEST(UnbufferedOperationBuilderTest, DeduplicateIdenticalRequests) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  UnbufferedOperation op1;
  op1.vmo = zx::unowned_vmo(vmo.get());
  op1.op.type = OperationType::kWrite;
  op1.op.vmo_offset = 0;
  op1.op.dev_offset = 0;
  op1.op.length = 5;

  UnbufferedOperation op2 = op1;

  builder.Add(op1);
  EXPECT_EQ(builder.BlockCount(), 5ul);
  builder.Add(op2);
  EXPECT_EQ(builder.BlockCount(), 5ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 1ul);
  EXPECT_EQ(requests[0].op.vmo_offset, 0ul);
  EXPECT_EQ(requests[0].op.dev_offset, 0ul);
  EXPECT_EQ(requests[0].op.length, 5ul);
}

TEST(UnbufferedOperationBuilderTest, OverwriteMiddleWithDifferentVmoSplitsPriorOperation) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo1, vmo2;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo1), ZX_OK);
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo2), ZX_OK);

  UnbufferedOperation op1;
  op1.vmo = zx::unowned_vmo(vmo1.get());
  op1.op.type = OperationType::kWrite;
  op1.op.vmo_offset = 0;
  op1.op.dev_offset = 0;
  op1.op.length = 10;

  UnbufferedOperation op2;
  op2.vmo = zx::unowned_vmo(vmo2.get());
  op2.op.type = OperationType::kWrite;
  op2.op.vmo_offset = 100;
  op2.op.dev_offset = 3;
  op2.op.length = 4;

  builder.Add(op1);
  EXPECT_EQ(builder.BlockCount(), 10ul);
  builder.Add(op2);
  EXPECT_EQ(builder.BlockCount(), 10ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 3ul);

  // [0, 3) from vmo1
  EXPECT_EQ(requests[0].vmo->get(), vmo1.get());
  EXPECT_EQ(requests[0].op.vmo_offset, 0ul);
  EXPECT_EQ(requests[0].op.dev_offset, 0ul);
  EXPECT_EQ(requests[0].op.length, 3ul);

  // [3, 7) from vmo2
  EXPECT_EQ(requests[1].vmo->get(), vmo2.get());
  EXPECT_EQ(requests[1].op.vmo_offset, 100ul);
  EXPECT_EQ(requests[1].op.dev_offset, 3ul);
  EXPECT_EQ(requests[1].op.length, 4ul);

  // [7, 10) from vmo1 (vmo_offset shifted by 7)
  EXPECT_EQ(requests[2].vmo->get(), vmo1.get());
  EXPECT_EQ(requests[2].op.vmo_offset, 7ul);
  EXPECT_EQ(requests[2].op.dev_offset, 7ul);
  EXPECT_EQ(requests[2].op.length, 3ul);
}

TEST(UnbufferedOperationBuilderTest, OverwriteStartWithDifferentVmo) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo1, vmo2;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo1), ZX_OK);
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo2), ZX_OK);

  UnbufferedOperation op1;
  op1.vmo = zx::unowned_vmo(vmo1.get());
  op1.op.type = OperationType::kWrite;
  op1.op.vmo_offset = 0;
  op1.op.dev_offset = 0;
  op1.op.length = 5;

  UnbufferedOperation op2;
  op2.vmo = zx::unowned_vmo(vmo2.get());
  op2.op.type = OperationType::kWrite;
  op2.op.vmo_offset = 50;
  op2.op.dev_offset = 0;
  op2.op.length = 2;

  builder.Add(op1);
  builder.Add(op2);
  EXPECT_EQ(builder.BlockCount(), 5ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 2ul);

  // [0, 2) from vmo2
  EXPECT_EQ(requests[0].vmo->get(), vmo2.get());
  EXPECT_EQ(requests[0].op.vmo_offset, 50ul);
  EXPECT_EQ(requests[0].op.dev_offset, 0ul);
  EXPECT_EQ(requests[0].op.length, 2ul);

  // [2, 5) from vmo1
  EXPECT_EQ(requests[1].vmo->get(), vmo1.get());
  EXPECT_EQ(requests[1].op.vmo_offset, 2ul);
  EXPECT_EQ(requests[1].op.dev_offset, 2ul);
  EXPECT_EQ(requests[1].op.length, 3ul);
}

TEST(UnbufferedOperationBuilderTest, OverwriteEndWithDifferentVmo) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmo1, vmo2;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo1), ZX_OK);
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo2), ZX_OK);

  UnbufferedOperation op1;
  op1.vmo = zx::unowned_vmo(vmo1.get());
  op1.op.type = OperationType::kWrite;
  op1.op.vmo_offset = 0;
  op1.op.dev_offset = 0;
  op1.op.length = 5;

  UnbufferedOperation op2;
  op2.vmo = zx::unowned_vmo(vmo2.get());
  op2.op.type = OperationType::kWrite;
  op2.op.vmo_offset = 50;
  op2.op.dev_offset = 3;
  op2.op.length = 2;

  builder.Add(op1);
  builder.Add(op2);
  EXPECT_EQ(builder.BlockCount(), 5ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 2ul);

  // [0, 3) from vmo1
  EXPECT_EQ(requests[0].vmo->get(), vmo1.get());
  EXPECT_EQ(requests[0].op.vmo_offset, 0ul);
  EXPECT_EQ(requests[0].op.dev_offset, 0ul);
  EXPECT_EQ(requests[0].op.length, 3ul);

  // [3, 5) from vmo2
  EXPECT_EQ(requests[1].vmo->get(), vmo2.get());
  EXPECT_EQ(requests[1].op.vmo_offset, 50ul);
  EXPECT_EQ(requests[1].op.dev_offset, 3ul);
  EXPECT_EQ(requests[1].op.length, 2ul);
}

TEST(UnbufferedOperationBuilderTest, OverwriteSpanningMultipleOperations) {
  UnbufferedOperationsBuilder builder;

  zx::vmo vmos[4];
  for (auto& vmo : vmos) {
    ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);
  }

  // op1: [0, 2)
  builder.Add(
      {.vmo = zx::unowned_vmo(vmos[0].get()),
       .op = {.type = OperationType::kWrite, .vmo_offset = 0, .dev_offset = 0, .length = 2}});
  // op2: [3, 5)
  builder.Add(
      {.vmo = zx::unowned_vmo(vmos[1].get()),
       .op = {.type = OperationType::kWrite, .vmo_offset = 0, .dev_offset = 3, .length = 2}});
  // op3: [6, 8)
  builder.Add(
      {.vmo = zx::unowned_vmo(vmos[2].get()),
       .op = {.type = OperationType::kWrite, .vmo_offset = 0, .dev_offset = 6, .length = 2}});

  EXPECT_EQ(builder.BlockCount(), 6ul);

  // op4: [1, 7) from vmos[3]
  builder.Add(
      {.vmo = zx::unowned_vmo(vmos[3].get()),
       .op = {.type = OperationType::kWrite, .vmo_offset = 10, .dev_offset = 1, .length = 6}});

  EXPECT_EQ(builder.BlockCount(), 8ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 3ul);

  // [0, 1) from vmos[0]
  EXPECT_EQ(requests[0].vmo->get(), vmos[0].get());
  EXPECT_EQ(requests[0].op.vmo_offset, 0ul);
  EXPECT_EQ(requests[0].op.dev_offset, 0ul);
  EXPECT_EQ(requests[0].op.length, 1ul);

  // [1, 7) from vmos[3]
  EXPECT_EQ(requests[1].vmo->get(), vmos[3].get());
  EXPECT_EQ(requests[1].op.vmo_offset, 10ul);
  EXPECT_EQ(requests[1].op.dev_offset, 1ul);
  EXPECT_EQ(requests[1].op.length, 6ul);

  // [7, 8) from vmos[2] (vmo_offset = 0 + 1 = 1)
  EXPECT_EQ(requests[2].vmo->get(), vmos[2].get());
  EXPECT_EQ(requests[2].op.vmo_offset, 1ul);
  EXPECT_EQ(requests[2].op.dev_offset, 7ul);
  EXPECT_EQ(requests[2].op.length, 1ul);
}

TEST(UnbufferedOperationBuilderTest, OperationsWithPointersDeduplicateAndSplit) {
  UnbufferedOperationsBuilder builder;
  const char* buf = "foo";
  builder.Add({.data = buf, .op = {.type = OperationType::kWrite, .dev_offset = 1, .length = 7}});
  builder.Add({.data = buf, .op = {.type = OperationType::kWrite, .dev_offset = 2, .length = 13}});
  EXPECT_EQ(builder.BlockCount(), 14ul);
  EXPECT_THAT(
      builder.TakeOperations(),
      ElementsAre(
          AllOf(Field(&UnbufferedOperation::data, buf),
                Field(&UnbufferedOperation::op,
                      AllOf(Field(&Operation::type, OperationType::kWrite),
                            Field(&Operation::dev_offset, 1), Field(&Operation::length, 1)))),
          AllOf(Field(&UnbufferedOperation::data, buf),
                Field(&UnbufferedOperation::op,
                      AllOf(Field(&Operation::type, OperationType::kWrite),
                            Field(&Operation::dev_offset, 2), Field(&Operation::length, 13))))));
}

TEST(UnbufferedOperationBuilderTest, OperationsWithPointersMergeWhenContiguous) {
  UnbufferedOperationsBuilder builder;
  const char* buf = "foo";
  builder.Add(
      {.data = buf,
       .op = {.type = OperationType::kWrite, .vmo_offset = 0, .dev_offset = 0, .length = 2}});
  builder.Add(
      {.data = buf,
       .op = {.type = OperationType::kWrite, .vmo_offset = 2, .dev_offset = 2, .length = 3}});
  EXPECT_EQ(builder.BlockCount(), 5ul);

  auto requests = builder.TakeOperations();
  ASSERT_EQ(requests.size(), 1ul);
  EXPECT_EQ(requests[0].data, buf);
  EXPECT_EQ(requests[0].op.vmo_offset, 0ul);
  EXPECT_EQ(requests[0].op.dev_offset, 0ul);
  EXPECT_EQ(requests[0].op.length, 5ul);
}

TEST(UnbufferedOperationsBuilderDeathTest, BlockCountOverflowAsserts) {
  std::vector<UnbufferedOperation> operations = {
      UnbufferedOperation{.op = {.length = std::numeric_limits<uint64_t>::max()}},
      UnbufferedOperation{.op = {.length = std::numeric_limits<uint64_t>::max()}},
  };
  ASSERT_DEATH({ BlockCount(operations); }, _);
}

}  // namespace
}  // namespace storage
