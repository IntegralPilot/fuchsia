// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42124308): Use std::map instead of FBL in this file.
#include <lib/inspect/cpp/vmo/block.h>
#include <lib/inspect/cpp/vmo/limits.h>
#include <lib/inspect/cpp/vmo/scanner.h>
#include <lib/inspect/cpp/vmo/snapshot.h>
#include <lib/inspect/cpp/vmo/state.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/stdcompat/optional.h>
#include <lib/stdcompat/string_view.h>
#include <threads.h>
#include <zircon/errors.h>
#include <zircon/rights.h>

#include <cstdint>
#include <iomanip>
#include <iostream>
#include <memory>
#include <ranges>
#include <sstream>
#include <string>

#include <fbl/intrusive_wavl_tree.h>
#include <fbl/vector.h>
#include <pretty/hexdump.h>
#include <zxtest/zxtest.h>

namespace {

using inspect::BoolProperty;
using inspect::ByteVectorProperty;
using inspect::DoubleArray;
using inspect::DoubleProperty;
using inspect::IntArray;
using inspect::IntProperty;
using inspect::Link;
using inspect::Node;
using inspect::Snapshot;
using inspect::StringArray;
using inspect::StringProperty;
using inspect::UintArray;
using inspect::UintProperty;
using inspect::internal::ArrayBlockFormat;
using inspect::internal::ArrayBlockPayload;
using inspect::internal::Block;
using inspect::internal::BlockIndex;
using inspect::internal::BlockType;
using inspect::internal::ExtentBlockFields;
using inspect::internal::GetType;
using inspect::internal::HeaderBlockFields;
using inspect::internal::Heap;
using inspect::internal::kMagicNumber;
using inspect::internal::kNumOrders;
using inspect::internal::LinkBlockDisposition;
using inspect::internal::LinkBlockPayload;
using inspect::internal::PropertyBlockFormat;
using inspect::internal::PropertyBlockPayload;
using inspect::internal::ScanBlocks;
using inspect::internal::State;
using inspect::internal::StringReferenceBlockFields;
using inspect::internal::StringReferenceBlockPayload;
using inspect::internal::TesterGetStringReferenceCount;
using inspect::internal::TesterLoadStringReference;
using inspect::internal::TesterSetStringReferenceCount;
using inspect::internal::ValueBlockFields;

std::shared_ptr<State> InitState(size_t size) {
  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(size, 0, &vmo));
  if (!bool(vmo)) {
    return NULL;
  }
  auto heap = std::make_unique<Heap>(std::move(vmo));
  return State::Create(std::move(heap));
}

// Container for scanned blocks from the buffer.
// TODO(https://fxbug.dev/42117368): Use std::map instead of intrusive containers when
// libstd++ is available.
struct ScannedBlock : public fbl::WAVLTreeContainable<std::unique_ptr<ScannedBlock>> {
  BlockIndex index;
  const Block* block;

  ScannedBlock(BlockIndex index, const Block* block) : index(index), block(block) {}

  BlockIndex GetKey() const { return index; }
};

union BlockToByte {
  Block b;
  std::uint8_t bytes[sizeof(Block)];
};

std::string block_to_hex_str(const Block* block) {
  BlockToByte coerce{*block};
  std::stringstream ret;
  ret << "[ ";
  for (std::uint64_t i = 0; i < sizeof(Block); i++) {
    if (coerce.bytes[i] == '\0') {
      ret << "00";
      if (i < sizeof(Block) - 1) {
        ret << ", ";
      }
    } else {
      ret << std::hex << std::setw(2) << std::setfill('0') << static_cast<int>(coerce.bytes[i]);
      if (i < sizeof(Block) - 1) {
        ret << ", ";
      }
    }
  }
  ret << " ]";
  return ret.str();
}

/// Compare two blocks and fail if they are not equal
///
/// actual: const Block*
/// expected: const Block
///
/// This is a macro so that it can generate a failure message with the real line number
#define CompareBlock(actual, expected) __CompareBlock(actual, expected, __LINE__)

void __CompareBlock(const Block* actual, const Block expected, int line) {
  if (memcmp((const uint8_t*)(&expected), (const uint8_t*)(actual), sizeof(Block)) != 0) {
    const std::string failure_msg =
        "Block header contents did not match. Expected BlockType: " +
        std::to_string(static_cast<int>(GetType(&expected))) + ". " +
        "Actual BlockType: " + std::to_string(static_cast<int>(GetType(actual))) + "\n" +
        "Expected: " + block_to_hex_str(&expected) + "\n" +
        "Actual:   " + block_to_hex_str(actual) + "\n";
    ADD_FAILURE("actual failure at %s:%d\n%s", __FILE__, line, failure_msg.c_str());
  }
}

template <typename T>
void PrintArray(const T* value, size_t count) {
  std::cout << "Array payload contents, interpreted as given type: ";
  for (size_t i = 0; i < count; i++) {
    std::cout << value[i] << " ";
  }

  std::cout << std::endl;
}

template <typename T>
void CompareArray(const Block* block, const T* expected, size_t count) {
  if (0 != memcmp(reinterpret_cast<const uint8_t*>(expected),
                  reinterpret_cast<const uint8_t*>(&block->payload) + 8, sizeof(T) * count)) {
    std::cout << "Compare Array Failed:\n"
              << "Expected: ";
    PrintArray(expected, count);

    std::cout << "Actual: ";
    PrintArray(reinterpret_cast<const T*>(block->payload_ptr() + 8),
               ArrayBlockPayload::Count::Get<size_t>(block->payload.u64));
    EXPECT_TRUE(false, "This assertion is not related to test contents; it only marks failure.");
  }
}

Block MakeBlock(uint64_t header) {
  Block ret;
  ret.header = header;
  ret.payload.u64 = 0;
  return ret;
}

Block MakeBlock(uint64_t header, const char payload[9]) {
  Block ret;
  ret.header = header;
  memcpy(ret.payload.data, payload, 8);
  return ret;
}

// MakeInlinedStringReferenceBlock will truncate to 4 bytes if data is longer, because allocating
// larger than sizeof(Block) (AKA order 0) would be a memory error in this context.
// This will also reduce the order of the block to 0, even if `data` could be stored in its
// entirety in a larger order block.
Block MakeInlinedOrder0StringReferenceBlock(std::string_view data,
                                            const uint64_t reference_count = 1) {
  EXPECT_LE(data.size(), 4);

  auto block = Block{};
  block.header = StringReferenceBlockFields::Order::Make(0) |
                 StringReferenceBlockFields::Type::Make(BlockType::kStringReference) |
                 StringReferenceBlockFields::NextExtentIndex::Make(0) |
                 StringReferenceBlockFields::ReferenceCount::Make(reference_count);

  block.payload.u64 = StringReferenceBlockPayload::TotalLength::Make(data.size());
  memcpy(block.payload.data + StringReferenceBlockPayload::TotalLength::SizeInBytes(), data.data(),
         std::min(data.size(), size_t{4}));

  return block;
}

Block MakeBlock(uint64_t header, uint64_t payload) {
  Block ret;
  ret.header = header;
  ret.payload.u64 = payload;
  return ret;
}

Block MakeIntBlock(uint64_t header, int64_t payload) {
  Block ret;
  ret.header = header;
  ret.payload.i64 = payload;
  return ret;
}

Block MakeBoolBlock(uint64_t header, bool payload) {
  Block ret;
  ret.header = header;
  ret.payload.u64 = payload;
  return ret;
}

Block MakeDoubleBlock(uint64_t header, double payload) {
  Block ret;
  ret.header = header;
  ret.payload.f64 = payload;
  return ret;
}

Block MakeHeader(uint64_t generation) {
  Block ret;
  ret.header = HeaderBlockFields::Type::Make(BlockType::kHeader) |
               HeaderBlockFields::Order::Make(inspect::internal::kVmoHeaderOrder) |
               HeaderBlockFields::Version::Make(inspect::internal::kVersion);
  memcpy(&ret.header_data[4], kMagicNumber, 4);
  ret.payload.u64 = generation;
  return ret;
}

Snapshot SnapshotAndScan(const zx::vmo& vmo,
                         fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>>* blocks,
                         size_t* free_blocks, size_t* allocated_blocks) {
  *free_blocks = *allocated_blocks = 0;

  Snapshot snapshot;
  Snapshot::Create(vmo, &snapshot);
  if (snapshot) {
    ScanBlocks(snapshot.data(), snapshot.size(), [&](BlockIndex index, const Block* block) {
      if (GetType(block) == BlockType::kFree) {
        *free_blocks += 1;
      } else {
        *allocated_blocks += 1;
      }
      blocks->insert(std::make_unique<ScannedBlock>(index, block));
      return true;
    });
  }
  return snapshot;
}

void CheckVmoGenCount(uint64_t expected, const zx::vmo& vmo) {
  auto expected_header = MakeHeader(expected);

  size_t free_blocks, allocated_blocks;
  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  auto snapshot = SnapshotAndScan(vmo, &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);
  auto actual_block = blocks.find(0)->block;

  CompareBlock(actual_block, expected_header);
  uint64_t size;
  vmo.get_size(&size);
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(actual_block), size);
}

