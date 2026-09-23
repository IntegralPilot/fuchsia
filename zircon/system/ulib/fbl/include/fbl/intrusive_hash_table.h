// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FBL_INTRUSIVE_HASH_TABLE_H_
#define FBL_INTRUSIVE_HASH_TABLE_H_

#include <zircon/assert.h>

#include <cstddef>
#include <iterator>
#include <limits>
#include <utility>

#include <fbl/intrusive_container_utils.h>
#include <fbl/intrusive_pointer_traits.h>
#include <fbl/intrusive_single_list.h>
#include <fbl/macros.h>

namespace fbl {

// Fwd decl of checker class used by tests.
namespace tests {
namespace intrusive_containers {
class HashTableChecker;
}  // namespace intrusive_containers
}  // namespace tests

// Static vs. Dynamic bucket initialization.
//
// fbl::HashTables may have their bucket counts and bucket storage determined
// "statically" (at compile time) or "dynamically" (determined at runtime).
// There are some important differences between the two, which this comment
// will explain.
//
// * Declaration and Initialization *
//
// The first primary difference between the two comes in how they are declared
// and initialized.  In order to declare a HashTable with static bucket storage,
// we provide the number of buckets to use as a template parameter of the type.
// For example, declaring a HashTable using `SinglyLinkedLists` of
// `std::unique_ptr<Foo>`s for buckets, and 107 bucket, would look something
// like this.
//
// ```
// using KeyType = size_t;
// using PtrType = std::unique_ptr<Foo>;
// using BucketType = SinglyLinkedList<PtrType>;
// using HashType = size_t;
// using MyHashTable =
//   fbl::HashTable<KeyType, PtrType, BucketType, HashType, 107>;
//
// MyHashTable ht{};
// /* Do things with ht */
// ```
//
// The storage for the buckets is part of the HashTable itself, meaning that the
// object will be rather large, but it can always be default constructed without
// any fear of allocation failure.  Additionally, the HashTable is always
// "valid"; there is never an invalid state for the HashTable instance to be in.
//
// The bucket count for a HashTable may also be configured at runtime,
// supporting use cases where the proper "tuned" size of the HashTable's bucket
// array may not be known until runtime.  To do this, we simply pass the
// `kDynamicBucketCount` (see below) value to the type definition of the
// HashTable, and then provide storage for the buckets during construction.  For
// example:
//
// ```
// using KeyType = size_t;
// using PtrType = std::unique_ptr<Foo>;
// using BucketType = SinglyLinkedList<PtrType>;
// using HashType = size_t;
// using MyHashTable =
//   fbl::HashTable<KeyType, PtrType, BucketType, HashType, kDynamicBucketCount>;
//
// const size_t cnt = GetBucketCount();
// auto storage = std::unique_ptr<BucketType[]>(new BucketType[cnt]);
// fbl::HashTable ht{std::move(storage), cnt};
// ```
//
// Once again, this HashTable is _always_ valid, there is never an invalid state
// for it because its storage was supplied at construction time and can never be
// released until destruction time.  In order to achieve this, however, we need
// to demand that the storage *must* be provided at construction time, which can
// be inconvenient for some patterns.  For these situations, one more option is
// provided; dynamic HashTables with delayed initialization.  As with all
// dynamically initialized HashTables, users must specify `kDynamicBucketCount`
// as the number of buckets in the template definition, but they must also opt
// into delayed initialization by passing `HashTableOptions::DelayedInit` to the
// constructor.  Afterwards, before using the instance in *any* way, users
// *must* call the Init method on the HashTable instance to provide bucket
// storage.  Following construction, the HashTable instance is "invalid" until
// it has been properly initialized.  Initialization is *not* an idempotent
// operation and must be performed exactly once before use.  For example...
//
// ```
// using KeyType = size_t;
// using PtrType = std::unique_ptr<Foo>;
// using BucketType = SinglyLinkedList<PtrType>;
// using HashType = size_t;
// using MyHashTable =
//   fbl::HashTable<KeyType, PtrType, BucketType, HashType, kDynamicBucketCount>;
//
// const size_t cnt = GetBucketCount();
// auto storage = std::unique_ptr<BucketType[]>(new BucketType[cnt]);
// fbl::HashTable ht{HashTableOptions::DelayedInit), cnt};
// ht.Init(std::move(storage), cnt);
// ```
//
// * Hash function requirements and performance implications *
//
// There are two ways users may use to specify the bucket that their object
// belongs in for a HashTable.  The easiest way is to make sure that their
// objects have a GetHash function defined which returns what amounts to a
// "large number".  The default implementation of the hash-traits for the
// HashTable will use the value returned from this function modulo the bucket
// count for the hash table to determine the bucket index that the item belongs
// in.  This is true regardless of whether or not the buckets for the hash table
// are statically or dynamically defined.
//
// If a user can define a hash function which is guaranteed to always produce a
// valid bucket index, they can skip the modulus operation by defining their own
// hash traits for their hash table instance.  That said, this currently only
// works for hash tables with statically defined bucket storage.  _HashTables
// with dynamically defined bucket storage will always perform the modulus
// operation, regardless of whether or not the user defined custom hash traits
// for their hash table_.
//
// Finally, because HashTables with dynamically defined storage possess an
// invalid state (whereas, HashTables with statically defined bucket storage do
// not), a validity check in the form of a ZX_DEBUG_ASSERT will be made on the
// bucket storage for every public facing operation.  These checks may or may
// not be present in the final build depending on the specific build
// configuration.  See the ZX_DEBUG_ASSERT docs for details.
//
inline constexpr size_t kDynamicBucketCount = 0;
enum class HashTableOption { DelayedInit };

namespace internal {
inline constexpr size_t kDefaultNumBuckets = 37;

template <typename HashType, typename BucketType, size_t NumBuckets>
class BucketStorage {
 public:
  BucketStorage() = default;
  ~BucketStorage() = default;

