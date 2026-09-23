// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fpromise/bridge.h>
#include <lib/fpromise/sequencer.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/vmo/block.h>
#include <lib/inspect/cpp/vmo/limits.h>
#include <lib/inspect/cpp/vmo/state.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/stdcompat/optional.h>
#include <lib/stdcompat/string_view.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <mutex>
#include <string>
#include <utility>
#include <vector>

namespace inspect {
namespace internal {

namespace {

// Freeze_t is a tag type for overload resolution of `AutoGenerationIncrement`.
struct Freeze_t {
} Freeze;

BlockIndex GetParentIndex(const Heap& heap, BlockIndex value_index) {
  const Block* block = heap.GetBlock(value_index);
  ZX_ASSERT(block);
  return ValueBlockFields::ParentIndex::Get<BlockIndex>(block->header);
}

}  // namespace

// Helper class to support RAII locking of the generation count.
class AutoGenerationIncrement final {
 public:
  AutoGenerationIncrement(BlockIndex target, Heap* heap);

  // This version of `AutoGenerationIncrement` puts RAII semantics on freezing a
  // VMO. I.e. it will write kVmoFrozen to the `target_`'s payload at the beginning
  // of existence and put back the original value at the end.
  //
  // This is used over directly writing to the frozen VMO duplicate because
  // we want the duplicate to be read-only.
  AutoGenerationIncrement(Freeze_t, BlockIndex target, Heap* heap);
  ~AutoGenerationIncrement();

  // Disallow copy assign and move.
  AutoGenerationIncrement(AutoGenerationIncrement&&) = delete;
  AutoGenerationIncrement(const AutoGenerationIncrement&) = delete;
  AutoGenerationIncrement& operator=(AutoGenerationIncrement&&) = delete;
  AutoGenerationIncrement& operator=(const AutoGenerationIncrement&) = delete;

 private:
  // Acquire the generation count lock.
  // This consists of atomically incrementing the count using
  // acquire-release ordering, ensuring readers see this increment before
  // any changes to the buffer.
  void Acquire(Block* block);

  // Set the generation count to kVmoFrozen.
  void Acquire(Freeze_t, Block* block);

  // Release the generation count lock.
  // This consists of either a) atomically incrementing the count using release
  // ordering, if the VMO was not frozen, or b) resetting the generation count
  // to last_gen_count_. The memory ordering will ensure readers see this increment
  // after all changes to the buffer are committed.
  void Release(Block* block);

  std::optional<uint64_t> last_gen_count_;
  BlockIndex target_;
  Heap* heap_;
};

AutoGenerationIncrement::AutoGenerationIncrement(BlockIndex target, Heap* heap)
    : target_(target), heap_(heap) {
  Acquire(heap_->GetBlock(target_));
}
AutoGenerationIncrement::~AutoGenerationIncrement() { Release(heap_->GetBlock(target_)); }

AutoGenerationIncrement::AutoGenerationIncrement(Freeze_t, BlockIndex target, Heap* heap)
    : target_(target), heap_(heap) {
  Acquire(Freeze, heap_->GetBlock(target_));
}

void AutoGenerationIncrement::Acquire(Freeze_t, Block* block) {
  uint64_t* ptr = &block->payload.u64;
  last_gen_count_ = block->payload.u64;
  __atomic_store_n(ptr, kVmoFrozen, __ATOMIC_SEQ_CST);
}

void AutoGenerationIncrement::Acquire(Block* block) {
  uint64_t* ptr = &block->payload.u64;
  __atomic_fetch_add(ptr, 1, __ATOMIC_ACQ_REL);
}

void AutoGenerationIncrement::Release(Block* block) {
  uint64_t* ptr = &block->payload.u64;
  if (last_gen_count_.has_value()) {
    __atomic_store_n(ptr, last_gen_count_.value(), __ATOMIC_SEQ_CST);
  } else {
    __atomic_fetch_add(ptr, 1, __ATOMIC_RELEASE);
  }
}

class State::Txn final {
 public:
  explicit Txn(
      State* state,
      std::lock_guard<std::mutex>& /* this is a token to help ensure users have locked State */)
      : state_(state),  // This should be a locked state object, as we make internal modifications
        committed_(false) {}
  // We disable thread safety analysis on Txn methods because Txn operates on state_,
  // whose mutex is held by the caller of Txn. Clang's thread-safety analyzer cannot
  // statically prove that this->mutex_ held in caller scopes is identical to txn.state_->mutex_,
  // which would cause false-positive lock warnings without this annotation.
  ~Txn() __TA_NO_THREAD_SAFETY_ANALYSIS {
    if (!committed_) {
      for (auto it = undos_.rbegin(); it != undos_.rend(); ++it) {
        (*it)();
      }
    }
  }

  Txn(const Txn&) = delete;
  Txn(Txn&&) = delete;
  Txn& operator=(const Txn&) = delete;
  Txn& operator=(Txn&&) = delete;

  void Commit() { committed_ = true; }
  void RegisterUndo(std::function<void()> undo) { undos_.push_back(std::move(undo)); }

  zx_status_t Allocate(size_t size, BlockIndex* out) __TA_NO_THREAD_SAFETY_ANALYSIS {
    zx_status_t status = state_->heap_->Allocate(size, out);
    if (status == ZX_OK) {
      BlockIndex idx = *out;
      undos_.push_back([this, idx]() __TA_NO_THREAD_SAFETY_ANALYSIS { state_->heap_->Free(idx); });
    }
    return status;
  }

  zx_status_t CreateAndIncrementStringReference(std::string_view value, BlockIndex* out,
                                                bool cached) __TA_NO_THREAD_SAFETY_ANALYSIS {
    zx_status_t status = state_->InnerCreateAndIncrementStringReference(value, out, cached);
    if (status == ZX_OK) {
      BlockIndex idx = *out;
      undos_.push_back([this, idx]() __TA_NO_THREAD_SAFETY_ANALYSIS {
        state_->InnerReleaseStringReference(idx);
      });
    }
    return status;
  }

  zx_status_t IncrementParentRefcount(BlockIndex parent_index) __TA_NO_THREAD_SAFETY_ANALYSIS {
    Block* parent = state_->heap_->GetBlock(parent_index);
    ZX_DEBUG_ASSERT_MSG(parent, "Index %lu is invalid", parent_index);
    if (!parent) {
      return ZX_ERR_INVALID_ARGS;
    }
    BlockType parent_type = GetType(parent);
    switch (parent_type) {
      case BlockType::kHeader:
        break;
      case BlockType::kNodeValue:
      case BlockType::kTombstone:
        parent->payload.u64++;
        undos_.push_back([this, parent_index]() __TA_NO_THREAD_SAFETY_ANALYSIS {
          state_->DecrementParentRefcount(parent_index);
        });
        break;
      default:
        ZX_DEBUG_ASSERT_MSG(false, "Invalid parent block type %u for 0x%lx",
                            static_cast<uint32_t>(parent_type), parent_index);
        return ZX_ERR_INVALID_ARGS;
    }
    return ZX_OK;
  }

