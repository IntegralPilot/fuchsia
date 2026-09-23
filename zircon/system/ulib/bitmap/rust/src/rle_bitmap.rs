// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bitmap::{Bitmap, GetResult};

use pin_init::{PinInit, pin_data, pin_init};
use zx_status::Status;

/// An element representing a run of set bits in an `RleBitmapBase`.
#[derive(Default, Debug, fbl::DoublyLinkedListContainable, fbl::Recyclable)]
#[repr(C)]
pub struct Element<T> {
    /// The start offset of this run of 1-bits.
    pub bitoff: T,
    /// The number of 1-bits in this run.
    pub bitlen: T,
    #[dll_node]
    node: fbl::DoublyLinkedListNode<Self>,
}

impl<T> Element<T> {
    /// Create a new Element with the given range.
    pub fn new(bitoff: T, bitlen: T) -> Self {
        Self { bitoff, bitlen, node: fbl::DoublyLinkedListNode::new() }
    }
}

impl<T: Copy + core::ops::Add<Output = T>> Element<T> {
    /// Returns the (inclusive) start of this run of 1-bits.
    pub fn start(&self) -> T {
        self.bitoff
    }

    /// Returns the (exclusive) end of this run of 1-bits.
    pub fn end(&self) -> T {
        self.bitoff + self.bitlen
    }
}

pub type ElementPtr<T> = fbl::UniquePtr<Element<T>>;
pub type FreeList<T> = fbl::DoublyLinkedList<ElementPtr<T>>;

/// A run-length encoded bitmap.
#[pin_data]
#[derive(Debug)]
pub struct RleBitmapBase<T> {
    #[pin]
    elems: fbl::DoublyLinkedList<ElementPtr<T>>,
    num_elems: usize,
    num_bits: T,
}

fn allocate_element<T: Default>(
    free_list: Option<&mut FreeList<T>>,
) -> Result<ElementPtr<T>, Status> {
    if let Some(fl) = free_list {
        if let Some(elem) = fl.pop_front() {
            return Ok(elem);
        }
        return Err(Status::NO_MEMORY);
    }
    let elem = Element::default();
    fbl::UniquePtr::try_new(elem).map_err(|_| Status::NO_MEMORY)
}

fn release_element<T>(free_list: Option<&mut FreeList<T>>, elem: ElementPtr<T>) {
    if let Some(fl) = free_list {
        fl.push_back(elem);
    }
}

impl<T> RleBitmapBase<T> {
    /// Returns an iterator over the elements of this bitmap.
    pub fn iter(&self) -> fbl::Iterator<'_, ElementPtr<T>> {
        self.elems.iter()
    }
}

