// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_INCLUDE_DEV_IOMMU_BTI_H_
#define ZIRCON_KERNEL_DEV_IOMMU_INCLUDE_DEV_IOMMU_BTI_H_

#include <lib/zx/result.h>
#include <stdint.h>
#include <sys/types.h>
#include <zircon/types.h>

#include <dev/iommu/common.h>
#include <fbl/name.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <vm/pinned_vm_object.h>

class VmObject;  // fwd decl; declared in <vm/vm_object.h>.

namespace iommu {

class Iommu;  // fwd decl; declared in <dev/iommu/iommu.h>.
class Pmt;    // fwd decl; declared in <dev/iommu/pmt.h>.
class Bti;

}  // namespace iommu

extern "C" {
void cpp_bti_recycle(iommu::Bti* bti);
void* cpp_bti_get_ref_counted(const iommu::Bti* bti);
void cpp_bti_release_quarantine(iommu::Bti* bti);
void cpp_bti_on_dispatcher_zero_handles(iommu::Bti* bti);
uint64_t cpp_bti_minimum_contiguity(const iommu::Bti* bti);
uint64_t cpp_bti_aspace_size(const iommu::Bti* bti);
uint64_t cpp_bti_pmo_count(const iommu::Bti* bti);
uint64_t cpp_bti_quarantine_count(const iommu::Bti* bti);
bool cpp_bti_in_fault_state(const iommu::Bti* bti);
uint64_t cpp_bti_bti_id(const iommu::Bti* bti);
zx_status_t cpp_bti_set_name(iommu::Bti* bti, const char* name, size_t len);
zx_status_t cpp_bti_get_name(const iommu::Bti* bti, char out_name[ZX_MAX_NAME_LEN]);
zx_status_t cpp_bti_map(iommu::Bti* bti, PinnedVmObject* pinned_vmo, uint32_t perms,
                        bool require_contiguous,
                        ffi::Uninitialized<fbl::RefPtr<iommu::Pmt>>* pmt_out);
}

namespace iommu {

class Bti : public fbl::RefCounted<Bti> {
 public:
  enum class BtiPageLeakReason {
    // The final handle to a BTI was closed while it still had quarantined PMTs.
    // User-mode no longer has any way to release the quarantine on these PMTs,
    // and they are now permanently leaked.
    BtiOrphanedWithQuarantinedPmts,

    // The final handle to a PMT was closed without it having been formally
    // unpinned, and after its BTI had become orphaned.  Once again, user-mode
    // no longer has any way to release the quarantine on these PMTs, and they
    // are now permanently leaked.
    PmtQuarantinedWhenBtiOrphaned,

    // The final handle to a PMT was closed without it having been unpinned.
    PmtQuarantined,
  };

  Bti(const Bti&) = delete;
  Bti(Bti&&) = delete;
  Bti& operator=(const Bti&) = delete;
  Bti& operator=(Bti&&) = delete;

  // Grant the device identified by |bus_txn_id| access to the range of
  // pages given by [offset, offset + size) in |vmo|. An opaque token that
  // represents the mapping is returned, and this token can be given to |Unmap|
  // or |QueryAddress|.
  //
  // The memory in the given range of |vmo| MUST have been pinned before
  // calling this function, and if this function returns ZX_OK,
  // MUST NOT be unpinned until after Unmap() is called on the returned range.
  //
  // |perms| defines the access permissions, using the IOMMU_FLAG_PERM_*
  // flags.
  //
  // If |size| is no more than |minimum_contiguity()|, this will never return
  // a partial mapping.
  //
  // If |size| req_contig is RequireContiguousMapping::Yes, then the mapping is
  // guaranteed to be contiguous in the BTI device's address space, or an error
  // will be returned.
  //
  // Returns ZX_ERR_INVALID_ARGS if:
  //  |size| is zero.
  //  |offset| is not aligned to kPageSize
  // Returns ZX_ERR_OUT_OF_RANGE if [offset, offset + size) is not a valid range in |vmo|.
  // Returns ZX_ERR_NOT_FOUND if |bus_txn_id| is not valid.
  // Returns ZX_ERR_NO_RESOURCES if the mapping could not be made due to lack
  // of an available address range.
  virtual zx::result<fbl::RefPtr<Pmt>> Map(PinnedVmObject pinned_vmo, uint32_t perms,
                                           RequireContiguousMapping req_contig) = 0;

  virtual void ReleaseQuarantine() = 0;

  // Called when the dispatcher which owns this BTI reaches the end of its
  // user-mode life.  Lets the driver level know in case there is cleanup to be
  // done.
  virtual void OnDispatcherZeroHandles() = 0;

  // Returns the number of bytes that Map() can guarantee, upon success, to find
  // a contiguous address range for.  This function is only returns meaningful
  // values if |IsValidBusTxnId(bus_txn_id)|.
  virtual uint64_t minimum_contiguity() const = 0;

