// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fbl/intrusive_double_list.h>
#include <fbl/intrusive_hash_table.h>
#include <fbl/tests/intrusive_containers/associative_container_test_environment.h>
#include <fbl/tests/intrusive_containers/intrusive_hash_table_checker.h>
#include <fbl/tests/intrusive_containers/test_thunks.h>
#include <zxtest/zxtest.h>

namespace fbl {
namespace tests {
namespace intrusive_containers {

using OtherKeyType = uint16_t;
using OtherHashType = uint32_t;
static constexpr OtherHashType kOtherNumBuckets = 23;

template <typename PtrType>
struct OtherHashTraits {
  using ObjType = typename ::fbl::internal::ContainerPtrTraits<PtrType>::ValueType;
  using BucketStateType = DoublyLinkedListNodeState<PtrType>;

  // Linked List Traits
  static BucketStateType& node_state(ObjType& obj) {
    return obj.other_container_state_.bucket_state_;
  }

  // Keyed Object Traits
  static OtherKeyType GetKey(const ObjType& obj) { return obj.other_container_state_.key_; }

  static bool LessThan(const OtherKeyType& key1, const OtherKeyType& key2) { return key1 < key2; }

  static bool EqualTo(const OtherKeyType& key1, const OtherKeyType& key2) { return key1 == key2; }

  // Hash Traits
  static OtherHashType GetHash(const OtherKeyType& key) {
    return static_cast<OtherHashType>((key * 0xaee58187) % kOtherNumBuckets);
  }

  // Set key is a trait which is only used by the tests, not by the containers
  // themselves.
  static void SetKey(ObjType& obj, OtherKeyType key) { obj.other_container_state_.key_ = key; }
};

template <typename PtrType>
struct OtherHashState {
 private:
  friend struct OtherHashTraits<PtrType>;
  OtherKeyType key_;
  typename OtherHashTraits<PtrType>::BucketStateType bucket_state_;
};

// Traits for a HashTable with a template-defined number of DoublyLinkedList
// buckets.
template <typename PtrType, NodeOptions kNodeOptions = NodeOptions::None>
class HTDLLTraits {
 public:
  // clang-format off
    using ObjType = typename ::fbl::internal::ContainerPtrTraits<PtrType>::ValueType;

    using ContainerType           = HashTable<size_t, PtrType, DoublyLinkedList<PtrType>>;
    using ContainableBaseClass    = DoublyLinkedListable<PtrType, kNodeOptions>;
    using ContainerStateType      = DoublyLinkedListNodeState<PtrType, kNodeOptions>;
    using KeyType                 = typename ContainerType::KeyType;
    using HashType                = typename ContainerType::HashType;

    using OtherContainerTraits    = OtherHashTraits<PtrType>;
    using OtherContainerStateType = OtherHashState<PtrType>;
    using OtherBucketType         = DoublyLinkedListCustomTraits<PtrType, OtherContainerTraits>;
    using OtherContainerType      = HashTable<OtherKeyType,
                                              PtrType,
                                              OtherBucketType,
                                              OtherHashType,
                                              kOtherNumBuckets,
                                              OtherContainerTraits,
                                              OtherContainerTraits>;

    using TestObjBaseType  = HashedTestObjBase<typename ContainerType::KeyType,
                                               typename ContainerType::HashType>;
  // clang-format on

  struct Tag1 {};
  struct Tag2 {};
  struct Tag3 {};

  using TaggedContainableBaseClasses =
      fbl::ContainableBaseClasses<TaggedDoublyLinkedListable<PtrType, Tag1>,
                                  TaggedDoublyLinkedListable<PtrType, Tag2>,
                                  TaggedDoublyLinkedListable<PtrType, Tag3>>;

