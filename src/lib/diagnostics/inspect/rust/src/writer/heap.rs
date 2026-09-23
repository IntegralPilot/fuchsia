// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements the buddy allocation algorithm for the [Inspect VMO][inspect-vmo]
//!
//! [inspect-vmo]: https://fuchsia.dev/fuchsia-src/reference/diagnostics/inspect/vmo-format

use crate::writer::Error;
use inspect_format::{
    Block, BlockAccessorExt, BlockAccessorMutExt, BlockIndex, BlockType, Free, ReadBytes, Reserved,
    WriteBytes, constants, utils,
};
use std::cmp::min;

/// The inspect heap.
#[derive(Debug)]
pub struct Heap<T> {
    pub(crate) container: T,
    current_size_bytes: usize,
    free_head_per_order: [BlockIndex; constants::NUM_ORDERS as usize],
    allocated_blocks: usize,
    deallocated_blocks: usize,
    failed_allocations: usize,
    has_header: bool,
}

impl<T: ReadBytes + WriteBytes> Heap<T> {
    /// Creates a new heap on the underlying mapped VMO and initializes the header block in it.
    pub fn new(container: T) -> Result<Self, Error> {
        let mut heap = Self::empty(container)?;
        heap.init_header()?;
        Ok(heap)
    }

    /// Creates a new heap on the underlying mapped VMO without initializing the header block.
    pub fn empty(container: T) -> Result<Self, Error> {
        let mut heap = Heap {
            container,
            current_size_bytes: 0,
            free_head_per_order: [BlockIndex::EMPTY; constants::NUM_ORDERS as usize],
            allocated_blocks: 0,
            deallocated_blocks: 0,
            failed_allocations: 0,
            has_header: false,
        };
        heap.grow_heap(constants::PAGE_SIZE_BYTES)?;
        Ok(heap)
    }

    #[inline]
    fn init_header(&mut self) -> Result<(), Error> {
        let header_index =
            self.allocate_block(inspect_format::utils::order_to_size(constants::HEADER_ORDER))?;
        let heap_current_size = self.current_size_bytes;
        self.container
            .block_at_unchecked_mut::<Reserved>(header_index)
            .become_header(heap_current_size)?;
        self.has_header = true;
        Ok(())
    }

    /// Returns the current size of this heap in bytes.
    pub fn current_size(&self) -> usize {
        self.current_size_bytes
    }

    /// Returns the maximum size of this heap in bytes.
    pub fn maximum_size(&self) -> usize {
        self.container.len()
    }

    /// Returns the number of blocks allocated since the creation of this heap.
    pub fn total_allocated_blocks(&self) -> usize {
        self.allocated_blocks
    }

    /// Returns the number blocks deallocated since the creation of this heap.
    pub fn total_deallocated_blocks(&self) -> usize {
        self.deallocated_blocks
    }

    /// Returns the number of failed allocations since the creation of this heap.
    pub fn failed_allocations(&self) -> usize {
        self.failed_allocations
    }

    /// Allocates a new block of the given `min_size`.
    pub fn allocate_block(&mut self, min_size: usize) -> Result<BlockIndex, Error> {
        let min_fit_order = utils::fit_order(min_size);
        if min_fit_order >= constants::NUM_ORDERS as usize {
            return Err(Error::InvalidBlockOrder(min_fit_order));
        }
        let min_fit_order = min_fit_order as u8;
        // Find free block with order >= min_fit_order
        let order_found = (min_fit_order..constants::NUM_ORDERS)
            .find(|&i| self.is_free_block(self.free_head_per_order[i as usize], i).is_some());
        let next_order = match order_found {
            Some(order) => order,
            None => {
                self.grow_heap(self.current_size_bytes + constants::PAGE_SIZE_BYTES)?;
                constants::NUM_ORDERS - 1
            }
        };
        let block_index = self.free_head_per_order[next_order as usize];
        while self.container.block_at(block_index).order() > min_fit_order {
            self.split_block(block_index)?;
        }
        self.remove_free(block_index);
        let _ = self.container.block_at_unchecked_mut::<Free>(block_index).become_reserved();
        self.allocated_blocks += 1;
        Ok(block_index)
    }