  constexpr HashType size() const { return static_cast<HashType>(NumBuckets); }
  BucketType& operator[](size_t ndx) { return buckets_[ndx]; }
  const BucketType& operator[](size_t ndx) const { return buckets_[ndx]; }

  // Range based iteration support.
  BucketType* begin() { return &buckets_[0]; }
  BucketType* end() { return &buckets_[NumBuckets]; }
  const BucketType* begin() const { return &buckets_[0]; }
  const BucketType* end() const { return &buckets_[NumBuckets]; }

  // Static bucket storage is always valid.
  constexpr void AssertValid() const {}

 private:
  static_assert(NumBuckets > 0, "Hash tables must have at least one bucket");
  static_assert(NumBuckets <= std::numeric_limits<HashType>::max(),
                "Too many buckets for HashType");
  BucketType buckets_[NumBuckets];
};

template <typename HashType, typename BucketType>
class BucketStorage<HashType, BucketType, kDynamicBucketCount> {
 public:
  BucketStorage(std::unique_ptr<BucketType[]> buckets, size_t bucket_count)
      : buckets_(std::move(buckets)), bucket_count_(static_cast<HashType>(bucket_count)) {
    ZX_DEBUG_ASSERT(bucket_count_ <= std::numeric_limits<HashType>::max());
  }
  ~BucketStorage() = default;

  constexpr HashType size() const { return static_cast<HashType>(bucket_count_); }
  BucketType& operator[](size_t ndx) { return buckets_[ndx]; }
  const BucketType& operator[](size_t ndx) const { return buckets_[ndx]; }

  // Range based iteration support.
  BucketType* begin() { return &buckets_[0]; }
  BucketType* end() { return &buckets_[bucket_count_]; }
  const BucketType* begin() const { return &buckets_[0]; }
  const BucketType* end() const { return &buckets_[bucket_count_]; }

  // Support for late init.
  void Init(std::unique_ptr<BucketType[]> buckets, size_t bucket_count) {
    ZX_DEBUG_ASSERT(buckets_.get() == nullptr);
    ZX_DEBUG_ASSERT(bucket_count_ == 0);

    ZX_DEBUG_ASSERT(buckets.get() != nullptr);
    ZX_DEBUG_ASSERT(bucket_count > 0);
    ZX_DEBUG_ASSERT(bucket_count <= std::numeric_limits<HashType>::max());

    buckets_ = std::move(buckets);
    bucket_count_ = static_cast<HashType>(bucket_count);
  }

