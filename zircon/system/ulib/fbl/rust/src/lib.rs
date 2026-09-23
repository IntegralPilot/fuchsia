// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![no_std]

extern crate self as fbl;

mod array;
mod canary;
mod conditional_select_nospec;
mod confine_array_index;
mod doubly_linked_list;
mod inline_array;
#[cfg(feature = "kernel")]
mod name;
mod opaque_ref_counted;
mod packed_pointer;
mod ptr_traits;
mod recyclable;
mod ref_counted;
mod ref_ptr;
mod ring_buffer;
mod sentinel;
mod singly_linked_list;
mod size_tracker;
mod slab_allocator;
mod string_buffer;
mod tag;
mod unique_ptr;
mod vector;
mod wavl_tree;

#[cfg(test)]
mod intrusive_container_test_support;

pub use array::Array;
pub use canary::{Canary, magic};
pub use conditional_select_nospec::{conditional_select_nospec_eq, conditional_select_nospec_lt};
pub use confine_array_index::confine_array_index;
pub use doubly_linked_list::{
    DoublyLinkedList, DoublyLinkedListContainable, DoublyLinkedListNode, ForwardIterator, Iterator,
    ReverseIterator, remove_from_container,
};
pub use fbl_macros::{
    DoublyLinkedListContainable, Recyclable, SinglyLinkedListContainable, WavlTreeContainable,
    ref_counted,
};
pub use inline_array::InlineArray;
#[cfg(feature = "kernel")]
pub use name::Name;
pub use opaque_ref_counted::{IsOpaqueRefCounted, OpaqueRefCounted, OpaqueRefCountedFacade};
pub use packed_pointer::PackedPointer;
pub use pin_init;
pub use ptr_traits::{ManagedPtr, PtrTraits};
pub use recyclable::{Recyclable, UninitRecyclable};
pub use ref_counted::{HasRefCount, RefCounted};
pub use ref_ptr::RefPtr;
pub use ring_buffer::RingBuffer;
pub use sentinel::*;
pub use singly_linked_list::{SinglyLinkedList, SinglyLinkedListContainable, SinglyLinkedListNode};
pub use size_tracker::{NonTrackingSize, SizeTracker, TrackingSize};
pub use slab_allocator::{
    DEFAULT_SLAB_ALLOCATOR_SLAB_SIZE, InstancedSlabAllocated, IsRawMutex, RawLock, SlabAllocator,
    SlabOrigin, StaticSlabAllocated,
};
pub use string_buffer::StringBuffer;
pub use tag::DefaultObjectTag;
pub use unique_ptr::UniquePtr;
pub use vector::Vector;
pub use wavl_tree::{
    Cursor, CursorMut, WavlTree, WavlTreeAugmentedInvariantObserver,
    WavlTreeAugmentedInvariantObserverTraits, WavlTreeContainable, WavlTreeKeyable, WavlTreeNode,
    WavlTreeObserver,
};

#[doc(hidden)]
pub use zr::static_assert as __static_assert;
