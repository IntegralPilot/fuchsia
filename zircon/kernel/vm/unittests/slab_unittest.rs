// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// Slab allocator tests duplicated from slab_unittest.cc.
#[cfg(ktest)]
#[unittest::suite]
mod slab_rs {
    use crate::vm::page::VmPage;
    use crate::vm::page_slab_allocator::{
        BaseSlabProvider, IdSlabAllocator, PageSlabAllocator, SlabAllocationError, SlabProvider,
    };
    use crate::vm_unittests::test_helper::TestRand;
    use core::convert::Infallible;
    use core::ffi::c_void;
    use core::mem::size_of;
    use core::pin::Pin;
    use core::ptr::NonNull;
    use pin_init::{PinInit, pin_data, pin_init, stack_pin_init};
    use rand::RngExt as _;
    use rand::seq::SliceRandom;
    use unittest::{expect_eq, expect_le, expect_lt, expect_true, unwrap_ok};

    struct TestObject {
        _data: [u64; 32],
    }

    const OBJECTS_PER_SLAB: usize =
        PageSlabAllocator::<{ size_of::<TestObject>() }, BaseSlabProvider>::ALLOCS_PER_SLAB;

    struct TestSlabProvider {
        base: BaseSlabProvider,
        allocated: usize,
        freed: usize,
    }

    impl SlabProvider for TestSlabProvider {
        fn alloc_slab(self: Pin<&mut Self>) -> Result<NonNull<VmPage>, SlabAllocationError> {
            let this = self.get_mut();
            let slab = Pin::new(&mut this.base).alloc_slab()?;
            this.allocated += 1;
            Ok(slab)
        }

        unsafe fn free_slab(self: Pin<&mut Self>, slab: NonNull<VmPage>) {
            let this = self.get_mut();
            let base = Pin::new(&mut this.base);
            // SAFETY: Caller guarantees `slab` was allocated by `alloc_slab` and has not
            // been freed.
            unsafe {
                base.free_slab(slab);
            }
            this.freed += 1;
        }
    }

    #[pin_data(PinnedDrop)]
    struct TestSlabAllocator {
        #[pin]
        allocator: PageSlabAllocator<{ size_of::<TestObject>() }, TestSlabProvider>,
    }

    impl TestSlabAllocator {
        fn new() -> impl PinInit<Self, Infallible> {
            pin_init!(Self {
                allocator <- PageSlabAllocator::new_with(pin_init::init!(TestSlabProvider {
                    base: BaseSlabProvider {},
                    allocated: 0,
                    freed: 0,
                })),
            })
        }

        fn slabs_allocated(&self) -> usize {
            self.allocator.provider().allocated
        }

        fn slabs_freed(&self) -> usize {
            self.allocator.provider().freed
        }

        fn active_slabs(&self) -> usize {
            self.allocator.provider().allocated - self.allocator.provider().freed
        }

        const fn slabs_required(num_allocs: usize) -> usize {
            PageSlabAllocator::<{ size_of::<TestObject>() }, TestSlabProvider>::slabs_required(
                num_allocs,
            )
        }

        fn as_allocator(
            self: Pin<&mut Self>,
        ) -> Pin<&mut PageSlabAllocator<{ size_of::<TestObject>() }, TestSlabProvider>> {
            self.project().allocator
        }
    }

    #[pin_init::pinned_drop]
    impl PinnedDrop for TestSlabAllocator {
        fn drop(self: Pin<&mut Self>) {
            self.project().allocator.debug_free_all_slabs();
        }
    }

    /// Smoke test allocating and freeing a single object from a slab allocator.
    #[test]
    fn slab_smoke_test() {
        stack_pin_init!(
            let alloc = PageSlabAllocator::<{ size_of::<TestObject>() }, _>::new_with(
                pin_init::init!(BaseSlabProvider {})
            )
        );
        let obj: NonNull<TestObject> = unwrap_ok!(alloc.as_mut().allocate_object());
        // SAFETY: `obj` is valid for writing TestObject.
        unsafe {
            obj.write(TestObject { _data: [0; 32] });
        }
        // SAFETY: `obj` was allocated by `alloc`.
        unsafe {
            alloc.as_mut().deallocate_bytes(obj.cast());
        }
    }

    /// Tests that emptying a slab returns it to the provider.
    #[test]
    fn slab_return_slab_test() {
        stack_pin_init!(let alloc = TestSlabAllocator::new());

        // Allocate one slab worth of objects.
        for _ in 0..OBJECTS_PER_SLAB {
            unwrap_ok!(alloc.as_mut().as_allocator().allocate_bytes());
        }
        expect_eq!(alloc.active_slabs(), 1);

        // Allocate a second slab worth of objects.
        let mut objects: [NonNull<TestObject>; OBJECTS_PER_SLAB] =
            [NonNull::dangling(); OBJECTS_PER_SLAB];
        for object in &mut objects {
            let obj: NonNull<TestObject> =
                unwrap_ok!(alloc.as_mut().as_allocator().allocate_object());
            // SAFETY: `obj` points to valid uninitialized memory allocated by `alloc`.
            unsafe {
                obj.write(TestObject { _data: [0; 32] });
            }
            *object = obj;
        }
        expect_eq!(alloc.active_slabs(), 2);

        // Allocate into a third slab.
        unwrap_ok!(alloc.as_mut().as_allocator().allocate_bytes());
        expect_eq!(alloc.active_slabs(), 3);

        // Free all the objects on the second slab and ensure the slab was returned.
        for object in objects {
            expect_eq!(alloc.active_slabs(), 3);
            // SAFETY: `object` was allocated by `alloc`.
            unsafe {
                alloc.as_mut().as_allocator().deallocate_bytes(object.cast());
            }
        }
        expect_eq!(alloc.active_slabs(), 2);
    }

