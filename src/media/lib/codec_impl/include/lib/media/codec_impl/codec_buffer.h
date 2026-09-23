// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_CODEC_BUFFER_H_
#define SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_CODEC_BUFFER_H_

#include <fuchsia/media/cpp/fidl.h>
#include <lib/async/cpp/wait.h>
#include <lib/fit/defer.h>
#include <lib/media/codec_impl/codec_port.h>
#include <lib/media/codec_impl/codec_vmo_range.h>

#include <memory>

#include <fbl/macros.h>

class CodecImpl;
class CodecBufferForTest;

// Deprecated; please sub-class CodecAdapterFrameBase and/or
// CodecAdapterBufferContextBase instead.
//
// Core codec representation of a video frame.  Different core codecs may have
// very different implementations of this.
//
// TODO(dustingreen): Have this be a base class that's defined by the CodecImpl
// source_set, and have amlogic-video VideoFrame derive from that base class.
// We're doing this via soft transition to CodecAdapterFrameBase and/or
// CodecAdapterBufferContextBase. This change is because sub-classing is more
// idiomatic and because we don't want to make it seem like a CodecAdapter must
// implement this class. Also, sub-classes can be searched for more efficiently.
//
// Regardless of codec, these will be managed by shared_ptr<>, because for video
// decoder reference frames, shared_ptr<> makes sense.
//
// Deprecated.
struct VideoFrame;

class ScopedLock;

// The CodecAdapterFrameBase is a base class that a CodecAdapter can
// optionally sub-class for use with CodecBuffer.SetCodecAdapterFrame.
//
// Regardless of codec, these will be managed by shared_ptr<>, because for video
// decoder reference frames, shared_ptr<> makes sense.
class CodecAdapterFrameBase {
 public:
  virtual ~CodecAdapterFrameBase() = default;
};
// The CodecAdapterBufferContextBase is a base class which can be optionally
// sub-classed by a CodecAdapter for use with SetCodecAdapterBufferContext.
//
// Regardless of codec, these will be managed by shared_ptr<>, because some
// CodecAdapter implementations may want that.
class CodecAdapterBufferContextBase {
 public:
  virtual ~CodecAdapterBufferContextBase() = default;
};

// This object is a buffer both known to CodecImpl and exposed to the
// CodecAdapter with the buffer mapped (as appropriate) and pinned (as
// appropriate).
//
// The CodecAdapter can choose to use the mapping or pin that the CodecBuffer
// provides, but this only lasts as long as the CodecBuffer instance.
//
// The CodecBuffer instance remains allocated until CoreCodecRemoveBuffer
// specifying the instance or CoreCodecEnsureBuffersNotConfigured (impliciatly
// specifying all configured CodecBuffer(s)) is called, and the CodecAdapter has
// closed all handles and mappings derived from GetChildVmo().
//
// The CodecAdapter should not make any handles, mappings, or pins derived from
// vmo() (including duplicates and child VMOs). These won't keep the CodecBuffer
// allocated. Instead, the CodecAdapter can use GetChildVmo(); the returned
// handle (and handles/mappings/pins derived from that) will keep the
// CodecBuffer allocated. Ensuring that the CodecBuffer pointer remains valid
// until the CodecAdapter has fully closed/unmapped/unpinned all its
// buffer-derived stuff reduces the chance of a CodecAdapter accessing a
// no-longer-allocated CodecBuffer.
//
// The const-ness of a CodecBuffer refers to the fields of the CodecBuffer
// instance, not to the data pointed at by buffer_base(). The context accessors
// re. VideoFrame, CodecAdapterFrameBase, CodecAdapterBufferContextBase are
// marked mutable, as those are for use by the CodecAdapter whenever is
// convenient to the CodecAdapter (as long as CodecBuffer is known still
// allocated).
class CodecBuffer {
 public:
  // This is the same value as buffer_lifetime_ordinal in StreamProcessor FIDL.
  uint64_t lifetime_ordinal() const { return buffer_info_.lifetime_ordinal; }

