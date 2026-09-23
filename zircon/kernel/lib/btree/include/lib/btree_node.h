// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#ifndef ZIRCON_KERNEL_LIB_BTREE_INCLUDE_LIB_BTREE_NODE_H_
#define ZIRCON_KERNEL_LIB_BTREE_INCLUDE_LIB_BTREE_NODE_H_

#include <lib/btree_util.h>

#include <algorithm>
#include <array>
#include <bit>

#include <fbl/packed_pointer.h>

namespace btree {

// BTree node implementation.
//
// This is a B-tree variant that resembles a B+tree in that all values are stored in the leaf
// nodes. Intermediate nodes store keys that represent the *minimum* possible key for each of their
// child subtrees.
//
// A key property of this representation is that the first key (at index 0) of every non-leaf node
// must be the minimum possible key value, which also must be the default initialized value of Key.
// This minimum key effectively represents -infinity and ensures that any key less than the first
// explicit key in the second slot will correctly traverse into the first child. This differs from
// standard B-trees where keys are stored 'between' child pointers.
//
// Because intermediate nodes store minimum keys rather than exact keys, symmetric lower_bound
// operation across both leaf and intermediate nodes is not possible. Search operations therefore
// always use upper_bound to find the correct subtree.
template <typename Key, typename Value, size_t NodeSize, TreeValidation Validator,
          typename AugmentedState>
class Node {
 public:
  using ItemType = Item<Key, Value>;

 public:
  // All node constructors need to be told what size they are and whether or not they are a leaf
  // type. Additional constructors also take various initial items or nodes to copy from.
  Node(size_t size_bytes, bool leaf) : Node(size_bytes, nullptr, nullptr, leaf, 0) {}
  Node(size_t size_bytes, Node&& node, const ItemType& item, uint32_t index)
      : Node(size_bytes, node.prev(), node.next(), node.is_leaf(), node.count() + 1) {
    BTREE_CHECK(size_bytes > node.size_bytes());
    copy_insert(0, index, item, node, 0, node.count());
  }
  Node(size_t size_bytes, bool leaf, const ItemType& item)
      : Node(size_bytes, nullptr, nullptr, leaf, 1) {
    items_[0] = item;
  }
  Node(size_t size_bytes, bool leaf, const ItemType& item1, const ItemType& item2)
      : Node(size_bytes, nullptr, nullptr, leaf, 2) {
    items_[0] = item1;
    items_[1] = item2;
  }

  ~Node() = default;

  Node() = delete;
  Node(const Node&) = delete;
  Node(Node&&) = delete;
  Node& operator=(const Node&) = delete;
  Node& operator=(Node&&) = delete;

  bool is_full() const { return count() == max_count(); }
  bool is_empty() const { return count() == 0; }

  bool valid_index(uint32_t index) const { return index < count(); }

  const ItemType& get(uint32_t index) const {
    BTREE_CHECK(valid_index(index));
    return items_[index];
  }

  // Updates the key at the given index. It is the callers responsibility to ensure this results in
  // a valid tree.
  void update_key(uint32_t index, const Key& key) {
    ZX_DEBUG_ASSERT(valid_index(index) && (index == 0 || items_[index - 1].key < key) &&
                    (index == count() - 1 || key < items_[index + 1].key));
    items_[index].key = key;
  }

  void update_value(uint32_t index, uint64_t value) {
    BTREE_CHECK(valid_index(index));
    items_[index].value = value;
  }

  // Returns the index of the upper bound of key, or count() if key is beyond this node. Due to how
  // intermediate nodes are represented there is no equivalent lower_bound operation as it does not
  // have a symmetric implementation for both intermediate and leaf nodes.
  uint32_t upper_bound(const Key& key) {
    const uint32_t c = count();
    for (uint32_t i = 0; i < c; i++) {
      if (key < items_[i].key) {
        return i;
      }
    }
    return c;
  }

  // Inserts an item at the specified index, which must either be a valid_index, or at the end of
  // the valid indexes and shifts the rest of the items up and updates the count.
  void insert(uint32_t index, const ItemType& item) {
    // This will not catch all duplicate key insertions, since the duplicate key could be in a
    // neighboring (full) node, but should catch most accidents.
    ZX_DEBUG_ASSERT((index == 0 || items_[index - 1].key < item.key) &&
                    (index == count() || item.key < items_[index].key));
    expand_at(index, 1);
    items_[index] = item;
  }

  // Erase the range items, shifting down the remainder and updating the count.
  void erase(uint32_t index, uint32_t amount) {
    const uint32_t c = count();
    BTREE_CHECK(amount > 0 && (index + amount) <= c);
    move_down(index, index + amount, c - (index + amount));
    set_count(c - amount);
  }

  // Pushes a single item onto the end of the node.
  void push(const ItemType& item) {
    BTREE_CHECK(!is_full());
    const uint32_t c = count();
    ZX_DEBUG_ASSERT(c == 0 || items_[c - 1].key < item.key);
    items_[c] = item;
    set_count(c + 1);
  }