    /// Marks the memory region pointed by the given `block` as free.
    pub fn free_block(&mut self, mut block_index: BlockIndex) -> Result<(), Error> {
        let block = self.container.block_at(block_index);
        if block.block_type() == Some(BlockType::Free) {
            return Err(Error::BlockAlreadyFree(block_index));
        }
        let mut buddy_index = buddy(block_index, block.order());

        while self.possible_to_merge(buddy_index, block_index) {
            self.remove_free(buddy_index);
            if buddy_index < block_index {
                std::mem::swap(&mut buddy_index, &mut block_index);
            }
            let mut block = self.container.block_at_mut(block_index);
            let order = block.order();
            block.set_order(order + 1)?;
            buddy_index = buddy(block_index, order + 1);
        }
        let block = self.container.block_at_unchecked_mut::<Reserved>(block_index);
        let order = block.order();
        let _ = block.become_free(self.free_head_per_order[order as usize]);
        self.free_head_per_order[order as usize] = block_index;
        self.deallocated_blocks += 1;
        Ok(())
    }

    #[inline]
    fn possible_to_merge(&self, buddy_index: BlockIndex, block_index: BlockIndex) -> bool {
        let max_block_index = self.current_size_bytes / constants::MIN_ORDER_SIZE;
        if *buddy_index as usize >= max_block_index {
            return false;
        }
        self.container
            .maybe_block_at::<Free>(buddy_index)
            .map(|buddy_block| {
                let block = self.container.block_at(block_index);
                block.order() < constants::NUM_ORDERS - 1 && block.order() == buddy_block.order()
            })
            .unwrap_or(false)
    }

    /// Returns a copy of the bytes stored in this Heap.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.container.get_slice(self.current_size_bytes).unwrap().to_vec()
    }

    #[inline]
    fn grow_heap(&mut self, requested_size: usize) -> Result<(), Error> {
        let container_size = self.container.len();
        if requested_size > container_size || requested_size > constants::MAX_VMO_SIZE {
            self.failed_allocations += 1;
            return Err(Error::HeapMaxSizeReached);
        }
        let new_size = min(container_size, requested_size);
        let min_index = BlockIndex::from_offset(self.current_size_bytes);
        let mut last_index = self.free_head_per_order[(constants::NUM_ORDERS - 1) as usize];
        let mut curr_index =
            BlockIndex::from_offset(new_size - new_size % constants::PAGE_SIZE_BYTES);
        loop {
            curr_index -= BlockIndex::from_offset(constants::MAX_ORDER_SIZE);
            Block::free(&mut self.container, curr_index, constants::NUM_ORDERS - 1, last_index)
                .expect("Failed to create free block");
            last_index = curr_index;
            if curr_index <= min_index {
                break;
            }
        }
        self.free_head_per_order[(constants::NUM_ORDERS - 1) as usize] = last_index;
        self.current_size_bytes = new_size;
        if self.has_header {
            self.container
                .block_at_unchecked_mut(BlockIndex::HEADER)
                // Safety: the current size can't be larger than a max u32 value
                .set_vmo_size(self.current_size_bytes as u32)?;
        }
        Ok(())
    }

    #[inline]
    fn is_free_block(
        &mut self,
        index: BlockIndex,
        expected_order: u8,
    ) -> Option<Block<&mut T, Free>> {
        // Safety: promoting from u32 to usize
        if (*index as usize) >= self.current_size_bytes / constants::MIN_ORDER_SIZE {
            return None;
        }
        self.container
            .maybe_block_at_mut::<Free>(index)
            .filter(|block| block.order() == expected_order)
    }

    #[inline]
    fn remove_free(&mut self, block_index: BlockIndex) {
        let block = self.container.block_at_unchecked::<Free>(block_index);
        let free_next_index = block.free_next_index();
        let order = block.order();
        if order >= constants::NUM_ORDERS {
            return;
        }
        let mut next_index = self.free_head_per_order[order as usize];
        if next_index == block_index {
            self.free_head_per_order[order as usize] = free_next_index;
            return;
        }
        while let Some(mut curr_block) = self.is_free_block(next_index, order) {
            next_index = curr_block.free_next_index();
            if next_index == block_index {
                curr_block.set_free_next_index(free_next_index);
                return;
            }
        }
    }

    #[inline]
    fn split_block(&mut self, block_index: BlockIndex) -> Result<(), Error> {
        let block_order = self.container.block_at(block_index).order();
        if block_order >= constants::NUM_ORDERS {
            return Err(Error::InvalidBlockOrderAtIndex(block_order, block_index));
        }
        self.remove_free(block_index);
        let buddy_index = buddy(block_index, block_order - 1);
        let mut block = self.container.block_at_mut(block_index);
        block.set_order(block_order - 1)?;
        block.become_free(buddy_index);

        let mut buddy = self.container.block_at_mut(buddy_index);
        let buddy_order = block_order - 1;
        buddy.set_order(buddy_order)?;
        buddy.become_free(self.free_head_per_order[buddy_order as usize]);
        self.free_head_per_order[buddy_order as usize] = block_index;
        Ok(())
    }
}