    /// Tests that active slabs do not exceed the maximum required under churn.
    #[test]
    fn slab_no_leak_test() {
        // Continually allocate and free a range of objects, ensuring that the number of slabs
        // allocated at one time is never more than required to store all the objects.
        const NUM_OBJECTS: usize = 400;
        let max_slabs = TestSlabAllocator::slabs_required(NUM_OBJECTS);

        stack_pin_init!(let alloc = TestSlabAllocator::new());
        let mut allocated = 0;
        let mut objects: [NonNull<TestObject>; NUM_OBJECTS] = [NonNull::dangling(); NUM_OBJECTS];

        let mut r = TestRand::new(42);

        for _ in 0..10000 {
            // Allocate some number of objects.
            let to_alloc = r.random_range(0..NUM_OBJECTS - allocated);
            for _ in 0..to_alloc {
                let object: NonNull<TestObject> =
                    unwrap_ok!(alloc.as_mut().as_allocator().allocate_object());
                // SAFETY: `object` is valid for writing TestObject.
                unsafe {
                    object.write(TestObject { _data: [0; 32] });
                }
                objects[allocated] = object;
                allocated += 1;
            }
            expect_le!(alloc.active_slabs(), max_slabs);

            // Randomize the objects.
            objects[..allocated].shuffle(&mut r);

            // Free some subset if there are any allocations.
            let to_free = if allocated > 0 { r.random_range(0..allocated) } else { 0 };
            for _ in 0..to_free {
                allocated -= 1;
                // SAFETY: `objects[allocated]` was allocated by `alloc`.
                unsafe {
                    alloc.as_mut().as_allocator().deallocate_bytes(objects[allocated].cast());
                }
            }
        }

        // Free any that are left.
        for object in &objects[..allocated] {
            // SAFETY: `*object` was allocated by `alloc`.
            unsafe {
                alloc.as_mut().as_allocator().deallocate_bytes(object.cast());
            }
        }

        expect_eq!(alloc.active_slabs(), 0);
    }

    /// Tests ID allocation and exhaustion in an IdSlabAllocator.
    #[test]
    fn slab_max_id_test() {
        // To satisfy the requirement that the number of objects completely fill any slabs we
        // need to cause sufficient top level slabs to be allocated and used. Every top level
        // slab requires an allocation in the bottom level slab. To completely fill one bottom
        // level slab then requires `page::SIZE / size_of::<NonNull<VmPage>>()` top level slabs,
        // each requiring `page::SIZE / size_of::<TestObject>()` objects to fill.
        //
        // For this test we want enough objects for two bottom level slabs.
        const NUM_OBJECTS: usize = (page::SIZE * 2 / size_of::<NonNull<VmPage>>())
            * (page::SIZE / size_of::<TestObject>());
        const SLABS_REQUIRED: usize = PageSlabAllocator::<
            { size_of::<TestObject>() },
            BaseSlabProvider,
        >::slabs_required(NUM_OBJECTS);
        const SLAB_ID_SLABS_REQUIRED: usize = PageSlabAllocator::<
            { size_of::<NonNull<VmPage>>() },
            BaseSlabProvider,
        >::slabs_required(SLABS_REQUIRED);

        stack_pin_init!(
            let alloc = IdSlabAllocator::<
                { size_of::<TestObject>() },
                NUM_OBJECTS,
                SLABS_REQUIRED,
                SLAB_ID_SLABS_REQUIRED,
            >::new()
        );

        // Allocate all the objects we are supposed to be able to have.
        for _ in 0..NUM_OBJECTS {
            let object: NonNull<c_void> = unwrap_ok!(alloc.as_mut().allocate_bytes());
            let id = alloc.alloc_to_id(object);
            expect_lt!(id as usize, NUM_OBJECTS);
        }

        // Attempting another allocation should fail, as we are out of IDs.
        {
            let object: Result<NonNull<TestObject>, _> = alloc.as_mut().allocate_object();
            expect_true!(object.is_err());
        }

        // Free all the objects.
        for i in 0..NUM_OBJECTS {
            let object: NonNull<c_void> = alloc.id_to_alloc(i as u32);
            // SAFETY: `object` was allocated by `alloc`.
            unsafe {
                alloc.as_mut().deallocate_object(object);
            }
        }
    }
}
