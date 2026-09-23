// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_H_

#include <align.h>
#include <lib/fit/function.h>
#include <lib/page/size.h>
#include <lib/user_copy/user_iovec.h>
#include <lib/user_copy/user_ptr.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdint.h>
#include <zircon/syscalls-next.h>
#include <zircon/types.h>

#include <arch/aspace.h>
#include <fbl/array.h>
#include <fbl/canary.h>
#include <fbl/enum_bits.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/intrusive_single_list.h>
#include <fbl/macros.h>
#include <fbl/name.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_counted_upgradeable.h>
#include <fbl/ref_ptr.h>
#include <kernel/lockdep.h>
#include <kernel/mutex.h>
#include <ktl/utility.h>
#include <vm/attribution.h>
#include <vm/page.h>
#include <vm/vm.h>
#include <vm/vm_mapping_observer.h>
#include <vm/vm_page_list.h>

class StreamSizeManager;
class VmMapping;
class MultiPageRequest;
class VmObjectPaged;
class VmObjectPhysical;
class VmAspace;
class VmObject;
class VmHierarchyBase;

class VmObjectChildObserver {
 public:
  // Called anytime a VMO has zero children. This call is synchronized with
  // |VmObject::SetChildObserver|, but is not otherwise synchronized with other VMO operations
  // such as creating additional children. As such it is the users responsibility to synchronize
  // with child creation.
  virtual void OnZeroChild() = 0;
};

// Typesafe enum for resizability arguments.
enum class Resizability {
  Resizable,
  NonResizable,
};

// Argument which specifies the required snapshot semantics for the clone.
enum class SnapshotType {
  // All pages must appear as if a snapshot is performed at the moment of the clone.
  Full,
  // Only pages already modified in the hierarchy need to appear as if a snapshot is performed at
  // the moment of the clone.
  Modified,
  // No pages need to be initially snapshot, but they must have a snapshot taken if written.
  OnWrite,
};

// Argument that specifies the context in which we are supplying pages.
enum class SupplyOptions : uint8_t {
  PagerSupply,
  TransferData,
  PhysicalPageProvider,
};

namespace internal {
struct ChildListTag {};
struct GlobalListTag {};
}  // namespace internal

// Base for opting an object into a deferred deletion strategy that allows for object chains to be
// deleted without causing unbounded recursion due to dropping refptrs in destructors.
template <typename T>
class VmDeferredDeleter {
 public:
  // Calls MaybeDeadTransition and then drops the refptr to the given object by either placing it on
  // the deferred delete list for another thread already running deferred delete to drop, or drops
  // itself.
  // This can be used to avoid unbounded recursion when dropping chained refptrs, as found in
  // vmo parent_ refs.
  static void DoDeferredDelete(fbl::RefPtr<T> object) {
    Guard<CriticalMutex> guard{DeferredDeleteLock::Get()};
    // If a parent has multiple children then it's possible for a given object to already be
    // queued for deletion.
    if (!object->deferred_delete_state_.InContainer()) {
      delete_list_.push_front(ktl::move(object));
    } else {
      // We know a refptr is being held by the container (which we are holding the lock to), so can
      // safely drop the vmo ref.
      object.reset();
    }
    if (!running_delete_) {
      running_delete_ = true;
      while (!delete_list_.is_empty()) {
        guard.CallUnlocked([ptr = delete_list_.pop_front()]() mutable {
          ptr->MaybeDeadTransition();
          ptr.reset();
        });
      }
      running_delete_ = false;
    }
  }

 private:
  struct ListTraits {
    static fbl::SinglyLinkedListNodeState<fbl::RefPtr<T>>& node_state(VmDeferredDeleter<T>& node) {
      return node.deferred_delete_state_;
    }
  };

  // Mutex that protects the global delete list. As this class is templated we actually end up with
  // a single mutex and a global list for each unique object type.
  DECLARE_SINGLETON_CRITICAL_MUTEX(DeferredDeleteLock);

  static fbl::SinglyLinkedListCustomTraits<fbl::RefPtr<T>, ListTraits> delete_list_
      TA_GUARDED(DeferredDeleteLock::Get());

  static bool running_delete_ TA_GUARDED(DeferredDeleteLock::Get());

  using DeferredDeleteState = fbl::SinglyLinkedListNodeState<fbl::RefPtr<T>>;
  DeferredDeleteState deferred_delete_state_ TA_GUARDED(DeferredDeleteLock::Get());
};

template <typename T>
bool VmDeferredDeleter<T>::running_delete_ = false;