  void AssertValid() const { ZX_DEBUG_ASSERT(buckets_.get() != nullptr); }

 private:
  std::unique_ptr<BucketType[]> buckets_;
  HashType bucket_count_;
};

}  // namespace internal

// DefaultHashTraits defines a default implementation of traits used to
// define the hash function for a hash table.
//
// At a minimum, a class or a struct which is to be used to define the
// hash traits of a hashtable must define...
//
// GetHash : A static method which take a constant reference to an instance of
//           the container's KeyType and returns an instance of the container's
//           HashType representing the hashed value of the key.  If the hash
//           table is configured to have a compile time number of buckets, then
//           the value returned must be on the range from
//           [0, Container::NumBuckets - 1]
//
// DefaultHashTraits generates a compliant implementation of hash traits taking
// its KeyType, ObjType, HashType and NumBuckets from template parameters. Users
// of DefaultHashTraits only need to implement a static method of ObjType named
// GetHash which takes a const reference to a KeyType and returns a HashType.
// For hash tables configured to have a compile time defined number of buckets,
// the default implementation will automatically mod by the number of buckets
// given in the template parameters.  If the user's hash function already
// automatically guarantees that the returned hash value will be in the proper
// range, they should implement their own hash traits to avoid the extra div/mod
// operation.
template <typename KeyType, typename ObjType, typename HashType, HashType kNumBuckets>
struct DefaultHashTraits {
  static_assert(std::is_unsigned_v<HashType>, "HashTypes must be unsigned integers");
  static HashType GetHash(const KeyType& key) {
    if constexpr (kNumBuckets == kDynamicBucketCount) {
      return static_cast<HashType>(ObjType::GetHash(key));
    } else {
      return static_cast<HashType>(ObjType::GetHash(key)) % kNumBuckets;
    }
  }
};

template <typename _KeyType, typename _PtrType, typename _BucketType = SinglyLinkedList<_PtrType>,
          typename _HashType = size_t, _HashType NumBuckets = internal::kDefaultNumBuckets,
          typename _KeyTraits = DefaultKeyedObjectTraits<
              _KeyType, typename internal::ContainerPtrTraits<_PtrType>::ValueType>,
          typename _HashTraits = DefaultHashTraits<
              _KeyType, typename internal::ContainerPtrTraits<_PtrType>::ValueType, _HashType,
              NumBuckets>>
class __POINTER(_KeyType) HashTable {
 private:
  // Private fwd decls of the iterator implementation.
  template <typename IterTraits>
  class iterator_impl;
  struct iterator_traits;
  struct const_iterator_traits;

 public:
  // Pointer types/traits
  using PtrType = _PtrType;
  using PtrTraits = internal::ContainerPtrTraits<PtrType>;
  using ValueType = typename PtrTraits::ValueType;
  using RefType = typename PtrTraits::RefType;

  // Key types/traits
  using KeyType = _KeyType;
  using KeyTraits = _KeyTraits;

  // Hash types/traits
  using HashType = _HashType;
  using HashTraits = _HashTraits;

  // Bucket types/traits
  using BucketType = _BucketType;
  using NodeTraits = typename BucketType::NodeTraits;

  // Declarations of the standard iterator types.
  using iterator = iterator_impl<iterator_traits>;
  using const_iterator = iterator_impl<const_iterator_traits>;

  // An alias for the type of this specific HashTable<...> and its test's validity checker.
  using ContainerType =
      HashTable<_KeyType, _PtrType, _BucketType, _HashType, NumBuckets, _KeyTraits, _HashTraits>;
  using CheckerType = ::fbl::tests::intrusive_containers::HashTableChecker;

  // Hash tables only support constant order erase if their underlying bucket
  // type does.
  static constexpr bool SupportsConstantOrderErase = BucketType::SupportsConstantOrderErase;
  static constexpr bool SupportsConstantOrderSize = true;
  static constexpr bool IsAssociative = true;
  static constexpr bool IsSequenced = false;

  static_assert(std::is_unsigned_v<HashType>, "HashTypes must be unsigned integers");