TEST(State, DoFrozenVmoCopy) {
  auto state = State::CreateWithSize(4096);
  ASSERT_TRUE(state);

  const auto copy = state->FrozenVmoCopy();
  ASSERT_TRUE(copy.has_value());

  CheckVmoGenCount(inspect::internal::kVmoFrozen, copy.value());
  CheckVmoGenCount(0, state->GetVmo());
}

TEST(State, CreateAndCopy) {
  auto state = State::CreateWithSize(4096);
  ASSERT_TRUE(state);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  EXPECT_EQ(1u, allocated_blocks);
  EXPECT_EQ(7u, free_blocks);
  blocks.clear();

  zx::vmo copy;
  ASSERT_TRUE(state->Copy(&copy));

  snapshot = SnapshotAndScan(copy, &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  EXPECT_EQ(1u, allocated_blocks);
  EXPECT_EQ(7u, free_blocks);
}

TEST(State, CreateAndFreeStringReference) {
  auto state = InitState(8192);
  ASSERT_TRUE(state != nullptr);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t pre_free_blocks, pre_allocated_blocks;
  auto snapshot =
      SnapshotAndScan(state->GetVmo(), &blocks, &pre_free_blocks, &pre_allocated_blocks);
  ASSERT_TRUE(snapshot);

  BlockIndex idx;
  std::string_view sr("abcdefg");
  ASSERT_EQ(ZX_OK, state->CreateAndIncrementStringReference(sr, &idx));
  ASSERT_EQ("abcdefg", TesterLoadStringReference(*state, idx));

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks1;
  size_t free_blocks1, allocated_blocks1;
  auto snapshot1 = SnapshotAndScan(state->GetVmo(), &blocks1, &free_blocks1, &allocated_blocks1);
  ASSERT_TRUE(snapshot1);

  ASSERT_EQ(pre_allocated_blocks + 1, allocated_blocks1);

  state->ReleaseStringReference(idx);
}

TEST(State, CreateSeveralStringReferences) {
  auto state = InitState(8192);
  ASSERT_TRUE(state != nullptr);

  const auto one = std::string(150, '1');
  const auto two = std::string(150, '2');
  const auto three = std::string(200, '3');

  BlockIndex idx1, idx2, idx3;
  ASSERT_EQ(ZX_OK, state->CreateAndIncrementStringReference(one, &idx1));
  ASSERT_EQ(ZX_OK, state->CreateAndIncrementStringReference(two, &idx2));
  ASSERT_EQ(ZX_OK, state->CreateAndIncrementStringReference(three, &idx3));

  ASSERT_NE(idx1, idx2);
  ASSERT_NE(idx1, idx3);
  ASSERT_NE(idx2, idx3);

  ASSERT_EQ(one, TesterLoadStringReference(*state, idx1));
  ASSERT_EQ(two, TesterLoadStringReference(*state, idx2));
  ASSERT_EQ(three, TesterLoadStringReference(*state, idx3));

  state->ReleaseStringReference(idx1);
  state->ReleaseStringReference(idx2);
  state->ReleaseStringReference(idx3);
}

TEST(State, CreateLargeStringReference) {
  auto state = InitState(8192);
  ASSERT_TRUE(state != nullptr);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks1;
  size_t free_blocks1, allocated_blocks1;
  auto snapshot1 = SnapshotAndScan(state->GetVmo(), &blocks1, &free_blocks1, &allocated_blocks1);
  ASSERT_TRUE(snapshot1);

  BlockIndex idx;
  std::string data(6000, '.');

  ASSERT_EQ(ZX_OK, state->CreateAndIncrementStringReference(data, &idx));
  ASSERT_EQ(data, TesterLoadStringReference(*state, idx));

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks2;
  size_t free_blocks2, allocated_blocks2;
  auto snapshot2 = SnapshotAndScan(state->GetVmo(), &blocks2, &free_blocks2, &allocated_blocks2);
  ASSERT_TRUE(snapshot2);

  // StringReference + 2 extents
  ASSERT_EQ(allocated_blocks1 + 3, allocated_blocks2);

  state->ReleaseStringReference(idx);

  // Note: at this point we don't need to assert that the blocks are released properly,
  // because the Heap destructor will verify that it is empty.
}

TEST(State, CreateAndFreeFromSameReference) {
  auto state = InitState(8192);
  ASSERT_TRUE(state != nullptr);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks1;
  size_t free_blocks1, allocated_blocks1;
  auto snapshot1 = SnapshotAndScan(state->GetVmo(), &blocks1, &free_blocks1, &allocated_blocks1);
  ASSERT_TRUE(snapshot1);

  BlockIndex idx2;
  std::string data(3000, '.');

  ASSERT_EQ(ZX_OK, state->CreateAndIncrementStringReference(data, &idx2));
  ASSERT_EQ(data, TesterLoadStringReference(*state, idx2));

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks2;
  size_t free_blocks2, allocated_blocks2;
  auto snapshot2 = SnapshotAndScan(state->GetVmo(), &blocks2, &free_blocks2, &allocated_blocks2);
  ASSERT_TRUE(snapshot2);

  // StringReference + 1 extent
  ASSERT_EQ(allocated_blocks1 + 2, allocated_blocks2);

  // CreateStringReferenceWithCount will bump the reference count
  BlockIndex should_be_same;
  ASSERT_EQ(ZX_OK, state->CreateAndIncrementStringReference(data, &should_be_same));
  ASSERT_EQ(data, TesterLoadStringReference(*state, idx2));
  ASSERT_EQ(data, TesterLoadStringReference(*state, should_be_same));
  ASSERT_EQ(idx2, should_be_same);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks3;
  size_t free_blocks3, allocated_blocks3;
  auto snapshot3 = SnapshotAndScan(state->GetVmo(), &blocks3, &free_blocks3, &allocated_blocks3);
  ASSERT_TRUE(snapshot3);

  ASSERT_EQ(allocated_blocks2, allocated_blocks3);

  state->ReleaseStringReference(idx2);
  // still works, because reference count was bumped and therefore nothing was deallocated
  ASSERT_EQ(data, TesterLoadStringReference(*state, should_be_same));
  state->ReleaseStringReference(should_be_same);

  // After release, this causes a re-allocation
  ASSERT_EQ(ZX_OK, state->CreateAndIncrementStringReference(data, &idx2));
  ASSERT_EQ(data, TesterLoadStringReference(*state, idx2));

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks4;
  size_t free_blocks4, allocated_blocks4;
  auto snapshot4 = SnapshotAndScan(state->GetVmo(), &blocks4, &free_blocks4, &allocated_blocks4);
  ASSERT_TRUE(snapshot4);

  ASSERT_EQ(allocated_blocks3, allocated_blocks4);
  state->ReleaseStringReference(idx2);
}

TEST(State, StringReferenceAllocationFailureNoCache) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != nullptr);

  // A string large enough that extents will fail to allocate in a 4096-byte VMO.
  std::string data(6000, '.');
  BlockIndex idx;
  ASSERT_EQ(ZX_ERR_NO_MEMORY, state->CreateAndIncrementStringReference(data, &idx));

  // If caching happened before extent allocation failed, calling CreateAndIncrementStringReference
  // again with the same string would find the stale freed index in string_reference_ids_ and
  // return ZX_OK with a dangling index. With caching after success, it should fail again with
  // ZX_ERR_NO_MEMORY.
  BlockIndex idx2;
  ASSERT_EQ(ZX_ERR_NO_MEMORY, state->CreateAndIncrementStringReference(data, &idx2));
}

TEST(State, StringReferenceCountSaturation) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  const std::string_view data = "saturated_string";
  BlockIndex idx;
  ASSERT_EQ(ZX_OK, state->CreateAndIncrementStringReference(data, &idx));
  ASSERT_EQ(1u, TesterGetStringReferenceCount(*state, idx));

  // Manually set the reference count to the maximum value (kMask).
  TesterSetStringReferenceCount(*state, idx, StringReferenceBlockFields::ReferenceCount::kMask);
  ASSERT_EQ(StringReferenceBlockFields::ReferenceCount::kMask,
            TesterGetStringReferenceCount(*state, idx));

  // Incrementing at max count should saturate and not wrap around to 0.
  BlockIndex idx2;
  ASSERT_EQ(ZX_OK, state->CreateAndIncrementStringReference(data, &idx2));
  ASSERT_EQ(idx, idx2);
  ASSERT_EQ(StringReferenceBlockFields::ReferenceCount::kMask,
            TesterGetStringReferenceCount(*state, idx));

  // Releasing at max count should also saturate (pin) and not decrement.
  state->ReleaseStringReference(idx);
  ASSERT_EQ(StringReferenceBlockFields::ReferenceCount::kMask,
            TesterGetStringReferenceCount(*state, idx));

  // Verify the string reference is still valid and not freed.
  ASSERT_EQ(data, TesterLoadStringReference(*state, idx));

  // Manually reset reference count to 1 and release to clean up before Heap destruction assert.
  TesterSetStringReferenceCount(*state, idx, 1);
  state->ReleaseStringReference(idx);
}