template <typename T>
fbl::SinglyLinkedListCustomTraits<fbl::RefPtr<T>, typename VmDeferredDeleter<T>::ListTraits>
    VmDeferredDeleter<T>::delete_list_;

// Cursor to allow for walking global vmo lists without needing to hold the lock protecting them all
// the time. This can be required to enforce order of acquisition with another lock (as in the case
// of |discardable_reclaim_candidates_|), or it can be desirable for performance reasons (as in the
// case of |all_vmos_|).
// In practice at most one cursor is expected to exist, but as the cursor list is global the
// overhead of being generic to support multiple cursors is negligible.
//
// |ObjType| is the type of object being tracked in the list (VmObject, VmCowPages etc).
// |LockType| is the singleton global lock used to protect the list.
// |ListType| is the type of the global vmo list.
// |ListIteratorType| is the iterator for |ListType|.
template <typename ObjType, typename LockType, typename ListType, typename ListIteratorType>
class VmoCursor
    : public fbl::DoublyLinkedListable<VmoCursor<ObjType, LockType, ListType, ListIteratorType>*> {
 public:
  VmoCursor() = delete;

  using CursorsList =
      fbl::DoublyLinkedList<VmoCursor<ObjType, LockType, ListType, ListIteratorType>*>;

  // Constructor takes as arguments the global lock, the global vmo list, and the global list of
  // cursors to add the newly created cursor to. Should be called while holding the global |lock|.
  VmoCursor(LockType* lock, ListType& vmos, CursorsList& cursors)
      : lock_(*lock), vmos_list_(vmos), cursors_list_(cursors) {
    AssertHeld(lock_);

    if (!vmos_list_.is_empty()) {
      vmos_iter_ = vmos_list_.begin();
    } else {
      vmos_iter_ = vmos_list_.end();
    }

    cursors_list_.push_front(this);
  }

  // Destructor removes this cursor from the global list of all cursors.
  ~VmoCursor() TA_REQ(lock_) { cursors_list_.erase(*this); }

  // Advance the cursor and return the next element or nullptr if at the end of the list.
  //
  // Once |Next| has returned nullptr, all subsequent calls will return nullptr.
  //
  // The caller must hold the global |lock_|.
  ObjType* Next() TA_REQ(lock_) {
    if (vmos_iter_ == vmos_list_.end()) {
      return nullptr;
    }

    ObjType* result = &*vmos_iter_;
    vmos_iter_++;
    return result;
  }

  // If the next element is |h|, advance the cursor past it.
  //
  // The caller must hold the global |lock_|.
  void AdvanceIf(const ObjType* h) TA_REQ(lock_) {
    if (vmos_iter_ != vmos_list_.end()) {
      if (&*vmos_iter_ == h) {
        vmos_iter_++;
      }
    }
  }

  // Advances all the cursors in |cursors_list|, calling |AdvanceIf(h)| on each cursor.
  //
  // The caller must hold the global lock protecting the |cursors_list|.
  static void AdvanceCursors(CursorsList& cursors_list, const ObjType* h) {
    for (auto& cursor : cursors_list) {
      AssertHeld(cursor.lock_ref());
      cursor.AdvanceIf(h);
    }
  }

  LockType& lock_ref() TA_RET_CAP(lock_) { return lock_; }

 private:
  VmoCursor(const VmoCursor&) = delete;
  VmoCursor& operator=(const VmoCursor&) = delete;
  VmoCursor(VmoCursor&&) = delete;
  VmoCursor& operator=(VmoCursor&&) = delete;

  LockType& lock_;
  ListType& vmos_list_ TA_GUARDED(lock_);
  CursorsList& cursors_list_ TA_GUARDED(lock_);

  ListIteratorType vmos_iter_ TA_GUARDED(lock_);
};

enum class VmObjectReadWriteOptions : uint8_t {
  None = 0,

  // If set, attempts to read past the end of a VMO will not cause a failure and only copy the
  // existing bytes instead (i.e. the requested length will be trimmed to the actual VMO size).
  TrimLength = (1 << 0),
};
FBL_ENABLE_ENUM_BITS(VmObjectReadWriteOptions)

extern "C" void cpp_vm_object_free(VmObject* vmo);