  using TaggedType1 = HashTable<size_t, PtrType, DoublyLinkedList<PtrType, Tag1>>;
  using TaggedType2 = HashTable<size_t, PtrType, DoublyLinkedList<PtrType, Tag2>>;
  using TaggedType3 = HashTable<size_t, PtrType, DoublyLinkedList<PtrType, Tag3>>;
};

// Traits for a HashTable with a dynamic number of DoublyLinkedList buckets,
// defined at object construction time.
template <typename PtrType, NodeOptions kNodeOptions = NodeOptions::None>
class DHTDLLTraits {
 public:
  template <typename DynamicHashTableType, size_t BucketCount>
  class DynamicHashTableWrapper : public DynamicHashTableType {
   public:
    using BucketType = typename DynamicHashTableType::BucketType;
    DynamicHashTableWrapper()
        : DynamicHashTableType{std::unique_ptr<BucketType[]>(new BucketType[BucketCount]),
                               BucketCount} {}
    ~DynamicHashTableWrapper() = default;
  };

  // clang-format off
  using ObjType = typename ::fbl::internal::ContainerPtrTraits<PtrType>::ValueType;

  using ContainerType           = DynamicHashTableWrapper
                                    <HashTable<size_t,
                                               PtrType,
                                               DoublyLinkedList<PtrType>,
                                               size_t,
                                               kDynamicBucketCount>,
                                     37>;
  using ContainableBaseClass    = DoublyLinkedListable<PtrType, kNodeOptions>;
  using ContainerStateType      = DoublyLinkedListNodeState<PtrType, kNodeOptions>;
  using KeyType                 = typename ContainerType::KeyType;
  using HashType                = typename ContainerType::HashType;

  using OtherContainerTraits    = OtherHashTraits<PtrType>;
  using OtherContainerStateType = OtherHashState<PtrType>;
  using OtherBucketType         = DoublyLinkedListCustomTraits<PtrType, OtherContainerTraits>;
  using OtherContainerType      = DynamicHashTableWrapper<
                                    HashTable<OtherKeyType,
                                              PtrType,
                                              OtherBucketType,
                                              OtherHashType,
                                              kDynamicBucketCount,
                                              OtherContainerTraits,
                                              OtherContainerTraits>,
                                    kOtherNumBuckets>;

  using TestObjBaseType  = HashedTestObjBase<typename ContainerType::KeyType,
                                             typename ContainerType::HashType>;
  // clang-format on

  struct Tag1 {};
  struct Tag2 {};
  struct Tag3 {};

  using TaggedContainableBaseClasses =
      fbl::ContainableBaseClasses<TaggedDoublyLinkedListable<PtrType, Tag1>,
                                  TaggedDoublyLinkedListable<PtrType, Tag2>,
                                  TaggedDoublyLinkedListable<PtrType, Tag3>>;

  using TaggedType1 = HashTable<size_t, PtrType, DoublyLinkedList<PtrType, Tag1>>;
  using TaggedType2 = HashTable<size_t, PtrType, DoublyLinkedList<PtrType, Tag2>>;
  using TaggedType3 = HashTable<size_t, PtrType, DoublyLinkedList<PtrType, Tag3>>;
};

// Traits for a HashTable with a dynamic number of DoublyLinkedList buckets,
// defined at after construction time but before use (eg; DelayedInit).
template <typename PtrType, NodeOptions kNodeOptions = NodeOptions::None>
class DIDHTDLLTraits {
 public:
  template <typename DynamicHashTableType, size_t BucketCount>
  class DynamicHashTableWrapper : public DynamicHashTableType {
   public:
    using BucketType = typename DynamicHashTableType::BucketType;
    DynamicHashTableWrapper() : DynamicHashTableType{HashTableOption::DelayedInit} {
      this->Init(std::unique_ptr<BucketType[]>(new BucketType[BucketCount]), BucketCount);
    }
    ~DynamicHashTableWrapper() = default;
  };

  // clang-format off
  using ObjType = typename ::fbl::internal::ContainerPtrTraits<PtrType>::ValueType;

  using ContainerType           = DynamicHashTableWrapper
                                    <HashTable<size_t,
                                               PtrType,
                                               DoublyLinkedList<PtrType>,
                                               size_t,
                                               kDynamicBucketCount>,
                                     37>;
  using ContainableBaseClass    = DoublyLinkedListable<PtrType, kNodeOptions>;
  using ContainerStateType      = DoublyLinkedListNodeState<PtrType, kNodeOptions>;
  using KeyType                 = typename ContainerType::KeyType;
  using HashType                = typename ContainerType::HashType;