fn buddy(index: BlockIndex, order: u8) -> BlockIndex {
    index ^ BlockIndex::from_offset(utils::order_to_size(order))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::snapshot::{BackingBuffer, BlockIterator};
    use inspect_format::{BlockType, Container, Header, block_testing};

    #[derive(Debug)]
    struct BlockDebug {
        index: BlockIndex,
        order: u8,
        block_type: BlockType,
    }

    fn validate<T: WriteBytes + ReadBytes>(expected: &[BlockDebug], heap: &Heap<T>) {
        let buffer = BackingBuffer::Bytes(heap.bytes());
        let actual: Vec<BlockDebug> = BlockIterator::from(&buffer)
            .map(|block| BlockDebug {
                order: block.order(),
                index: block.index(),
                block_type: block.block_type().unwrap(),
            })
            .collect();
        assert_eq!(expected.len(), actual.len());
        for (i, result) in actual.iter().enumerate() {
            assert_eq!(result.block_type, expected[i].block_type);
            assert_eq!(result.index, expected[i].index);
            assert_eq!(result.order, expected[i].order);
        }
    }

    #[fuchsia::test]
    fn test_possible_to_merge_out_of_bounds() {
        let (container, _storage) = Container::read_and_write(4096).unwrap();
        let heap = Heap::empty(container).unwrap();
        let oob_buddy = BlockIndex::from_offset(8192);
        let block_idx = BlockIndex::from(0);
        assert!(!heap.possible_to_merge(oob_buddy, block_idx));
    }

    #[fuchsia::test]
    fn empty_heap() {
        let (container, _storage) = Container::read_and_write(4096).unwrap();
        let heap = Heap::empty(container).unwrap();
        assert_eq!(heap.current_size_bytes, 4096);
        assert_eq!(heap.free_head_per_order, [BlockIndex::EMPTY; 8]);

        let expected = [
            BlockDebug { index: 0.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Free },
        ];
        validate(&expected, &heap);
        assert_eq!(*heap.free_head_per_order[7], 0);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(0.into()).free_next_index(), 128);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(128.into()).free_next_index(), 0);
        assert_eq!(heap.failed_allocations, 0);
    }

    #[fuchsia::test]
    fn new_heap() {
        let (container, _storage) = Container::read_and_write(4096).unwrap();
        let heap = Heap::new(container).unwrap();
        assert_eq!(heap.current_size_bytes, 4096);
        assert_eq!(
            heap.free_head_per_order,
            [
                BlockIndex::from(0),
                BlockIndex::from(2),
                BlockIndex::from(4),
                BlockIndex::from(8),
                BlockIndex::from(16),
                BlockIndex::from(32),
                BlockIndex::from(64),
                BlockIndex::from(128)
            ]
        );

        let expected = [
            BlockDebug { index: 0.into(), order: 1, block_type: BlockType::Header },
            BlockDebug { index: 2.into(), order: 1, block_type: BlockType::Free },
            BlockDebug { index: 4.into(), order: 2, block_type: BlockType::Free },
            BlockDebug { index: 8.into(), order: 3, block_type: BlockType::Free },
            BlockDebug { index: 16.into(), order: 4, block_type: BlockType::Free },
            BlockDebug { index: 32.into(), order: 5, block_type: BlockType::Free },
            BlockDebug { index: 64.into(), order: 6, block_type: BlockType::Free },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Free },
        ];
        validate(&expected, &heap);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(128.into()).free_next_index(), 0);
        assert_eq!(heap.failed_allocations, 0);
    }

    #[fuchsia::test]
    fn allocate_and_free() {
        let (container, _storage) = Container::read_and_write(4096).unwrap();
        let mut heap = Heap::empty(container).unwrap();

        // Allocate some small blocks and ensure they are all in order.
        for i in 0..=5 {
            let block = heap.allocate_block(constants::MIN_ORDER_SIZE).unwrap();
            assert_eq!(*block, i);
        }

        // Free some blocks. Leaving some in the middle.
        assert!(heap.free_block(BlockIndex::from(2)).is_ok());
        assert!(heap.free_block(BlockIndex::from(4)).is_ok());
        assert!(heap.free_block(BlockIndex::from(0)).is_ok());

        // Allocate more small blocks and ensure we get the same ones in reverse
        // order.
        let b = heap.allocate_block(constants::MIN_ORDER_SIZE).unwrap();
        assert_eq!(*b, 0);
        let b = heap.allocate_block(constants::MIN_ORDER_SIZE).unwrap();
        assert_eq!(*b, 4);
        let b = heap.allocate_block(constants::MIN_ORDER_SIZE).unwrap();
        assert_eq!(*b, 2);

        // Free everything except the first two.
        assert!(heap.free_block(BlockIndex::from(4)).is_ok());
        assert!(heap.free_block(BlockIndex::from(2)).is_ok());
        assert!(heap.free_block(BlockIndex::from(3)).is_ok());
        assert!(heap.free_block(BlockIndex::from(5)).is_ok());

        let expected = [
            BlockDebug { index: 0.into(), order: 0, block_type: BlockType::Reserved },
            BlockDebug { index: 1.into(), order: 0, block_type: BlockType::Reserved },
            BlockDebug { index: 2.into(), order: 1, block_type: BlockType::Free },
            BlockDebug { index: 4.into(), order: 2, block_type: BlockType::Free },
            BlockDebug { index: 8.into(), order: 3, block_type: BlockType::Free },
            BlockDebug { index: 16.into(), order: 4, block_type: BlockType::Free },
            BlockDebug { index: 32.into(), order: 5, block_type: BlockType::Free },
            BlockDebug { index: 64.into(), order: 6, block_type: BlockType::Free },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Free },
        ];
        validate(&expected, &heap);
        assert!(heap.free_head_per_order.iter().enumerate().skip(2).all(|(i, &j)| (1 << i) == *j));
        let buffer = BackingBuffer::from(heap.bytes());
        assert!(
            BlockIterator::from(&buffer).skip(2).all(|b| *b
                .cast::<Free>()
                .unwrap()
                .free_next_index()
                == 0)
        );

        // Ensure a large block takes the first free large one.
        assert!(heap.free_block(BlockIndex::from(0)).is_ok());
        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 128);

        // Free last small allocation, next large takes first half of the
        // buffer.
        assert!(heap.free_block(BlockIndex::from(1)).is_ok());
        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 0);

        let expected = [
            BlockDebug { index: 0.into(), order: 7, block_type: BlockType::Reserved },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Reserved },
        ];
        validate(&expected, &heap);

        // Allocate twice in the first half, free in reverse order to ensure
        // freeing works left to right and right to left.
        assert!(heap.free_block(BlockIndex::from(0)).is_ok());
        let b = heap.allocate_block(1024).unwrap();
        assert_eq!(*b, 0);
        let b = heap.allocate_block(1024).unwrap();
        assert_eq!(*b, 64);
        assert!(heap.free_block(BlockIndex::from(0)).is_ok());
        assert!(heap.free_block(BlockIndex::from(64)).is_ok());

        // Ensure freed blocks are merged int a big one and that we can use all
        // space at 0.
        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 0);
        assert!(heap.free_block(BlockIndex::from(0)).is_ok());

        let expected = [
            BlockDebug { index: 0.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Reserved },
        ];
        validate(&expected, &heap);
        assert_eq!(*heap.free_head_per_order[7], 0);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(0.into()).free_next_index(), 0);

        assert!(heap.free_block(BlockIndex::from(128)).is_ok());
        let expected = [
            BlockDebug { index: 0.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Free },
        ];
        validate(&expected, &heap);
        assert_eq!(*heap.free_head_per_order[7], 128);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(0.into()).free_next_index(), 0);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(128.into()).free_next_index(), 0);
        assert_eq!(heap.failed_allocations, 0);
    }

    #[fuchsia::test]
    fn allocation_counters_work() {
        let (container, _storage) = Container::read_and_write(4096).unwrap();
        let mut heap = Heap::empty(container).unwrap();

        let block_count_to_allocate: usize = 50;
        for _ in 0..block_count_to_allocate {
            heap.allocate_block(constants::MIN_ORDER_SIZE).unwrap();
        }

        assert_eq!(heap.total_allocated_blocks(), block_count_to_allocate);

        let block_count_to_free: usize = 5;
        for i in 0..block_count_to_free {
            heap.free_block(BlockIndex::from(i as u32)).unwrap();
        }

        assert_eq!(heap.total_allocated_blocks(), block_count_to_allocate);
        assert_eq!(heap.total_deallocated_blocks(), block_count_to_free);

        for i in block_count_to_free..block_count_to_allocate {
            heap.free_block(BlockIndex::from(i as u32)).unwrap();
        }

        assert_eq!(heap.total_allocated_blocks(), block_count_to_allocate);
        assert_eq!(heap.total_deallocated_blocks(), block_count_to_allocate);
    }

    #[fuchsia::test]
    fn allocate_merge() {
        let (container, _storage) = Container::read_and_write(4096).unwrap();
        let mut heap = Heap::empty(container).unwrap();
        for i in 0..=3 {
            let block = heap.allocate_block(constants::MIN_ORDER_SIZE).unwrap();
            assert_eq!(*block, i);
        }

        assert!(heap.free_block(BlockIndex::from(2)).is_ok());
        assert!(heap.free_block(BlockIndex::from(0)).is_ok());
        assert!(heap.free_block(BlockIndex::from(1)).is_ok());

        let expected = [
            BlockDebug { index: 0.into(), order: 1, block_type: BlockType::Free },
            BlockDebug { index: 2.into(), order: 0, block_type: BlockType::Free },
            BlockDebug { index: 3.into(), order: 0, block_type: BlockType::Reserved },
            BlockDebug { index: 4.into(), order: 2, block_type: BlockType::Free },
            BlockDebug { index: 8.into(), order: 3, block_type: BlockType::Free },
            BlockDebug { index: 16.into(), order: 4, block_type: BlockType::Free },
            BlockDebug { index: 32.into(), order: 5, block_type: BlockType::Free },
            BlockDebug { index: 64.into(), order: 6, block_type: BlockType::Free },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Free },
        ];
        validate(&expected, &heap);
        assert!(heap.free_head_per_order.iter().enumerate().skip(3).all(|(i, &j)| (1 << i) == *j));
        let buffer = BackingBuffer::from(heap.bytes());
        assert!(
            BlockIterator::from(&buffer).skip(3).all(|b| *b
                .cast::<Free>()
                .unwrap()
                .free_next_index()
                == 0)
        );
        assert_eq!(*heap.free_head_per_order[1], 0);
        assert_eq!(*heap.free_head_per_order[0], 2);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(0.into()).free_next_index(), 0);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(2.into()).free_next_index(), 0);

        assert!(heap.free_block(BlockIndex::from(3)).is_ok());
        let expected = [
            BlockDebug { index: 0.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Free },
        ];
        validate(&expected, &heap);
        assert_eq!(*heap.free_head_per_order[1], 0);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(0.into()).free_next_index(), 128);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(128.into()).free_next_index(), 0);
    }

    #[fuchsia::test]
    fn extend() {
        let (container, _storage) = Container::read_and_write(8 * 2048).unwrap();
        let mut heap = Heap::empty(container).unwrap();

        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 0);
        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 128);
        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 256);

        let expected = [
            BlockDebug { index: 0.into(), order: 7, block_type: BlockType::Reserved },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Reserved },
            BlockDebug { index: 256.into(), order: 7, block_type: BlockType::Reserved },
            BlockDebug { index: 384.into(), order: 7, block_type: BlockType::Free },
        ];
        validate(&expected, &heap);
        assert_eq!(*heap.free_head_per_order[7], 384);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(384.into()).free_next_index(), 0);

        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 384);
        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 512);

        assert!(heap.free_block(BlockIndex::from(0)).is_ok());
        assert!(heap.free_block(BlockIndex::from(128)).is_ok());
        assert!(heap.free_block(BlockIndex::from(256)).is_ok());
        assert!(heap.free_block(BlockIndex::from(384)).is_ok());
        assert!(heap.free_block(BlockIndex::from(512)).is_ok());

        let expected = [
            BlockDebug { index: 0.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 256.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 384.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 512.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 640.into(), order: 7, block_type: BlockType::Free },
        ];
        validate(&expected, &heap);
        assert_eq!(heap.current_size_bytes, 2048 * 4 + 4096);
        assert_eq!(*heap.free_head_per_order[7], 512);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(512.into()).free_next_index(), 384);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(384.into()).free_next_index(), 256);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(256.into()).free_next_index(), 128);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(128.into()).free_next_index(), 0);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(0.into()).free_next_index(), 640);
        assert_eq!(*heap.container.block_at_unchecked::<Free>(640.into()).free_next_index(), 0);
        assert_eq!(heap.failed_allocations, 0);
    }

    #[fuchsia::test]
    fn extend_error() {
        let (container, _storage) = Container::read_and_write(4 * 2048).unwrap();
        let mut heap = Heap::empty(container).unwrap();

        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 0);
        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 128);
        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 256);

        let expected = [
            BlockDebug { index: 0.into(), order: 7, block_type: BlockType::Reserved },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Reserved },
            BlockDebug { index: 256.into(), order: 7, block_type: BlockType::Reserved },
            BlockDebug { index: 384.into(), order: 7, block_type: BlockType::Free },
        ];
        validate(&expected, &heap);

        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 384);
        assert_eq!(heap.failed_allocations, 0);
        assert!(heap.allocate_block(2048).is_err());
        assert_eq!(heap.failed_allocations, 1);
        assert!(heap.allocate_block(2048).is_err());
        assert_eq!(heap.failed_allocations, 2);

        assert!(heap.free_block(BlockIndex::from(0)).is_ok());
        assert!(heap.free_block(BlockIndex::from(128)).is_ok());
        assert!(heap.free_block(BlockIndex::from(256)).is_ok());
        assert!(heap.free_block(BlockIndex::from(384)).is_ok());

        let expected = [
            BlockDebug { index: 0.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 256.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 384.into(), order: 7, block_type: BlockType::Free },
        ];
        validate(&expected, &heap);
    }

    #[fuchsia::test]
    fn extend_vmo_greater_max_size() {
        let (container, _storage) =
            Container::read_and_write(constants::MAX_VMO_SIZE + 2048).unwrap();
        let mut heap = Heap::empty(container).unwrap();

        for n in 0_u32..(constants::MAX_VMO_SIZE / constants::MAX_ORDER_SIZE).try_into().unwrap() {
            let b = heap.allocate_block(2048).unwrap();
            assert_eq!(*b, n * 128);
        }
        assert_eq!(heap.failed_allocations, 0);
        assert!(heap.allocate_block(2048).is_err());
        assert_eq!(heap.failed_allocations, 1);

        for n in 0_u32..(constants::MAX_VMO_SIZE / constants::MAX_ORDER_SIZE).try_into().unwrap() {
            assert!(heap.free_block(BlockIndex::from(n * 128)).is_ok());
        }
    }

    #[fuchsia::test]
    fn dont_reinterpret_upper_block_contents() {
        let (container, _storage) = Container::read_and_write(4096).unwrap();
        let mut heap = Heap::empty(container).unwrap();

        // Allocate 3 blocks.
        assert_eq!(*heap.allocate_block(constants::MIN_ORDER_SIZE).unwrap(), 0);
        let b1 = heap.allocate_block(utils::order_to_size(1)).unwrap();
        assert_eq!(*b1, 2);
        assert_eq!(*heap.allocate_block(utils::order_to_size(1)).unwrap(), 4);

        // Write garbage to the second half of the order 1 block in index 2.
        {
            let mut block = heap.container.block_at_mut(3.into());
            block_testing::override_header(&mut block, 0xffffffff);
            block_testing::override_payload(&mut block, 0xffffffff);
        }

        // Free order 1 block in index 2.
        assert!(heap.free_block(b1).is_ok());

        // Allocate small blocks in free order 0 blocks.
        assert_eq!(*heap.allocate_block(constants::MIN_ORDER_SIZE).unwrap(), 1);
        assert_eq!(*heap.allocate_block(constants::MIN_ORDER_SIZE).unwrap(), 2);

        // This should succeed even if the bytes in this region were garbage.
        assert_eq!(*heap.allocate_block(constants::MIN_ORDER_SIZE).unwrap(), 3);

        let expected = [
            BlockDebug { index: 0.into(), order: 0, block_type: BlockType::Reserved },
            BlockDebug { index: 1.into(), order: 0, block_type: BlockType::Reserved },
            BlockDebug { index: 2.into(), order: 0, block_type: BlockType::Reserved },
            BlockDebug { index: 3.into(), order: 0, block_type: BlockType::Reserved },
            BlockDebug { index: 4.into(), order: 1, block_type: BlockType::Reserved },
            BlockDebug { index: 6.into(), order: 1, block_type: BlockType::Free },
            BlockDebug { index: 8.into(), order: 3, block_type: BlockType::Free },
            BlockDebug { index: 16.into(), order: 4, block_type: BlockType::Free },
            BlockDebug { index: 32.into(), order: 5, block_type: BlockType::Free },
            BlockDebug { index: 64.into(), order: 6, block_type: BlockType::Free },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Free },
        ];
        validate(&expected, &heap);
    }

    #[fuchsia::test]
    fn update_header_vmo_size() {
        let (container, _storage) = Container::read_and_write(3 * 4096).unwrap();
        let mut heap = Heap::new(container).unwrap();
        assert_eq!(
            heap.container
                .block_at_unchecked::<Header>(BlockIndex::HEADER)
                .vmo_size()
                .unwrap()
                .unwrap() as usize,
            heap.current_size()
        );
        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 128);
        assert_eq!(
            heap.container
                .block_at_unchecked::<Header>(BlockIndex::HEADER)
                .vmo_size()
                .unwrap()
                .unwrap() as usize,
            heap.current_size()
        );
        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 256);
        assert_eq!(
            heap.container
                .block_at_unchecked::<Header>(BlockIndex::HEADER)
                .vmo_size()
                .unwrap()
                .unwrap() as usize,
            heap.current_size()
        );
        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 384);
        assert_eq!(
            heap.container
                .block_at_unchecked::<Header>(BlockIndex::HEADER)
                .vmo_size()
                .unwrap()
                .unwrap() as usize,
            heap.current_size()
        );

        let expected = [
            BlockDebug { index: 0.into(), order: 1, block_type: BlockType::Header },
            BlockDebug { index: 2.into(), order: 1, block_type: BlockType::Free },
            BlockDebug { index: 4.into(), order: 2, block_type: BlockType::Free },
            BlockDebug { index: 8.into(), order: 3, block_type: BlockType::Free },
            BlockDebug { index: 16.into(), order: 4, block_type: BlockType::Free },
            BlockDebug { index: 32.into(), order: 5, block_type: BlockType::Free },
            BlockDebug { index: 64.into(), order: 6, block_type: BlockType::Free },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Reserved },
            BlockDebug { index: 256.into(), order: 7, block_type: BlockType::Reserved },
            BlockDebug { index: 384.into(), order: 7, block_type: BlockType::Reserved },
        ];
        validate(&expected, &heap);

        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 512);
        assert_eq!(
            heap.container
                .block_at_unchecked::<Header>(BlockIndex::HEADER)
                .vmo_size()
                .unwrap()
                .unwrap() as usize,
            heap.current_size()
        );
        let b = heap.allocate_block(2048).unwrap();
        assert_eq!(*b, 640);
        assert_eq!(
            heap.container
                .block_at_unchecked::<Header>(BlockIndex::HEADER)
                .vmo_size()
                .unwrap()
                .unwrap() as usize,
            heap.current_size()
        );
        assert_eq!(heap.failed_allocations, 0);
        assert!(heap.allocate_block(2048).is_err());
        assert_eq!(
            heap.container
                .block_at_unchecked::<Header>(BlockIndex::HEADER)
                .vmo_size()
                .unwrap()
                .unwrap() as usize,
            heap.current_size()
        );
        assert_eq!(heap.failed_allocations, 1);

        assert!(heap.free_block(BlockIndex::from(128)).is_ok());
        assert!(heap.free_block(BlockIndex::from(256)).is_ok());
        assert!(heap.free_block(BlockIndex::from(384)).is_ok());
        assert!(heap.free_block(BlockIndex::from(512)).is_ok());
        assert!(heap.free_block(BlockIndex::from(640)).is_ok());
        assert_eq!(
            heap.container
                .block_at_unchecked::<Header>(BlockIndex::HEADER)
                .vmo_size()
                .unwrap()
                .unwrap() as usize,
            heap.current_size()
        );

        assert!(heap.free_block(BlockIndex::HEADER).is_ok());

        let expected = [
            BlockDebug { index: 0.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 128.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 256.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 384.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 512.into(), order: 7, block_type: BlockType::Free },
            BlockDebug { index: 640.into(), order: 7, block_type: BlockType::Free },
        ];
        validate(&expected, &heap);
    }
}