  std::tuple<BlockIndex, size_t, zx_status_t> CreateExtentChain(const char* value, size_t length)
      __TA_NO_THREAD_SAFETY_ANALYSIS {
    auto [first_extent_index, written, status] = state_->InnerCreateExtentChain(value, length);
    if (status == ZX_OK && first_extent_index != 0) {
      undos_.push_back(
          [this, first_extent_index = first_extent_index]()
              __TA_NO_THREAD_SAFETY_ANALYSIS { state_->InnerFreeExtentChain(first_extent_index); });
    }
    return {first_extent_index, written, status};
  }

 private:
  State* state_;
  bool committed_;
  std::vector<std::function<void()>> undos_;
};

State::Txn State::OpenTransaction(std::lock_guard<std::mutex>& token) {
  return State::Txn(this, token);
}

State::State(std::unique_ptr<Heap> heap, BlockIndex header)
    : heap_(std::move(heap)),
      header_(header),
      next_unique_id_(0),
      next_unique_link_number_(0),
      transaction_count_(0) {}

template <typename WrapperType, BlockType BlockTypeValue>
WrapperType State::InnerCreateArray(std::string_view name, BlockIndex parent, size_t slots,
                                    ArrayBlockFormat format) {
  const auto size_of_payload_type = SizeForArrayPayload(BlockTypeValue);
  if (!size_of_payload_type.has_value()) {
    return WrapperType();
  }

  size_t block_size_needed = slots * size_of_payload_type.value() + kMinOrderSize;
  ZX_DEBUG_ASSERT_MSG(block_size_needed <= kMaxOrderSize,
                      "The requested array size cannot fit in a block");
  if (block_size_needed > kMaxOrderSize) {
    return WrapperType();
  }

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto txn = OpenTransaction(lock);
  BlockIndex name_index, value_index;
  zx_status_t status;
  status = InnerCreateValue(txn, name, BlockType::kArrayValue, parent, &name_index, &value_index,
                            block_size_needed);
  if (status != ZX_OK) {
    return WrapperType();
  }

  auto* block = heap_->GetBlock(value_index);
  block->payload.u64 = ArrayBlockPayload::EntryType::Make(BlockTypeValue) |
                       ArrayBlockPayload::Flags::Make(format) |
                       ArrayBlockPayload::Count::Make(slots);

  txn.Commit();
  return WrapperType(weak_self_ptr_.lock(), name_index, value_index);
}

template <typename ValueType, typename WrapperType, BlockType BlockTypeValue>
void State::InnerSetArray(WrapperType* metric, size_t index_into_array, ValueType value) {
  ZX_ASSERT(metric->state_.get() == this);
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(metric->value_index_);
  ZX_ASSERT(GetType(block) == BlockType::kArrayValue);
  auto entry_type = ArrayBlockPayload::EntryType::Get<BlockType>(block->payload.u64);
  ZX_ASSERT(entry_type == BlockTypeValue);
  if (BlockTypeValue == BlockType::kStringReference) {
    // compile time check that the static_cast used below is legal
    static_assert(BlockTypeValue == BlockType::kStringReference
                      ? std::is_same<ValueType, BlockIndex>::value
                      : true,
                  "Invalid type set in string array");
    auto current_value = GetArraySlotForString(block, index_into_array);
    if (current_value.has_value() && *current_value != kEmptyStringSlotIndex) {
      InnerReleaseStringReference(*current_value);
    }
    // static_cast to get rid of incorrect lint errors
    auto index_of_string_ref_block = static_cast<BlockIndex>(value);
    SetArraySlotForString(block, index_into_array, index_of_string_ref_block);
  } else {
    auto* slot = GetArraySlot<ValueType>(block, index_into_array);
    if (slot != nullptr) {
      *slot = value;
    }
  }
}

template <typename NumericType, typename WrapperType, BlockType BlockTypeValue, typename Operation>
void State::InnerOperationArray(WrapperType* metric, size_t index, NumericType value) {
  ZX_ASSERT(metric->state_.get() == this);
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(metric->value_index_);
  ZX_ASSERT(GetType(block) == BlockType::kArrayValue);
  auto entry_type = ArrayBlockPayload::EntryType::Get<BlockType>(block->payload.u64);
  ZX_ASSERT(entry_type == BlockTypeValue);
  auto* slot = GetArraySlot<NumericType>(block, index);
  if (slot != nullptr) {
    *slot = Operation()(*slot, value);
  }
}

template <typename WrapperType>
void State::InnerFreeArray(WrapperType* value) {
  ZX_DEBUG_ASSERT_MSG(value->state_.get() == this, "Array being freed from the wrong state");
  if (value->state_.get() != this) {
    return;
  }

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(value->value_index_);
  ZX_ASSERT(block);
  DecrementParentRefcount(GetParentIndex(*heap_, value->value_index_));

  InnerReleaseStringReference(value->name_index_);
  if (ArrayBlockPayload::EntryType::Get<BlockType>(block->payload.u64) ==
      BlockType::kStringReference) {
    // free/decrease ref count of string references
    for (size_t i = 0; i < ArrayBlockPayload::Count::Get<size_t>(block->payload.u64); i++) {
      const auto string_index = GetArraySlotForString(block, i);
      if (!string_index.has_value()) {
        continue;
      }

      InnerReleaseStringReference(string_index.value());
      SetArraySlotForString(block, i, kEmptyStringSlotIndex);
    }
  }

  heap_->Free(value->value_index_);
  value->state_ = nullptr;
}

std::shared_ptr<State> State::Create(std::unique_ptr<Heap> heap) {
  BlockIndex header;
  if (heap->Allocate(OrderToSize(kVmoHeaderOrder), &header) != ZX_OK) {
    return nullptr;
  }

  ZX_DEBUG_ASSERT_MSG(header == 0, "Header must be at index 0");
  if (header != 0) {
    return nullptr;
  }

  auto* block = heap->GetBlock(header);
  block->header = HeaderBlockFields::Order::Make(GetOrder(block)) |
                  HeaderBlockFields::Type::Make(BlockType::kHeader) |
                  HeaderBlockFields::Version::Make(kVersion);
  memcpy(&block->header_data[4], kMagicNumber, 4);
  block->payload.u64 = 0;
  SetHeaderVmoSize(block, heap->size());
  heap->SetHeaderBlock(block);

  std::shared_ptr<State> ret(new State(std::move(heap), header));
  ret->weak_self_ptr_ = ret;
  return ret;
}

std::shared_ptr<State> State::CreateWithSize(size_t size) {
  zx::vmo vmo;
  if (size == 0 || ZX_OK != zx::vmo::create(size, 0, &vmo)) {
    return nullptr;
  }
  static const char kName[] = "InspectHeap";
  vmo.set_property(ZX_PROP_NAME, kName, strlen(kName));
  return State::Create(std::make_unique<Heap>(std::move(vmo)));
}

State::~State() { heap_->Free(header_); }

const zx::vmo& State::GetVmo() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return heap_->GetVmo();
}

bool State::DuplicateVmo(zx::vmo* vmo) const {
  std::lock_guard<std::mutex> lock(mutex_);
  return ZX_OK == heap_->GetVmo().duplicate(
                      ZX_RIGHTS_BASIC | ZX_RIGHT_READ | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY, vmo);
}

std::optional<zx::vmo> State::FrozenVmoCopy() const {
  std::lock_guard<std::mutex> lock(mutex_);

  if (transaction_count_ > 0) {
    return {};
  }

  std::unique_ptr<AutoGenerationIncrement> gen = MaybeFreezeAndIncrementGeneration();

  uint64_t size;
  heap_->GetVmo().get_size(&size);
  zx::vmo vmo;
  if (heap_->GetVmo().create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_NO_WRITE, 0, size, &vmo) !=
      ZX_OK) {
    return {};
  }