impl<T> RleBitmapBase<T>
where
    T: Copy
        + Eq
        + Ord
        + Default
        + core::ops::Add<Output = T>
        + core::ops::Sub<Output = T>
        + From<u8>,
{
    /// Create a new, empty run-length encoded bitmap.
    ///
    /// Since the underlying intrusive list must be pinned in memory, this returns
    /// an initializer that must be pinned (e.g. using `pin_init::pin_init!`).
    pub fn new() -> impl PinInit<Self, core::convert::Infallible> {
        pin_init!(Self {
            elems <- fbl::DoublyLinkedList::new(),
            num_elems: 0,
            num_bits: T::default(),
        })
    }

    /// Returns the current number of ranges (runs of set bits) in the bitmap.
    pub fn num_ranges(&self) -> usize {
        self.num_elems
    }

    /// Returns the current total number of set bits in the bitmap.
    pub fn num_bits(&self) -> T {
        self.num_bits
    }

    /// Sets all bits in the range `[bitoff, bitmax)`.
    ///
    /// Only fails if `bitmax < bitoff` or if an allocation is needed and `free_list`
    /// does not contain one.
    ///
    /// `free_list` is a list of usable allocations. If an allocation is needed,
    /// it will be drawn from it. This function is guaranteed to need at most
    /// one allocation. If any nodes need to be deleted, they will be appended
    /// to `free_list`.
    pub fn set_no_alloc(
        &mut self,
        bitoff: T,
        bitmax: T,
        free_list: &mut FreeList<T>,
    ) -> Result<(), Status> {
        self.set_internal(bitoff, bitmax, Some(free_list))
    }

    /// Clears all bits in the range `[bitoff, bitmax)`.
    ///
    /// Only fails if `bitmax < bitoff` or if an allocation is needed and `free_list`
    /// does not contain one.
    ///
    /// `free_list` is a list of usable allocations. If an allocation is needed,
    /// it will be drawn from it. This function is guaranteed to need at most
    /// one allocation. If any nodes need to be deleted, they will be appended
    /// to `free_list`.
    pub fn clear_no_alloc(
        &mut self,
        bitoff: T,
        bitmax: T,
        free_list: &mut FreeList<T>,
    ) -> Result<(), Status> {
        self.clear_internal(bitoff, bitmax, Some(free_list))
    }

    fn set_internal(
        &mut self,
        bitoff: T,
        bitmax: T,
        mut free_list: Option<&mut FreeList<T>>,
    ) -> Result<(), Status> {
        if bitmax < bitoff {
            return Err(Status::INVALID_ARGS);
        }
        let bitlen = bitmax - bitoff;
        if bitlen == T::default() {
            return Ok(());
        }

        let free_list_ref = free_list.as_mut().map(|f| &mut **f);
        let mut new_elem = allocate_element(free_list_ref)?;
        self.num_elems += 1;
        new_elem.bitoff = bitoff;
        new_elem.bitlen = bitlen;

        let mut cursor = self.elems.cursor_front_mut();
        loop {
            let (e_bitoff, e_bitlen) = match cursor.get() {
                Some(e) => (e.bitoff, e.bitlen),
                None => break,
            };
            if e_bitoff + e_bitlen >= bitoff {
                break;
            }
            cursor.move_next();
        }

        cursor.insert_before(new_elem);
        self.num_bits = self.num_bits + bitlen;

        let mut has_successor = false;
        let mut successor_bitoff = T::default();
        if let Some(succ) = cursor.get() {
            has_successor = true;
            successor_bitoff = succ.bitoff;
        }

        cursor.move_prev();
        let mut elem_bitoff = cursor.get().unwrap().bitoff;
        let mut elem_bitlen = cursor.get().unwrap().bitlen;

        if has_successor && elem_bitoff >= successor_bitoff {
            let diff = elem_bitoff - successor_bitoff;
            elem_bitlen = elem_bitlen + diff;
            elem_bitoff = successor_bitoff;
            let elem = cursor.get_mut().unwrap();
            elem.bitoff = elem_bitoff;
            elem.bitlen = elem_bitlen;
            self.num_bits = self.num_bits + diff;
        }

        cursor.move_next();
        let mut max = elem_bitoff + elem_bitlen;
        loop {
            let (succ_bitoff, succ_bitlen) = match cursor.get() {
                Some(s) => (s.bitoff, s.bitlen),
                None => break,
            };
            if succ_bitoff > max {
                break;
            }
            let succ_max = succ_bitoff + succ_bitlen;
            max = core::cmp::max(max, succ_max);
            self.num_bits = self.num_bits - elem_bitlen - succ_bitlen + (max - elem_bitoff);
            elem_bitlen = max - elem_bitoff;
            let erased = cursor.erase().unwrap();
            self.num_elems -= 1;
            let free_list_ref = free_list.as_mut().map(|f| &mut **f);
            release_element(free_list_ref, erased);
        }

        cursor.move_prev();
        cursor.get_mut().unwrap().bitlen = elem_bitlen;
        Ok(())
    }

    fn clear_internal(
        &mut self,
        bitoff: T,
        bitmax: T,
        mut free_list: Option<&mut FreeList<T>>,
    ) -> Result<(), Status> {
        if bitmax < bitoff {
            return Err(Status::INVALID_ARGS);
        }
        let bitlen = bitmax - bitoff;
        if bitlen == T::default() {
            return Ok(());
        }

        let mut cursor = self.elems.cursor_front_mut();
        loop {
            let (elem_bitoff, elem_bitlen) = match cursor.get() {
                Some(e) => (e.bitoff, e.bitlen),
                None => break,
            };

            if elem_bitoff + elem_bitlen < bitoff {
                cursor.move_next();
                continue;
            }
            if bitmax < elem_bitoff {
                break;
            }
            if elem_bitoff < bitoff {
                if elem_bitoff + elem_bitlen <= bitmax {
                    let new_bitlen = bitoff - elem_bitoff;
                    self.num_bits = self.num_bits - (elem_bitlen - new_bitlen);
                    cursor.get_mut().unwrap().bitlen = new_bitlen;
                    cursor.move_next();
                    continue;
                }
                let free_list_ref = free_list.as_mut().map(|f| &mut **f);
                let mut new_elem = allocate_element(free_list_ref)?;
                self.num_elems += 1;
                new_elem.bitoff = bitmax;
                new_elem.bitlen = elem_bitoff + elem_bitlen - bitmax;
                cursor.insert_after(new_elem);
                let new_bitlen = bitoff - elem_bitoff;
                self.num_bits = self.num_bits - (bitmax - bitoff);
                cursor.get_mut().unwrap().bitlen = new_bitlen;
                break;
            }
            if bitmax < elem_bitoff + elem_bitlen {
                self.num_bits = self.num_bits - (bitmax - elem_bitoff);
                let elem_mut = cursor.get_mut().unwrap();
                elem_mut.bitlen = elem_mut.bitoff + elem_mut.bitlen - bitmax;
                elem_mut.bitoff = bitmax;
                break;
            }
            self.num_bits = self.num_bits - elem_bitlen;
            self.num_elems -= 1;
            let erased = cursor.erase().unwrap();
            let free_list_ref = free_list.as_mut().map(|f| &mut **f);
            release_element(free_list_ref, erased);
        }
        Ok(())
    }
}