  // Moves all the items from |other| onto the end of |this|.
  void merge_from(Node& __restrict other) __restrict {
    const uint32_t c = count();
    const uint32_t oc = other.count();
    ZX_DEBUG_ASSERT(c == 0 || oc == 0 || items_[c - 1].key < other.items_[0].key);
    copy(c, other, 0, oc);
    set_count(c + oc);
    other.set_count(0);
  }

  // Moves |num| items from the start of this node onto the end of |left|.
  void rotate_left(Node* __restrict left, uint32_t num) __restrict {
    const uint32_t lc = left->count();
    left->copy(lc, *this, 0, num);
    left->set_count(lc + num);
    erase(0, num);
  }

  // Moves as many items as possible from the start of this node onto the end of |left|.
  void rotate_left_max(Node* __restrict left) __restrict {
    const uint32_t available = left->max_count() - left->count();
    BTREE_CHECK(available > 0);
    rotate_left(left, std::min(available, count()));
  }

  // Moves |num| items from the end of this node into the start of |right|.
  void rotate_right(Node* __restrict right, uint32_t num) __restrict {
    const uint32_t c = count();
    right->expand_at(0, num);
    right->copy(0, *this, c - num, num);
    set_count(c - num);
  }

  Node* prev() const { return prev_.ptr(); }
  Node* next() const { return next_.ptr(); }

  void set_next(Node* next) { next_.set_ptr(next); }
  void set_prev(Node* prev) { prev_.set_ptr(prev); }

  size_t size_bytes() const {
    return 1ul << (size_class() + std::bit_width(kSmallestOneItemNode) - 1);
  }

  static_assert(std::has_single_bit(NodeSize), "NodeSize must be power of 2");

  // This is validated in the constructor so that offsetof can see the full definition of Node.
  // The AugmentedState is declared with [[no_unique_address]], which allows for empty objects to
  // overlap (effectively making them zero sized).
  static constexpr size_t kHeaderSize =
      16 + (std::is_empty_v<AugmentedState> ? 0 : sizeof(AugmentedState));

  static constexpr size_t SizeForCount(size_t count) {
    return std::bit_ceil(kHeaderSize + (sizeof(ItemType) * count));
  }

  // Size of the smallest node that can hold at least 1 item. It is invalid to attempt to create a
  // node smaller than this.
  static constexpr size_t kSmallestOneItemNode = SizeForCount(1);

  // Size of the smallest node that can hold at least 2 items.
  static constexpr size_t kSmallestTwoItemNode = SizeForCount(2);

  static_assert(kSmallestOneItemNode <= NodeSize);
  static_assert(kSmallestTwoItemNode <= NodeSize);

  // Number of different node sizes possible.
  static constexpr size_t kNumSizeClasses =
      std::bit_width(NodeSize) - std::bit_width(kSmallestOneItemNode) + 1;

  // Number of bits needed to represent the different sizes.
  static constexpr size_t kSizeClassSizeBits = std::bit_width(kNumSizeClasses);

  static constexpr size_t kSizeClassMask = (1ul << kSizeClassSizeBits) - 1;
  // Use the next bit as the 'is_leaf' flag. This is the 0-based index of the leaf bit.
  static constexpr size_t kIsLeafBit = kSizeClassSizeBits;
  static constexpr size_t kIsLeafMask = (1ul << kIsLeafBit);

  // The size class and is_leaf flag are stored with the prev_ pointer and must fit into the
  // pointer to every size class, i.e. at least a pointer to the smallest class.
  static constexpr size_t kPrevDataBits = kIsLeafBit + 1;
  static_assert((1ul << kPrevDataBits) <= kSmallestOneItemNode);

  // Calculates our maximum count given a size in bytes.
  static constexpr uint32_t MaxCountFromSize(size_t bytes) {
    return static_cast<uint32_t>((bytes - kHeaderSize) / sizeof(ItemType));
  }

  // Ensure that the largest node can hold a 'decent' number of items. This ensures that a tree is
  // not accidentally built that will easily overflow the 8 depth limit.
  static_assert(MaxCountFromSize(NodeSize) >= 10);

  static constexpr size_t SizeBytesToClass(size_t size_bytes) {
    return std::bit_width(size_bytes) - std::bit_width(kSmallestOneItemNode);
  }
  static_assert(SizeBytesToClass(kSmallestOneItemNode) == 0);
  static_assert(SizeBytesToClass(NodeSize) == kNumSizeClasses - 1);

  // Similarly the maximum count must fit into a pointer to every size class. This requirement
  // avoids having to dynamically size the mask of the count based on the current size size class.
  static constexpr uint32_t kMaxValues = MaxCountFromSize(NodeSize);
  static constexpr size_t kNextDataBits = std::bit_width(kMaxValues);
  static_assert((1ul << kNextDataBits) <= kSmallestOneItemNode);