  return {std::move(vmo)};
}

bool State::Copy(zx::vmo* vmo) const {
  std::lock_guard<std::mutex> lock(mutex_);

  if (transaction_count_ > 0) {
    return false;
  }

  size_t size = heap_->size();
  if (zx::vmo::create(size, 0, vmo) != ZX_OK) {
    return false;
  }

  if (vmo->write(heap_->data(), 0, size) != ZX_OK) {
    return false;
  }

  return true;
}

bool State::CopyBytes(std::vector<uint8_t>* out) const {
  std::lock_guard<std::mutex> lock(mutex_);

  if (transaction_count_ > 0) {
    return false;
  }

  size_t size = heap_->size();
  if (size == 0) {
    return false;
  }

  out->resize(size);
  memcpy(out->data(), heap_->data(), size);

  return true;
}

IntProperty State::CreateIntProperty(std::string_view name, BlockIndex parent, int64_t value) {
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto txn = OpenTransaction(lock);
  BlockIndex name_index, value_index;
  zx_status_t status;
  status = InnerCreateValue(txn, name, BlockType::kIntValue, parent, &name_index, &value_index);
  if (status != ZX_OK) {
    return IntProperty();
  }

  auto* block = heap_->GetBlock(value_index);
  block->payload.i64 = value;

  txn.Commit();
  return IntProperty(weak_self_ptr_.lock(), name_index, value_index);
}

UintProperty State::CreateUintProperty(std::string_view name, BlockIndex parent, uint64_t value) {
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto txn = OpenTransaction(lock);
  BlockIndex name_index, value_index;
  zx_status_t status;
  status = InnerCreateValue(txn, name, BlockType::kUintValue, parent, &name_index, &value_index);
  if (status != ZX_OK) {
    return UintProperty();
  }

  auto* block = heap_->GetBlock(value_index);
  block->payload.u64 = value;

  txn.Commit();
  return UintProperty(weak_self_ptr_.lock(), name_index, value_index);
}

DoubleProperty State::CreateDoubleProperty(std::string_view name, BlockIndex parent, double value) {
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto txn = OpenTransaction(lock);
  BlockIndex name_index, value_index;
  zx_status_t status;
  status = InnerCreateValue(txn, name, BlockType::kDoubleValue, parent, &name_index, &value_index);
  if (status != ZX_OK) {
    return DoubleProperty();
  }

  auto* block = heap_->GetBlock(value_index);
  block->payload.f64 = value;

  txn.Commit();
  return DoubleProperty(weak_self_ptr_.lock(), name_index, value_index);
}

BoolProperty State::CreateBoolProperty(std::string_view name, BlockIndex parent, bool value) {
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto txn = OpenTransaction(lock);
  BlockIndex name_index, value_index;
  zx_status_t status;
  status = InnerCreateValue(txn, name, BlockType::kBoolValue, parent, &name_index, &value_index);
  if (status != ZX_OK) {
    return BoolProperty();
  }

  auto* block = heap_->GetBlock(value_index);
  block->payload.u64 = value;
  txn.Commit();
  return BoolProperty(weak_self_ptr_.lock(), name_index, value_index);
}

IntArray State::CreateIntArray(std::string_view name, BlockIndex parent, size_t slots,
                               ArrayBlockFormat format) {
  return InnerCreateArray<IntArray, BlockType::kIntValue>(name, parent, slots, format);
}

UintArray State::CreateUintArray(std::string_view name, BlockIndex parent, size_t slots,
                                 ArrayBlockFormat format) {
  return InnerCreateArray<UintArray, BlockType::kUintValue>(name, parent, slots, format);
}

DoubleArray State::CreateDoubleArray(std::string_view name, BlockIndex parent, size_t slots,
                                     ArrayBlockFormat format) {
  return InnerCreateArray<DoubleArray, BlockType::kDoubleValue>(name, parent, slots, format);
}

StringArray State::CreateStringArray(std::string_view name, BlockIndex parent, size_t slots,
                                     ArrayBlockFormat format) {
  return InnerCreateArray<StringArray, BlockType::kStringReference>(name, parent, slots, format);
}

template <typename WrapperType, typename ValueType>
WrapperType State::InnerCreateProperty(std::string_view name, BlockIndex parent, const char* value,
                                       size_t length, PropertyBlockFormat format) {
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto txn = OpenTransaction(lock);
  BlockIndex name_index, value_index;
  zx_status_t status =
      InnerCreateValue(txn, name, BlockType::kBufferValue, parent, &name_index, &value_index);
  if (status != ZX_OK) {
    return WrapperType();
  }

  auto [first_extent_index, written, extent_status] = txn.CreateExtentChain(value, length);

  auto* block = heap_->GetBlock(value_index);
  block->payload.u64 = PropertyBlockPayload::TotalLength::Make(written) |
                       PropertyBlockPayload::ExtentIndex::Make(first_extent_index) |
                       PropertyBlockPayload::Flags::Make(format);

  if (extent_status != ZX_OK) {
    return WrapperType();
  }

  txn.Commit();
  return WrapperType(weak_self_ptr_.lock(), name_index, value_index);
}