TEST(State, CreateIntProperty) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  IntProperty a = state->CreateIntProperty("a", 0, 0);
  IntProperty b = state->CreateIntProperty("b", 0, 0);
  IntProperty c = state->CreateIntProperty("c", 0, 0);

  a.Set(10);
  b.Add(5);
  b.Subtract(10);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header and 2 for each metric.
  EXPECT_EQ(7u, allocated_blocks);
  EXPECT_EQ(5u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(12));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);
  CompareBlock(blocks.find(2)->block,
               MakeIntBlock(ValueBlockFields::Type::Make(BlockType::kIntValue) |
                                ValueBlockFields::NameIndex::Make(3),
                            10));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("a"));
  CompareBlock(blocks.find(4)->block,
               MakeIntBlock(ValueBlockFields::Type::Make(BlockType::kIntValue) |
                                ValueBlockFields::NameIndex::Make(5),
                            -5));
  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("b"));
  CompareBlock(blocks.find(6)->block,
               MakeIntBlock(ValueBlockFields::Type::Make(BlockType::kIntValue) |
                                ValueBlockFields::NameIndex::Make(7),
                            0));
  CompareBlock(blocks.find(7)->block, MakeInlinedOrder0StringReferenceBlock("c"));
}

TEST(State, CreateUintProperty) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  UintProperty a = state->CreateUintProperty("a", 0, 0);
  UintProperty b = state->CreateUintProperty("b", 0, 0);
  UintProperty c = state->CreateUintProperty("c", 0, 0);

  a.Set(10);
  b.Add(15);
  b.Subtract(10);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header and 2 for each metric.
  EXPECT_EQ(7u, allocated_blocks);
  EXPECT_EQ(5u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(12));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);
  CompareBlock(blocks.find(2)->block,
               MakeBlock(ValueBlockFields::Type::Make(BlockType::kUintValue) |
                             ValueBlockFields::NameIndex::Make(3),
                         10));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("a"));
  CompareBlock(blocks.find(4)->block,
               MakeBlock(ValueBlockFields::Type::Make(BlockType::kUintValue) |
                             ValueBlockFields::NameIndex::Make(5),
                         5));
  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("b"));
  CompareBlock(blocks.find(6)->block,
               MakeIntBlock(ValueBlockFields::Type::Make(BlockType::kUintValue) |
                                ValueBlockFields::NameIndex::Make(7),
                            0));
  CompareBlock(blocks.find(7)->block, MakeInlinedOrder0StringReferenceBlock("c"));
}

TEST(State, CreateDoubleProperty) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  DoubleProperty a = state->CreateDoubleProperty("a", 0, 0);
  DoubleProperty b = state->CreateDoubleProperty("b", 0, 0);
  DoubleProperty c = state->CreateDoubleProperty("c", 0, 0);

  a.Set(3.25);
  b.Add(0.5);
  b.Subtract(0.25);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header and 2 for each metric.
  EXPECT_EQ(7u, allocated_blocks);
  EXPECT_EQ(5u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(12));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);
  CompareBlock(blocks.find(2)->block,
               MakeDoubleBlock(ValueBlockFields::Type::Make(BlockType::kDoubleValue) |
                                   ValueBlockFields::NameIndex::Make(3),
                               3.25));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("a"));
  CompareBlock(blocks.find(4)->block,
               MakeDoubleBlock(ValueBlockFields::Type::Make(BlockType::kDoubleValue) |
                                   ValueBlockFields::NameIndex::Make(5),
                               0.25));
  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("b"));
  CompareBlock(blocks.find(6)->block,
               MakeIntBlock(ValueBlockFields::Type::Make(BlockType::kDoubleValue) |
                                ValueBlockFields::NameIndex::Make(7),
                            0));
  CompareBlock(blocks.find(7)->block, MakeInlinedOrder0StringReferenceBlock("c"));
}

TEST(State, CreateBoolProperty) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);
  BoolProperty t = state->CreateBoolProperty("t", 0, true);
  BoolProperty f = state->CreateBoolProperty("f", 0, false);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  EXPECT_EQ(5u, allocated_blocks);
  EXPECT_EQ(6u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(4));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);
  CompareBlock(blocks.find(2)->block,
               MakeBoolBlock(ValueBlockFields::Type::Make(BlockType::kBoolValue) |
                                 ValueBlockFields::NameIndex::Make(3),
                             true));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("t"));
  CompareBlock(blocks.find(4)->block,
               MakeBoolBlock(ValueBlockFields::Type::Make(BlockType::kBoolValue) |
                                 ValueBlockFields::NameIndex::Make(5),
                             false));
  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("f"));
}

TEST(State, CreateStringArray) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != nullptr);

  StringArray d = state->CreateStringArray("d", 0, 2, ArrayBlockFormat::kDefault);
  d.Set(0, "abc");
  d.Set(1, "wxyz");

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  EXPECT_EQ(allocated_blocks, 5u);

  const auto expected_gen_count = allocated_blocks * 2;

  CompareBlock(blocks.find(0)->block, MakeHeader(expected_gen_count));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  CompareBlock(blocks.find(4)->block, MakeInlinedOrder0StringReferenceBlock("d"));

  CompareBlock(
      blocks.find(2)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                    ValueBlockFields::Order::Make(1) | ValueBlockFields::NameIndex::Make(4),
                ArrayBlockPayload::EntryType::Make(BlockType::kStringReference) |
                    ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kDefault) |
                    ArrayBlockPayload::Count::Make(2)));
  uint32_t value_indexes[] = {5, 6};
  CompareArray(blocks.find(2)->block, value_indexes, 2);

  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("abc"));
  CompareBlock(blocks.find(6)->block, MakeInlinedOrder0StringReferenceBlock("wxyz"));

  state->FreeStringArray(&d);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks_2;
  free_blocks = allocated_blocks = 0;
  snapshot = SnapshotAndScan(state->GetVmo(), &blocks_2, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  EXPECT_EQ(allocated_blocks, 1u);
}

TEST(State, UpdateStringArrayValue) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != nullptr);

  StringArray d = state->CreateStringArray("d", 0, 2, ArrayBlockFormat::kDefault);
  d.Set(0, "abc");
  d.Set(1, "wxyz");

  d.Set(0, "cba");
  d.Set(1, "zyxw");

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  CompareBlock(blocks.find(4)->block, MakeInlinedOrder0StringReferenceBlock("d"));

  CompareBlock(
      blocks.find(2)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                    ValueBlockFields::Order::Make(1) | ValueBlockFields::NameIndex::Make(4),
                ArrayBlockPayload::EntryType::Make(BlockType::kStringReference) |
                    ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kDefault) |
                    ArrayBlockPayload::Count::Make(2)));
  uint32_t value_indexes[] = {7, 5};
  CompareArray(blocks.find(2)->block, value_indexes, 2);

  CompareBlock(blocks.find(7)->block, MakeInlinedOrder0StringReferenceBlock("cba"));
  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("zyxw"));

  state->FreeStringArray(&d);

  // debug assert in heap insures that at this point there are no leaked blocks
}

TEST(State, CreateNumericArrays) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  IntArray a = state->CreateIntArray("a", 0, 10, ArrayBlockFormat::kDefault);
  UintArray b = state->CreateUintArray("b", 0, 10, ArrayBlockFormat::kDefault);
  DoubleArray c = state->CreateDoubleArray("c", 0, 10, ArrayBlockFormat::kDefault);

  a.Add(0, 10);
  a.Set(1, -10);
  a.Subtract(2, 9);
  // out of bounds
  a.Set(10, -10);
  a.Add(10, 0xFF);
  a.Subtract(10, 0xDD);

  b.Add(0, 10);
  b.Set(1, 10);
  b.Subtract(1, 9);
  // out of bounds
  b.Set(10, 10);
  b.Add(10, 10);
  b.Subtract(10, 10);

  c.Add(0, .25);
  c.Set(1, 1.25);
  c.Subtract(1, .5);
  // out of bounds
  c.Set(10, 10);
  c.Add(10, 10);
  c.Subtract(10, 10);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header and 2 for each metric.
  EXPECT_EQ(7u, allocated_blocks);
  EXPECT_EQ(5u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(42));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  {
    CompareBlock(blocks.find(2)->block, MakeInlinedOrder0StringReferenceBlock("a"));
    CompareBlock(
        blocks.find(8)->block,
        MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                      ValueBlockFields::Order::Make(3) | ValueBlockFields::NameIndex::Make(2),
                  ArrayBlockPayload::EntryType::Make(BlockType::kIntValue) |
                      ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kDefault) |
                      ArrayBlockPayload::Count::Make(10)));
    int64_t a_array_values[] = {10, -10, -9, 0, 0, 0, 0, 0, 0, 0};
    CompareArray(blocks.find(8)->block, a_array_values, 10);
  }

  {
    CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("b"));

    CompareBlock(
        blocks.find(16)->block,
        MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                      ValueBlockFields::Order::Make(3) | ValueBlockFields::NameIndex::Make(3),
                  ArrayBlockPayload::EntryType::Make(BlockType::kUintValue) |
                      ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kDefault) |
                      ArrayBlockPayload::Count::Make(10)));
    uint64_t b_array_values[] = {10, 1, 0, 0, 0, 0, 0, 0, 0, 0};
    CompareArray(blocks.find(16)->block, b_array_values, 10);
  }

  {
    CompareBlock(blocks.find(4)->block, MakeInlinedOrder0StringReferenceBlock("c"));

    CompareBlock(
        blocks.find(24)->block,
        MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                      ValueBlockFields::Order::Make(3) | ValueBlockFields::NameIndex::Make(4),
                  ArrayBlockPayload::EntryType::Make(BlockType::kDoubleValue) |
                      ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kDefault) |
                      ArrayBlockPayload::Count::Make(10)));
    double c_array_values[] = {.25, .75, 0, 0, 0, 0, 0, 0, 0, 0};
    CompareArray(blocks.find(24)->block, c_array_values, 10);
  }
}