  constexpr HashTable() noexcept {
    static_assert(NumBuckets != kDynamicBucketCount,
                  "Constant default HashTable constructor cannot be used with "
                  "dynamic bucket count!.  Either explicitly pass bucket storage "
                  "to the constructor, or pass HashTableOption::DelayedInit and then "
                  "provide storage via Init before use.");

    CommonStaticAsserts();
  }

  HashTable(std::unique_ptr<BucketType[]> buckets_storage, size_t bucket_count) noexcept
      : buckets_{std::move(buckets_storage), bucket_count} {
    static_assert(NumBuckets == kDynamicBucketCount,
                  "Dynamic HashTable constructor used with template defined bucket count!");

    CommonStaticAsserts();
  }

  explicit constexpr HashTable(HashTableOption) noexcept : buckets_{nullptr, 0} {
    static_assert(NumBuckets == kDynamicBucketCount,
                  "Dynamic HashTable constructor used with template defined bucket count!");

    CommonStaticAsserts();
  }

  void Init(std::unique_ptr<BucketType[]> buckets_storage, size_t bucket_count) {
    static_assert(NumBuckets == kDynamicBucketCount,
                  "HashTable::Init cannot be used with template defined bucket count!");
    buckets_.Init(std::move(buckets_storage), bucket_count);
  }

  ~HashTable() { ZX_DEBUG_ASSERT(PtrTraits::IsManaged || is_empty()); }

  // Standard begin/end, cbegin/cend iterator accessors.
  iterator begin() { return iterator(this, iterator::BEGIN); }
  const_iterator begin() const { return const_iterator(this, const_iterator::BEGIN); }
  const_iterator cbegin() const { return const_iterator(this, const_iterator::BEGIN); }

  iterator end() { return iterator(this, iterator::END); }
  const_iterator end() const { return const_iterator(this, const_iterator::END); }
  const_iterator cend() const { return const_iterator(this, const_iterator::END); }

  // make_iterator : construct an iterator out of a reference to an object.
  iterator make_iterator(ValueType& obj) {
    buckets_.AssertValid();
    HashType ndx = GetHash(KeyTraits::GetKey(obj));
    return iterator(this, ndx, buckets_[ndx].make_iterator(obj));
  }
  const_iterator make_iterator(const ValueType& obj) const {
    buckets_.AssertValid();
    HashType ndx = GetHash(KeyTraits::GetKey(obj));
    return const_iterator(this, ndx, buckets_[ndx].make_iterator(obj));
  }

  void insert(const PtrType& ptr) { insert(PtrType(ptr)); }
  void insert(PtrType&& ptr) {
    ZX_DEBUG_ASSERT(ptr != nullptr);
    buckets_.AssertValid();

    KeyType key = KeyTraits::GetKey(*ptr);
    BucketType& bucket = GetBucket(key);

    // Duplicate keys are disallowed.  Debug assert if someone tries to to
    // insert an element with a duplicate key.  If the user thought that
    // there might be a duplicate key in the HashTable already, he/she
    // should have used insert_or_find() instead.
    ZX_DEBUG_ASSERT(FindInBucket(bucket, key).IsValid() == false);

    bucket.push_front(std::move(ptr));
    ++count_;
  }

  // insert_or_find
  //
  // Insert the element pointed to by ptr if it is not already in the
  // HashTable, or find the element that the ptr collided with instead.
  //
  // 'iter' is an optional out parameter pointer to an iterator which
  // will reference either the newly inserted item, or the item whose key
  // collided with ptr.
  //
  // insert_or_find returns true if there was no collision and the item was
  // successfully inserted, otherwise it returns false.
  //
  // Note: When using the r-value reference form of |insert_or_find|, the
  // pointer passed to the function is *only* consumed when there is no
  // collision and the item is successfully inserted into the collection (in
  // which case, the collection owns the passed pointer).  In the case that
  // there is a collision, the user's pointer is not consumed and is left as is.
  //
  bool insert_or_find(const PtrType& ptr, iterator* iter = nullptr) {
    return insert_or_find(PtrType(ptr), iter);
  }