StringProperty State::CreateStringProperty(std::string_view name, BlockIndex parent,
                                           const std::string& value) {
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto txn = OpenTransaction(lock);
  BlockIndex name_index, value_index;
  zx_status_t status =
      InnerCreateValue(txn, name, BlockType::kBufferValue, parent, &name_index, &value_index);
  if (status != ZX_OK) {
    return StringProperty();
  }

  BlockIndex data_index;
  status = txn.CreateAndIncrementStringReference(value, &data_index, true);
  if (status != ZX_OK) {
    return StringProperty();
  }

  heap_->GetBlock(value_index)->payload.u64 =
      PropertyBlockPayload::ExtentIndex::Make(data_index) |
      PropertyBlockPayload::TotalLength::Make(0) |
      PropertyBlockPayload::Flags::Make(PropertyBlockFormat::kStringReference);

  txn.Commit();
  return StringProperty(weak_self_ptr_.lock(), name_index, value_index);
}

ByteVectorProperty State::CreateByteVectorProperty(std::string_view name, BlockIndex parent,
                                                   cpp20::span<const uint8_t> value) {
  return InnerCreateProperty<ByteVectorProperty, cpp20::span<const uint8_t>>(
      name, parent, reinterpret_cast<const char*>(value.data()), value.size(),
      PropertyBlockFormat::kBinary);
}

Link State::CreateLink(std::string_view name, BlockIndex parent, std::string_view content,
                       LinkBlockDisposition disposition) {
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto txn = OpenTransaction(lock);
  BlockIndex name_index, value_index, content_index;
  zx_status_t status;
  status = InnerCreateValue(txn, name, BlockType::kLinkValue, parent, &name_index, &value_index);
  if (status != ZX_OK) {
    return Link();
  }

  // `content` is always unique (passed through UniqueLinkName), so caching is unneeded
  status = txn.CreateAndIncrementStringReference(content, &content_index, false);
  if (status != ZX_OK || content_index > 0xFFFFF) {
    return Link();
  }

  auto* block = heap_->GetBlock(value_index);

  block->payload.u64 = LinkBlockPayload::ContentIndex::Make(content_index) |
                       LinkBlockPayload::Flags::Make(disposition);

  txn.Commit();
  return Link(weak_self_ptr_.lock(), name_index, value_index, content_index);
}

Node State::CreateRootNode() {
  std::lock_guard<std::mutex> lock(mutex_);
  return Node(weak_self_ptr_.lock(), 0, 0);
}

LazyNode State::InnerCreateLazyLink(std::string_view name, BlockIndex parent,
                                    LazyNodeCallbackFn callback, LinkBlockDisposition disposition) {
  std::string content = UniqueLinkName(name);
  auto link = CreateLink(name, parent, content, disposition);

  {
    std::lock_guard<std::mutex> lock(mutex_);

    link_callbacks_.emplace(content, LazyNodeCallbackHolder(std::move(callback)));

    return LazyNode(weak_self_ptr_.lock(), std::move(content), std::move(link));
  }
}

LazyNode State::CreateLazyNode(std::string_view name, BlockIndex parent,
                               LazyNodeCallbackFn callback) {
  return InnerCreateLazyLink(name, parent, std::move(callback), LinkBlockDisposition::kChild);
}

LazyNode State::CreateLazyValues(std::string_view name, BlockIndex parent,
                                 LazyNodeCallbackFn callback) {
  return InnerCreateLazyLink(name, parent, std::move(callback), LinkBlockDisposition::kInline);
}

Node State::CreateNode(std::string_view name, BlockIndex parent) {
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto txn = OpenTransaction(lock);
  BlockIndex name_index, value_index;
  zx_status_t status;
  status = InnerCreateValue(txn, name, BlockType::kNodeValue, parent, &name_index, &value_index);
  if (status != ZX_OK) {
    return Node();
  }

  txn.Commit();
  return Node(weak_self_ptr_.lock(), name_index, value_index);
}

void State::SetIntProperty(IntProperty* metric, int64_t value) {
  ZX_ASSERT(metric->state_.get() == this);
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(metric->value_index_);
  ZX_DEBUG_ASSERT_MSG(GetType(block) == BlockType::kIntValue, "Expected int metric, got %d",
                      static_cast<int>(GetType(block)));
  block->payload.i64 = value;
}

void State::SetUintProperty(UintProperty* metric, uint64_t value) {
  ZX_ASSERT(metric->state_.get() == this);

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(metric->value_index_);
  ZX_DEBUG_ASSERT_MSG(GetType(block) == BlockType::kUintValue, "Expected uint metric, got %d",
                      static_cast<int>(GetType(block)));
  block->payload.u64 = value;
}

void State::SetDoubleProperty(DoubleProperty* metric, double value) {
  ZX_ASSERT(metric->state_.get() == this);

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(metric->value_index_);
  ZX_DEBUG_ASSERT_MSG(GetType(block) == BlockType::kDoubleValue, "Expected double metric, got %d",
                      static_cast<int>(GetType(block)));
  block->payload.f64 = value;
}

void State::SetBoolProperty(BoolProperty* metric, bool value) {
  ZX_ASSERT(metric->state_.get() == this);
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(metric->value_index_);
  ZX_DEBUG_ASSERT_MSG(GetType(block) == BlockType::kBoolValue, "Expected bool metric, got %d",
                      static_cast<int>(GetType(block)));
  block->payload.u64 = value;
}

void State::SetIntArray(IntArray* array, size_t index, int64_t value) {
  InnerSetArray<int64_t, IntArray, BlockType::kIntValue>(array, index, value);
}

void State::SetUintArray(UintArray* array, size_t index, uint64_t value) {
  InnerSetArray<uint64_t, UintArray, BlockType::kUintValue>(array, index, value);
}

void State::SetDoubleArray(DoubleArray* array, size_t index, double value) {
  InnerSetArray<double, DoubleArray, BlockType::kDoubleValue>(array, index, value);
}

void State::SetStringArray(StringArray* array, size_t index, std::string_view value) {
  BlockIndex value_index;
  if (CreateAndIncrementStringReference(value, &value_index) != ZX_OK) {
    return;
  }
  InnerSetArray<BlockIndex, StringArray, BlockType::kStringReference>(array, index, value_index);
}

void State::AddIntProperty(IntProperty* metric, int64_t value) {
  ZX_ASSERT(metric->state_.get() == this);

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(metric->value_index_);
  ZX_DEBUG_ASSERT_MSG(GetType(block) == BlockType::kIntValue, "Expected int metric, got %d",
                      static_cast<int>(GetType(block)));
  block->payload.i64 += value;
}

void State::AddUintProperty(UintProperty* metric, uint64_t value) {
  ZX_ASSERT(metric->state_.get() == this);

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(metric->value_index_);
  ZX_DEBUG_ASSERT_MSG(GetType(block) == BlockType::kUintValue, "Expected uint metric, got %d",
                      static_cast<int>(GetType(block)));
  block->payload.u64 += value;
}