impl<T> Bitmap<T> for RleBitmapBase<T>
where
    T: Copy
        + Eq
        + Ord
        + Default
        + core::ops::Add<Output = T>
        + core::ops::Sub<Output = T>
        + From<u8>,
{
    fn find(&self, is_set: bool, mut bitoff: T, bitmax: T, run_len: T) -> Result<T, Status> {
        for elem in self.elems.iter() {
            if bitoff >= elem.end() {
                continue;
            }
            if bitmax - bitoff < run_len {
                return Err(Status::NO_RESOURCES);
            }
            let elem_min = core::cmp::max(bitoff, elem.bitoff);
            let elem_max = core::cmp::min(bitmax, elem.end());
            if is_set && elem_max > elem_min && elem_max - elem_min >= run_len {
                return Ok(elem_min);
            }
            if !is_set && bitoff < elem.bitoff && elem.bitoff - bitoff >= run_len {
                return Ok(bitoff);
            }
            if bitmax < elem.end() {
                return Err(Status::NO_RESOURCES);
            }
            bitoff = elem.end();
        }
        if !is_set && bitmax - bitoff >= run_len {
            return Ok(bitoff);
        }
        Err(Status::NO_RESOURCES)
    }

    fn get(&self, mut bitoff: T, bitmax: T) -> GetResult<T> {
        for elem in self.elems.iter() {
            if bitoff < elem.bitoff {
                break;
            }
            if bitoff < elem.bitoff + elem.bitlen {
                bitoff = elem.bitoff + elem.bitlen;
                break;
            }
        }
        if bitoff > bitmax {
            bitoff = bitmax;
        }
        GetResult { all_set: bitoff == bitmax, first_unset: bitoff }
    }

    fn set(&mut self, bitoff: T, bitmax: T) -> Result<(), Status> {
        self.set_internal(bitoff, bitmax, None)
    }

    fn clear(&mut self, bitoff: T, bitmax: T) -> Result<(), Status> {
        self.clear_internal(bitoff, bitmax, None)
    }

    fn clear_all(&mut self) {
        self.elems.clear();
        self.num_elems = 0;
        self.num_bits = T::default();
    }
}

impl<'a, T> IntoIterator for &'a RleBitmapBase<T> {
    type Item = &'a Element<T>;
    type IntoIter = fbl::Iterator<'a, ElementPtr<T>>;

    fn into_iter(self) -> Self::IntoIter {
        self.elems.iter()
    }
}

pub type RleBitmap = RleBitmapBase<usize>;
pub type RleBitmapElement = Element<usize>;