TEST(State, CreateArrayChildren) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  Node root = state->CreateNode("root", 0);

  IntArray a = root.CreateIntArray("a", 10);
  UintArray b = root.CreateUintArray("b", 10);
  DoubleArray c = root.CreateDoubleArray("c", 10);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header and 2 for each metric.
  EXPECT_EQ(9u, allocated_blocks);
  EXPECT_EQ(4u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(8));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  CompareBlock(
      blocks.find(2)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kNodeValue) |
                    ValueBlockFields::ParentIndex::Make(0) | ValueBlockFields::NameIndex::Make(3),
                3));

  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("root"));

  {
    CompareBlock(blocks.find(4)->block, MakeInlinedOrder0StringReferenceBlock("a"));
    CompareBlock(
        blocks.find(8)->block,
        MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                      ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::Order::Make(3) |
                      ValueBlockFields::NameIndex::Make(4),
                  ArrayBlockPayload::EntryType::Make(BlockType::kIntValue) |
                      ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kDefault) |
                      ArrayBlockPayload::Count::Make(10)));
    int64_t a_array_values[] = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0};
    CompareArray(blocks.find(8)->block, a_array_values, 10);
  }

  {
    CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("b"));

    CompareBlock(
        blocks.find(16)->block,
        MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                      ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::Order::Make(3) |
                      ValueBlockFields::NameIndex::Make(5),
                  ArrayBlockPayload::EntryType::Make(BlockType::kUintValue) |
                      ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kDefault) |
                      ArrayBlockPayload::Count::Make(10)));
    uint64_t b_array_values[] = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0};
    CompareArray(blocks.find(16)->block, b_array_values, 10);
  }

  {
    CompareBlock(blocks.find(6)->block, MakeInlinedOrder0StringReferenceBlock("c"));

    CompareBlock(
        blocks.find(24)->block,
        MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                      ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::Order::Make(3) |
                      ValueBlockFields::NameIndex::Make(6),
                  ArrayBlockPayload::EntryType::Make(BlockType::kDoubleValue) |
                      ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kDefault) |
                      ArrayBlockPayload::Count::Make(10)));
    double c_array_values[] = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0};
    CompareArray(blocks.find(24)->block, c_array_values, 10);
  }
}

TEST(State, CreateLinearHistogramChildren) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  Node root = state->CreateNode("root", 0);

  auto a = root.CreateLinearIntHistogram("a", 10 /*floor*/, 5 /*step_size*/, 6 /*buckets*/);
  auto b = root.CreateLinearUintHistogram("b", 10 /*floor*/, 5 /*step_size*/, 6 /*buckets*/);
  auto c = root.CreateLinearDoubleHistogram("c", 10 /*floor*/, 5 /*step_size*/, 6 /*buckets*/);

  // Test moving of underlying LinearHistogram type.
  {
    inspect::LinearIntHistogram temp;
    temp = std::move(a);
    a = std::move(temp);
  }

  a.Insert(0, 3);
  a.Insert(10);
  a.Insert(1000);
  a.Insert(21);

  b.Insert(0, 3);
  b.Insert(10);
  b.Insert(1000);
  b.Insert(21);

  c.Insert(0, 3);
  c.Insert(10);
  c.Insert(1000);
  c.Insert(21);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header and 2 for each metric.
  EXPECT_EQ(9u, allocated_blocks);
  EXPECT_EQ(4u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(2 + 6 * 3 + 8 * 3));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  CompareBlock(
      blocks.find(2)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kNodeValue) |
                    ValueBlockFields::ParentIndex::Make(0) | ValueBlockFields::NameIndex::Make(3),
                3));

  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("root"));

  {
    CompareBlock(blocks.find(4)->block, MakeInlinedOrder0StringReferenceBlock("a"));
    CompareBlock(
        blocks.find(8)->block,
        MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                      ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::Order::Make(3) |
                      ValueBlockFields::NameIndex::Make(4),
                  ArrayBlockPayload::EntryType::Make(BlockType::kIntValue) |
                      ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kLinearHistogram) |
                      ArrayBlockPayload::Count::Make(10)));
    // Array is:
    // <floor>, <step_size>, <underflow>, <N buckets>..., <overflow>
    int64_t a_array_values[] = {10, 5, 3, 1, 0, 1, 0, 0, 0, 1};
    CompareArray(blocks.find(8)->block, a_array_values, 10);
  }

  {
    CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("b"));

    CompareBlock(
        blocks.find(16)->block,
        MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                      ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::Order::Make(3) |
                      ValueBlockFields::NameIndex::Make(5),
                  ArrayBlockPayload::EntryType::Make(BlockType::kUintValue) |
                      ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kLinearHistogram) |
                      ArrayBlockPayload::Count::Make(10)));
    // Array is:
    // <floor>, <step_size>, <underflow>, <N buckets>..., <overflow>
    uint64_t b_array_values[] = {10, 5, 3, 1, 0, 1, 0, 0, 0, 1};
    CompareArray(blocks.find(16)->block, b_array_values, 10);
  }

  {
    CompareBlock(blocks.find(6)->block, MakeInlinedOrder0StringReferenceBlock("c"));

    CompareBlock(
        blocks.find(24)->block,
        MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                      ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::Order::Make(3) |
                      ValueBlockFields::NameIndex::Make(6),
                  ArrayBlockPayload::EntryType::Make(BlockType::kDoubleValue) |
                      ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kLinearHistogram) |
                      ArrayBlockPayload::Count::Make(10)));
    // Array is:
    // <floor>, <step_size>, <underflow>, <N buckets>..., <overflow>
    double c_array_values[] = {10, 5, 3, 1, 0, 1, 0, 0, 0, 1};
    CompareArray(blocks.find(24)->block, c_array_values, 10);
  }
}

TEST(State, CreateExponentialHistogramChildren) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  Node root = state->CreateNode("root", 0);

  auto a = root.CreateExponentialIntHistogram("a", 1 /*floor*/, 1 /*initial_step*/,
                                              2 /*step_multiplier*/, 5 /*buckets*/);
  auto b = root.CreateExponentialUintHistogram("b", 1 /*floor*/, 1 /*initial_step*/,
                                               2 /*step_multiplier*/, 5 /*buckets*/);
  auto c = root.CreateExponentialDoubleHistogram("c", 1 /*floor*/, 1 /*initial_step*/,
                                                 2 /*step_multiplier*/, 5 /*buckets*/);

  // Test moving of underlying ExponentialHistogram type.
  {
    inspect::ExponentialIntHistogram temp;
    temp = std::move(a);
    a = std::move(temp);
  }

  a.Insert(0, 3);
  a.Insert(4);
  a.Insert(1000);
  a.Insert(30);

  b.Insert(0, 3);
  b.Insert(4);
  b.Insert(1000);
  b.Insert(30);

  c.Insert(0, 3);
  c.Insert(4);
  c.Insert(1000);
  c.Insert(30);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header and 2 for each metric.
  EXPECT_EQ(9u, allocated_blocks);
  EXPECT_EQ(4u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(2 + 8 * 3 + 8 * 3));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  CompareBlock(
      blocks.find(2)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kNodeValue) |
                    ValueBlockFields::ParentIndex::Make(0) | ValueBlockFields::NameIndex::Make(3),
                3));

  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("root"));

  {
    CompareBlock(blocks.find(4)->block, MakeInlinedOrder0StringReferenceBlock("a"));
    CompareBlock(
        blocks.find(8)->block,
        MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                      ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::Order::Make(3) |
                      ValueBlockFields::NameIndex::Make(4),
                  ArrayBlockPayload::EntryType::Make(BlockType::kIntValue) |
                      ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kExponentialHistogram) |
                      ArrayBlockPayload::Count::Make(10)));
    // Array is:
    // <floor>, <initial_step>, <step_multipler>, <underflow>, <N buckets>..., <overflow>
    int64_t a_array_values[] = {1, 1, 2, 3, 0, 0, 1, 0, 0, 2};
    CompareArray(blocks.find(8)->block, a_array_values, 10);
  }

  {
    CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("b"));

    CompareBlock(
        blocks.find(16)->block,
        MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                      ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::Order::Make(3) |
                      ValueBlockFields::NameIndex::Make(5),
                  ArrayBlockPayload::EntryType::Make(BlockType::kUintValue) |
                      ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kExponentialHistogram) |
                      ArrayBlockPayload::Count::Make(10)));
    // Array is:
    // <floor>, <initial_step>, <step_multipler>, <underflow>, <N buckets>..., <overflow>
    uint64_t b_array_values[] = {1, 1, 2, 3, 0, 0, 1, 0, 0, 2};
    CompareArray(blocks.find(16)->block, b_array_values, 10);
  }

  {
    CompareBlock(blocks.find(6)->block, MakeInlinedOrder0StringReferenceBlock("c"));

    CompareBlock(
        blocks.find(24)->block,
        MakeBlock(ValueBlockFields::Type::Make(BlockType::kArrayValue) |
                      ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::Order::Make(3) |
                      ValueBlockFields::NameIndex::Make(6),
                  ArrayBlockPayload::EntryType::Make(BlockType::kDoubleValue) |
                      ArrayBlockPayload::Flags::Make(ArrayBlockFormat::kExponentialHistogram) |
                      ArrayBlockPayload::Count::Make(10)));
    // Array is:
    // <floor>, <initial_step>, <step_multipler>, <underflow>, <N buckets>..., <overflow>
    double c_array_values[] = {1, 1, 2, 3, 0, 0, 1, 0, 0, 2};
    CompareArray(blocks.find(24)->block, c_array_values, 10);
  }
}

