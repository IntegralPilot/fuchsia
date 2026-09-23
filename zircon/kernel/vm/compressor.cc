// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/compressor.h"

#include <vm/compression.h>
#include <vm/physmap.h>
#include <vm/pmm.h>

VmCompressor::~VmCompressor() {
  // Should not have an in progress compression.
  ASSERT(IsIdle());
  ASSERT(!IsTempReferenceInUse());
}

zx_status_t VmCompressor::Arm() {
  ASSERT(IsIdle());
  if (!spare_page_) {
    // Allocate a new one. Explicitly do not use delayed allocations.
    zx_status_t status = pmm_alloc_page(0, &spare_page_);
    if (status != ZX_OK) {
      return status;
    }
  }
  state_ = State::Ready;
  return ZX_OK;
}

VmCompressor::CompressedRef VmCompressor::Start(VmCompressor::PageAndMetadata src_page) {
  ASSERT(state_ == State::Ready);
  ASSERT(spare_page_);

  using_temp_reference_ = true;
  page_ = src_page.page;
  temp_reference_metadata_ = src_page.metadata;
  state_ = State::Started;
  return CompressedRef{temp_reference_};
}

void VmCompressor::Compress() {
  ASSERT(state_ == State::Started);
  void* addr = paddr_to_physmap(page_->paddr());
  ASSERT(addr);

  compression_result_ = compressor_.Compress(addr);
  state_ = State::Compressed;
}

VmCompressor::CompressResult VmCompressor::TakeCompressionResult() {
  ASSERT(state_ == State::Compressed);
  ASSERT(compression_result_);

  // Invalidate the result after it is retrieved.
  CompressResult ret = *compression_result_;
  compression_result_.reset();

  // Ensure any pending modifications to the temp reference's metadata are reflected in the final
  // result used by the caller.
  if (IsTempReferenceInUse()) {
    if (const VmPageOrMarker::ReferenceValue* ref =
            ktl::get_if<VmPageOrMarker::ReferenceValue>(&ret)) {
      compressor_.SetMetadata(*ref, temp_reference_metadata_);
    } else if (FailTag* fail = ktl::get_if<FailTag>(&ret)) {
      fail->src_page.page = page_;
      fail->src_page.metadata = temp_reference_metadata_;
    }
  }

  return ret;
}

void VmCompressor::Finalize() {
  ASSERT(state_ == State::Compressed);
  // The temporary reference must no longer be in use.
  ASSERT(!IsTempReferenceInUse());
  ASSERT(page_);
  page_ = nullptr;
  compression_result_.reset();
  state_ = State::Finalized;
}

void VmCompressor::Free(CompressedRef ref) {
  // Forbid returning the temporary reference this way.
  ASSERT(!IsTempReference(ref));
  compressor_.Free(ref);
}

void VmCompressor::ReturnTempReference(CompressedRef ref) {
  ASSERT(IsTempReference(ref));
  ASSERT(using_temp_reference_);
  ASSERT(page_);
  // We must not invalidate page_ here, as this can race with |Compress| which reads from page_. We
  // will invalidate page_ later when the caller reaccquires the VMO lock and invokes |Finalize|.
  using_temp_reference_ = false;
  temp_reference_metadata_ = 0;
}