  // This matches the buffer_index field of fuchsia::media::Packet when the packet refers to this
  // buffer.
  uint32_t index() const { return buffer_info_.index; }

  CodecPort port() const { return buffer_info_.port; }

  bool is_secure() const { return buffer_info_.is_secure; }
  // The vaddr of the start of the mapped VMO for this buffer.
  //
  // This will return nullptr if there's no VMO mapping because CPU access isn't
  // possible.  In that case the vaddr data pointer passed around regarding a
  // packet will be an offset into the buffer / VMO, and is only meaningful
  // with respect to a CodecBuffer that's also passed alongside.
  uint8_t* base() const;

  bool is_known_contiguous() const;

  // This will ZX_PANIC() if the buffer hasn't been pinned yet, or if !is_known_contiguous().
  zx_paddr_t physical_base() const;

  size_t size() const;

  // This VMO is owned by CodecBuffer, but can be used temporarily (in a
  // non-owned fashion) to get VMO info or similar.
  //
  // If the CodecAdapter wants to hold a VMO handle, a mapping, or a pin of its
  // own, the CodecAdapter should start with GetChildVmo() instead of vmo().
  // Holding a handle, mapping, or pin based on vmo() won't keep the CodecBuffer
  // instance allocated (but it will prevent the underlying memory from being
  // reused). Instead, by using GetChildVmo(), the CodecBuffer instance stays
  // allocated until the CodecAdapter has closed all its GetChildVmo-derived
  // handles to the buffer and unmapped all its GetChildVmo-derived mappings to
  // the buffer (and all pins). This helps avoid potential use-after-free of the
  // CodecBuffer and can make debugging / diagnosing simpler.
  const zx::vmo& vmo() const;
  // Up until the first buffer-relevant call to CoreCodecRemoveBuffer or
  // CoreCodecEnsureBuffersNotConfigured, the CodecBuffer will remain allocated
  // even if the CodecAdapter is not holding a handle obtained from
  // GetChildVmo() or anything derived from such a handle. If the CodecAdapter
  // wants/needs the CodecBuffer to remain allocated beyond said call, the
  // CodecAdapter must hold at least one handle obtained from GetChildVmo(), or
  // something derived from such a handle that keeps the VMO alive.
  //
  // If a CodecAdapter does not keep any vmo(s) from GetChildVmo(), including
  // duplicates, mappings, or pins, that CodecAdapter must ensure that no buffer
  // usage of any form will occur beyond the first relevant
  // CoreCodecRemoveBuffer or CoreCodecEnsureBuffersNotConfigured.
  //
  // If a CodecAdapter makes a separate pin of its own using GetChildVmo() (or a
  // duplicate and/or descendent), the CodecAdapter can continue doing DMA
  // to/from the buffer until its own unpin.
  //
  // The CodecAdapter should not rely on the returned VMO to be derived from
  // vmo(), though the returned vmo does refer to the same underlying buffer.
  zx::vmo GetChildVmo() const;

  // The offset within the main VMO where data of this CodecBuffer starts.  The vmo_offset() is not
  // required to be divisible by page size.
  uint64_t vmo_offset() const;

  // Deprecated. See SetCodecAdapterFrame and/or
  // SetCodecAdapterBufferContext instead.
  void SetVideoFrame(std::weak_ptr<VideoFrame> video_frame) const;
  std::weak_ptr<VideoFrame> video_frame() const;
  // The CodecAdapter can set a CodecAdapterFrameBase to avoid needing to
  // look up a CodecBuffer* later to find the relevant "frame" (from the
  // CodecAdapter's point of view).
  void SetCodecAdapterFrame(std::weak_ptr<CodecAdapterFrameBase> video_frame) const;
  std::weak_ptr<CodecAdapterFrameBase> codec_adapter_frame() const;
  // The CodecAdapter can set a CodecAdapterBufferContextBase to avoid needing
  // to look up a CodecBuffer* later to find the relevant "buffer context" (from
  // the CodecAdapter's point of view).
  void SetCodecAdapterBufferContext(
      std::weak_ptr<CodecAdapterBufferContextBase> buffer_context) const;
  std::weak_ptr<CodecAdapterBufferContextBase> codec_adapter_buffer_context() const;