TEST(State, FailedAllocationIsNoOp) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  const std::string s(5000, 'a');

  auto p1 = state->CreateStringProperty("", 0, s);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  auto everything_but_header =
      blocks | std::views::filter([](const auto& block) { return block.index != 0; });

  for (const auto& block : everything_but_header) {
    EXPECT_EQ(BlockType::kFree, GetType(block.block));
  }
}

TEST(State, StringPropertyRemainsValidAfterFailedAllocation) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  const std::string s(2000, 'a');

  auto p1 = state->CreateStringProperty("", 0, s);

  {
    fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
    size_t free_blocks, allocated_blocks;
    auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
    ASSERT_TRUE(snapshot);

    const auto ref_idx =
        PropertyBlockPayload::ExtentIndex::Get<BlockIndex>(blocks.find(2)->block->payload.u64);

    ASSERT_EQ(s, TesterLoadStringReference(*state, ref_idx));
  }

  // should fail to allocate due to not being able to get a sufficiently large block,
  // leaving an intact `s`
  p1.Set(std::string(2500, 'b'));

  {
    fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
    size_t free_blocks, allocated_blocks;
    auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
    ASSERT_TRUE(snapshot);

    const auto ref_idx =
        PropertyBlockPayload::ExtentIndex::Get<BlockIndex>(blocks.find(2)->block->payload.u64);

    ASSERT_EQ(s, TesterLoadStringReference(*state, ref_idx));
  }
}

TEST(State, StringPropertiesInternValues) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  const auto p1 = state->CreateStringProperty("a", 0, "b");
  const auto p2 = state->CreateStringProperty("b", 0, "a");

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  ASSERT_EQ(ValueBlockFields::NameIndex::Get<BlockIndex>(blocks.find(2)->block->header),
            PropertyBlockPayload::ExtentIndex::Get<BlockIndex>(blocks.find(5)->block->payload.u64));
  ASSERT_EQ(ValueBlockFields::NameIndex::Get<BlockIndex>(blocks.find(5)->block->header),
            PropertyBlockPayload::ExtentIndex::Get<BlockIndex>(blocks.find(2)->block->payload.u64));
}

TEST(State, CreateSmallProperties) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  std::vector<uint8_t> temp = {'8', '8', '8', '8', '8', '8', '8', '8'};
  StringProperty a = state->CreateStringProperty("a", 0, "abcd");
  ByteVectorProperty b = state->CreateByteVectorProperty("b", 0, temp);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header, 3x STRING_REFERENCE, 1x EXTENT, 2x BUFFER_VALUE
  EXPECT_EQ(7u, allocated_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(4));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  // Property a fits in the first 3 blocks (value, name, extent).
  CompareBlock(blocks.find(2)->block,
               MakeBlock(ValueBlockFields::Type::Make(BlockType::kBufferValue) |
                             ValueBlockFields::NameIndex::Make(3),
                         PropertyBlockPayload::Flags::Make(PropertyBlockFormat::kStringReference) |
                             PropertyBlockPayload::ExtentIndex::Make(4)));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("a"));
  CompareBlock(blocks.find(4)->block, MakeInlinedOrder0StringReferenceBlock("abcd"));

  // Property b fits in the next 3 blocks (value, name, extent).

  CompareBlock(blocks.find(5)->block,
               MakeBlock(ValueBlockFields::Type::Make(BlockType::kBufferValue) |
                             ValueBlockFields::NameIndex::Make(6),
                         PropertyBlockPayload::ExtentIndex::Make(7) |
                             PropertyBlockPayload::TotalLength::Make(8) |
                             PropertyBlockPayload::Flags::Make(PropertyBlockFormat::kBinary)));
  CompareBlock(blocks.find(6)->block, MakeInlinedOrder0StringReferenceBlock("b"));

  CompareBlock(blocks.find(7)->block,
               MakeBlock(ExtentBlockFields::Type::Make(BlockType::kExtent), "88888888"));
}

TEST(State, CreateLargeSingleExtentProperties) {
  auto state = InitState(2 * 4096);  // Need to extend to 2 pages to store both properties.
  ASSERT_TRUE(state != NULL);

  char input[] = "abcdefg";
  size_t input_size = 7;
  std::vector<uint8_t> contents;
  contents.reserve(2040);
  for (int i = 0; i < 2040; i++) {
    contents.push_back(input[i % input_size]);
  }

  ByteVectorProperty b = state->CreateByteVectorProperty("b", 0, contents);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header, 1x STRING_REFERENCE, 1x BUFFER_VALUE, 1x EXTENT
  EXPECT_EQ(4u, allocated_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(2));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  CompareBlock(blocks.find(2)->block,
               MakeBlock(ValueBlockFields::Type::Make(BlockType::kBufferValue) |
                             ValueBlockFields::NameIndex::Make(3),
                         PropertyBlockPayload::ExtentIndex::Make(128) |
                             PropertyBlockPayload::Flags::Make(PropertyBlockFormat::kBinary) |
                             PropertyBlockPayload::TotalLength::Make(2040)));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("b"));
  CompareBlock(blocks.find(128)->block,
               MakeBlock(ExtentBlockFields::Type::Make(BlockType::kExtent) |
                             ExtentBlockFields::Order::Make(kNumOrders - 1),
                         "abcdefga"));
  EXPECT_EQ(0, memcmp(blocks.find(128)->block->payload.data, contents.data(), 2040));
}

TEST(State, CreateMultiExtentProperty) {
  auto state = InitState(2 * 4096);  // Need 4 pages to store 12K of properties.
  ASSERT_TRUE(state != NULL);

  char input[] = "abcdefg";
  size_t input_size = 7;
  std::vector<std::uint8_t> contents;
  for (int i = 0; i < 6000; i++) {
    contents.push_back(input[i % input_size]);
  }
  const auto a = state->CreateByteVectorProperty("a", 0, contents);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header (1), 1 property (2) with 3 extents (3)
  EXPECT_EQ(1u + 2u + 3u, allocated_blocks);
  EXPECT_EQ(5u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(2));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  // Property a has the first 2 blocks for its value and name.
  CompareBlock(blocks.find(2)->block,
               MakeBlock(ValueBlockFields::Type::Make(BlockType::kBufferValue) |
                             ValueBlockFields::NameIndex::Make(3),
                         PropertyBlockPayload::ExtentIndex::Make(128) |
                             PropertyBlockPayload::Flags::Make(PropertyBlockFormat::kBinary) |
                             PropertyBlockPayload::TotalLength::Make(6000)));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("a"));
  // Extents are threaded between blocks 128, 256, and 384.
  CompareBlock(blocks.find(128)->block,
               MakeBlock(ExtentBlockFields::Type::Make(BlockType::kExtent) |
                             ExtentBlockFields::Order::Make(kNumOrders - 1) |
                             ExtentBlockFields::NextExtentIndex::Make(256),
                         "abcdefga"));
  EXPECT_EQ(0, memcmp(blocks.find(128)->block->payload.data, contents.data(), 2040));
  CompareBlock(blocks.find(256)->block,
               MakeBlock(ExtentBlockFields::Type::Make(BlockType::kExtent) |
                             ExtentBlockFields::Order::Make(kNumOrders - 1) |
                             ExtentBlockFields::NextExtentIndex::Make(384),
                         "defgabcd"));
  EXPECT_EQ(0, memcmp(blocks.find(256)->block->payload.data, contents.data() + 2040, 2040));
  CompareBlock(blocks.find(384)->block,
               MakeBlock(ExtentBlockFields::Type::Make(BlockType::kExtent) |
                             ExtentBlockFields::Order::Make(kNumOrders - 1),
                         "gabcdefg"));
  EXPECT_EQ(0, memcmp(blocks.find(384)->block->payload.data, contents.data() + 2 * 2040,
                      6000 - 2 * 2040));
}

TEST(State, SetSmallStringProperty) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  StringProperty a = state->CreateStringProperty("a", 0, "1234");

  a.Set("abcd");

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header, 2x STRING_REFERENCE, 1x BUFFER_VALUE
  EXPECT_EQ(1u + 3u, allocated_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(4));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  // Property a fits in the first 3 blocks (value, name, extent).
  CompareBlock(
      blocks.find(2)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kBufferValue) |
                    ValueBlockFields::NameIndex::Make(3),
                PropertyBlockPayload::ExtentIndex::Make(5) |
                    PropertyBlockPayload::Flags::Make(PropertyBlockFormat::kStringReference)));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("a"));
  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("abcd"));
}