  using OtherContainerTraits    = OtherHashTraits<PtrType>;
  using OtherContainerStateType = OtherHashState<PtrType>;
  using OtherBucketType         = DoublyLinkedListCustomTraits<PtrType, OtherContainerTraits>;
  using OtherContainerType      = DynamicHashTableWrapper<
                                    HashTable<OtherKeyType,
                                              PtrType,
                                              OtherBucketType,
                                              OtherHashType,
                                              kDynamicBucketCount,
                                              OtherContainerTraits,
                                              OtherContainerTraits>,
                                    kOtherNumBuckets>;

  using TestObjBaseType  = HashedTestObjBase<typename ContainerType::KeyType,
                                             typename ContainerType::HashType>;
  // clang-format on

  struct Tag1 {};
  struct Tag2 {};
  struct Tag3 {};

  using TaggedContainableBaseClasses =
      fbl::ContainableBaseClasses<TaggedDoublyLinkedListable<PtrType, Tag1>,
                                  TaggedDoublyLinkedListable<PtrType, Tag2>,
                                  TaggedDoublyLinkedListable<PtrType, Tag3>>;

  using TaggedType1 = HashTable<size_t, PtrType, DoublyLinkedList<PtrType, Tag1>>;
  using TaggedType2 = HashTable<size_t, PtrType, DoublyLinkedList<PtrType, Tag2>>;
  using TaggedType3 = HashTable<size_t, PtrType, DoublyLinkedList<PtrType, Tag3>>;
};

// Negative compilation test which make sure that we cannot try to use a node
// flagged with AllowRemoveFromContainer with a hashtable with doubly linked
// list buckets.  Even though the buckets themselves _could_ do this, the
// HashTable currently tracks its size which makes direct node removal
// impossible.  This could be relaxed if we chose to introduce a version of the
// hashtable which did not maintain an ongoing size count.
TEST(DoublyLinkedHashTableTest, NoRemoveFromContainer) {
  struct Obj : public DoublyLinkedListable<Obj*, NodeOptions::AllowRemoveFromContainer> {
    uintptr_t GetKey() const { return reinterpret_cast<uintptr_t>(this); }
  };
#if TEST_WILL_NOT_COMPILE || 0
  [[maybe_unused]] fbl::HashTable<uintptr_t, Obj*, fbl::DoublyLinkedList<Obj*>> hashtable;
#endif
}

// Small helper which will generate tests for static, dynamic, and
// dynamic-delayed-init versions of the HashTable
#define RUN_HT_ZXTEST(_group, _flavor, _test) \
  RUN_ZXTEST(_group, _flavor, _test)          \
  RUN_ZXTEST(_group, D##_flavor, _test)       \
  RUN_ZXTEST(_group, DID##_flavor, _test)

// clang-format off
// Statically sized hashtable
DEFINE_TEST_OBJECTS(HTDLL);
using UMTE   = DEFINE_TEST_THUNK(Associative, HTDLL, Unmanaged);
using UPDDTE = DEFINE_TEST_THUNK(Associative, HTDLL, UniquePtrDefaultDeleter);
using UPCDTE = DEFINE_TEST_THUNK(Associative, HTDLL, UniquePtrCustomDeleter);
using RPTE   = DEFINE_TEST_THUNK(Associative, HTDLL, RefPtr);

// Dynamically sized hashtable
DEFINE_TEST_OBJECTS(DHTDLL);
using DUMTE   = DEFINE_TEST_THUNK(Associative, DHTDLL, Unmanaged);
using DUPDDTE = DEFINE_TEST_THUNK(Associative, DHTDLL, UniquePtrDefaultDeleter);
using DUPCDTE = DEFINE_TEST_THUNK(Associative, DHTDLL, UniquePtrCustomDeleter);
using DRPTE   = DEFINE_TEST_THUNK(Associative, DHTDLL, RefPtr);

// Dynamically sized hashtable, with delayed initialization
DEFINE_TEST_OBJECTS(DIDHTDLL);
using DIDUMTE   = DEFINE_TEST_THUNK(Associative, DIDHTDLL, Unmanaged);
using DIDUPDDTE = DEFINE_TEST_THUNK(Associative, DIDHTDLL, UniquePtrDefaultDeleter);
using DIDUPCDTE = DEFINE_TEST_THUNK(Associative, DIDHTDLL, UniquePtrCustomDeleter);
using DIDRPTE   = DEFINE_TEST_THUNK(Associative, DIDHTDLL, RefPtr);

// Versions of the test objects which support clear_unsafe.
template <typename PtrType>
using CU_HTDLLTraits = HTDLLTraits<PtrType, fbl::NodeOptions::AllowClearUnsafe>;
DEFINE_TEST_OBJECTS(CU_HTDLL);
using CU_UMTE   = DEFINE_TEST_THUNK(Associative, CU_HTDLL, Unmanaged);
using CU_UPDDTE = DEFINE_TEST_THUNK(Associative, CU_HTDLL, UniquePtrDefaultDeleter);

//////////////////////////////////////////
// General container specific tests.
//////////////////////////////////////////
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     Clear)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   Clear)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   Clear)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     Clear)

#if TEST_WILL_NOT_COMPILE || 0
// Won't compile because node lacks AllowClearUnsafe option.
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     ClearUnsafe)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   ClearUnsafe)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   ClearUnsafe)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     ClearUnsafe)
#endif