  // Unpin is automatic during ~CodecBuffer.
  zx_status_t Pin();
  bool is_pinned() const;

  void CacheFlush(uint32_t flush_offset, uint32_t length) const;
  void CacheFlushAndInvalidate(uint32_t flush_offset, uint32_t length) const;

  // This returns true from just before CoreCodecRemoveBuffer or CoreCodecEnsureBuffersNotConfigured
  // is called, until destruction.
  //
  // A CodecAdapter may choose to ignore this accessor (relying only on CoreCodecRemoveBuffer and/or
  // CoreCodecEnsureBuffersNotConfigured).
  //
  // If a CodecAdapter does not support dynamic buffers, this bool isn't particularly interesting
  // other than possibly for some debug asserts.
  //
  // If a CodecAdapter does support dynamic buffers, this bool being true means it's appropriate to
  // close all handles derived from GetChildVmo() (and any derived from vmo(), though creating those
  // isn't recommended in the first place). Upon parent_vmo_ seeing ZX_VMO_ZERO_CHILDREN, the
  // CodecBuffer will be deleted, so the CodecAdapter should take care to remove all dependence on
  // the CodecBuffer* before closing the last handle.
  //
  // For output buffers, the CodecAdapter can look at is_remove_pending() just before it would
  // otherwise be putting an output buffer back on its free list, for buffers that weren't already
  // free when CoreCodecRemoveBuffer or mid-stream CoreCodecEnsureBuffersNotConfigured was called.
  //
  // For input buffers, this will only become true if the buffer is not with the CodecAdapter and
  // won't be referenced by any subsequent input packet sent to the CodecAdapter. So for most
  // CodecAdapter(s), input buffer removal can happen during the call to CoreCodecRemoveBuffer or
  // CoreCodecEnsureBuffersNotConfigured, without needing to call is_remove_pending() (may still be
  // useful for asserts).
  //
  // This is public to avoid the CodecAdapter needing to keep a redundant bool in VideoFrame,
  // CodecAdapterFrame, or CodecAdapterBufferContext.
  bool is_remove_pending() const { return is_remove_pending_; }

  bool was_ever_added_to_core_codec() const { return was_ever_added_to_core_codec_; }
  void SetWasEverAddedToCoreCodec() {
    ZX_DEBUG_ASSERT(!was_ever_added_to_core_codec_);
    was_ever_added_to_core_codec_ = true;
  }

  void AssertMagic() const { ZX_ASSERT(magic_ == kMagic); }

  // The rest is protected for the benefit of tests. Sub-classes outside tests
  // are not supported.
 protected:
  friend class CodecImpl;
  friend class std::unique_ptr<CodecBuffer>;
  friend struct std::default_delete<CodecBuffer>;
  friend class CodecBufferForTest;
  friend class CodecPacket;

  using DoDelete = fit::callback<void(CodecBuffer* buffer)>;

  // Helper struct for encapsulating the properties of a Buffer
  struct Info {
    CodecPort port = kFirstPort;
    // aka buffer_lifetime_ordinal
    uint64_t lifetime_ordinal;
    // For non-dynamic buffers these values will be from 0..num_buffers-1. For
    // dynamic buffers these values are only required to be unique from
    // AddBuffer until the buffer is done being removed (via RemoveBuffer or an
    // action that removes all buffers).
    //
    // aka buffer_index
    uint32_t index;
    bool is_secure;
  };

  CodecBuffer(CodecImpl* parent, Info buffer_info, CodecVmoRange vmo_range);
  ~CodecBuffer();

  // Separate from constructor because some tests don't need this.
  void SetDoDelete(DoDelete do_delete);

  // Maps a page-aligned portion of the VMO including vmo_usable_start to vmo_usable_start +
  // vmo_usable_size.
  bool Map();