TEST(State, SetSmallBinaryProperty) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  const uint8_t binary[] = {
      'a',
      'b',
      'c',
      'd',
  };
  ByteVectorProperty a = state->CreateByteVectorProperty("a", 0, binary);

  a.Set({'a', 'a', 'a', 'a'});

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header (1), 1 single extent property (3)
  EXPECT_EQ(1u + 3u, allocated_blocks);
  EXPECT_EQ(7u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(4));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  // Property a fits in the first 3 blocks (value, name, extent).
  CompareBlock(blocks.find(2)->block,
               MakeBlock(ValueBlockFields::Type::Make(BlockType::kBufferValue) |
                             ValueBlockFields::NameIndex::Make(3),
                         PropertyBlockPayload::ExtentIndex::Make(4) |
                             PropertyBlockPayload::TotalLength::Make(4) |
                             PropertyBlockPayload::Flags::Make(PropertyBlockFormat::kBinary)));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("a"));

  CompareBlock(blocks.find(4)->block,
               MakeBlock(ValueBlockFields::Type::Make(BlockType::kExtent), "aaaa\0\0\0\0"));
}

TEST(State, SetLargeProperty) {
  auto state = InitState(2 * 4096);  // Need space for 6K of contents.
  ASSERT_TRUE(state != NULL);

  char input[] = "abcdefg";
  size_t input_size = 7;
  std::string contents;
  for (int i = 0; i < 6000; i++) {
    contents.push_back(input[i % input_size]);
  }

  StringProperty a = state->CreateStringProperty("a", 0, contents);

  a.Set("abcd");

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header, BUFFER_VALUE, 2x STRING_REFERENCE
  EXPECT_EQ(4u, allocated_blocks);
  EXPECT_EQ(9u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(4));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  // Property a fits in 3 blocks
  CompareBlock(blocks.find(2)->block,
               MakeBlock(ValueBlockFields::Type::Make(BlockType::kBufferValue) |
                             ValueBlockFields::NameIndex::Make(3),
                         PropertyBlockPayload::Flags::Make(PropertyBlockFormat::kStringReference) |
                             PropertyBlockPayload::ExtentIndex::Make(4)));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("a"));
  CompareBlock(blocks.find(4)->block, MakeInlinedOrder0StringReferenceBlock("abcd"));
}

TEST(State, SetPropertyTruncatedOnOutOfMemory) {
  auto state = InitState(16 * 1024);  // Only 16K of space, property will not fit completely.
  ASSERT_TRUE(state != nullptr);

  std::vector<uint8_t> vec;
  for (int i = 0; i < 65000; i++) {
    vec.push_back('a');
  }

  ByteVectorProperty a = state->CreateByteVectorProperty("a", 0, vec);
  EXPECT_TRUE(bool(a));

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header (1) + Value (1) + Name (1) + 7 extents = 10 allocated blocks.
  EXPECT_EQ(10u, allocated_blocks);
  EXPECT_EQ(5u, free_blocks);
}

TEST(State, CreateNodeHierarchy) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  Node root = state->CreateNode("objs", 0);
  auto req = root.CreateChild("reqs");
  auto network = req.CreateUint("netw", 10);
  auto wifi = req.CreateUint("wifi", 5);

  auto version = root.CreateString("vrsn", "1.0b");

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header (1), root (2), requests (2), 2 metrics (4), small property (3)
  EXPECT_EQ(1u + 2u + 2u + 4u + 3u, allocated_blocks);
  EXPECT_EQ(6u, free_blocks);
  CompareBlock(blocks.find(0)->block, MakeHeader(10));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  // Root object is at index 2.
  // It has 2 references (req and version).
  CompareBlock(
      blocks.find(2)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kNodeValue) |
                    ValueBlockFields::ParentIndex::Make(0) | ValueBlockFields::NameIndex::Make(3),
                2));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("objs"));

  // Requests object is at index 4.
  // It has 2 references (wifi and network).
  CompareBlock(
      blocks.find(4)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kNodeValue) |
                    ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::NameIndex::Make(5),
                2));
  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("reqs"));

  // Network value
  CompareBlock(
      blocks.find(6)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kUintValue) |
                    ValueBlockFields::ParentIndex::Make(4) | ValueBlockFields::NameIndex::Make(7),
                10));
  CompareBlock(blocks.find(7)->block, MakeInlinedOrder0StringReferenceBlock("netw"));

  // Wifi value
  CompareBlock(
      blocks.find(8)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kUintValue) |
                    ValueBlockFields::ParentIndex::Make(4) | ValueBlockFields::NameIndex::Make(9),
                5));
  CompareBlock(blocks.find(9)->block, MakeInlinedOrder0StringReferenceBlock("wifi"));

  // Version property
  CompareBlock(
      blocks.find(10)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kBufferValue) |
                    ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::NameIndex::Make(11),
                PropertyBlockPayload::Flags::Make(PropertyBlockFormat::kStringReference) |
                    PropertyBlockPayload::ExtentIndex::Make(12)));
  CompareBlock(blocks.find(11)->block, MakeInlinedOrder0StringReferenceBlock("vrsn"));

  CompareBlock(blocks.find(12)->block, MakeInlinedOrder0StringReferenceBlock("1.0b"));
}

TEST(State, TombstoneTest) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  std::unique_ptr<Node> requests;
  {
    // Root going out of scope causes a tombstone to be created,
    // but since requests is referencing it it will not be deleted.
    Node root = state->CreateNode("objs", 0);
    requests = std::make_unique<Node>(root.CreateChild("reqs"));
    auto a = root.CreateInt("a", 1);
    auto b = root.CreateUint("b", 1);
    auto c = root.CreateDouble("c", 1);
  }

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header (1), root tombstone (2), requests (2)
  EXPECT_EQ(1u + 2u + 2u, allocated_blocks);
  EXPECT_EQ(6u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(18));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  // Root object is at index 2, but has been tombstoned.
  // It has 1 reference (requests)
  CompareBlock(
      blocks.find(2)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kTombstone) |
                    ValueBlockFields::ParentIndex::Make(0) | ValueBlockFields::NameIndex::Make(3),
                1));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("objs"));
  CompareBlock(
      blocks.find(4)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kNodeValue) |
                ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::NameIndex::Make(5)));
  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("reqs"));
}

TEST(State, TombstoneCleanup) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  IntProperty metric = state->CreateIntProperty("a", 0, 0);

  Node root = state->CreateNode("root", 0);
  {
    Node child1 = state->CreateNode("chi1", 0);
    Node child2 = child1.CreateChild("chi2");

    {
      Node child = child1.CreateChild("chi3");
      std::unique_ptr<IntProperty> m;
      {
        Node new_child = root.CreateChild("chi");
        m = std::make_unique<IntProperty>(new_child.CreateInt("val", -1));
      }
      auto temp = child.CreateString("temp", "test");
      m.reset();
    }
  }

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // 2 each for:
  // metric create
  // root create
  // child1 create
  // child2 create
  // child create
  // new_child
  // m create
  // new_child delete (tombstone)
  // temp create
  // m delete
  // temp delete
  // child delete
  // child2 delete
  // child1 delete
  CompareBlock(blocks.find(0)->block, MakeHeader(14 * 2));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  // Property "a" is at index 2.
  CompareBlock(blocks.find(2)->block,
               MakeIntBlock(ValueBlockFields::Type::Make(BlockType::kIntValue) |
                                ValueBlockFields::ParentIndex::Make(0) |
                                ValueBlockFields::NameIndex::Make(3),
                            0));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("a"));

  // Root object is at index 4.
  // It has 0 references since the children should be removed.
  CompareBlock(
      blocks.find(4)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kNodeValue) |
                ValueBlockFields::ParentIndex::Make(0) | ValueBlockFields::NameIndex::Make(5)));
  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("root"));
}

TEST(State, LinkTest) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  // root will be at block index 2
  Node root = state->CreateNode("root", 0);
  Link link = state->CreateLink("lnk1", 2u /* root index */, "tst1", LinkBlockDisposition::kChild);
  Link link2 =
      state->CreateLink("lnk2", 2u /* root index */, "tst2", LinkBlockDisposition::kInline);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header (1), root (2), link (3), link2 (3)
  EXPECT_EQ(1u + 2u + 3u + 3u, allocated_blocks);
  EXPECT_EQ(6u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(6));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  // Root node has 2 children.
  CompareBlock(
      blocks.find(2)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kNodeValue) |
                    ValueBlockFields::ParentIndex::Make(0) | ValueBlockFields::NameIndex::Make(3),
                2));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("root"));
  CompareBlock(
      blocks.find(4)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kLinkValue) |
                    ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::NameIndex::Make(5),
                LinkBlockPayload::ContentIndex::Make(6)));
  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("lnk1"));
  CompareBlock(blocks.find(6)->block, MakeInlinedOrder0StringReferenceBlock("tst1"));
  CompareBlock(
      blocks.find(7)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kLinkValue) |
                    ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::NameIndex::Make(8),
                LinkBlockPayload::ContentIndex::Make(9) |
                    LinkBlockPayload::Flags::Make(LinkBlockDisposition::kInline)));
  CompareBlock(blocks.find(8)->block, MakeInlinedOrder0StringReferenceBlock("lnk2"));
  CompareBlock(blocks.find(9)->block, MakeInlinedOrder0StringReferenceBlock("tst2"));
}