#if TEST_WILL_NOT_COMPILE || 0
// Won't compile because pointer type is managed.
RUN_ZXTEST(DoublyLinkedHashTableTest, CU_UPDDTE,  ClearUnsafe)
#endif

RUN_ZXTEST(DoublyLinkedHashTableTest, CU_UMTE,  ClearUnsafe)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     IsEmpty)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   IsEmpty)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   IsEmpty)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     IsEmpty)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     Iterate)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   Iterate)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   Iterate)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     Iterate)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     IterErase)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   IterErase)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   IterErase)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     IterErase)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     DirectErase)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   DirectErase)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   DirectErase)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     DirectErase)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     MakeIterator)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   MakeIterator)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   MakeIterator)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     MakeIterator)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     ReverseIterErase)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   ReverseIterErase)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   ReverseIterErase)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     ReverseIterErase)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     ReverseIterate)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   ReverseIterate)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   ReverseIterate)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     ReverseIterate)

// Hash tables do not support swapping or Rvalue operations (Assignment or
// construction) as doing so would be an O(n) operation (With 'n' == to the
// number of buckets in the hashtable)
#if TEST_WILL_NOT_COMPILE || 0
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     Swap)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   Swap)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   Swap)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     Swap)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     RvalueOps)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   RvalueOps)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   RvalueOps)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     RvalueOps)
#endif

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   Scope)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   Scope)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     Scope)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     TwoContainer)
#if TEST_WILL_NOT_COMPILE || 0
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   TwoContainer)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   TwoContainer)
#endif
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     TwoContainer)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     ThreeContainerHelper)
#if TEST_WILL_NOT_COMPILE || 0
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   ThreeContainerHelper)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   ThreeContainerHelper)
#endif
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     ThreeContainerHelper)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     IterCopyPointer)
#if TEST_WILL_NOT_COMPILE || 0
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   IterCopyPointer)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   IterCopyPointer)
#endif
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     IterCopyPointer)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     EraseIf)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   EraseIf)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   EraseIf)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     EraseIf)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     FindIf)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   FindIf)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   FindIf)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     FindIf)

//////////////////////////////////////////
// Associative container specific tests.
//////////////////////////////////////////
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     InsertByKey)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   InsertByKey)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   InsertByKey)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     InsertByKey)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     FindByKey)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   FindByKey)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   FindByKey)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     FindByKey)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     EraseByKey)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   EraseByKey)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   EraseByKey)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     EraseByKey)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     InsertOrFind)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   InsertOrFind)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   InsertOrFind)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     InsertOrFind)

RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UMTE,     InsertOrReplace)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPDDTE,   InsertOrReplace)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, UPCDTE,   InsertOrReplace)
RUN_HT_ZXTEST(DoublyLinkedHashTableTest, RPTE,     InsertOrReplace)
// clang-format on

}  // namespace intrusive_containers
}  // namespace tests
}  // namespace fbl
