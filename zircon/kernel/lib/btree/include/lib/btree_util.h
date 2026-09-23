// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#ifndef ZIRCON_KERNEL_LIB_BTREE_INCLUDE_LIB_BTREE_UTIL_H_
#define ZIRCON_KERNEL_LIB_BTREE_INCLUDE_LIB_BTREE_UTIL_H_

#include <zircon/types.h>

#include <type_traits>
#include <utility>

#include <fbl/ref_ptr.h>

// Collection of utility objects and helper type definitions for the btree.

namespace btree {

// Use a custom datatype for items, instead of a pair, both for improved readability and because
// pair value initializes its elements, which we would like to avoid in certain circumstances.
template <typename Key, typename Value>
struct Item {
  Key key;
  Value value;
};

// An observer allows augmenting the BTree with custom state that is maintained at each node.
// Implementors of custom observers should provide a structure with the following interface.
struct NoopObserver {
  struct State {
    bool operator==(const State& other) const { return true; }
    bool operator!=(const State& other) const { return !(*this == other); }
  };

  // The type of the augmented state stored in each node. Must be default constructible and
  // support equality operators (== and !=).
  using AugmentedState = State;

  // Calculates the augmented state for a sequence of items in a leaf node.
  // |node_start| and |node_end| define the inclusive bounds of the items to calculate the state
  // over.
  template <typename iterator>
  static AugmentedState Calculate(iterator node_start, iterator node_end) {
    return AugmentedState{};
  }

  // Combines the augmented state of two nodes/subtrees.
  // |left| and |right| are the states of two adjacent subtrees in their natural key order.
  static AugmentedState Fold(AugmentedState left, AugmentedState right) { return AugmentedState{}; }
};

// Default allocator that provides nodes from a series of global slabs, one for each size class,
// shared by all btrees.
struct GlobalSlabAllocator {
  void* allocate(size_t size_align);
  void deallocate(size_t size_align, void* ptr);
};

// Helper object for gathering all the allocations needed to perform an insert. Internally places
// the allocations in an intrusively linked list until needed. Allocations must be registered, via
// reserve, in the same order (i.e. size_bytes) that they are then retrieved with take_next.
template <typename NodeType, typename Allocator>
class Allocations {
 public:
  explicit Allocations(Allocator& allocator) : allocator_(allocator) {}
  ~Allocations() {
    // Free any remaining allocations.
    while (head_) {
      Alloc* next = head_->next;
      const size_t size_bytes = head_->size_bytes;
      std::destroy_at(head_);
      allocator_.deallocate(size_bytes, head_);
      head_ = next;
    }
  }
  Allocations(const Allocations&) = delete;
  Allocations(Allocations&&) = delete;
  Allocations& operator=(const Allocations&) = delete;
  Allocations& operator=(Allocations&&) = delete;

  // Register another allocation. This will attempt the allocation and return false if it failed.
  // The allocation is added to the internal list.
  bool reserve(size_t size_bytes) {
    void* ptr = allocator_.allocate(size_bytes);
    if (!ptr) {
      return false;
    }

    Alloc* alloc = std::construct_at<Alloc>(static_cast<Alloc*>(ptr), nullptr, size_bytes);
    if (tail_) {
      tail_->next = alloc;
    } else {
      head_ = alloc;
    }
    tail_ = alloc;
    return true;
  }

  // Retrieves the next allocation in order. It is an error to call this if a matching reserve was
  // not already performed.
  template <typename... Args>
  NodeType* take_next(size_t size_bytes, Args&&... args) {
    ZX_ASSERT(head_);
    ZX_ASSERT(size_bytes == head_->size_bytes);
    Alloc* alloc = head_;
    head_ = head_->next;
    if (!head_) {
      tail_ = nullptr;
    }
    std::destroy_at(alloc);
    NodeType* ret = std::construct_at<NodeType>(reinterpret_cast<NodeType*>(alloc), size_bytes,
                                                std::forward<Args>(args)...);
    return ret;
  }

 private:
  struct Alloc {
    Alloc* next;
    uint32_t size_bytes;
  };
  Alloc* head_ = nullptr;
  Alloc* tail_ = nullptr;
  Allocator& allocator_;
};

// Base iterator type that is just a convenience wrapper around a node,index pair.
template <typename NodeType>
struct iterator_base {
  struct NoInitTag {};
  // Allow for explicitly constructing without initializing the fields.
  explicit constexpr iterator_base(NoInitTag) {}
  iterator_base() : node_(nullptr), index_(0) {}
  iterator_base(NodeType* node, uint32_t index) : node_(node), index_(index) {}
  template <typename T>
  explicit iterator_base(iterator_base<T>&& other) : node_(other.node_), index_(other.index_) {}
  template <typename T>
  explicit iterator_base(const iterator_base<T>& other)
      : node_(other.node_), index_(other.index_) {}

  // Ensures that the iterator is pointing to a valid item or the end sentinel. If the current
  // index is past the last item in the current node, it advances the iterator to the start of the
  // next node in the linked list. If no more nodes exist, it sets the index to the end sentinel.
  void wrap_to_next() {
    if (!this->node_->valid_index(this->index_)) {
      if (NodeType* next = this->node_->next(); next) {
        this->index_ = 0;
        this->node_ = next;
      } else {
        this->index_ = NodeType::kMaxValues;
      }
    }
  }

  bool end_sentinel() const { return this->index_ >= NodeType::kMaxValues; }