TEST(State, LinkContentsAllocationFailure) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  // root will be at block index 2
  Node root = state->CreateNode("root", 0);
  std::string name(2000, 'a');
  std::string content(2000, 'b');
  Link link = state->CreateLink(name, 2u /* root index */, content, LinkBlockDisposition::kChild);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header (1), root (2).
  EXPECT_EQ(1u + 2u, allocated_blocks);
  EXPECT_EQ(6u, free_blocks);

  CompareBlock(blocks.find(0)->block, MakeHeader(4));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  // Root node has 0 children.
  CompareBlock(
      blocks.find(2)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kNodeValue) |
                    ValueBlockFields::ParentIndex::Make(0) | ValueBlockFields::NameIndex::Make(3),
                "\0\0\0\0\0\0\0\0"));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("root"));
}

TEST(State, GetStatsTest) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  inspect::InspectStats stats = state->GetStats();
  EXPECT_EQ(0u, stats.dynamic_child_count);
  EXPECT_EQ(4096u, stats.maximum_size);
  EXPECT_EQ(4096u, stats.size);
  EXPECT_EQ(1u, stats.allocated_blocks);
  EXPECT_EQ(0u, stats.deallocated_blocks);
  EXPECT_EQ(0u, stats.failed_allocations);
}

TEST(State, GetStatsWithFailedAllocationTest) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != NULL);

  BlockIndex idx;
  std::string data(5000, '.');
  ASSERT_EQ(ZX_ERR_NO_MEMORY, state->CreateAndIncrementStringReference(data, &idx));

  inspect::InspectStats stats = state->GetStats();
  EXPECT_EQ(0u, stats.dynamic_child_count);
  EXPECT_EQ(4096u, stats.maximum_size);
  EXPECT_EQ(4096u, stats.size);
  EXPECT_EQ(2u, stats.allocated_blocks);
  EXPECT_EQ(1u, stats.deallocated_blocks);
  EXPECT_EQ(1u, stats.failed_allocations);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  auto everything_but_header =
      blocks | std::views::filter([](const auto& block) { return block.index != 0; });

  for (const auto& block : everything_but_header) {
    EXPECT_EQ(BlockType::kFree, GetType(block.block));
  }
}

constexpr size_t kThreadTimes = 1024 * 10;

struct ThreadArgs {
  IntProperty* metric;
  uint64_t value;
  bool add;
};

int ValueThread(void* input) {
  auto* args = reinterpret_cast<ThreadArgs*>(input);
  for (size_t i = 0; i < kThreadTimes; i++) {
    if (args->add) {
      args->metric->Add(args->value);
    } else {
      args->metric->Subtract(args->value);
    }
  }

  return 0;
}

int ChildThread(void* input) {
  Node* object = reinterpret_cast<Node*>(input);
  for (size_t i = 0; i < kThreadTimes; i++) {
    Node child = object->CreateChild("chi");
    auto temp = child.CreateString("temp", "test");
  }
  return 0;
}

TEST(State, MultithreadingTest) {
  auto state = InitState(10 * 4096);
  ASSERT_TRUE(state != NULL);

  size_t per_thread_times_operation_count = 0;
  size_t other_operation_count = 0;

  other_operation_count += 1;  // create a
  IntProperty metric = state->CreateIntProperty("a", 0, 0);

  ThreadArgs adder{.metric = &metric, .value = 2, .add = true};
  ThreadArgs subtractor{.metric = &metric, .value = 1, .add = false};

  thrd_t add_thread, subtract_thread, child_thread_1, child_thread_2;

  other_operation_count += 1;  // create root
  Node root = state->CreateNode("root", 0);
  {
    other_operation_count += 2;  // create and delete
    Node child1 = state->CreateNode("chi1", 0);
    other_operation_count += 2;  // create and delete
    Node child2 = child1.CreateChild("chi2");

    per_thread_times_operation_count += 1;  // add metric
    thrd_create(&add_thread, ValueThread, &adder);

    per_thread_times_operation_count += 1;  // subtract metric
    thrd_create(&subtract_thread, ValueThread, &subtractor);

    per_thread_times_operation_count += 4;  // create child, create temp, delete both
    thrd_create(&child_thread_1, ChildThread, &child1);
    per_thread_times_operation_count += 4;  // create child, create temp, delete both
    thrd_create(&child_thread_2, ChildThread, &child2);

    per_thread_times_operation_count += 4;  // create child, create m, delete both;
    for (size_t i = 0; i < kThreadTimes; i++) {
      Node child = root.CreateChild("chi");
      IntProperty m = child.CreateInt("val", -1);
    }
    thrd_join(add_thread, nullptr);
    thrd_join(subtract_thread, nullptr);
    thrd_join(child_thread_1, nullptr);
    thrd_join(child_thread_2, nullptr);
  }

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  CompareBlock(
      blocks.find(0)->block,
      MakeHeader(kThreadTimes * per_thread_times_operation_count * 2 + other_operation_count * 2));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  // Property "a" is at index 2.
  // Its value should be equal to kThreadTimes since subtraction
  // should cancel out half of addition.
  CompareBlock(blocks.find(2)->block,
               MakeIntBlock(ValueBlockFields::Type::Make(BlockType::kIntValue) |
                                ValueBlockFields::ParentIndex::Make(0) |
                                ValueBlockFields::NameIndex::Make(3),
                            kThreadTimes));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("a"));

  // Root object is at index 4.
  // It has 0 references since the children should be removed.
  CompareBlock(
      blocks.find(4)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kNodeValue) |
                ValueBlockFields::ParentIndex::Make(0) | ValueBlockFields::NameIndex::Make(5)));
  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("root"));
}

TEST(State, OutOfOrderDeletion) {
  // Ensure that deleting properties after their parent does not cause a crash.
  auto state = State::CreateWithSize(4096);
  {
    auto root = state->CreateRootNode();

    inspect::StringProperty a, b, c;
    auto base = root.CreateChild("base");
    c = base.CreateString("c", "test");
    b = base.CreateString("b", "test");
    a = base.CreateString("a", "test");
    ASSERT_TRUE(!!base);
    ASSERT_TRUE(!!c);
    ASSERT_TRUE(!!b);
    ASSERT_TRUE(!!a);
  }
}

TEST(State, CreateNodeHierarchyInTransaction) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != nullptr);

  CheckVmoGenCount(0, state->GetVmo());
  state->BeginTransaction();
  Node root = state->CreateNode("objs", 0);
  auto req = root.CreateChild("reqs");
  auto network = req.CreateUint("netw", 10);
  auto wifi = req.CreateUint("wifi", 5);

  auto version = root.CreateString("vrsn", "1.0b");
  state->EndTransaction();
  CheckVmoGenCount(2, state->GetVmo());

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Header (1), root (2), requests (2), 2 metrics (4), small property (3)
  EXPECT_EQ(1u + 2u + 2u + 4u + 3u, allocated_blocks);
  EXPECT_EQ(6u, free_blocks);
  CompareBlock(blocks.find(0)->block, MakeHeader(2));
  EXPECT_EQ(inspect::internal::GetHeaderVmoSize(blocks.find(0)->block), state->GetStats().size);

  // Root object is at index 2.
  // It has 2 references (req and version).
  CompareBlock(
      blocks.find(2)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kNodeValue) |
                    ValueBlockFields::ParentIndex::Make(0) | ValueBlockFields::NameIndex::Make(3),
                2));
  CompareBlock(blocks.find(3)->block, MakeInlinedOrder0StringReferenceBlock("objs"));

  // Requests object is at index 4.
  // It has 2 references (wifi and network).
  CompareBlock(
      blocks.find(4)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kNodeValue) |
                    ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::NameIndex::Make(5),
                2));
  CompareBlock(blocks.find(5)->block, MakeInlinedOrder0StringReferenceBlock("reqs"));

  // Network value
  CompareBlock(
      blocks.find(6)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kUintValue) |
                    ValueBlockFields::ParentIndex::Make(4) | ValueBlockFields::NameIndex::Make(7),
                10));
  CompareBlock(blocks.find(7)->block, MakeInlinedOrder0StringReferenceBlock("netw"));

  // Wifi value
  CompareBlock(
      blocks.find(8)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kUintValue) |
                    ValueBlockFields::ParentIndex::Make(4) | ValueBlockFields::NameIndex::Make(9),
                5));
  CompareBlock(blocks.find(9)->block, MakeInlinedOrder0StringReferenceBlock("wifi"));

  // Version property
  CompareBlock(
      blocks.find(10)->block,
      MakeBlock(ValueBlockFields::Type::Make(BlockType::kBufferValue) |
                    ValueBlockFields::ParentIndex::Make(2) | ValueBlockFields::NameIndex::Make(11),
                PropertyBlockPayload::Flags::Make(PropertyBlockFormat::kStringReference) |
                    PropertyBlockPayload::ExtentIndex::Make(12)));
  CompareBlock(blocks.find(11)->block, MakeInlinedOrder0StringReferenceBlock("vrsn"));

  CompareBlock(blocks.find(12)->block, MakeInlinedOrder0StringReferenceBlock("1.0b"));
}