  // Returns the total size of the space the addresses are mapped into.  This
  // function is only returns meaningful values if |IsValidBusTxnId(bus_txn_id)|.
  virtual uint64_t aspace_size() const = 0;

  // The current total number of pinned memory objects, including both active
  // and quarantined objects.
  virtual uint64_t pmo_count() const = 0;

  // For BTI drivers which use quarantines, reports the number of currently
  // quarantined PMTs.
  virtual size_t quarantine_count() const = 0;

  // Returns `true` when the underlying BTI is in a "Fault" state and `false`
  // otherwise.  BTIs generally enter a fault state for one of two reasons.
  //
  // ** Leaked PMTs **
  //
  // If user-mode closes all of the handles to a PMT without making a call to
  // `zx_pmt_unpin`, the PMT has been "leaked".  This is considered to be a
  // violation of the IOMMU/BTI contract as drivers, when they are finished with
  // pinned memory, are supposed to make absolutely certain that their hardware
  // is no longer attempting to access the pinned memory before formally
  // signaling to the kernel that they have stopped their HW access by calling
  // `zx_pmt_unpin`.
  //
  // For things like the Stub IOMMU implementation, this is a big problem.  They
  // don't know if the driver's HW might still be attempting to access the memory, and
  // have no ability to stop it (as a real HW IOMMU would).  So, they are forced
  // to put the memory into a quarantine pool (not returning it to the physical
  // page pool) until the driver is restarted, shots down its hardware, and calls
  // `zx_bti_release_quarantine` to indicate that it is finally safe to return
  // the memory to the physical page pool.
  //
  // Similarly, an ARM SMMU operating in passthru mode has limited tools to deal
  // with a situation like this.  It is able to grant or revoke access to/from a
  // specific BTI, but only in an all or nothing fashion.  The initiator can
  // either access any memory, or no memory.  In a situation like this, the
  // leaked PMT memory _can_ be returned to the pool, but *only* after all
  // ability to access RAM has been revoked from the initiator.
  //
  // In either of these cases, the driver level BTI has entered a "Fault" state.
  // Requests to pin new memory will be denied with "Bad State", and some (or
  // all) of the device's ability to access memory will have been revoked from
  // it depending on the ability of the underlying HW to enforce policy.
  //
  // ** HW Induced Faults **
  //
  // The other reason a BTI might enter into the fault state is because of a
  // transaction fault caused by an invalid access from an initiator.  The HW
  // might have attempted to touch a page of memory which was never pinned (bad
  // address), or it might have attempted to touch a page which was pinned, but
  // with an invalid type of access (eg, it tried to write to a read only page).
  //
  // Stub IOMMUs cannot detect this as they have no hardware enforcement, but
  // actual HW IOMMUs who have translation and enforcement capabilities have a
  // problem in a situation like this.  If they simply print a warning (or
  // ignore the error entirely) and re-enable their fault interrupt, they are
  // almost certainly going to end up in a situation where they are constantly
  // taking interrupts from the disallowed HW accesses.
  //
  // Something has gone very wrong with the HW managed by the user-mode driver,
  // and only user-mode can fix it.  Part of the driver's behavior will be to
  // put the BTI into a "Fault" state, revoking all of the initiator's access to
  // memory and refusing to pin new memory until the driver has taken control of
  // their hardware and released their quarantine.
  //
  virtual bool in_fault_state() const = 0;

  uint64_t bti_id() const { return bti_id_; }

  [[nodiscard]] zx_status_t set_name(const char* name, size_t len) __NONNULL((2)) {
    // The kernel implementation of fbl::Name is protected using an internal
    // spinlock.  No need for any special locks here.
    return name_.set(name, len);
  }

  [[nodiscard]] zx_status_t get_name(char (&out_name)[ZX_MAX_NAME_LEN]) const {
    // The kernel implementation of fbl::Name is protected using an internal
    // spinlock.  No need for any special locks here.
    name_.get(ZX_MAX_NAME_LEN, out_name);
    return ZX_OK;
  }

  void PrintQuarantineWarning(BtiPageLeakReason reason, uint64_t total_leaked_pages,
                              size_t total_leaked_vmos);

 protected:
  friend class fbl::RefPtr<Bti>;
  friend void ::cpp_bti_recycle(iommu::Bti*);

  Bti(uint64_t bti_id) : bti_id_(bti_id) {}
  virtual ~Bti() = default;

  const uint64_t bti_id_;

  // The user-friendly BTI name. For debug purposes only.
  fbl::Name<ZX_MAX_NAME_LEN> name_;
};

}  // namespace iommu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_INCLUDE_DEV_IOMMU_BTI_H_