  bool insert_or_find(PtrType&& ptr, iterator* iter = nullptr) {
    ZX_DEBUG_ASSERT(ptr != nullptr);
    buckets_.AssertValid();

    KeyType key = KeyTraits::GetKey(*ptr);
    HashType ndx = GetHash(key);
    auto& bucket = buckets_[ndx];
    auto bucket_iter = FindInBucket(bucket, key);

    if (bucket_iter.IsValid()) {
      if (iter)
        *iter = iterator(this, ndx, bucket_iter);
      return false;
    }

    bucket.push_front(std::move(ptr));
    ++count_;
    if (iter)
      *iter = iterator(this, ndx, bucket.begin());
    return true;
  }

  // insert_or_replace
  //
  // Find the element in the hashtable with the same key as *ptr and replace
  // it with ptr, then return the pointer to the element which was replaced.
  // If no element in the hashtable shares a key with *ptr, simply add ptr to
  // the hashtable and return nullptr.
  //
  PtrType insert_or_replace(const PtrType& ptr) { return insert_or_replace(PtrType(ptr)); }

  PtrType insert_or_replace(PtrType&& ptr) {
    ZX_DEBUG_ASSERT(ptr != nullptr);
    buckets_.AssertValid();

    KeyType key = KeyTraits::GetKey(*ptr);
    HashType ndx = GetHash(key);
    auto& bucket = buckets_[ndx];
    auto orig = PtrTraits::GetRaw(ptr);

    PtrType replaced = bucket.replace_if(
        [key](const ValueType& other) -> bool {
          return KeyTraits::EqualTo(key, KeyTraits::GetKey(other));
        },
        std::move(ptr));

    if (orig == PtrTraits::GetRaw(replaced)) {
      bucket.push_front(std::move(replaced));
      count_++;
      return nullptr;
    }

    return replaced;
  }

  iterator find(const KeyType& key) {
    buckets_.AssertValid();

    HashType ndx = GetHash(key);
    auto& bucket = buckets_[ndx];
    auto bucket_iter = FindInBucket(bucket, key);

    return bucket_iter.IsValid() ? iterator(this, ndx, bucket_iter) : iterator(this, iterator::END);
  }

  const_iterator find(const KeyType& key) const {
    buckets_.AssertValid();

    HashType ndx = GetHash(key);
    const auto& bucket = buckets_[ndx];
    auto bucket_iter = FindInBucket(bucket, key);

    return bucket_iter.IsValid() ? const_iterator(this, ndx, bucket_iter)
                                 : const_iterator(this, const_iterator::END);
  }

  PtrType erase(const KeyType& key) {
    buckets_.AssertValid();

    BucketType& bucket = GetBucket(key);

    PtrType ret = internal::KeyEraseUtils<BucketType, KeyTraits>::erase(bucket, key);
    if (ret != nullptr)
      --count_;

    return ret;
  }

  PtrType erase(const iterator& iter) {
    buckets_.AssertValid();

    if (!iter.IsValid())
      return PtrType(nullptr);

    return direct_erase(buckets_[iter.bucket_ndx_], *iter);
  }

  PtrType erase(ValueType& obj) { return direct_erase(GetBucket(obj), obj); }

  // clear
  //
  // Clear out the all of the hashtable buckets.  For managed pointer types,
  // this will release all references held by the hashtable to the objects
  // which were in it.
  void clear() {
    // No need to assert that delayed-init buckets are valid here; clearing a
    // non-initialized set of delayed-init buckets is a safe operation.
    for (auto& e : buckets_)
      e.clear();
    count_ = 0;
  }

  // clear_unsafe
  //
  // Perform a clear_unsafe on all buckets and reset the internal count to
  // zero.  See comments in fbl/intrusive_single_list.h
  // Think carefully before calling this!
  void clear_unsafe() {
    // No need to assert that delayed-init buckets are valid here; clearing a
    // non-initialized set of delayed-init buckets is a safe operation.
    static_assert(PtrTraits::IsManaged == false,
                  "clear_unsafe is not allowed for containers of managed pointers");
    static_assert(NodeTraits::NodeState::kNodeOptions & NodeOptions::AllowClearUnsafe,
                  "Container does not support clear_unsafe.  Consider adding "
                  "NodeOptions::AllowClearUnsafe to your node storage.");

    for (auto& e : buckets_)
      e.clear_unsafe();

    count_ = 0;
  }