  // FakeMap() exists because most CodecAdapter(s) expect to have a CodecBuffer::base() and "data"
  // vaddr(s) within the buffer, even when buffers are secure.  IIUC, mapping to secure buffer +
  // cached policy on the VMO + speculative execution + aarch64 potentially would
  // randomly/spuriously fault even if the code never actually touched the mapping.  So instead of
  // mapping, we use a VMAR to reserve some vaddr space, but without any VMOs backing the VMAR, so
  // any actual accesses to any part of the VMAR will fault, and any speculative accesses won't
  // spuriuously/randomly fault.  We only need one VMAR across all buffers of a BufferCollection, so
  // CodecImpl passes in the vaddr of that VMAR here.  The fake_map_addr is in keeping with trying
  // to minimize the differences between non-secure and secure cases; it's just that we can't have
  // an actual mapping to the secure physical pages at the moment.  In addition, by not actually
  // mapping buffers we can't touch anyway, we presumably save some page table resources.
  //
  // The fake_map_addr is used as the a page-aligned base address for a fake mapping. Client code
  // must not touch memory at buffer_base() when a fake mapping is in effect, but if client code
  // does anyway, that thread will cleanly fault (not get stuck reading, not seem to let a write
  // happen, not be reading/writing any arbitrary other data in the process's address space).  The
  // fake_map_addr vaddr region is guaranteed to have enough vaddr pages to accommodate
  // vmo_usable_start % PAGE_SIZE + vmo_usable_size (so that an access within the bounds of the
  // buffer will reliably fault cleanly).
  void FakeMap(uint8_t* fake_map_addr);

  void CacheFlushInternal(uint32_t flush_offset, uint32_t length, bool also_invalidate) const;

  void BeginWaitForZeroChildren(async_dispatcher_t* dispatcher);

  void OnZeroChildren(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
                      const zx_packet_signal_t* signal);

  const zx::vmo& original_vmo() const { return vmo_range_.vmo(); }

  fit::function<void(ScopedLock&)> TakePendingRemoveCompletion();

  class KeepAlive {
   public:
    KeepAlive(const KeepAlive& to_copy) = delete;
    KeepAlive& operator=(const KeepAlive& to_copy) = delete;
    KeepAlive(KeepAlive&& to_move) = default;
    KeepAlive& operator=(KeepAlive&& to_move) = default;

   private:
    // CodecImpl doesn't need to know the details of this class, just that having the instance means
    // a RemoveBuffer won't complete yet. In contrast to std::shared_ptr, this is move-only to force
    // all KeepAlive instances to originate directly from GetKeepAlive().
    friend class CodecBuffer;

    KeepAlive(std::shared_ptr<zx::vmo> kept_alive) : kept_alive_(kept_alive) {}

    // This is a shared_ptr to same zx::vmo as until_remove_started_child_vmo_.
    std::shared_ptr<zx::vmo> kept_alive_;
  };
  KeepAlive GetKeepAlive() const;

  // The parent CodecImpl instance.  Just so we can call parent_->Fail().
  // The parent_ CodecImpl out-lives the CodecImpl::Buffer.
  CodecImpl* parent_;

  Info buffer_info_;

  // Set from just before CoreCodecRemoveBuffer or CoreCodecEnsureBuffersNotConfigured. Stays set
  // until CodecBuffer destruction, at which point the deferred_action runs to complete the
  // StreamProcessor.RemoveBuffer. This member must be before any of the zx::vmo fields so this will
  // run after those handles are closed.
  //
  // A client doing more than one RemoveBuffer on the same buffer is a protocol error enforced by
  // checking whether this field is already set.
  fit::function<void(ScopedLock&)> pending_remove_completion_;