// The base vm object that holds a range of bytes of data
//
// Can be created without mapping and used as a container of data, or mappable
// into an address space via VmAddressRegion::CreateVmMapping
class VmObject : public fbl::ContainableBaseClasses<
                     fbl::TaggedDoublyLinkedListable<VmObject*, internal::ChildListTag>,
                     fbl::TaggedDoublyLinkedListable<VmObject*, internal::GlobalListTag>>,
                 public fbl::RefCountedUpgradeable<VmObject> {
 public:
  // public API
  virtual zx_status_t Resize(uint64_t size) { return ZX_ERR_NOT_SUPPORTED; }

  virtual Lock<CriticalMutex>* lock() const = 0;
  virtual Lock<CriticalMutex>& lock_ref() const = 0;

  virtual uint64_t size_locked() const TA_REQ(lock()) = 0;
  uint64_t size() const TA_EXCL(lock()) {
    Guard<CriticalMutex> guard{lock()};
    return size_locked();
  }
  virtual uint32_t create_options() const { return 0; }

  // Returns true if the object is backed by RAM and this object can be cast to a VmObjectPaged, if
  // false this is a VmObjectPhysical.
  bool is_paged() const { return options_ & kPaged; }
  // Returns true if the object is backed by a contiguous range of physical
  // memory.
  virtual bool is_contiguous() const { return false; }
  // Returns true if the object size can be changed.
  virtual bool is_resizable() const { return false; }
  // Returns true if the object's pages are discardable by the kernel.
  virtual bool is_discardable() const { return false; }
  // Returns true if the VMO was created via CreatePagerVmo().
  virtual bool is_user_pager_backed() const { return false; }
  // Returns true if the VMO's pages require dirty bit tracking.
  virtual bool is_dirty_tracked() const { return false; }
  // Returns true if streams are supported on the VMO.
  virtual bool is_stream_compatible() const { return false; }
  // Marks the VMO as modified if the VMO tracks modified state (only supported for pager-backed
  // VMOs).
  virtual void mark_modified_locked() TA_REQ(lock()) {}

  using AttributionCounts = struct vm::AttributionCounts;

  // Returns the number of physical bytes currently attributed to a range of this VMO.
  // The range is `[offset_bytes, offset_bytes+len_bytes)`.
  virtual AttributionCounts GetAttributedMemoryInRange(uint64_t offset_bytes,
                                                       uint64_t len_bytes) const {
    return AttributionCounts{};
  }

  // Returns the number of physical bytes currently attributed to this VMO's parent when this VMO
  // is a reference.
  virtual AttributionCounts GetAttributedMemoryInReferenceOwner() const {
    return AttributionCounts{};
  }

  // Returns the number of physical bytes currently attributed to this VMO.
  AttributionCounts GetAttributedMemory() const { return GetAttributedMemoryInRange(0, size()); }

  // find physical pages to back the range of the object
  // May block on user pager requests and must be called without locks held.
  virtual zx_status_t CommitRange(uint64_t offset, uint64_t len) { return ZX_ERR_NOT_SUPPORTED; }

  // Fetches content in the given range of the object. This should operate logically equivalent to
  // reading such that future reads are quicker.
  // May block on user pager requests and must be called without locks held.
  virtual zx_status_t PrefetchRange(uint64_t offset, uint64_t len) = 0;

  // find physical pages to back the range of the object and pin them.
  // |len| must be non-zero. |write| indicates whether the range is being pinned for a write or a
  // read.
  // May block on user pager requests and must be called without locks held.
  virtual zx_status_t CommitRangePinned(uint64_t offset, uint64_t len, bool write) = 0;

  // free a range of the vmo back to the default state
  virtual zx_status_t DecommitRange(uint64_t offset, uint64_t len) { return ZX_ERR_NOT_SUPPORTED; }

  // Zero a range of the VMO. May release physical pages in the process.
  // May block on user pager requests and must be called without locks held.
  virtual zx_status_t ZeroRange(uint64_t offset, uint64_t len) { return ZX_ERR_NOT_SUPPORTED; }

  // Zero a range of the VMO and also untrack it from any kind of dirty tracking. For committed
  // pages, this means that they are released. And any kind of zero markers or intervals that are
  // inserted will not subscribe to dirty tracking.
  virtual zx_status_t ZeroRangeUntracked(uint64_t offset, uint64_t len) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Unpin the given range of the vmo.  This asserts if it tries to unpin a
  // page that is already not pinned (do not expose this function to
  // usermode).
  virtual void Unpin(uint64_t offset, uint64_t len) = 0;

  // Checks if all pages in the provided range are pinned.
  // This is only intended to be used for debugging checks.
  virtual bool DebugIsRangePinned(uint64_t offset, uint64_t len) = 0;

  // Lock a range from being discarded by the kernel. Can fail if the range was already discarded.
  virtual zx_status_t TryLockRange(uint64_t offset, uint64_t len) { return ZX_ERR_NOT_SUPPORTED; }

  // Lock a range from being discarded by the kernel. Guaranteed to succeed. |lock_state_out| is
  // populated with relevant information about the locked and discarded ranges.
  virtual zx_status_t LockRange(uint64_t offset, uint64_t len,
                                zx_vmo_lock_state_t* lock_state_out) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Unlock a range, making it available for the kernel to discard. The range could have been locked
  // either by |TryLockRange| or |LockRange|.
  virtual zx_status_t UnlockRange(uint64_t offset, uint64_t len) { return ZX_ERR_NOT_SUPPORTED; }

  // read/write operators against kernel pointers only
  // May block on user pager requests and must be called without locks held.

  virtual zx_status_t Read(void* ptr, uint64_t offset, size_t len) { return ZX_ERR_NOT_SUPPORTED; }
  virtual zx_status_t Write(const void* ptr, uint64_t offset, size_t len) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // execute lookup_fn on a given range of physical addresses within the vmo. Only pages that are
  // present and writable in this VMO will be enumerated. Any copy-on-write pages in our parent
  // will not be enumerated. The physical addresses given to the lookup_fn should not be retained in
  // any way unless the range has also been pinned by the caller. Offsets provided will be in
  // relation to the object being queried, even if pages are actually from a parent object where
  // this is a slice.
  // Ranges of length zero are considered invalid and will return ZX_ERR_INVALID_ARGS. The lookup_fn
  // can terminate iteration early by returning ZX_ERR_STOP.
  using LookupFunction =
      fit::inline_function<zx_status_t(uint64_t offset, paddr_t pa), 4 * sizeof(void*)>;
  virtual zx_status_t Lookup(uint64_t offset, uint64_t len, LookupFunction lookup_fn) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Attempts to lookup the given range in the VMO. If it exists and is physically contiguous
  // returns the paddr of the start of the range. The offset must be page aligned.
  // Ranges of length zero are considered invalid and will return ZX_ERR_INVALID_ARGS.
  // A null |paddr| may be passed to just check for contiguity.
  virtual zx_status_t LookupContiguous(uint64_t offset, uint64_t len, paddr_t* out_paddr) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // read/write operators against user space pointers only
  //
  // The number of bytes successfully processed is always returned, even upon error. This allows for
  // callers to still pass on this bytes transferred if a particular error was expected.
  //
  // May block on user pager requests and must be called without locks held.
  //
  // Bytes are guaranteed to be transferred in order from low to high offset.
  virtual ktl::pair<zx_status_t, size_t> ReadUser(user_out_ptr<char> ptr, uint64_t offset,
                                                  size_t len, VmObjectReadWriteOptions options) {
    return {ZX_ERR_NOT_SUPPORTED, 0};
  }

  // |OnWriteBytesTransferredCallback| is guaranteed to be called after bytes have been successfully
  // transferred from the user source to the VMO and will be called before the VMO lock is dropped.
  // As a result, operations performed within the callback should not take any other locks or be
  // long-running.
  using OnWriteBytesTransferredCallback = fit::inline_function<void(uint64_t offset, size_t len)>;
  virtual ktl::pair<zx_status_t, size_t> WriteUser(
      user_in_ptr<const char> ptr, uint64_t offset, size_t len, VmObjectReadWriteOptions options,
      const OnWriteBytesTransferredCallback& on_bytes_transferred) {
    return {ZX_ERR_NOT_SUPPORTED, 0};
  }

  // Removes the pages from this vmo in the range [offset, offset + len) and returns
  // them in pages.  This vmo must be a paged vmo with no parent, and it cannot have any
  // pinned pages in the source range. |offset| and |len| must be page aligned.
  virtual zx_status_t TakePages(uint64_t offset, uint64_t len, VmPageSpliceList* pages) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Supplies this vmo with pages for the range [offset, offset + len). If this vmo
  // already has pages in the target range, the |options| field will dictate what happens:
  // If options is SupplyOptions::TransferData, the pages in the target range will be overwritten,
  // Otherwise, the corresponding pages in |pages| will be freed.
  // |offset| and |len| must be page aligned.
  virtual zx_status_t SupplyPages(uint64_t offset, uint64_t len, VmPageSpliceList* pages,
                                  SupplyOptions options) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Indicates that page requests in the range [offset, offset + len) could not be fulfilled.
  // |error_status| specifies the error encountered. |offset| and |len| must be page aligned.
  virtual zx_status_t FailPageRequests(uint64_t offset, uint64_t len, zx_status_t error_status) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Dirties pages in the vmo in the range [offset, offset + len).
  virtual zx_status_t DirtyPages(uint64_t offset, uint64_t len) { return ZX_ERR_NOT_SUPPORTED; }

  using DirtyRangeEnumerateFunction = fit::inline_function<zx_status_t(
      uint64_t range_offset, uint64_t range_len, bool range_is_zero)>;
  // Enumerates dirty ranges in the range [offset, offset + len) in ascending order, updating any
  // relevant VMO internal state required to perform the enumeration, and calls |dirty_range_fn| on
  // each dirty range (spanning [range_offset, range_offset + range_len) where |range_is_zero|
  // indicates whether the range is all zeros). |dirty_range_fn| can return ZX_ERR_NEXT to continue
  // with the enumeration, ZX_ERR_STOP to terminate the enumeration successfully, and any other
  // error code to terminate the enumeration early with that error code.
  virtual zx_status_t EnumerateDirtyRanges(uint64_t offset, uint64_t len,
                                           DirtyRangeEnumerateFunction&& dirty_range_fn) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Query pager relevant VMO stats, e.g. whether the VMO has been modified. If |reset| is set to
  // true, the queried stats are reset as well, potentially affecting the queried state returned by
  // future calls to this function.
  virtual zx_status_t QueryPagerVmoStats(bool reset, zx_pager_vmo_stats_t* stats) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Indicates start of writeback for the range [offset, offset + len). Any Dirty pages in the range
  // are transitioned to AwaitingClean, in preparation for transition to Clean when the writeback is
  // done (See VmCowPages::DirtyState for details of these states). |offset| and |len| must be page
  // aligned. |is_zero_range| specifies whether the caller intends to write back the specified range
  // as zeros.
  virtual zx_status_t WritebackBegin(uint64_t offset, uint64_t len, bool is_zero_range) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Indicates end of writeback for the range [offset, offset + len). Any AwaitingClean pages in the
  // range are transitioned to Clean (See VmCowPages::DirtyState for details of these states).
  // |offset| and |len| must be page aligned.
  virtual zx_status_t WritebackEnd(uint64_t offset, uint64_t len) { return ZX_ERR_NOT_SUPPORTED; }

  enum EvictionHint {
    DontNeed,
    AlwaysNeed,
  };
  // Hint how the specified range is intended to be used, so that the hint can be taken into
  // consideration when reclaiming pages under memory pressure (if applicable).
  // May block on user pager requests and must be called without locks held.
  virtual zx_status_t HintRange(uint64_t offset, uint64_t len, EvictionHint hint) {
    // Hinting trivially succeeds for unsupported VMO types.
    return ZX_OK;
  }

  // Performs any page commits necessary for a VMO with high memory priority over the given range.
  // This method is always safe to call as it will internally check the memory priority status and
  // skip if necessary, so the caller does not need to worry about races with the VMO no longer
  // being high priority.
  // As this may need to acquire the lock even to check the memory priority, if the caller knows
  // they have not caused this VMO to become high priority (i.e. they have not called
  // ChangeHighPriorityCountLocked with a positive value), then calling this should be skipped for
  // performance.
  // This method has no return value as it is entirely best effort and no part of its operation is
  // needed for correctness.
  virtual void CommitHighPriorityPages(uint64_t offset, uint64_t len) TA_EXCL(lock()) {
    // This does nothing by default.
  }

  // Provides the VMO with a user defined queryable byte aligned size. This provided size can then
  // be referenced in other operations, but otherwise has no effect. The VMO will never read or act
  // on this value unless instructed by user operations, and it is therefore the responsibility of
  // the user to ensure any synchronization of the reported value with the operation being
  // requested.
  virtual void SetUserStreamSize(fbl::RefPtr<StreamSizeManager> ssm) = 0;

  // The associated VmObjectDispatcher will set an observer to notify user mode.
  void SetChildObserver(VmObjectChildObserver* child_observer);

  bool is_child_observer_valid() const {
    Guard<Mutex> guard{ChildObserverLock::Get()};
    return child_observer_ != nullptr;
  }

  // Returns a null-terminated name, or the empty string if set_name() has not
  // been called.
  void get_name(char* out_name, size_t len) const TA_EXCL(lock());
  void get_name_locked(char* out_name, size_t len) const TA_REQ(lock());

  // Sets the name of the object. May truncate internally. |len| is the size
  // of the buffer pointed to by |name|.
  zx_status_t set_name(const char* name, size_t len) TA_EXCL(lock());

  // Returns a user ID associated with this VMO, or zero.
  // Used to hold a zircon koid for Dispatcher-wrapped VMOs.
  uint64_t user_id() const;

  // Returns the parent's user_id() if this VMO has a parent,
  // otherwise returns zero.
  virtual uint64_t parent_user_id() const = 0;

  // Sets the value returned by |user_id()|. May only be called once.
  void set_user_id(uint64_t user_id);

  // Returns the maximum possible size of a VMO.
  static size_t max_size() { return MAX_SIZE; }

  virtual void Dump(uint depth, bool verbose) = 0;

  // Returns the number of lookup steps that might be done by operations on this VMO. This would
  // typically be the depth of a parent chain and represent how many parents might need to be
  // traversed to find a page.
  // What this returns is imprecise and not well defined, and so is for debug / diagnostic usage
  // only.
  virtual uint32_t DebugLookupDepth() const { return 0; }

  // perform a cache maintenance operation against the vmo.
  enum class CacheOpType { Invalidate, Clean, CleanInvalidate, Sync };
  virtual zx_status_t CacheOp(uint64_t offset, uint64_t len, CacheOpType type) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  arch_mmu_flags_t GetMappingCachePolicyLocked() const TA_REQ(lock()) { return cache_policy_; }
  arch_mmu_flags_t GetMappingCachePolicy() const {
    Guard<CriticalMutex> guard{lock()};
    return GetMappingCachePolicyLocked();
  }
  virtual zx_status_t SetMappingCachePolicy(arch_mmu_flags_t cache_policy) = 0;

  // create a copy-on-write clone vmo at the page-aligned offset and length
  // note: it's okay to start or extend past the size of the parent
  virtual zx_status_t CreateClone(Resizability resizable, SnapshotType type, uint64_t offset,
                                  uint64_t size, bool copy_name, fbl::RefPtr<VmObject>* child_vmo) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t CreateChildSlice(uint64_t offset, uint64_t size, bool copy_name,
                                       fbl::RefPtr<VmObject>* child_vmo) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // TODO: use a zx::result return instead of multiple out parameters and be consistent with the
  // other Create* methods.
  virtual zx_status_t CreateChildReference(Resizability resizable, uint64_t offset, uint64_t size,
                                           bool copy_name, bool* first_child,
                                           fbl::RefPtr<VmObject>* child_vmo) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Extend this enum when new child types are supported with zx_vmo_create_child().
  // All SNAPSHOT* types are reported as kCowClone, because they all implement CoW semantics, albeit
  // in different ways to provide different guarantees.
  enum ChildType { kNotChild, kCowClone, kSlice, kReference };
  virtual ChildType child_type() const = 0;

  virtual uint64_t HeapAllocationBytes() const { return 0; }

  // Number of times pages have been evicted over the lifetime of this VMO. Evicted counts for any
  // decommit style event such as user pager eviction or zero page merging. One eviction event
  // could count for multiple pages being evicted, if those pages were evicted as a group.
  virtual uint64_t ReclamationEventCount() const { return 0; }

  // Get a pointer to the page structure and/or physical address at the specified offset.
  // valid flags are VMM_PF_FLAG_*.
  //
  // |page_request| must be non-null if any flags in VMM_PF_FLAG_FAULT_MASK are set, unless
  // the caller knows that the vm object is not paged.
  //
  // Returns ZX_ERR_SHOULD_WAIT if the caller should try again after waiting on the
  // PageRequest.
  //
  // Returns ZX_ERR_NEXT if |page_request| supports batching and the current request
  // can be batched. The caller should continue to make successive GetPage requests
  // until this returns ZX_ERR_SHOULD_WAIT. If the caller runs out of requests, it
  // should finalize the request with PageSource::FinalizeRequest.
  virtual zx_status_t GetPage(uint64_t offset, uint pf_flags, MultiPageRequest* page_request,
                              vm_page_t** page, paddr_t* pa) = 0;

  // Helper variant of GetPage that will retry the operation after waiting on a PageRequest if
  // required.
  // Must not be called with any locks held.
  zx_status_t GetPageBlocking(uint64_t offset, uint pf_flags, vm_page_t** page, paddr_t* pa);

  // Attempt to add a new mapping to the mapping list of this object. Can fail with
  // ZX_ERR_OUT_OF_MEMORY.
  zx_status_t AddMappingLocked(VmMapping* r) TA_REQ(lock());
  void RemoveMappingLocked(VmMapping* r) TA_REQ(lock());
  uint32_t num_mappings() const;
  uint32_t num_mappings_locked() const TA_REQ(lock()) { return mapping_list_len_; }

  // Returns true if this VMO is mapped into any VmAspace whose is_user()
  // returns true.
  bool IsMappedByUser() const;

  // Returns an estimate of the number of unique VmAspaces that this object
  // is mapped into.
  uint32_t share_count() const;

  // Adds a child to this VMO and returns true if the dispatcher which matches
  // user_id should be notified about the first child being added.
  bool AddChildLocked(VmObject* child) TA_REQ(ChildListLock::Get());
  bool AddChild(VmObject* child) TA_EXCL(ChildListLock::Get());

  // Removes the child |child| from this VMO and notifies the child observer if the new child count
  // is zero. The |guard| must be this VMO's lock.
  void RemoveChild(VmObject* child, Guard<CriticalMutex>::Adoptable adopt);

  // Drops |c| from the child list without going through the full removal
  // process. ::RemoveChild is probably what you want here.
  void DropChildLocked(VmObject* c) TA_REQ(ChildListLock::Get());

  uint32_t num_children() const;

  // Helper to round to the VMO size multiple (which is kPageSize) without overflowing.
  static zx_status_t RoundSize(uint64_t size, uint64_t* out_size) {
    *out_size = RoundUpPageSize(size);
    if (*out_size < size) {
      return ZX_ERR_OUT_OF_RANGE;
    }
    return ZX_OK;
  }

  // Calls the provided |func(const VmObject&)| on every VMO in the system,
  // from oldest to newest. Stops if |func| returns an error, returning the
  // error value.
  template <typename T>
  static zx_status_t ForEach(T func) {
    Guard<CriticalMutex> guard{AllVmosLock::Get()};
    for (const auto& iter : all_vmos_) {
      zx_status_t s = func(iter);
      if (s != ZX_OK) {
        return s;
      }
    }
    return ZX_OK;
  }

  // Same as ForEach but works with VmObject*.
  template <typename T>
  static zx_status_t ForEachRef(T func) {
    Guard<CriticalMutex> guard{AllVmosLock::Get()};
    for (auto& iter : all_vmos_) {
      zx_status_t s = func(&iter);
      if (s != ZX_OK) {
        return s;
      }
    }
    return ZX_OK;
  }

  // If this VMO has any mappings, return the maximal range spanning all the mappings.
  ktl::optional<ktl::pair<uint64_t, uint64_t>> GetMaximalMappedRange() const;

  // Detaches the underlying page source, if present. Can be called multiple times.
  virtual void DetachSource() {}

  // If this VMO has a backing page source, and that page source has a koid, then it is returned.
  // Otherwise returns a nullopt.
  virtual ktl::optional<zx_koid_t> GetPageSourceKoid() const { return ktl::nullopt; }

  // Different operations that RangeChangeUpdate* can perform against any VmMappings that are found.
  enum class RangeChangeOp {
    Unmap,
    // Specialized case of Unmap where the caller is stating that it knows that any pages that might
    // need to be unmapped are all read instances of the shared zero page.
    UnmapZeroPage,
    // Unmap, harvest accessed bit & update the page queues.
    UnmapAndHarvest,
    RemoveWrite,
    // Unpin is not a 'real' operation in that it does not cause any actions, and is simply used as
    // a mechanism to allow the VmCowPages to trigger a search for any kernel mappings that are
    // still referencing an unpinned page.
    DebugUnpin,
  };
  // Apply the specified operation to all mappings in the given range. The provided offset and len
  // must both be page aligned.
  void RangeChangeUpdateMappingsLocked(uint64_t offset, uint64_t len, RangeChangeOp op)
      TA_REQ(lock());

  // Mappings are keyed in the btree primarily by their offset, however as there can be
  // multiple mappings starting at the same base offset the address of the mapping object is used
  // as a tiebreaker.
  struct MappingKey {
    uint64_t object_offset;
    VmMapping* mapping;
    bool operator<(const MappingKey& a) const {
      if (object_offset != a.object_offset) {
        return object_offset < a.object_offset;
      }
      return mapping < a.mapping;
    }
    bool operator==(const MappingKey& a) const {
      return object_offset == a.object_offset && mapping == a.mapping;
    }
  };
  using MappingTree =
      btree::BTree<MappingKey, VmMapping*, VmMappingObserver, btree::GlobalSlabAllocator,
                   btree::DefaultTypeTraits<VmMapping*>, 512>;

 protected:
  explicit VmObject(uint32_t options);

  // private destructor, only called from refptr
  virtual ~VmObject();
  friend fbl::RefPtr<VmObject>;
  friend void ::cpp_vm_object_free(VmObject*);

  DISALLOW_COPY_ASSIGN_AND_MOVE(VmObject);

  void AddToGlobalList();
  void RemoveFromGlobalList();
  bool InGlobalList() const { return fbl::InContainer<internal::GlobalListTag>(*this); }

  // Performs the requested cache op against a physical address range. The requested physical range
  // must be accessible via the physmap.
  static void CacheOpPhys(paddr_t pa, uint64_t length, CacheOpType op,
                          ArchVmICacheConsistencyManager& cm);

  // magic value
  fbl::Canary<fbl::magic("VMO_")> canary_;

  // Options is extensible by any derived classes. VmObject only defines one bit:
  static constexpr uint32_t kPaged = (1u << 0);
  const uint32_t options_;

  // Cache policy to use for mappings.
  arch_mmu_flags_t cache_policy_ TA_GUARDED(lock()) = ARCH_MMU_FLAG_CACHED;

  // list of every mapping
  MappingTree mapping_list_;

  // list of every child. Usage of this lock happens on VmObject creation/deletion in situations
  // where we are also either manipulating the heap and/or the AllVmosList lock. As such this lock
  // does not end up receiving any contention, due to both being an infrequent operation and already
  // effectively serialized by the aforementioned other global locks.
  // This lock is expected to be acquired after the VMO lock.
  DECLARE_SINGLETON_CRITICAL_MUTEX(ChildListLock);
  fbl::TaggedDoublyLinkedList<VmObject*, internal::ChildListTag> children_list_
      TA_GUARDED(ChildListLock::Get());

  // The user_id_ is semi-const in that it is set once, before the VMO becomes publicly visible, by
  // the dispatcher layer. While the dispatcher setting the ID and querying it is trivially
  // synchronized by the dispatcher, other parts of the VMO code (mostly debug related) may racily
  // inspect this ID before it gets set and so to avoid technical undefined behavior use a relaxed
  // atomic.
  RelaxedAtomic<uint64_t> user_id_ = 0;

  uint32_t mapping_list_len_ TA_GUARDED(lock()) = 0;
  uint32_t children_list_len_ TA_GUARDED(ChildListLock::Get()) = 0;

  // The user-friendly VMO name. For debug purposes only. That
  // is, there is no mechanism to get access to a VMO via this name.
  fbl::Name<ZX_MAX_NAME_LEN, fbl::ThreadSafe::No> name_ TA_GUARDED(lock());

  static constexpr uint64_t MAX_SIZE = VmPageList::MAX_SIZE;
  // Ensure that MAX_SIZE + kPageSize doesn't overflow so no VmObjects
  // need to worry about overflow for loop bounds.
  static_assert(MAX_SIZE <= RoundDownPageSize(UINT64_MAX) - kPageSize);
  static_assert(MAX_SIZE % kPageSize == 0);

 private:
  // Usage of this lock happens on VmObject creation/deletion in situations where we are also either
  // manipulating the heap and/or the AllVmosList lock. As such this lock does not end up receiving
  // any contention, due to both being an infrequent operation and already effectively serialized by
  // the aforementioned other global locks.
  // This lock is expected to be acquired before the VMO lock.
  DECLARE_SINGLETON_MUTEX(ChildObserverLock);

  // This member, if not null, is used to signal the user facing Dispatcher.
  VmObjectChildObserver* child_observer_ TA_GUARDED(ChildObserverLock::Get()) = nullptr;

  using GlobalList = fbl::TaggedDoublyLinkedList<VmObject*, internal::GlobalListTag>;

  DECLARE_SINGLETON_CRITICAL_MUTEX(AllVmosLock);
  static GlobalList all_vmos_ TA_GUARDED(AllVmosLock::Get());
};

namespace internal {
template <typename T>
struct VmObjectTypeTag;
template <>
struct VmObjectTypeTag<VmObjectPaged> {
  static constexpr bool PAGED = true;
};
template <>
struct VmObjectTypeTag<VmObjectPhysical> {
  static constexpr bool PAGED = false;
};
}  // namespace internal

template <typename T>
fbl::RefPtr<T> DownCastVmObject(fbl::RefPtr<VmObject> vmo) {
  if (likely(internal::VmObjectTypeTag<T>::PAGED == vmo->is_paged())) {
    return fbl::RefPtr<T>::Downcast(ktl::move(vmo));
  }
  return nullptr;
}

template <typename T>
inline T* DownCastVmObject(VmObject* vmo) {
  if (likely(internal::VmObjectTypeTag<T>::PAGED == vmo->is_paged())) {
    return static_cast<T*>(vmo);
  }
  return nullptr;
}

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_H_