  NodeType* node_;
  uint32_t index_;
};

// Used to remember a path to a node. This allows for efficient upwards traversal, as parent
// pointers are not otherwise stored in the nodes.
//
// This implementation has a fixed maximum depth (kMaxPath). If an operation would exceed this
// depth, it must be detected and failed before it occurs.
template <typename NodeType>
struct PathTracker {
  static constexpr uint32_t kMaxPath = 8;
  uint32_t nodes = 0;
  uint32_t next = 0;
  iterator_base<NodeType> path[kMaxPath] = {
      iterator_base<NodeType>{typename iterator_base<NodeType>::NoInitTag()}};

  void push(iterator_base<NodeType> location) {
    ZX_ASSERT(next < kMaxPath);
    path[next] = location;
    next++;
    nodes = next;
  }

  iterator_base<NodeType> pop() {
    ZX_ASSERT(next > 0);
    next--;
    return path[next];
  }

  bool is_full() const { return nodes == kMaxPath; }

  // Resets back to the start of the path, allowing it to be pop'ed again.
  void reset_path() { next = nodes; }

  PathTracker() = default;
};

// The BTree supports an additional layer of validation checks intended for use during development.
// These checks are typically expensive and not suitable for a normal ASSERT or DEBUG_ASSERT. To
// allow for tests to always have these enabled the validator is a template parameter. The default
// is to use TreeValidation::None, which elides these checks.
enum class TreeValidation {
  None,
  Assert,
};

// Helper macro used through all the btree code for invoking the validator in such a way that it
// will be correctly elided when validation is disabled. By using an if constexpr, this avoids
// evaluating the condition or checking it when the validator is None.
#define BTREE_CHECK(condition)                           \
  do {                                                   \
    if constexpr (Validator == TreeValidation::Assert) { \
      ZX_ASSERT(condition);                              \
    }                                                    \
  } while (0)

template <typename T>
struct DefaultTypeTraits;

// Trait for managing raw pointers.
template <typename T>
struct DefaultTypeTraits<T*> {
  using ValueType = T*;
  using RawType = T*;
  using RefType = T*;
  using ConstRefType = const T*;

  static RawType Leak(ValueType v) { return v; }
  static ValueType Reclaim(RawType raw) { return raw; }
};

// Trait for managing std::unique_ptrs (arrays not supported).
template <typename T, typename Deleter>
struct DefaultTypeTraits<std::unique_ptr<T, Deleter>> {
  using ValueType = std::unique_ptr<T, Deleter>;
  using RawType = T*;
  using RefType = T*;
  using ConstRefType = const T*;

  static RawType Leak(ValueType& v) { return v.release(); }
  static ValueType Reclaim(RawType raw) { return ValueType(raw); }
};

// Trait for managing ref_counted pointers.
template <typename T>
struct DefaultTypeTraits<fbl::RefPtr<T>> {
  using ValueType = fbl::RefPtr<T>;
  using RawType = T*;
  using RefType = T*;
  using ConstRefType = const T*;

  static RawType Leak(ValueType& v) { return fbl::ExportToRawPtr(&v); }
  static ValueType Reclaim(RawType ptr) { return fbl::ImportFromRawPtr(ptr); }
};

// Trait for managing integral types up to 64 bits.
template <typename T>
  requires(std::is_integral_v<T> && sizeof(T) <= sizeof(uint64_t))
struct DefaultTypeTraits<T> {
  using ValueType = T;
  using RawType = uint64_t;
  using RefType = T;
  using ConstRefType = T;

  static RawType Leak(ValueType v) { return static_cast<uint64_t>(v); }

  static ValueType Reclaim(RawType v) { return static_cast<ValueType>(v); }
};

// For development the BTree<> supports tracking the validity of iterators, allowing iterator misuse
// to be caught with an assertion failure instead of crashing (or spuriously succeeding). Validity
// is tracked with a generation count in the the BTree and every iterator.
// The differently specialized BTreeGeneration and IteratorGeneration types allow for having no
// storage of code generated when tracking is disabled.
enum class IteratorValidation : bool {
  Untracked,
  Tracked,
};

template <IteratorValidation Type>
class BTreeGeneration;
template <IteratorValidation Type>
class IteratorGeneration;

template <>
class BTreeGeneration<IteratorValidation::Tracked> {
 public:
  void operator++(int) { generation_++; }
  bool operator==(const BTreeGeneration& other) const { return generation_ == other.generation_; }

 private:
  uint64_t generation_ = 0;
};

template <>
class BTreeGeneration<IteratorValidation::Untracked> {
 public:
  void operator++(int) {}
};

template <>
class IteratorGeneration<IteratorValidation::Tracked> {
 public:
  using Generation = BTreeGeneration<IteratorValidation::Tracked>;
  explicit IteratorGeneration(const Generation* tree_generation)
      : tree_generation_(tree_generation), generation_(*tree_generation) {}
  __NO_INLINE void validate() const {
    ZX_ASSERT(tree_generation_ && *tree_generation_ == generation_);
  }
  IteratorGeneration() = default;

 private:
  const Generation* tree_generation_ = nullptr;
  Generation generation_;
};

template <>
class IteratorGeneration<IteratorValidation::Untracked> {
 public:
  using Generation = BTreeGeneration<IteratorValidation::Untracked>;
  explicit IteratorGeneration(const Generation* tree_generation) {}
  void validate() const {}
  IteratorGeneration() = default;
};

}  // namespace btree

#endif  // ZIRCON_KERNEL_LIB_BTREE_INCLUDE_LIB_BTREE_UTIL_H_