  // This owns the vmo handle originally used to create this CodecBuffer.
  CodecVmoRange vmo_range_;
  // This is a child of vmo_range_.vmo that's returned from vmo(). This is part of enforcing that
  // the CodecAdapter can't assume that vmo(s) returned from GetChildVmo() are derived from vmo().
  //
  // Handles/mappings/pins derived from vmo_ don't keep CodecBuffer allocated.
  zx::vmo vmo_;
  // This is the parent_vmo_ on which CodecImpl waits for ZX_VMO_ZERO_CHILDREN. This is in turn a
  // child of vmo_range_.vmo. This is not handed out to the CodecAdapter.
  //
  // Child VMOs of this VMO are handled out to the CodecAdapter via GetChildVmo.
  zx::vmo parent_vmo_;
  // This is a child of parent_vmo_ that's closed by CodecImpl just after the first relevant
  // CoreCodecRemoveBuffer or CoreCodecEnsureBuffersNotConfigured. This handle being open is how the
  // CodecImpl ensures that CodecBuffer will remain allocated until said call even if the
  // CodecAdapter is not keeping any VMO handle/mapping of its own. This is also how CodecImpl knows
  // that ZX_VMO_ZERO_CHILDREN won't be signalled too soon.
  //
  // This is a std::shared_ptr to allow items in the output queue to prevent completion of
  // RemoveBuffer until after items in the output queue are sent.
  std::shared_ptr<zx::vmo> until_remove_started_child_vmo_;

  std::optional<async::WaitMethod<CodecBuffer, &CodecBuffer::OnZeroChildren>> zero_children_wait_;
  DoDelete do_delete_;

  // Deprecated; CodecAdapter impls should instead implement a sub-class of
  // CodecAdapterFrameBase and/or CodecAdapterBufferContextBase; see codec_adapter_frame_
  // and codec_adapter_buffer_context_ below.
  //
  // Mutable only in the sense that it's set later than the constructor.  The association does not
  // switch to a different VideoFrame once set.
  mutable std::weak_ptr<VideoFrame> deprecated_video_frame_;
  // This is mutable to allow the overall instance to be passed around as const
  // (no other justification). Some CodecAdapter impls may choose to set this once and then never
  // change it, but that's not a required constraint on the CodecAdapter impl.
  mutable std::weak_ptr<CodecAdapterFrameBase> codec_adapter_frame_;
  // This is mutable to allow the overall instance to be passed around as const
  // (no other justification). Some CodecAdapter impls may choose to set this once and then never
  // change it, but that's not a required constraint on the CodecAdapter impl.
  mutable std::weak_ptr<CodecAdapterBufferContextBase> codec_adapter_buffer_context_;

  // This accounts for vmo_usable_start.  The content bytes are not part of
  // a Buffer instance from a const-ness point of view.
  uint8_t* buffer_base_ = nullptr;
  // This remains false if fake_map_addr is passed to Map().  Not to be exposed to clients of
  // CodecBuffer.
  bool is_mapped_ = false;

  zx::pmt pinned_;
  // We use is_known_contiguous_ to check that physical_base() is only called after Pin() succeeded.
  // We check during Pin() that the VMO is really contiguous.
  bool is_known_contiguous_ = false;
  // This includes the low-order bits of the vmo_usable_start offset, so this is not necessarily
  // page-aligned.
  zx_paddr_t contiguous_paddr_base_ = {};

  // This is how we track whether a buffer already has an output packet in flight with the client
  // referencing this buffer. This is allowed to be greater than 1 only if
  // is_enable_same_output_buffer_concurrently_in_flight_ is true.
  mutable uint32_t output_in_flight_count_ = 0;

  // For optional use by CodecAdapter. Once this becomes true it remains true until destruction.
  // This tracks whether CoreCodecRemoveBuffer or CoreCodecEnsureBuffersNotConfigured will be called
  // or has already been called applicable to this buffer.
  bool is_remove_pending_ = false;

  bool was_ever_added_to_core_codec_ = false;

  static constexpr uint32_t kMagic = 0xCB0150A;
  uint32_t magic_ = kMagic;

  CodecBuffer(const CodecBuffer& to_copy) = delete;
  CodecBuffer& operator=(const CodecBuffer& to_copy) = delete;
  CodecBuffer(CodecBuffer&& to_move) = delete;
  CodecBuffer& operator=(CodecBuffer&& to_move) = delete;
};

#endif  // SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_CODEC_BUFFER_H_