void State::AddDoubleProperty(DoubleProperty* metric, double value) {
  ZX_ASSERT(metric->state_.get() == this);

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(metric->value_index_);
  ZX_DEBUG_ASSERT_MSG(GetType(block) == BlockType::kDoubleValue, "Expected double metric, got %d",
                      static_cast<int>(GetType(block)));
  block->payload.f64 += value;
}

void State::SubtractIntProperty(IntProperty* metric, int64_t value) {
  ZX_ASSERT(metric->state_.get() == this);

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(metric->value_index_);
  ZX_DEBUG_ASSERT_MSG(GetType(block) == BlockType::kIntValue, "Expected int metric, got %d",
                      static_cast<int>(GetType(block)));
  block->payload.i64 -= value;
}

void State::SubtractUintProperty(UintProperty* metric, uint64_t value) {
  ZX_ASSERT(metric->state_.get() == this);

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(metric->value_index_);
  ZX_DEBUG_ASSERT_MSG(GetType(block) == BlockType::kUintValue, "Expected uint metric, got %d",
                      static_cast<int>(GetType(block)));
  block->payload.u64 -= value;
}

void State::SubtractDoubleProperty(DoubleProperty* metric, double value) {
  ZX_ASSERT(metric->state_.get() == this);

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(metric->value_index_);
  ZX_DEBUG_ASSERT_MSG(GetType(block) == BlockType::kDoubleValue, "Expected double metric, got %d",
                      static_cast<int>(GetType(block)));
  block->payload.f64 -= value;
}

void State::AddIntArray(IntArray* array, size_t index, int64_t value) {
  InnerOperationArray<int64_t, IntArray, BlockType::kIntValue, std::plus<int64_t>>(array, index,
                                                                                   value);
}

void State::SubtractIntArray(IntArray* array, size_t index, int64_t value) {
  InnerOperationArray<int64_t, IntArray, BlockType::kIntValue, std::minus<int64_t>>(array, index,
                                                                                    value);
}

void State::AddUintArray(UintArray* array, size_t index, uint64_t value) {
  InnerOperationArray<uint64_t, UintArray, BlockType::kUintValue, std::plus<uint64_t>>(array, index,
                                                                                       value);
}

void State::SubtractUintArray(UintArray* array, size_t index, uint64_t value) {
  InnerOperationArray<uint64_t, UintArray, BlockType::kUintValue, std::minus<uint64_t>>(
      array, index, value);
}

void State::AddDoubleArray(DoubleArray* array, size_t index, double value) {
  InnerOperationArray<double, DoubleArray, BlockType::kDoubleValue, std::plus<double>>(array, index,
                                                                                       value);
}

void State::SubtractDoubleArray(DoubleArray* array, size_t index, double value) {
  InnerOperationArray<double, DoubleArray, BlockType::kDoubleValue, std::minus<double>>(
      array, index, value);
}

template <typename WrapperType>
void State::InnerSetBytesProperty(WrapperType* property, const char* value, size_t length) {
  auto* block = heap_->GetBlock(property->value_index_);
  InnerFreeExtentChain(PropertyBlockPayload::ExtentIndex::Get<BlockIndex>(block->payload.u64));

  auto [first_extent_index, written, status] = InnerCreateExtentChain(value, length);

  const auto length_maybe_zeroed = status == ZX_OK ? written : 0;

  block->payload.u64 = PropertyBlockPayload::TotalLength::Make(length_maybe_zeroed) |
                       PropertyBlockPayload::ExtentIndex::Make(first_extent_index) |
                       PropertyBlockPayload::Flags::Make(
                           PropertyBlockPayload::Flags::Get<uint8_t>(block->payload.u64));
}

void State::InnerSetStringProperty(StringProperty* property, const std::string& value) {
  auto* property_block = heap_->GetBlock(property->value_index_);
  const auto old_string_ref_idx =
      PropertyBlockPayload::ExtentIndex::Get<BlockIndex>(property_block->payload.u64);

  BlockIndex new_string_ref_idx;
  const auto status = InnerCreateAndIncrementStringReference(value, &new_string_ref_idx, true);

  if (status != ZX_OK) {
    return;
  }

  InnerReleaseStringReference(old_string_ref_idx);

  property_block->payload.u64 =
      PropertyBlockPayload::ExtentIndex::Make(new_string_ref_idx) |
      PropertyBlockPayload::Flags::Make(
          PropertyBlockPayload::Flags::Get<uint8_t>(property_block->payload.u64));
}

void State::SetStringProperty(StringProperty* property, const std::string& value) {
  ZX_ASSERT(property->state_.get() == this);

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  InnerSetStringProperty(property, value);
}

void State::SetByteVectorProperty(ByteVectorProperty* property, cpp20::span<const uint8_t> value) {
  ZX_ASSERT(property->state_.get() == this);

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  InnerSetBytesProperty(property, reinterpret_cast<const char*>(value.data()), value.size());
}

void State::DecrementParentRefcount(BlockIndex parent_index) {
  Block* parent;
  while ((parent = heap_->GetBlock(parent_index)) != nullptr) {
    switch (GetType(parent)) {
      case BlockType::kHeader:
        return;
      case BlockType::kNodeValue:
        ZX_ASSERT(parent->payload.u64 != 0);
        --parent->payload.u64;
        return;
      case BlockType::kTombstone:
        ZX_ASSERT(parent->payload.u64 != 0);
        if (--parent->payload.u64 == 0) {
          BlockIndex next_parent_index =
              ValueBlockFields::ParentIndex::Get<BlockIndex>(parent->header);
          InnerReleaseStringReference(ValueBlockFields::NameIndex::Get<BlockIndex>(parent->header));
          heap_->Free(parent_index);
          parent_index = next_parent_index;
          break;
        }
        return;
      default:
        ZX_DEBUG_ASSERT_MSG(false, "Invalid parent type %u",
                            static_cast<uint32_t>(GetType(parent)));
        return;
    }
  }
}

std::unique_ptr<AutoGenerationIncrement> State::MaybeIncrementGeneration() {
  if (transaction_count_ > 0) {
    return nullptr;
  }
  return std::make_unique<AutoGenerationIncrement>(header_, heap_.get());
}

std::unique_ptr<AutoGenerationIncrement> State::MaybeFreezeAndIncrementGeneration() const {
  if (transaction_count_ > 0) {
    return nullptr;
  }
  return std::make_unique<AutoGenerationIncrement>(Freeze, header_, heap_.get());
}

void State::FreeIntProperty(IntProperty* metric) {
  ZX_DEBUG_ASSERT_MSG(metric->state_.get() == this, "Property being freed from the wrong state");
  if (metric->state_.get() != this) {
    return;
  }

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  DecrementParentRefcount(GetParentIndex(*heap_, metric->value_index_));

  InnerReleaseStringReference(metric->name_index_);
  heap_->Free(metric->value_index_);
  metric->state_ = nullptr;
}