TEST(State, ArrayCapacityOffByOne) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != nullptr);

  // Allocate an array of capacity 2
  IntArray a = state->CreateIntArray("a", 0, 2, ArrayBlockFormat::kDefault);
  // Allocate another property.
  IntProperty b = state->CreateIntProperty("b", 0, 0);

  // Take a snapshot and locate the array block and its adjacent block.
  BlockIndex array_idx = 0;
  BlockIndex adjacent_idx = 0;
  {
    fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
    size_t free_blocks, allocated_blocks;
    auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
    ASSERT_TRUE(snapshot);

    for (const auto& pair : blocks) {
      const Block* block = pair.block;
      if (GetType(block) == BlockType::kArrayValue) {
        array_idx = pair.GetKey();
        size_t size_in_indices = inspect::internal::OrderToSize(GetOrder(block)) / 16;
        adjacent_idx = array_idx + size_in_indices;
        break;
      }
    }
  }

  ASSERT_NE(array_idx, 0u);
  ASSERT_NE(adjacent_idx, 0u);

  // Read the adjacent block's content before the write.
  uint8_t buffer_before[16];
  ASSERT_EQ(ZX_OK, state->GetVmo().read(buffer_before, adjacent_idx * 16, 16));

  // try to write at capacity
  a.Set(2, 999);

  // Read the adjacent block's content after the write.
  uint8_t buffer_after[16];
  ASSERT_EQ(ZX_OK, state->GetVmo().read(buffer_after, adjacent_idx * 16, 16));

  // The adjacent block must NOT be modified.
  EXPECT_EQ(0, memcmp(buffer_before, buffer_after, 16));

  state->FreeIntArray(&a);
  state->FreeIntProperty(&b);
}

TEST(State, SetStringArrayAllocationFailure) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != nullptr);

  StringArray d = state->CreateStringArray("d", 0, 2, ArrayBlockFormat::kDefault);

  // Set index 0 to a valid string first.
  d.Set(0, "abc");

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Find index of slot 0. It should be non-zero (since "abc" is allocated).
  auto initial_slot_val = GetArraySlotForString(blocks.find(2)->block, 0);
  ASSERT_TRUE(initial_slot_val.has_value());
  ASSERT_NE(*initial_slot_val, 0u);

  // Attempt to write a string that is too large to fit in the VMO.
  // This allocation will fail.
  std::string large_string(5000, '.');
  d.Set(0, large_string);

  // Re-scan and verify that the slot value was NOT overwritten with a garbage/uninitialized value,
  // and instead remains unchanged (the previous valid string reference).
  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks_after;
  snapshot = SnapshotAndScan(state->GetVmo(), &blocks_after, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  auto post_fail_slot_val = GetArraySlotForString(blocks_after.find(2)->block, 0);
  ASSERT_TRUE(post_fail_slot_val.has_value());
  EXPECT_EQ(*post_fail_slot_val, *initial_slot_val);

  state->FreeStringArray(&d);
}

TEST(StateTest, TransactionRollbackOnPropertyFailure) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != nullptr);

  // Create a parent node
  Node parent = state->CreateNode("parent", 0);
  ASSERT_TRUE(parent);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  // Parent node value index is 2 (header is 0, name index is 1, value index is 2).
  auto parent_index = BlockIndex(2);
  auto* parent_block = blocks.find(parent_index)->block;
  ASSERT_EQ(GetType(parent_block), BlockType::kNodeValue);
  ASSERT_EQ(parent_block->payload.u64, 0u);  // child refcount is 0

  // Let's fill the heap almost completely.
  std::vector<IntProperty> properties;
  int i = 0;
  while (true) {
    auto prop = state->CreateIntProperty("p" + std::to_string(i++), parent_index, 0);
    if (!prop) {
      break;
    }
    properties.push_back(std::move(prop));
  }

  // Now the heap is completely full.
  // Let's free exactly one block of size 32 by deleting the last property.
  ASSERT_TRUE(!properties.empty());
  properties.pop_back();

  // Re-scan to get the current allocated blocks count.
  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks_full;
  snapshot = SnapshotAndScan(state->GetVmo(), &blocks_full, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);
  size_t allocated_count_before_failed_creation = allocated_blocks;

  auto* parent_block_full = blocks_full.find(parent_index)->block;
  uint64_t parent_refcount_before = parent_block_full->payload.u64;

  // Try to create a StringProperty.
  // This requires allocating a value block (size 32) and a name block (size 32).
  // The value block allocation will succeed, but name block will fail.
  // It should roll back completely.
  auto failed_prop = state->CreateStringProperty("string_prop", parent_index, "value");
  ASSERT_FALSE(failed_prop);

  // Verify parent refcount is unchanged.
  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks_after;
  snapshot = SnapshotAndScan(state->GetVmo(), &blocks_after, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  auto* parent_block_after = blocks_after.find(parent_index)->block;
  EXPECT_EQ(parent_block_after->payload.u64, parent_refcount_before);

  // Verify allocated blocks count is unchanged (no leaks).
  EXPECT_EQ(allocated_blocks, allocated_count_before_failed_creation);
}

TEST(StateTest, TransactionPartialSuccessOnPropertyFailure) {
  auto state = InitState(4096);
  ASSERT_TRUE(state != nullptr);

  // Create a parent node
  Node parent = state->CreateNode("parent", 0);
  ASSERT_TRUE(parent);

  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks;
  size_t free_blocks, allocated_blocks;
  auto snapshot = SnapshotAndScan(state->GetVmo(), &blocks, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);

  auto parent_index = BlockIndex(2);

  // We will use the name "p" for all filling properties.
  // First creation allocates the name block (16 bytes) and the value block (16 bytes).
  auto base_prop = state->CreateIntProperty("p", parent_index, 0);
  ASSERT_TRUE(base_prop);

  // Fill the heap completely using the same name.
  // Each of these allocates exactly 1 block of 16 bytes (value block).
  std::vector<IntProperty> fill_props;
  while (true) {
    auto prop = state->CreateIntProperty("p", parent_index, 0);
    if (!prop) {
      break;
    }
    fill_props.push_back(std::move(prop));
  }

  // Now the heap is completely full.
  // We want to free exactly 2080 bytes (one 2048-byte block and two 16-byte blocks).
  // Each pop_back frees 16 bytes.
  // 2080 / 16 = 130 properties.
  ASSERT_GE(fill_props.size(), 130u);
  for (int i = 0; i < 130; i++) {
    fill_props.pop_back();
  }

  // Re-scan to get the current allocated blocks count.
  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks_full;
  snapshot = SnapshotAndScan(state->GetVmo(), &blocks_full, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);
  size_t allocated_count_before = allocated_blocks;

  // Try to create a ByteVectorProperty with a NEW name.
  // This requires:
  // - Value block (16 bytes)
  // - Name block (16 bytes) (new name "n" -> 1 byte -> fits in 16 bytes block)
  // - Extents for value.
  // We will pass a value of 2041 bytes, which requires:
  // - Extent 1: 2048 bytes (holds 2040 bytes)
  // - Extent 2: 16 bytes (holds 1 byte)
  // Total needed: 16 + 16 + 2048 + 16 = 2096 bytes.
  // We only have 2080 bytes free.
  // The allocation of Extent 2 (16 bytes) will fail because we run out of space.
  // It should succeed partially, keeping Extent 1 (2040 bytes of data).
  std::vector<uint8_t> data(2041, 'A');
  auto partial_prop = state->CreateByteVectorProperty("n", parent_index, data);
  ASSERT_TRUE(partial_prop);  // Should succeed!

  // Verify VMO is consistent and we used the expected number of blocks.
  // We had 2080 bytes free (one 2048 block, one 32 block).
  // We allocated:
  // - Value: 16 bytes (from 32 block)
  // - Name: 16 bytes (from 32 block)
  // - Extent 1: 2048 bytes (from 2048 block)
  // Total allocated: 2080 bytes.
  // So all free space should be used, and allocated blocks should increase by 3.
  // (Value, Name, Extent 1).
  fbl::WAVLTree<BlockIndex, std::unique_ptr<ScannedBlock>> blocks_after;
  snapshot = SnapshotAndScan(state->GetVmo(), &blocks_after, &free_blocks, &allocated_blocks);
  ASSERT_TRUE(snapshot);
  EXPECT_EQ(allocated_blocks, allocated_count_before + 3);

  // Verify the property value was truncated to 2040 bytes (capacity of 1 max extent).
  BlockIndex prop_index = 0;
  for (const auto& pair : blocks_after) {
    const Block* block = pair.block;
    if (GetType(block) == BlockType::kBufferValue &&
        ValueBlockFields::ParentIndex::Get<BlockIndex>(block->header) == parent_index) {
      prop_index = pair.GetKey();
      break;
    }
  }
  ASSERT_NE(prop_index, 0u);

  auto* prop_block = blocks_after.find(prop_index)->block;
  ASSERT_EQ(GetType(prop_block), BlockType::kBufferValue);
  uint32_t total_length = PropertyBlockPayload::TotalLength::Get<uint32_t>(prop_block->payload.u64);
  EXPECT_EQ(total_length, 2040u);  // Truncated to 2040 bytes

  // Verify the extent index is not 0.
  BlockIndex extent_index =
      PropertyBlockPayload::ExtentIndex::Get<BlockIndex>(prop_block->payload.u64);
  EXPECT_NE(extent_index, 0u);

  // And the next extent index of that extent should be 0 (no second extent).
  auto* extent_block = blocks_after.find(extent_index)->block;
  ASSERT_EQ(GetType(extent_block), BlockType::kExtent);
  EXPECT_EQ(ExtentBlockFields::NextExtentIndex::Get<BlockIndex>(extent_block->header), 0u);
}

}  // namespace