  // The target min values only applies to non-root nodes, and so is derived from kMaxValues.
  static constexpr uint32_t kTargetMinValues = kMaxValues / 2;

  bool is_leaf() const { return prev_.data() & kIsLeafMask; }
  uint32_t max_count() const { return MaxCountFromSize(size_bytes()); }
  uint32_t count() const { return static_cast<uint32_t>(next_.data()); }
  void set_count(uint32_t count) {
    BTREE_CHECK(count <= max_count());
    next_.set_data(count);
  }

  AugmentedState& aug_state() { return aug_state_; }
  const AugmentedState& aug_state() const { return aug_state_; }

 private:
  // All other constructors route here. The size_bytes tells us how large the items_ array is by
  // informing us of the total size of the allocation we are situated in. This is stored with the
  // prev_ pointer along with whether or not this is a leaf node.
  Node(size_t size_bytes, Node* prev, Node* next, bool is_leaf, size_t initial_count)
      : prev_(prev, SizeBytesToClass(size_bytes) | (is_leaf ? kIsLeafMask : 0)),
        next_(next, initial_count) {
    static_assert(offsetof(Node, items_) == kHeaderSize, "kHeaderSize must match offsetof items_");
    BTREE_CHECK(size_bytes >= kSmallestOneItemNode && size_bytes <= NodeSize &&
                std::has_single_bit(size_bytes));
  }

  uint32_t size_class() const { return prev_.data() & kSizeClassMask; }

  // Creates |amount| uninitialized slots at |index| by shifting up the existing items. Also updates
  // the count. Caller is responsible for filling in the slots.
  void expand_at(uint32_t index, uint32_t amount) {
    const uint32_t c = count();
    BTREE_CHECK(index <= c);
    std::move_backward(items_ + index, items_ + c, items_ + c + amount);
    set_count(c + amount);
  }

  // Shift a range of slots down. This does not update the count and will overwrite target slots and
  // leave src slots as duplicated. Caller is responsible for updating the src slots with new items.
  void move_down(uint32_t dst_index, uint32_t src_index, uint32_t amount) {
    BTREE_CHECK(dst_index < src_index);
    BTREE_CHECK(src_index + amount <= count());
    std::move(items_ + src_index, items_ + src_index + amount, items_ + dst_index);
  }

  void copy(uint32_t dst_index, Node& __restrict src, uint32_t src_index,
            uint32_t count) __restrict {
    BTREE_CHECK(&src != this);
    BTREE_CHECK(dst_index + count <= max_count());
    BTREE_CHECK(src_index + count <= src.max_count());

    std::copy(src.items_ + src_index, src.items_ + src_index + count, items_ + dst_index);
  }

  void copy_insert(uint32_t dst_index, uint32_t dst_insert_index, const ItemType& insert_item,
                   Node& __restrict src, uint32_t src_index, uint32_t copy_count) __restrict {
    copy(dst_index, src, src_index, dst_insert_index - dst_index);
    items_[dst_insert_index] = insert_item;
    copy(dst_insert_index + 1, src, src_index + (dst_insert_index - dst_index),
         copy_count - (dst_insert_index - dst_index));
  }

  // TODO(https://fxbug.dev/494059275): The node knows if it is a leaf node or not and could avoid
  // using this extraneous left most key, avoiding the need to keep it consistent. Actually reusing
  // that memory to store an additional pointer in intermediate nodes would be more challenging.

  fbl::PackedPointer<Node, kPrevDataBits, false> prev_;
  fbl::PackedPointer<Node, kNextDataBits, false> next_;

  // If the AugmentedState is greater than 8 byte aligned then the kHeaderSize calculation will
  // potentially fail. This assertion provides a clearer and more obvious error for this case.
  // Although some alignments (such as 16) may presently result in a correct header calculation, we
  // still wish to forbid this as it will be unstable with any future layout changes to Node.
  static_assert(alignof(AugmentedState) <= 8);
  [[no_unique_address]] AugmentedState aug_state_;

  // The Node will be allocated in block of memory of varying sizes, which determines how many
  // items_ we are storing and so a flexible array memory is used to support this dynamism. The size
  // of this array can be inferred from the |size_bytes| passed into the constructor, which gets
  // stored as part of the packed data in the prev_ pointer (this is what the max_count helper
  // does).
  // |Item| itself must be trivially constructible and destructible as the items are implicitly
  // constructed via assignment when growing the number of active elements in the node, and are
  // implicitly destructed when the node goes away.
  static_assert(std::is_trivially_constructible_v<ItemType>);
  static_assert(std::is_trivially_destructible_v<ItemType>);
  static_assert(std::is_trivially_copyable_v<ItemType>);
  ItemType items_[];
};

}  // namespace btree

#endif  // ZIRCON_KERNEL_LIB_BTREE_INCLUDE_LIB_BTREE_NODE_H_