void State::FreeUintProperty(UintProperty* metric) {
  ZX_DEBUG_ASSERT_MSG(metric->state_.get() == this, "Property being freed from the wrong state");
  if (metric->state_.get() != this) {
    return;
  }

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  DecrementParentRefcount(GetParentIndex(*heap_, metric->value_index_));

  InnerReleaseStringReference(metric->name_index_);
  heap_->Free(metric->value_index_);
  metric->state_ = nullptr;
}

void State::FreeDoubleProperty(DoubleProperty* metric) {
  ZX_DEBUG_ASSERT_MSG(metric->state_.get() == this, "Property being freed from the wrong state");
  if (metric->state_.get() != this) {
    return;
  }

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  DecrementParentRefcount(GetParentIndex(*heap_, metric->value_index_));

  InnerReleaseStringReference(metric->name_index_);
  heap_->Free(metric->value_index_);
  metric->state_ = nullptr;
}

void State::FreeBoolProperty(BoolProperty* metric) {
  ZX_DEBUG_ASSERT_MSG(metric->state_.get() == this, "Property being freed from wrong state");
  if (metric->state_.get() != this) {
    return;
  }

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  DecrementParentRefcount(GetParentIndex(*heap_, metric->value_index_));

  InnerReleaseStringReference(metric->name_index_);
  heap_->Free(metric->value_index_);
  metric->state_ = nullptr;
}

void State::FreeIntArray(IntArray* array) { InnerFreeArray<IntArray>(array); }

void State::FreeUintArray(UintArray* array) { InnerFreeArray<UintArray>(array); }

void State::FreeDoubleArray(DoubleArray* array) { InnerFreeArray<DoubleArray>(array); }

void State::FreeStringArray(StringArray* array) { InnerFreeArray<StringArray>(array); }

template <typename WrapperType>
void State::InnerFreePropertyWithExtents(WrapperType* property) {
  ZX_DEBUG_ASSERT_MSG(property->state_.get() == this, "Property being freed from the wrong state");
  if (property->state_.get() != this) {
    return;
  }

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  DecrementParentRefcount(GetParentIndex(*heap_, property->value_index_));

  const auto* block = heap_->GetBlock(property->value_index_);

  switch (PropertyBlockPayload::Flags::Get<PropertyBlockFormat>(block->payload.u64)) {
    case PropertyBlockFormat::kBinary:
    case PropertyBlockFormat::kUtf8:
      InnerFreeExtentChain(PropertyBlockPayload::ExtentIndex::Get<BlockIndex>(block->payload.u64));
      break;
    case PropertyBlockFormat::kStringReference:
      InnerReleaseStringReference(
          PropertyBlockPayload::ExtentIndex::Get<BlockIndex>(block->payload.u64));
      break;
  }

  InnerReleaseStringReference(property->name_index_);
  heap_->Free(property->value_index_);
  property->state_ = nullptr;
}

void State::FreeStringProperty(StringProperty* property) { InnerFreePropertyWithExtents(property); }

void State::FreeByteVectorProperty(ByteVectorProperty* property) {
  InnerFreePropertyWithExtents(property);
}

void State::FreeLink(Link* link) {
  ZX_DEBUG_ASSERT_MSG(link->state_.get() == this, "Link being freed from the wrong state");
  if (link->state_.get() != this) {
    return;
  }

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  DecrementParentRefcount(GetParentIndex(*heap_, link->value_index_));

  InnerReleaseStringReference(link->name_index_);
  heap_->Free(link->value_index_);
  InnerReleaseStringReference(link->content_index_);
  link->state_ = nullptr;
}

void State::FreeNode(Node* object) {
  ZX_DEBUG_ASSERT_MSG(object->state_.get() == this, "Node being freed from the wrong state");
  if (object->state_.get() != this) {
    return;
  }

  if (object->value_index_ == 0) {
    // This is a special "root" node, it cannot be deleted.
    return;
  }

  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();

  auto* block = heap_->GetBlock(object->value_index_);
  if (block) {
    if (block->payload.u64 == 0) {
      // Actually free the block, decrementing parent refcounts.
      DecrementParentRefcount(ValueBlockFields::ParentIndex::Get<BlockIndex>(block->header));
      // Node has no refs, free it.
      InnerReleaseStringReference(object->name_index_);
      heap_->Free(object->value_index_);
    } else {
      // Node has refs, change type to tombstone so it can be removed
      // when the last ref is gone.
      ValueBlockFields::Type::Set(&block->header, static_cast<uint64_t>(BlockType::kTombstone));
    }
  }
}

void State::FreeLazyNode(LazyNode* object) {
  ZX_DEBUG_ASSERT_MSG(object->state_.get() == this, "Node being freed from the wrong state");
  if (object->state_.get() != this) {
    return;
  }

  // Free the contained link, which removes the reference to the value in the map.
  FreeLink(&object->link_);

  LazyNodeCallbackHolder holder;

  {
    // Separately lock the current state, and remove the callback for this lazy node.
    std::lock_guard<std::mutex> lock(mutex_);
    auto it = link_callbacks_.find(object->content_value_);
    if (it != link_callbacks_.end()) {
      holder = it->second;
      link_callbacks_.erase(it);
    }
    object->state_ = nullptr;
  }

  // Cancel the Holder without State locked. This avoids a deadlock in which we could be locking
  // the holder with the state lock held, meanwhile the callback itself is modifying state (with
  // holder locked).
  //
  // At this point in time, the LazyNode is still *live* and the callback may be getting executed.
  // Following this cancel call, the LazyNode is no longer live and the callback will never be
  // called again.
  holder.cancel();
}

void State::ReleaseStringReference(const BlockIndex index) {
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();
  InnerReleaseStringReference(index);
}

void State::InnerReleaseStringReference(const BlockIndex index) {
  auto* const block = heap_->GetBlock(index);
  if (BlockFields::Type::Get<BlockType>(block->header) != BlockType::kStringReference) {
    return;
  }

  const auto reference_count =
      StringReferenceBlockFields::ReferenceCount::Get<uint64_t>(block->header);
  if (reference_count < StringReferenceBlockFields::ReferenceCount::kMask) {
    StringReferenceBlockFields::ReferenceCount::Set(&block->header,
                                                    reference_count > 0 ? reference_count - 1 : 0);
  }
  InnerMaybeFreeStringReference(index, block);
}