  HashType bucket_count() const { return buckets_.size(); }
  size_t size() const { return count_; }
  bool is_empty() const { return count_ == 0; }

  // erase_if
  //
  // Find the first member of the hash table which satisfies the predicate
  // given by 'fn' and erase it from the list, returning a referenced pointer
  // to the removed element.  Return nullptr if no member satisfies the
  // predicate.
  template <typename UnaryFn>
  PtrType erase_if(UnaryFn fn) {
    buckets_.AssertValid();

    if (is_empty()) {
      return PtrType(nullptr);
    }

    for (HashType i = 0; i < buckets_.size(); ++i) {
      auto& bucket = buckets_[i];
      if (!bucket.is_empty()) {
        PtrType ret = bucket.erase_if(fn);
        if (ret != nullptr) {
          --count_;
          return ret;
        }
      }
    }

    return PtrType(nullptr);
  }

  // find_if
  //
  // Find the first member of the hash table which satisfies the predicate
  // given by 'fn' and return an iterator to it.  Return end() if no member
  // satisfies the predicate.
  template <typename UnaryFn>
  const_iterator find_if(UnaryFn fn) const {
    buckets_.AssertValid();

    for (auto iter = begin(); iter.IsValid(); ++iter) {
      if (fn(*iter)) {
        return iter;
      }
    }

    return end();
  }

  template <typename UnaryFn>
  iterator find_if(UnaryFn fn) {
    buckets_.AssertValid();

    for (auto iter = begin(); iter.IsValid(); ++iter) {
      if (fn(*iter)) {
        return iter;
      }
    }

    return end();
  }

 private:
  // The traits of a non-const iterator
  struct iterator_traits {
    using RefType = typename PtrTraits::RefType;
    using RawPtrType = typename PtrTraits::RawPtrType;
    using IterType = typename BucketType::iterator;

    static IterType BucketBegin(BucketType& bucket) { return bucket.begin(); }
    static IterType BucketEnd(BucketType& bucket) { return bucket.end(); }
  };

  // The traits of a const iterator
  struct const_iterator_traits {
    using RefType = typename PtrTraits::ConstRefType;
    using RawPtrType = typename PtrTraits::ConstRawPtrType;
    using IterType = typename BucketType::const_iterator;

    static IterType BucketBegin(const BucketType& bucket) { return bucket.cbegin(); }
    static IterType BucketEnd(const BucketType& bucket) { return bucket.cend(); }
  };

  // The shared implementation of the iterator
  template <class IterTraits>
  class iterator_impl {
   private:
    using IterType = typename IterTraits::IterType;

    template <typename IterCategory>
    static constexpr bool kIsBidirectional =
        std::is_base_of_v<std::bidirectional_iterator_tag, IterCategory>;

   public:
    using value_type = ValueType;
    using reference = typename IterTraits::RefType;
    using pointer = typename IterTraits::RawPtrType;
    using difference_type = std::ptrdiff_t;
    using iterator_category = typename std::iterator_traits<IterType>::iterator_category;

    iterator_impl() {}
    iterator_impl(const iterator_impl& other) {
      hash_table_ = other.hash_table_;
      bucket_ndx_ = other.bucket_ndx_;
      iter_ = other.iter_;
    }

    iterator_impl& operator=(const iterator_impl& other) {
      hash_table_ = other.hash_table_;
      bucket_ndx_ = other.bucket_ndx_;
      iter_ = other.iter_;
      return *this;
    }

    bool IsValid() const { return iter_.IsValid(); }
    bool operator==(const iterator_impl& other) const { return iter_ == other.iter_; }
    bool operator!=(const iterator_impl& other) const { return iter_ != other.iter_; }

    // Prefix
    iterator_impl& operator++() {
      if (!IsValid())
        return *this;
      ZX_DEBUG_ASSERT(hash_table_);

      // Bump the bucket iterator and go looking for a new bucket if the
      // iterator has become invalid.
      ++iter_;
      advance_if_invalid_iter();

      return *this;
    }