void State::InnerMaybeFreeStringReference(BlockIndex index, Block* block) {
  const auto reference_count =
      StringReferenceBlockFields::ReferenceCount::Get<uint64_t>(block->header);
  if (reference_count != 0) {
    return;
  }

  // If a reference ID is used again, it will just be re-allocated to the VMO.
  // Additionally, though the index might not have been mapped to a state ID,
  // failing to erase isn't an error.
  for (auto it = std::begin(string_reference_ids_); it != std::cend(string_reference_ids_); it++) {
    if (it->second == index) {
      string_reference_ids_.erase(it);
      break;
    }
  }

  const auto first_extent_index =
      StringReferenceBlockFields::NextExtentIndex::Get<BlockIndex>(block->header);
  heap_->Free(index);
  InnerFreeExtentChain(first_extent_index);
}

void State::InnerReadExtents(BlockIndex head_extent, size_t remaining_length,
                             std::vector<uint8_t>* buf) const {
  auto* extent = heap_->GetBlock(head_extent);
  while (remaining_length > 0) {
    if (!extent || GetType(extent) != BlockType::kExtent) {
      break;
    }
    size_t len = std::min(remaining_length, PayloadCapacity(GetOrder(extent)));
    buf->insert(buf->cend(), extent->payload_ptr(), extent->payload_ptr() + len);
    remaining_length -= len;

    BlockIndex next_extent = ExtentBlockFields::NextExtentIndex::Get<BlockIndex>(extent->header);

    extent = heap_->GetBlock(next_extent);
  }
}

void State::BeginTransaction() {
  std::lock_guard<std::mutex> lock(mutex_);
  if (transaction_count_ == 0) {
    transaction_gen_ = std::make_unique<AutoGenerationIncrement>(header_, heap_.get());
  }
  transaction_count_++;
}

void State::EndTransaction() {
  std::lock_guard<std::mutex> lock(mutex_);
  transaction_count_--;
  if (transaction_count_ == 0) {
    transaction_gen_.reset();
  }
}

std::vector<std::string> State::GetLinkNames() const {
  std::lock_guard<std::mutex> lock(mutex_);
  std::vector<std::string> ret;
  for (const auto& entry : link_callbacks_) {
    ret.push_back(entry.first);
  }
  return ret;
}

fpromise::promise<Inspector> State::CallLinkCallback(const std::string& name) {
  LazyNodeCallbackHolder holder;

  {
    std::lock_guard<std::mutex> lock(mutex_);
    auto it = link_callbacks_.find(name);
    if (it == link_callbacks_.end()) {
      return fpromise::make_result_promise<Inspector>(fpromise::error());
    }
    // Copy out the holder.
    holder = it->second;
  }

  // Call the callback.
  // This occurs without state locked, but deletion of the LazyNode synchronizes on the internal
  // mutex in the Holder. If the LazyNode is deleted before this call, the callback will not be
  // executed. If the LazyNode is being deleted concurrent with this call, it will be delayed
  // until after the callback returns.
  return holder.call();
}

zx_status_t State::InnerCreateValue(Txn& txn, std::string_view name, BlockType type,
                                    BlockIndex parent_index, BlockIndex* out_name,
                                    BlockIndex* out_value, size_t min_size_required) {
  BlockIndex value_index, name_index;
  zx_status_t status;
  status = txn.Allocate(min_size_required, &value_index);
  if (status != ZX_OK) {
    return status;
  }

  status = txn.CreateAndIncrementStringReference(name, &name_index, true);
  if (status != ZX_OK) {
    return status;
  }

  auto* block = heap_->GetBlock(value_index);
  block->header = ValueBlockFields::Order::Make(GetOrder(block)) |
                  ValueBlockFields::Type::Make(type) |
                  ValueBlockFields::ParentIndex::Make(parent_index) |
                  ValueBlockFields::NameIndex::Make(name_index);
  memset(&block->payload, 0, min_size_required - sizeof(block->header));

  status = txn.IncrementParentRefcount(parent_index);
  if (status != ZX_OK) {
    return status;
  }

  *out_name = name_index;
  *out_value = value_index;
  return ZX_OK;
}

// This function accepts either a BufferValue index or an Extent index.
// If passed a BufferValue, it will proceed to the extent in the ExtentIndex field.
void State::InnerFreeExtentChain(BlockIndex index) {
  auto* extent = heap_->GetBlock(index);
  ZX_DEBUG_ASSERT_MSG(IsExtent(extent) || index == 0,
                      "must pass extent index to InnerFreeExtentChain");

  while (IsExtent(extent)) {
    auto next_extent = ExtentBlockFields::NextExtentIndex::Get<BlockIndex>(extent->header);
    heap_->Free(index);
    index = next_extent;
    extent = heap_->GetBlock(index);
  }
}

std::tuple<BlockIndex, size_t, zx_status_t> State::InnerCreateExtentChain(const char* value,
                                                                          size_t length) {
  if (length == 0)
    return {0, 0, ZX_OK};

  BlockIndex extent_index;
  zx_status_t status;
  status = heap_->Allocate(std::min(kMaxOrderSize, BlockSizeForPayload(length)), &extent_index);
  if (status != ZX_OK) {
    return {0, 0, status};
  }

  // Thread the value through extents, creating new extents as needed.
  size_t offset = 0;
  const BlockIndex first_extent_index = extent_index;
  while (offset < length) {
    auto* extent = heap_->GetBlock(extent_index);

    extent->header = ExtentBlockFields::Order::Make(GetOrder(extent)) |
                     ExtentBlockFields::Type::Make(BlockType::kExtent) |
                     ExtentBlockFields::NextExtentIndex::Make(0);

    size_t len = std::min(PayloadCapacity(GetOrder(extent)), length - offset);
    memcpy(extent->payload.data, value + offset, len);
    offset += len;

    if (offset < length) {
      status = heap_->Allocate(std::min(kMaxOrderSize, BlockSizeForPayload(length - offset)),
                               &extent_index);
      if (status != ZX_OK) {
        // Do not free the chain. Return what we have written so far.
        return {first_extent_index, offset, ZX_OK};
      }
      ExtentBlockFields::NextExtentIndex::Set(&extent->header, extent_index);
    }
  }

  return {first_extent_index, offset, ZX_OK};
}

std::string State::UniqueLinkName(std::string_view prefix) {
  return std::string(prefix.data(), prefix.size()) + "-" +
         std::to_string(next_unique_link_number_.fetch_add(1, std::memory_order_relaxed));
}

zx_status_t State::CreateAndIncrementStringReference(std::string_view value, BlockIndex* out) {
  std::lock_guard<std::mutex> lock(mutex_);
  // Since InnerCreateStringReferenceWithCount might not actually allocate, a potential
  // optimzation here is to only conditionally increment the generation count.
  std::unique_ptr<AutoGenerationIncrement> gen = MaybeIncrementGeneration();
  return InnerCreateAndIncrementStringReference(value, out, true);
}

zx_status_t State::InnerCreateStringReference(std::string_view value, BlockIndex* const out,
                                              bool should_cache) {
  zx_status_t status = ZX_OK;

  const auto maybe_block_index = string_reference_ids_.find(value);
  if (maybe_block_index == std::cend(string_reference_ids_)) {
    status = InnerDoStringReferenceAllocations(value, out, should_cache);
  } else {
    *out = maybe_block_index->second;
  }

  return status;
}

namespace {

constexpr size_t GetOrderForSizeOfStringReference(const size_t data_size) {
  return std::min(
      BlockSizeForPayload(data_size + StringReferenceBlockPayload::TotalLength::SizeInBytes()),
      BlockSizeForPayload(kMaxPayloadSize));
}

}  // namespace

zx_status_t State::InnerDoStringReferenceAllocations(std::string_view data, BlockIndex* const out,
                                                     bool should_cache) {
  const auto order_for_size = GetOrderForSizeOfStringReference(data.size());
  auto status = heap_->Allocate(order_for_size, out);
  if (status != ZX_OK) {
    return status;
  }

  auto* block = heap_->GetBlock(*out);
  block->header = StringReferenceBlockFields::Order::Make(GetOrder(block)) |
                  StringReferenceBlockFields::Type::Make(BlockType::kStringReference) |
                  StringReferenceBlockFields::NextExtentIndex::Make(
                      0 /* this is potentially reset in WriteStringReferencePayload */) |
                  StringReferenceBlockFields::ReferenceCount::Make(0);
  block->payload.u64 = StringReferenceBlockPayload::TotalLength::Make(data.size());
  status = WriteStringReferencePayload(block, data);
  if (status != ZX_OK) {
    heap_->Free(*out);
    return status;
  }

  if (should_cache) {
    string_reference_ids_[std::string(data.data(), data.size())] = *out;
  }

  return ZX_OK;
}

zx_status_t State::WriteStringReferencePayload(Block* const block, std::string_view data) {
  // write the inline-portion first:
  auto inline_length =
      std::min(data.size(), PayloadCapacity(GetOrder(block)) -
                                StringReferenceBlockPayload::TotalLength::SizeInBytes());
  memcpy(block->payload.data + StringReferenceBlockPayload::TotalLength::SizeInBytes(), data.data(),
         inline_length);

  // Set initial total length to inline length. We will update it if we write extents.
  StringReferenceBlockPayload::TotalLength::Set(&block->payload.u64, inline_length);

  // this implies the whole piece of data fit inline, and we are done
  if (inline_length == data.size()) {
    return ZX_OK;
  }

  // allocate necessary extents, copying data
  auto [first_extent_index, written, status] =
      InnerCreateExtentChain(&*std::cbegin(data) + inline_length, data.size() - inline_length);

  if (status != ZX_OK) {
    return status;
  }

  if (first_extent_index != 0) {
    block->header =
        block->header | StringReferenceBlockFields::NextExtentIndex::Make(first_extent_index);
    StringReferenceBlockPayload::TotalLength::Set(&block->payload.u64, inline_length + written);
  }
  return ZX_OK;
}

zx_status_t State::InnerCreateAndIncrementStringReference(std::string_view name, BlockIndex* out,
                                                          bool should_cache) {
  const auto status = InnerCreateStringReference(name, out, should_cache);
  if (status != ZX_OK) {
    return status;
  }

  auto* const block = heap_->GetBlock(*out);

  // you must look up the reference count, because if the block already exists,
  // InnerCreateStringReference does not notify you in any way
  const auto count = StringReferenceBlockFields::ReferenceCount::Get<uint64_t>(block->header);
  if (count < StringReferenceBlockFields::ReferenceCount::kMask) {
    StringReferenceBlockFields::ReferenceCount::Set(&block->header, count + 1);
  }

  return status;
}

std::string State::UniqueName(const std::string& prefix) {
  uint64_t value = next_unique_id_.fetch_add(1, std::memory_order_relaxed);

  // enough space to write uint64_t max + null terminate in hex, ie
  // "0xffffffffffffffff\n"
  constexpr size_t max_hex_string_len = 19;

  std::array<char, max_hex_string_len> hex_buff;
  sprintf(hex_buff.data(), "0x%lx", value);

  return prefix + hex_buff.data();
}

InspectStats State::GetStats() const {
  InspectStats ret = {};
  std::lock_guard<std::mutex> lock(mutex_);

  ret.dynamic_child_count = link_callbacks_.size();
  ret.maximum_size = heap_->maximum_size();
  ret.size = heap_->size();
  ret.allocated_blocks = heap_->TotalAllocatedBlocks();
  ret.deallocated_blocks = heap_->TotalDeallocatedBlocks();
  ret.failed_allocations = heap_->TotalFailedAllocations();
  return ret;
}

std::optional<std::string> TesterLoadStringReference(const State& state, const BlockIndex index) {
  std::lock_guard<std::mutex> lock(state.mutex_);
  const auto* const block = state.heap_->GetBlock(index);
  if (!block) {
    return {};
  }

  std::vector<uint8_t> buffer;

  const auto total_length =
      StringReferenceBlockPayload::TotalLength::Get<size_t>(block->payload.u64);
  buffer.reserve(total_length);
  const auto max_inlinable_length =
      PayloadCapacity(GetOrder(block)) - StringReferenceBlockPayload::TotalLength::SizeInBytes();
  buffer.insert(buffer.cend(),
                block->payload_ptr() + StringReferenceBlockPayload::TotalLength::SizeInBytes(),
                block->payload_ptr() + StringReferenceBlockPayload::TotalLength::SizeInBytes() +
                    std::min(total_length, max_inlinable_length));

  if (total_length == buffer.size()) {
    return std::string{buffer.cbegin(), buffer.cend()};
  }

  state.InnerReadExtents(
      StringReferenceBlockFields::NextExtentIndex::Get<BlockIndex>(block->header),
      total_length - max_inlinable_length, &buffer);

  return std::string{buffer.cbegin(), buffer.cend()};
}

uint64_t TesterGetStringReferenceCount(const State& state, const BlockIndex index) {
  std::lock_guard<std::mutex> lock(state.mutex_);
  const auto* const block = state.heap_->GetBlock(index);
  if (!block) {
    return 0;
  }
  return StringReferenceBlockFields::ReferenceCount::Get<uint64_t>(block->header);
}

void TesterSetStringReferenceCount(const State& state, const BlockIndex index,
                                   const uint64_t count) {
  std::lock_guard<std::mutex> lock(state.mutex_);
  auto* const block = state.heap_->GetBlock(index);
  if (!block) {
    return;
  }
  StringReferenceBlockFields::ReferenceCount::Set(&block->header, count);
}

}  // namespace internal
}  // namespace inspect