    template <typename _IterCategory = iterator_category,
              typename = std::enable_if_t<kIsBidirectional<_IterCategory>>>
    iterator_impl& operator--() {
      // If we have never been bound to a HashTable instance, the we had
      // better be invalid.
      if (!hash_table_) {
        ZX_DEBUG_ASSERT(!IsValid());
        return *this;
      }

      // Back up the bucket iterator.  If it is still valid, then we are done.
      --iter_;
      if (iter_.IsValid())
        return *this;

      // If the iterator is invalid after backing up, check previous
      // buckets to see if they contain any nodes.
      while (bucket_ndx_) {
        --bucket_ndx_;
        auto& bucket = GetBucket(bucket_ndx_);
        if (!bucket.is_empty()) {
          iter_ = --IterTraits::BucketEnd(bucket);
          ZX_DEBUG_ASSERT(iter_.IsValid());
          return *this;
        }
      }

      // Looks like we have backed up past the beginning.  Update the
      // bookkeeping to point at the end of the last bucket.
      bucket_ndx_ = last_bucket_ndx();
      iter_ = IterTraits::BucketEnd(GetBucket(bucket_ndx_));

      return *this;
    }

    // Postfix
    iterator_impl operator++(int) {
      iterator_impl ret(*this);
      ++(*this);
      return ret;
    }

    template <typename _IterCategory = iterator_category,
              typename = std::enable_if_t<kIsBidirectional<_IterCategory>>>
    iterator_impl operator--(int) {
      iterator_impl ret(*this);
      --(*this);
      return ret;
    }

    typename PtrTraits::PtrType CopyPointer() const { return iter_.CopyPointer(); }
    typename IterTraits::RefType operator*() const { return iter_.operator*(); }
    typename IterTraits::RawPtrType operator->() const { return iter_.operator->(); }

   private:
    friend ContainerType;

    enum BeginTag { BEGIN };
    enum EndTag { END };

    iterator_impl(const ContainerType* hash_table, BeginTag)
        : hash_table_(hash_table), bucket_ndx_(0), iter_(IterTraits::BucketBegin(GetBucket(0))) {
      hash_table_->buckets_.AssertValid();
      advance_if_invalid_iter();
    }

    iterator_impl(const ContainerType* hash_table, EndTag)
        : hash_table_(hash_table),
          bucket_ndx_(last_bucket_ndx()),
          iter_(IterTraits::BucketEnd(GetBucket(last_bucket_ndx()))) {
      hash_table_->buckets_.AssertValid();
    }

    iterator_impl(const ContainerType* hash_table, HashType bucket_ndx, const IterType& iter)
        : hash_table_(hash_table), bucket_ndx_(bucket_ndx), iter_(iter) {
      hash_table_->buckets_.AssertValid();
    }

    BucketType& GetBucket(HashType ndx) {
      return const_cast<ContainerType*>(hash_table_)->buckets_[ndx];
    }

    void advance_if_invalid_iter() {
      // If the iterator has run off the end of it's current bucket, then
      // check to see if there are nodes in any of the remaining buckets.
      if (!iter_.IsValid()) {
        while (bucket_ndx_ < (last_bucket_ndx())) {
          ++bucket_ndx_;
          auto& bucket = GetBucket(bucket_ndx_);

          if (!bucket.is_empty()) {
            iter_ = IterTraits::BucketBegin(bucket);
            ZX_DEBUG_ASSERT(iter_.IsValid());
            break;
          } else if (bucket_ndx_ == (last_bucket_ndx())) {
            iter_ = IterTraits::BucketEnd(bucket);
          }
        }
      }
    }

    HashType last_bucket_ndx() const { return hash_table_->bucket_count() - 1; }

    const ContainerType* hash_table_ = nullptr;
    HashType bucket_ndx_ = 0;
    IterType iter_;
  };

  PtrType direct_erase(BucketType& bucket, ValueType& obj) {
    PtrType ret = internal::DirectEraseUtils<BucketType>::erase(bucket, obj);

    if (ret != nullptr)
      --count_;

    return ret;
  }

  static typename BucketType::iterator FindInBucket(BucketType& bucket, const KeyType& key) {
    return bucket.find_if([key](const ValueType& other) -> bool {
      return KeyTraits::EqualTo(key, KeyTraits::GetKey(other));
    });
  }

  static typename BucketType::const_iterator FindInBucket(const BucketType& bucket,
                                                          const KeyType& key) {
    return bucket.find_if([key](const ValueType& other) -> bool {
      return KeyTraits::EqualTo(key, KeyTraits::GetKey(other));
    });
  }

  static void CommonStaticAsserts() {
    using NodeState = internal::node_state_t<NodeTraits, RefType>;

    // Make certain that the type of pointer we are expected to manage matches
    // the type of pointer that our Node type expects to manage.  In theory, our
    // bucket has already performed this check for us, but extra checks are
    // always welcome.
    static_assert(std::is_same_v<PtrType, typename NodeState::PtrType>,
                  "HashTable's pointer type must match its Node's pointer type");

    // HashTable does not currently support direct remove-from-container (but
    // could do so if it did not track size).
    static_assert(!(NodeState::kNodeOptions & NodeOptions::AllowRemoveFromContainer),
                  "HashTable does not support nodes which allow RemoveFromContainer.");
  }

  // The test framework's 'checker' class is our friend.
  friend CheckerType;

  // Iterators need to access our bucket array in order to iterate.
  friend iterator;
  friend const_iterator;

  // Hash tables may not currently be copied, assigned or moved.
  DISALLOW_COPY_ASSIGN_AND_MOVE(HashTable);

  BucketType& GetBucket(const KeyType& key) { return buckets_[GetHash(key)]; }
  BucketType& GetBucket(const ValueType& obj) { return GetBucket(KeyTraits::GetKey(obj)); }

  HashType GetHash(const KeyType& obj) const {
    if constexpr (NumBuckets == kDynamicBucketCount) {
      return HashTraits::GetHash(obj) % buckets_.size();
    } else {
      return HashTraits::GetHash(obj);
    }
  }

  size_t count_ = 0UL;
  internal::BucketStorage<HashType, BucketType, NumBuckets> buckets_;
};

// TaggedHashTable<> is intended for use with ContainableBaseClasses<>.
//
// For an easy way to allow instances of your class to live in multiple
// intrusive containers at once, simply derive from
// ContainableBaseClasses<YourContainables<PtrType, TagType>...> and then use
// this template instead of HashTable<> as the container, passing the same tag
// type you used earlier as the third parameter.
//
// See comments on ContainableBaseClasses<> in fbl/intrusive_container_utils.h
// for more details.
//
template <typename KeyType, typename PtrType, typename TagType,
          typename BucketType = TaggedSinglyLinkedList<PtrType, TagType>,
          typename HashType = size_t, HashType NumBuckets = internal::kDefaultNumBuckets,
          typename KeyTraits = DefaultKeyedObjectTraits<
              KeyType, typename internal::ContainerPtrTraits<PtrType>::ValueType>,
          typename HashTraits =
              DefaultHashTraits<KeyType, typename internal::ContainerPtrTraits<PtrType>::ValueType,
                                HashType, NumBuckets>>
using TaggedHashTable =
    HashTable<KeyType, PtrType, BucketType, HashType, NumBuckets, KeyTraits, HashTraits>;

// Explicit declaration of constexpr storage.
#define HASH_TABLE_PROP(_type, _name)                                                   \
  template <typename KeyType, typename PtrType, typename BucketType, typename HashType, \
            HashType NumBuckets, typename KeyTraits, typename HashTraits>               \
  constexpr _type                                                                       \
      HashTable<KeyType, PtrType, BucketType, HashType, NumBuckets, KeyTraits, HashTraits>::_name

HASH_TABLE_PROP(bool, SupportsConstantOrderErase);
HASH_TABLE_PROP(bool, SupportsConstantOrderSize);
HASH_TABLE_PROP(bool, IsAssociative);
HASH_TABLE_PROP(bool, IsSequenced);

#undef HASH_TABLE_PROP

}  // namespace fbl

#endif  // FBL_INTRUSIVE_HASH_TABLE_H_
