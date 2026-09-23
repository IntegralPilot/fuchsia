// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
pub mod tests {
    use crate::binder::{BinderConnection, BinderDevice, BinderDriver, OperationContext};
    use crate::objects::{
        BinderObject, BinderObjectFlags, FlatBinderObject, Handle, LocalBinderObject,
        RefCountActions, SerializedBinderObject, StrongRefGuard, TransactionData,
    };
    use crate::process::{BinderProcess, HandleTable};
    use crate::resource_accessor::{
        RemoteIoctl, RemoteMemoryAccessor, RemoteResourceAccessor, ResourceAccessor,
    };
    use crate::shared_memory::{SharedMemory, TransactionBuffers};
    use crate::thread::{
        BinderThread, Command, QueuedCommand, RegistrationState, RequeueEventRegistration,
        TransactionError, TransactionRole, WeakBinderPeer,
    };
    use crate::user_memory_cursor::UserMemoryCursor;
    use assert_matches::assert_matches;
    use fidl::endpoints::{RequestStream, ServerEnd, create_endpoints};
    use fidl_fuchsia_posix as fposix;
    use fidl_fuchsia_starnix_binder as fbinder;
    use fidl_fuchsia_starnix_binder::FileFlags;
    use fuchsia_async as fasync;
    use fuchsia_async::LocalExecutor;
    use futures::TryStreamExt;
    use memoffset::offset_of;
    use starnix_core::device::DeviceOps;
    use starnix_core::device::mem::new_null_file;
    use starnix_core::fs::fuchsia::sync_file::{SyncFence, SyncFile, SyncPoint, Timeline};
    use starnix_core::mm::memory::MemoryObject;
    use starnix_core::mm::{
        DesiredAddress, MappingName, MappingOptions, MemoryAccessor, MemoryAccessorExt, PAGE_SIZE,
        ProtectionFlags,
    };
    use starnix_core::security;
    use starnix_core::task::{CurrentTask, ExitStatus, Kernel, SimpleWaiter, Waiter};
    use starnix_core::testing::*;
    use starnix_core::vfs::{Anon, FdFlags, FdNumber, FileHandle, FileObject, anon_fs};
    use starnix_logging::log_warn;
    use starnix_sync::InterruptibleEvent;
    use starnix_types::convert::IntoFidl;
    use starnix_types::ownership::{OwnedRef, Releasable, TempRef, WeakRef};
    use starnix_types::user_buffer::UserBuffer;
    use starnix_uapi::auth::Credentials;
    use starnix_uapi::device_id::DeviceId;
    use starnix_uapi::errors::{EBADF, EINVAL, Errno};
    use starnix_uapi::open_flags::OpenFlags;
    use starnix_uapi::union::struct_with_union_into_bytes;
    use starnix_uapi::user_address::{UserAddress, UserRef};
    use starnix_uapi::{
        BINDER_BUFFER_FLAG_HAS_PARENT, BINDER_TYPE_BINDER, BINDER_TYPE_FD, BINDER_TYPE_FDA,
        BINDER_TYPE_HANDLE, BINDER_TYPE_PTR, BINDER_TYPE_WEAK_HANDLE, binder_buffer_object,
        binder_fd_array_object, binder_fd_object, binder_freeze_info, binder_frozen_state_info,
        binder_frozen_status_info, binder_object_header, binder_transaction_data,
        binder_transaction_data__bindgen_ty_1, binder_transaction_data__bindgen_ty_2,
        binder_transaction_data__bindgen_ty_2__bindgen_ty_1, binder_transaction_data_sg,
        binder_uintptr_t, errno, flat_binder_object, transaction_flags_TF_ONE_WAY, uapi,
    };
    use static_assertions::const_assert;
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::ops::Deref;
    use std::sync::{Arc, Weak};
    use zerocopy::{FromBytes, FromZeros, Immutable, IntoBytes, KnownLayout};

    const BASE_ADDR: UserAddress = UserAddress::const_from(0x0000000000000100);
    const VMO_LENGTH: usize = 4096;

    impl ResourceAccessor for AutoReleasableTask {
        fn close_files(&self, fds: Vec<FdNumber>) -> Result<(), Errno> {
            self.deref().close_files(fds)
        }
        fn get_files_with_flags(
            &self,
            current_task: &CurrentTask,
            fds: Vec<FdNumber>,
        ) -> Result<Vec<(FileHandle, FdFlags)>, Errno> {
            self.deref().get_files_with_flags(current_task, fds)
        }
        fn add_files_with_flags(
            &self,
            current_task: &CurrentTask,
            files: Vec<(FileHandle, FdFlags)>,
            add_action: &mut dyn FnMut(FdNumber),
        ) -> Result<Vec<FdNumber>, Errno> {
            self.deref().add_files_with_flags(current_task, files, add_action)
        }
        fn as_memory_accessor(&self) -> Option<&dyn MemoryAccessor> {
            Some(self.deref())
        }
    }

    struct BinderProcessFixture {
        device: Weak<BinderDriver>,
        proc: OwnedRef<BinderProcess>,
        thread: OwnedRef<BinderThread>,
        connection_security_state: security::BinderConnectionState,
        kernel: Arc<Kernel>,
        task: Option<AutoReleasableTask>,
    }

    impl BinderProcessFixture {
        fn new(current_task: &CurrentTask, device: &BinderDevice) -> Self {
            let task = create_task(current_task.kernel(), "task");
            let (proc, thread) = device.create_process_and_thread(task.pid.clone(), &task.task);

            mmap_shared_memory(&device, &task, &proc);
            Self {
                device: Arc::downgrade(device),
                proc,
                thread,
                connection_security_state: security::binder_connection_alloc(&task),
                kernel: current_task.kernel().clone(),
                task: Some(task),
            }
        }

        fn new_current(current_task: &CurrentTask, device: &BinderDevice) -> Self {
            let (proc, thread) =
                device.create_process_and_thread(current_task.pid.clone(), &current_task.task);

            mmap_shared_memory(&device, current_task, &proc);
            Self {
                device: Arc::downgrade(device),
                proc,
                thread,
                connection_security_state: security::binder_connection_alloc(current_task),
                kernel: current_task.kernel().clone(),
                task: None,
            }
        }

        fn lock_shared_memory(&self) -> starnix_sync::MappedLockDepGuard<'_, SharedMemory> {
            starnix_sync::LockDepGuard::map(self.proc.shared_memory.lock(), |value| {
                value.as_mut().unwrap()
            })
        }

        fn task(&self) -> &CurrentTask {
            &self.task.as_ref().unwrap()
        }

        fn context<'a>(&'a self, current_task: &'a CurrentTask) -> OperationContext<'a> {
            let current_task =
                if let Some(task) = self.task.as_ref() { &task } else { current_task };
            OperationContext {
                current_task,
                connection_security_state: &self.connection_security_state,
                binder_proc: &self.proc,
                binder_thread: &self.thread,
                memory_accessor: current_task.as_memory_accessor().expect("as_memory_accessor"),
            }
        }
    }

    impl Drop for BinderProcessFixture {
        fn drop(&mut self) {
            OwnedRef::take(&mut self.thread).release(&self.kernel);
            if let Some(device) = self.device.upgrade() {
                device.procs.write().remove(&self.proc.identifier).release(&self.kernel);
            }
            OwnedRef::take(&mut self.proc).release(&self.kernel);
            if let Some(task) = self.task.as_ref() {
                task.write().set_exit_status_if_not_already(ExitStatus::Exit(0));
            }
        }
    }

    /// Fills the provided shared memory with n buffers, each spanning 1/n-th of the memory.
    fn fill_with_buffers(shared_memory: &mut SharedMemory, n: usize) -> Vec<UserAddress> {
        let mut addresses = vec![];
        for _ in 0..n {
            let address = {
                let buffer = shared_memory
                    .allocate_buffers(VMO_LENGTH / n, 0, 0, 0)
                    .unwrap_or_else(|_| panic!("allocate {n:?}-th buffer"))
                    .data_buffer;
                buffer.memory.user_address + buffer.offset
            };
            addresses.push(address.expect("buffer address range is out of bounds!"));
        }
        addresses
    }

    /// Simulates an mmap call on the binder driver, setting up shared memory between the driver and
    /// `proc`.
    fn mmap_shared_memory(driver: &BinderDriver, current_task: &CurrentTask, proc: &BinderProcess) {
        let prot_flags = ProtectionFlags::READ;
        driver
            .mmap(
                current_task,
                proc,
                DesiredAddress::Any,
                VMO_LENGTH,
                prot_flags,
                MappingOptions::empty(),
                MappingName::None,
            )
            .expect("mmap");
    }

    /// Registers a binder object to `owner`.
    fn register_binder_object(
        owner: &BinderProcess,
        weak_ref_addr: UserAddress,
        strong_ref_addr: UserAddress,
    ) -> (Arc<BinderObject>, StrongRefGuard) {
        let (object, guard) = BinderObject::new(
            owner,
            LocalBinderObject { weak_ref_addr, strong_ref_addr },
            BinderObjectFlags::empty(),
        );
        owner.lock().objects.insert(weak_ref_addr, object.clone());
        (object, guard)
    }

    fn assert_flags_are_equivalent(f1: fbinder::FileFlags, f2: OpenFlags) {
        assert_eq!(f1, f2.into_fidl());
        assert_eq!(f2, f1.into_fidl());
    }

    #[::fuchsia::test]
    fn test_flags_conversion() {
        assert_flags_are_equivalent(
            fbinder::FileFlags::RIGHT_READABLE | fbinder::FileFlags::RIGHT_WRITABLE,
            OpenFlags::RDWR,
        );
        assert_flags_are_equivalent(fbinder::FileFlags::RIGHT_READABLE, OpenFlags::RDONLY);
        assert_flags_are_equivalent(fbinder::FileFlags::RIGHT_WRITABLE, OpenFlags::WRONLY);
        assert_flags_are_equivalent(
            fbinder::FileFlags::RIGHT_READABLE | fbinder::FileFlags::DIRECTORY,
            OpenFlags::DIRECTORY,
        );
    }

    #[fuchsia::test]
    fn handle_tests() {
        assert_matches!(Handle::from(0), Handle::ContextManager);
        assert_matches!(Handle::from(1), Handle::Object { index: 0 });
        assert_matches!(Handle::from(2), Handle::Object { index: 1 });
        assert_matches!(Handle::from(99), Handle::Object { index: 98 });
    }

    #[fuchsia::test]
    async fn handle_0_succeeds_when_context_manager_is_set() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let context_manager =
                BinderObject::new_context_manager_marker(&sender.proc, BinderObjectFlags::empty());
            *device.context_manager.lock() = Some(context_manager.clone());
            let (object, owner) =
                device.get_context_manager(&current_task).expect("failed to find handle 0");
            assert_eq!(OwnedRef::as_ptr(&sender.proc), TempRef::as_ptr(&owner));
            assert!(Arc::ptr_eq(&context_manager, &object));
        })
        .await;
    }

    #[fuchsia::test]
    async fn set_context_mgr_enforces_ebusy_and_euid_lock() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let manager = BinderProcessFixture::new_current(current_task, &device);
            let security_state = security::binder_connection_alloc(current_task);

            device
                .ioctl(
                    current_task,
                    &security_state,
                    &manager.proc,
                    None,
                    uapi::BINDER_SET_CONTEXT_MGR,
                    UserAddress::NULL.into(),
                    Vec::new(),
                )
                .expect("first BINDER_SET_CONTEXT_MGR");

            match device.ioctl(
                current_task,
                &security_state,
                &manager.proc,
                None,
                uapi::BINDER_SET_CONTEXT_MGR,
                UserAddress::NULL.into(),
                Vec::new(),
            ) {
                Err(err) => assert_eq!(err, errno!(EBUSY)),
                Ok(_) => panic!("second SET_CONTEXT_MGR with a live manager succeeded"),
            }

            let other = BinderProcessFixture::new(current_task, &device);
            other.task().set_creds(Credentials::with_ids(1, 1));
            match device.ioctl(
                other.task(),
                &other.connection_security_state,
                &other.proc,
                None,
                uapi::BINDER_SET_CONTEXT_MGR,
                UserAddress::NULL.into(),
                Vec::new(),
            ) {
                Err(err) => assert_eq!(err, errno!(EPERM)),
                Ok(_) => panic!("SET_CONTEXT_MGR from a different euid succeeded"),
            }

            // Same euid may re-register after the previous manager process exits.
            drop(manager);
            let successor = BinderProcessFixture::new_current(current_task, &device);
            device
                .ioctl(
                    current_task,
                    &security::binder_connection_alloc(current_task),
                    &successor.proc,
                    None,
                    uapi::BINDER_SET_CONTEXT_MGR,
                    UserAddress::NULL.into(),
                    Vec::new(),
                )
                .expect("same euid can re-register after manager exits");
        })
        .await;
    }

    #[fuchsia::test]
    async fn fail_to_retrieve_non_existing_handle() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            assert!(&sender.proc.lock().handles.get(3).is_none());
        })
        .await;
    }

    #[fuchsia::test]
    async fn handle_is_not_dropped_after_transaction_finishes_if_it_already_existed() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let proc_1 = BinderProcessFixture::new(current_task, &device);
            let proc_2 = BinderProcessFixture::new(current_task, &device);

            let (transaction_ref, guard) = register_binder_object(
                &proc_1.proc,
                UserAddress::from(0xffffffffffffffff),
                UserAddress::from(0x1111111111111111),
            );
            scopeguard::defer! {
                transaction_ref.ack_acquire(&mut RefCountActions::default_released()).expect("ack_acquire");
                transaction_ref.apply_deferred_refcounts();
            }

            // Insert the transaction once.
            let _ = proc_2
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            let guard = transaction_ref.inc_strong_checked().expect("inc_strong");
            // Insert the same object.
            let handle = proc_2
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // The object should be present in the handle table until a strong decrement.
            let (retrieved_object, guard) =
                proc_2.proc.lock().handles.get(handle.object_index()).expect("valid object");
            assert!(Arc::ptr_eq(&retrieved_object, &transaction_ref));
            guard.release(&mut RefCountActions::default_released());

            // Drop the transaction reference.
            proc_2
                .proc
                .lock()
                .handles
                .dec_strong(handle.object_index(), &mut RefCountActions::default_released())
                .expect("dec_strong");

            // The handle should not have been dropped, as it was already in the table beforehand.
            let (retrieved_object, guard) =
                proc_2.proc.lock().handles.get(handle.object_index()).expect("valid object");
            assert!(Arc::ptr_eq(&retrieved_object, &transaction_ref));
            guard.release(&mut RefCountActions::default_released());
        }).await;
    }

    #[fuchsia::test]
    async fn handle_is_dropped_after_transaction_finishes() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let proc_1 = BinderProcessFixture::new(current_task, &device);
            let proc_2 = BinderProcessFixture::new(current_task, &device);

            let (transaction_ref, guard) = register_binder_object(
                &proc_1.proc,
                UserAddress::from(0xffffffffffffffff),
                UserAddress::from(0x1111111111111111),
            );
            scopeguard::defer! {
                transaction_ref.ack_acquire(&mut RefCountActions::default_released()).expect("ack_acquire");
                transaction_ref.apply_deferred_refcounts();
            }

            // Transactions always take a strong reference to binder objects.
            let handle = proc_2
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // The object should be present in the handle table until a strong decrement.
            let (retrieved_object, guard) =
                proc_2.proc.lock().handles.get(handle.object_index()).expect("valid object");
            assert!(Arc::ptr_eq(&retrieved_object, &transaction_ref));
            guard.release(&mut RefCountActions::default_released());

            // Drop the transaction reference.
            proc_2
                .proc
                .lock()
                .handles
                .dec_strong(handle.object_index(), &mut RefCountActions::default_released())
                .expect("dec_strong");

            // The handle should now have been dropped.
            assert!(proc_2.proc.lock().handles.get(handle.object_index()).is_none());
        }).await;
    }

    #[fuchsia::test]
    async fn handle_is_dropped_after_last_weak_ref_released() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let proc_1 = BinderProcessFixture::new(current_task, &device);
            let proc_2 = BinderProcessFixture::new(current_task, &device);

            let (transaction_ref, guard) = register_binder_object(
                &proc_1.proc,
                UserAddress::from(0xffffffffffffffff),
                UserAddress::from(0x1111111111111111),
            );

            // Keep guard to simulate another process keeping a reference.
            let other_process_guard = transaction_ref.inc_strong_checked();

            scopeguard::defer! {
                // Other process releases the object.
                other_process_guard.release(&mut RefCountActions::default_released());
                // Ack the initial acquire.
                transaction_ref.ack_acquire(&mut RefCountActions::default_released()).expect("ack_acquire");
                transaction_ref.apply_deferred_refcounts();
            }

            // The handle starts with a strong ref.
            let handle = proc_2
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Acquire a weak reference.
            proc_2
                .proc
                .lock()
                .handles
                .inc_weak(handle.object_index(), &mut RefCountActions::default_released())
                .expect("inc_weak");

            // The object should be present in the handle table.
            let (retrieved_object, guard) =
                proc_2.proc.lock().handles.get(handle.object_index()).expect("valid object");
            assert!(Arc::ptr_eq(&retrieved_object, &transaction_ref));
            guard.release(&mut RefCountActions::default_released());

            // Drop the strong reference. The handle should still be present as there is an outstanding
            // weak reference.
            proc_2
                .proc
                .lock()
                .handles
                .dec_strong(handle.object_index(), &mut RefCountActions::default_released())
                .expect("dec_strong");
            let (retrieved_object, guard) =
                proc_2.proc.lock().handles.get(handle.object_index()).expect("valid object");
            assert!(Arc::ptr_eq(&retrieved_object, &transaction_ref));
            guard.release(&mut RefCountActions::default_released());

            // Drop the weak reference. The handle should now be gone, even though the underlying object
            // is still alive (another process could have references to it).
            proc_2
                .proc
                .lock()
                .handles
                .dec_weak(handle.object_index(), &mut RefCountActions::default_released())
                .expect("dec_weak");
            assert!(
                proc_2.proc.lock().handles.get(handle.object_index()).is_none(),
                "handle should be dropped"
            );
        }).await;
    }

    #[fuchsia::test]
    fn shared_memory_allocation_fails_with_invalid_offsets_length() {
        let memory =
            MemoryObject::from(zx::Vmo::create(VMO_LENGTH as u64).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, VMO_LENGTH).expect("failed to map shared memory");
        shared_memory
            .allocate_buffers(3, 1, 0, 0)
            .expect_err("offsets_length should be multiple of 8");
        shared_memory
            .allocate_buffers(3, 8, 1, 0)
            .expect_err("buffers_length should be multiple of 8");
        shared_memory
            .allocate_buffers(3, 8, 0, 1)
            .expect_err("security_context_buffer_length should be multiple of 8");
    }

    #[fuchsia::test]
    fn shared_memory_allocation_aligns_offsets_buffer() {
        let memory =
            MemoryObject::from(zx::Vmo::create(VMO_LENGTH as u64).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, VMO_LENGTH).expect("failed to map shared memory");

        const DATA_LEN: usize = 3;
        const OFFSETS_COUNT: usize = 1;
        const OFFSETS_LEN: usize = std::mem::size_of::<binder_uintptr_t>() * OFFSETS_COUNT;
        const BUFFERS_LEN: usize = 8;
        const SECURITY_CONTEXT_BUFFER_LEN: usize = 24;
        let allocations = shared_memory
            .allocate_buffers(DATA_LEN, OFFSETS_LEN, BUFFERS_LEN, SECURITY_CONTEXT_BUFFER_LEN)
            .expect("allocate buffer");
        assert_eq!(
            allocations.data_buffer.user_buffer(),
            UserBuffer { address: BASE_ADDR, length: DATA_LEN }
        );
        assert_eq!(
            allocations.offsets_buffer.user_buffer(),
            UserBuffer { address: (BASE_ADDR + 8usize).unwrap(), length: OFFSETS_LEN }
        );
        assert_eq!(
            allocations.scatter_gather_buffer.user_buffer(),
            UserBuffer {
                address: (BASE_ADDR + (8usize + OFFSETS_LEN)).unwrap(),
                length: BUFFERS_LEN
            }
        );
        assert_eq!(
            allocations.security_context_buffer.as_ref().expect("security_context").user_buffer(),
            UserBuffer {
                address: (BASE_ADDR + (8usize + OFFSETS_LEN + BUFFERS_LEN)).unwrap(),
                length: SECURITY_CONTEXT_BUFFER_LEN
            }
        );
        assert_eq!(allocations.data_buffer.as_bytes().len(), DATA_LEN);
        assert_eq!(allocations.offsets_buffer.as_bytes().len(), OFFSETS_COUNT);
        assert_eq!(allocations.scatter_gather_buffer.as_bytes().len(), BUFFERS_LEN);
        assert_eq!(
            allocations
                .security_context_buffer
                .as_ref()
                .expect("security_context")
                .as_bytes()
                .len(),
            SECURITY_CONTEXT_BUFFER_LEN
        );
    }

    #[fuchsia::test]
    fn shared_memory_allocation_buffers_correctly_write_through() {
        let memory =
            MemoryObject::from(zx::Vmo::create(VMO_LENGTH as u64).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, VMO_LENGTH).expect("failed to map shared memory");

        const DATA_LEN: usize = 256;
        const OFFSETS_COUNT: usize = 4;
        const OFFSETS_LEN: usize = std::mem::size_of::<binder_uintptr_t>() * OFFSETS_COUNT;
        let mut allocations =
            shared_memory.allocate_buffers(DATA_LEN, OFFSETS_LEN, 0, 0).expect("allocate buffer");

        // Write data to the allocated buffers.
        const DATA_FILL: u8 = 0xff;
        allocations.data_buffer.as_mut_bytes().fill(0xff);

        const OFFSETS_FILL: binder_uintptr_t = 0xDEADBEEFDEADBEEF;
        allocations.offsets_buffer.as_mut_bytes().fill(OFFSETS_FILL);

        // Check that the correct bit patterns were written through to the underlying VMO.
        let mut data = [0u8; DATA_LEN];
        memory.read(&mut data, 0).expect("read VMO failed");
        assert!(data.iter().all(|b| *b == DATA_FILL));

        let mut data = [0u64; OFFSETS_COUNT];
        memory.read(data.as_mut_bytes(), DATA_LEN as u64).expect("read VMO failed");
        assert!(data.iter().all(|b| *b == OFFSETS_FILL));
    }

    #[fuchsia::test]
    fn shared_memory_allocates_multiple_buffers() {
        let memory =
            MemoryObject::from(zx::Vmo::create(VMO_LENGTH as u64).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, VMO_LENGTH).expect("failed to map shared memory");

        // Check that two buffers allocated from the same shared memory region don't overlap.
        const BUF1_DATA_LEN: usize = 64;
        const BUF1_OFFSETS_LEN: usize = 8;
        const BUF1_BUFFERS_LEN: usize = 8;
        const BUF1_SECURITY_CONTEXT_BUFFER_LEN: usize = 8;
        let allocations = shared_memory
            .allocate_buffers(
                BUF1_DATA_LEN,
                BUF1_OFFSETS_LEN,
                BUF1_BUFFERS_LEN,
                BUF1_SECURITY_CONTEXT_BUFFER_LEN,
            )
            .expect("allocate buffer 1");
        assert_eq!(
            allocations.data_buffer.user_buffer(),
            UserBuffer { address: BASE_ADDR, length: BUF1_DATA_LEN }
        );
        assert_eq!(
            allocations.offsets_buffer.user_buffer(),
            UserBuffer {
                address: BASE_ADDR.checked_add(BUF1_DATA_LEN).unwrap(),
                length: BUF1_OFFSETS_LEN
            }
        );
        assert_eq!(
            allocations.scatter_gather_buffer.user_buffer(),
            UserBuffer {
                address: BASE_ADDR
                    .checked_add(BUF1_DATA_LEN)
                    .unwrap()
                    .checked_add(BUF1_OFFSETS_LEN)
                    .unwrap(),
                length: BUF1_BUFFERS_LEN
            }
        );
        assert_eq!(
            allocations.security_context_buffer.expect("security_context").user_buffer(),
            UserBuffer {
                address: BASE_ADDR
                    .checked_add(BUF1_DATA_LEN)
                    .unwrap()
                    .checked_add(BUF1_OFFSETS_LEN)
                    .unwrap()
                    .checked_add(BUF1_BUFFERS_LEN)
                    .unwrap(),
                length: BUF1_SECURITY_CONTEXT_BUFFER_LEN
            }
        );

        const BUF2_DATA_LEN: usize = 32;
        const BUF2_OFFSETS_LEN: usize = 0;
        const BUF2_BUFFERS_LEN: usize = 0;
        const BUF2_SECURITY_CONTEXT_BUFFER_LEN: usize = 0;
        let allocations = shared_memory
            .allocate_buffers(
                BUF2_DATA_LEN,
                BUF2_OFFSETS_LEN,
                BUF2_BUFFERS_LEN,
                BUF2_SECURITY_CONTEXT_BUFFER_LEN,
            )
            .expect("allocate buffer 2");
        assert_eq!(
            allocations.data_buffer.user_buffer(),
            UserBuffer {
                address: BASE_ADDR
                    .checked_add(BUF1_DATA_LEN)
                    .unwrap()
                    .checked_add(BUF1_OFFSETS_LEN)
                    .unwrap()
                    .checked_add(BUF1_BUFFERS_LEN)
                    .unwrap()
                    .checked_add(BUF1_SECURITY_CONTEXT_BUFFER_LEN)
                    .unwrap(),
                length: BUF2_DATA_LEN
            }
        );
        assert_eq!(
            allocations.offsets_buffer.user_buffer(),
            UserBuffer {
                address: BASE_ADDR
                    .checked_add(BUF1_DATA_LEN)
                    .unwrap()
                    .checked_add(BUF1_OFFSETS_LEN)
                    .unwrap()
                    .checked_add(BUF1_BUFFERS_LEN)
                    .unwrap()
                    .checked_add(BUF1_SECURITY_CONTEXT_BUFFER_LEN)
                    .unwrap()
                    .checked_add(BUF2_DATA_LEN)
                    .unwrap(),
                length: BUF2_OFFSETS_LEN
            }
        );
    }

    #[fuchsia::test]
    fn shared_memory_too_large_allocation_fails() {
        let memory =
            MemoryObject::from(zx::Vmo::create(VMO_LENGTH as u64).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, VMO_LENGTH).expect("failed to map shared memory");

        shared_memory
            .allocate_buffers(VMO_LENGTH + 1, 0, 0, 0)
            .expect_err("out-of-bounds allocation");
        shared_memory.allocate_buffers(VMO_LENGTH, 8, 0, 0).expect_err("out-of-bounds allocation");
        shared_memory
            .allocate_buffers(VMO_LENGTH - 8, 8, 8, 0)
            .expect_err("out-of-bounds allocation");

        shared_memory.allocate_buffers(VMO_LENGTH, 0, 0, 0).expect("allocate buffer");

        // Now that the previous buffer allocation succeeded, there should be no more room.
        shared_memory.allocate_buffers(1, 0, 0, 0).expect_err("out-of-bounds allocation");
    }

    #[fuchsia::test]
    fn shared_memory_allocation_wraps_in_order() {
        let memory =
            MemoryObject::from(zx::Vmo::create(VMO_LENGTH as u64).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, VMO_LENGTH).expect("failed to map shared memory");
        let n = 4;

        for buffer in fill_with_buffers(&mut shared_memory, n) {
            shared_memory
                .allocate_buffers(VMO_LENGTH / n, 0, 0, 0)
                .expect_err(&format!("allocated buffer when shared memory was full {n:?}"));

            shared_memory.free_buffer(buffer).expect("didn't free buffer");

            shared_memory.allocate_buffers(VMO_LENGTH / n, 0, 0, 0).unwrap_or_else(|_| {
                panic!("couldn't allocate new buffer even after {n:?}-th was released")
            });
        }
    }

    #[fuchsia::test]
    fn shared_memory_allocation_single() {
        let memory =
            MemoryObject::from(zx::Vmo::create(VMO_LENGTH as u64).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, VMO_LENGTH).expect("failed to map shared memory");
        let n = 1;
        let buffers = fill_with_buffers(&mut shared_memory, n);

        shared_memory
            .allocate_buffers(VMO_LENGTH / n, 0, 0, 0)
            .expect_err("could allocate when buffer was full");
        shared_memory.free_buffer(buffers[0]).expect("didn't free buffer");
        shared_memory
            .allocate_buffers(VMO_LENGTH / n, 0, 0, 0)
            .expect("couldn't allocate even after first slot opened up");
    }

    #[fuchsia::test]
    fn shared_memory_allocation_can_allocate_in_hole() {
        let memory =
            MemoryObject::from(zx::Vmo::create(VMO_LENGTH as u64).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, VMO_LENGTH).expect("failed to map shared memory");
        let n = 4;

        let buffers = fill_with_buffers(&mut shared_memory, n);

        // Free all the buffers in reverse order, and verify that the new buffer isn't allocated
        // until the first buffer is freed.
        shared_memory
            .allocate_buffers(VMO_LENGTH / n, 0, 0, 0)
            .expect_err("cannot allocate when full");
        shared_memory.free_buffer(buffers[1]).expect("didn't free buffer");
        shared_memory.allocate_buffers(VMO_LENGTH / n, 0, 0, 0).expect("can allocate in hole");
    }

    #[fuchsia::test]
    fn shared_memory_allocation_doesnt_wrap_when_cant_fit() {
        let memory =
            MemoryObject::from(zx::Vmo::create(VMO_LENGTH as u64).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, VMO_LENGTH).expect("failed to map shared memory");
        let n = 4;

        let buffers = fill_with_buffers(&mut shared_memory, n);

        shared_memory.free_buffer(buffers[0]).expect("didn't free buffer");
        // Allocate slightly more than what can fit at the start (after freeing the first 1/4th).
        shared_memory
            .allocate_buffers((VMO_LENGTH / n) + 1, 0, 0, 0)
            .expect_err("allocated over existing buffer");
        shared_memory.free_buffer(buffers[1]).expect("didn't free buffer");
        shared_memory
            .allocate_buffers((VMO_LENGTH / n) + 1, 0, 0, 0)
            .expect("couldn't allocate when there was enough space");
    }

    #[fuchsia::test]
    fn shared_memory_allocation_doesnt_wrap_when_cant_fit_at_end() {
        let memory =
            MemoryObject::from(zx::Vmo::create(VMO_LENGTH as u64).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, VMO_LENGTH).expect("failed to map shared memory");

        // Test that a buffer can still be allocated even if it can't fit at the end, but can fit
        // at the start by first allocating 3/4 of the memory.
        let buffer_1 = {
            let allocations =
                shared_memory.allocate_buffers(VMO_LENGTH / 4, 0, 0, 0).expect("couldn't allocate");
            (allocations.data_buffer.memory.user_address + allocations.data_buffer.offset).unwrap()
        };
        let buffer_2 = {
            let allocations =
                shared_memory.allocate_buffers(VMO_LENGTH / 4, 0, 0, 0).expect("couldn't allocate");
            (allocations.data_buffer.memory.user_address + allocations.data_buffer.offset).unwrap()
        };
        let buffer_3 = {
            let allocations =
                shared_memory.allocate_buffers(VMO_LENGTH / 4, 0, 0, 0).expect("couldn't allocate");
            (allocations.data_buffer.memory.user_address + allocations.data_buffer.offset).unwrap()
        };

        // Attempt to allocate a buffer at the end that is larger than 1/4th.
        shared_memory
            .allocate_buffers(VMO_LENGTH / 3, 0, 0, 0)
            .expect_err("allocated over existing buffer");
        // Now free all the buffers at the start.
        shared_memory.free_buffer(buffer_1).expect("didn't free buffer");
        shared_memory.free_buffer(buffer_2).expect("didn't free buffer");
        shared_memory.free_buffer(buffer_3).expect("didn't free buffer");

        // Try the allocation again.
        shared_memory
            .allocate_buffers(VMO_LENGTH / 3, 0, 0, 0)
            .expect("failed even though there was room at start");
    }

    #[fuchsia::test]
    fn shared_memory_best_fit_and_coalescing() {
        let memory = MemoryObject::from(zx::Vmo::create(1024).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, 1024).expect("failed to map shared memory");

        // Allocate 4 chunks of 256 bytes each (A: 0..256, B: 256..512, C: 512..768, D: 768..1024)
        let addr_a = (BASE_ADDR
            + shared_memory.allocate_buffers(256, 0, 0, 0).unwrap().data_buffer.offset)
            .unwrap();
        let addr_b = (BASE_ADDR
            + shared_memory.allocate_buffers(256, 0, 0, 0).unwrap().data_buffer.offset)
            .unwrap();
        let addr_c = (BASE_ADDR
            + shared_memory.allocate_buffers(256, 0, 0, 0).unwrap().data_buffer.offset)
            .unwrap();
        let addr_d = (BASE_ADDR
            + shared_memory.allocate_buffers(256, 0, 0, 0).unwrap().data_buffer.offset)
            .unwrap();

        // Memory is full.
        shared_memory.allocate_buffers(8, 0, 0, 0).expect_err("memory should be full");

        // Free B (hole 256..512) and D (hole 768..1024)
        shared_memory.free_buffer(addr_b).expect("free B");
        shared_memory.free_buffer(addr_d).expect("free D");

        // Best-fit test: allocating 128 bytes should pick the smallest fitting hole (e.g. B at 256)
        let (small_offset, small_addr) = {
            let alloc_small = shared_memory.allocate_buffers(128, 0, 0, 0).expect("allocate small");
            (alloc_small.data_buffer.offset, (BASE_ADDR + alloc_small.data_buffer.offset).unwrap())
        };
        assert_eq!(small_offset, 256);

        // Free A (0..256), coalescing with remaining free space in slot B (384..512) when small is freed.
        shared_memory.free_buffer(addr_a).expect("free A");
        shared_memory.free_buffer(small_addr).expect("free small");

        // Now 0..512 is completely coalesced into a single 512-byte free chunk!
        let (large_offset, large_addr) = {
            let alloc_large =
                shared_memory.allocate_buffers(512, 0, 0, 0).expect("allocate 512 bytes");
            (alloc_large.data_buffer.offset, (BASE_ADDR + alloc_large.data_buffer.offset).unwrap())
        };
        assert_eq!(large_offset, 0);

        // Free C (512..768) and large (0..512). Since D was already freed, all 1024 bytes are coalesced!
        shared_memory.free_buffer(addr_c).expect("free C");
        shared_memory.free_buffer(large_addr).expect("free large");

        // Can allocate the entire 1024-byte buffer in one chunk!
        let alloc_full = shared_memory.allocate_buffers(1024, 0, 0, 0).expect("allocate full");
        assert_eq!(alloc_full.data_buffer.offset, 0);
    }

    #[fuchsia::test]
    fn shared_memory_invalid_and_double_free() {
        let memory = MemoryObject::from(zx::Vmo::create(1024).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, 1024).expect("failed to map shared memory");

        let addr = {
            let alloc = shared_memory.allocate_buffers(256, 0, 0, 0).expect("allocate 256");
            (BASE_ADDR + alloc.data_buffer.offset).unwrap()
        };

        // Valid free
        shared_memory.free_buffer(addr).expect("valid free");

        // Double free must fail with EINVAL
        assert_eq!(shared_memory.free_buffer(addr), Err(errno!(EINVAL)));

        // Freeing unmapped address must fail with EINVAL
        assert_eq!(shared_memory.free_buffer(UserAddress::from(0xdeadbeef)), Err(errno!(EINVAL)));
        assert_eq!(
            shared_memory.free_buffer((BASE_ADDR + 5000usize).unwrap()),
            Err(errno!(EINVAL))
        );
    }

    #[fuchsia::test]
    fn shared_memory_exact_fit_allocation() {
        let memory = MemoryObject::from(zx::Vmo::create(1024).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, 1024).expect("failed to map shared memory");

        // Allocate three 256-byte chunks: A (0..256), B (256..512), C (512..768)
        let addr_a = (BASE_ADDR
            + shared_memory.allocate_buffers(256, 0, 0, 0).unwrap().data_buffer.offset)
            .unwrap();
        let addr_b = (BASE_ADDR
            + shared_memory.allocate_buffers(256, 0, 0, 0).unwrap().data_buffer.offset)
            .unwrap();
        let addr_c = (BASE_ADDR
            + shared_memory.allocate_buffers(256, 0, 0, 0).unwrap().data_buffer.offset)
            .unwrap();

        // Free middle chunk B (hole of exactly 256 bytes)
        shared_memory.free_buffer(addr_b).expect("free B");

        // Allocate exact-fit chunk of 256 bytes (remainder_size == 0)
        let exact_addr = {
            let exact_alloc =
                shared_memory.allocate_buffers(256, 0, 0, 0).expect("exact-fit allocate");
            assert_eq!(exact_alloc.data_buffer.offset, 256);
            (BASE_ADDR + exact_alloc.data_buffer.offset).unwrap()
        };

        // Free all chunks and ensure full coalescing back to 1024 bytes
        shared_memory.free_buffer(addr_a).expect("free A");
        shared_memory.free_buffer(addr_c).expect("free C");
        shared_memory.free_buffer(exact_addr).expect("free exact alloc");

        let full_alloc =
            shared_memory.allocate_buffers(1024, 0, 0, 0).expect("full buffer allocate");
        assert_eq!(full_alloc.data_buffer.offset, 0);
    }

    #[fuchsia::test]
    fn shared_memory_zero_length_mapping() {
        let memory = MemoryObject::from(zx::Vmo::create(1024).expect("failed to create VMO"));
        assert_eq!(SharedMemory::map(&memory, BASE_ADDR, 0).unwrap_err(), errno!(EINVAL));
    }

    #[fuchsia::test]
    fn shared_memory_churn_coalescing() {
        let memory = MemoryObject::from(zx::Vmo::create(2048).expect("failed to create VMO"));
        let mut shared_memory =
            SharedMemory::map(&memory, BASE_ADDR, 2048).expect("failed to map shared memory");

        // Allocate chunks of varying sizes: [64, 128, 256, 512, 128, 64, 256] (sum = 1408)
        let sizes = [64, 128, 256, 512, 128, 64, 256];
        let mut addrs = Vec::new();
        for &sz in &sizes {
            let addr = {
                let alloc = shared_memory.allocate_buffers(sz, 0, 0, 0).unwrap();
                (BASE_ADDR + alloc.data_buffer.offset).unwrap()
            };
            addrs.push(addr);
        }

        // Free odd indices first: [1 (128), 3 (512), 5 (64)]
        shared_memory.free_buffer(addrs[1]).expect("free 1");
        shared_memory.free_buffer(addrs[3]).expect("free 3");
        shared_memory.free_buffer(addrs[5]).expect("free 5");

        // Allocate into freed holes (e.g. 512-byte hole at index 3)
        let addr_hole = {
            let alloc_hole = shared_memory.allocate_buffers(512, 0, 0, 0).unwrap();
            (BASE_ADDR + alloc_hole.data_buffer.offset).unwrap()
        };

        // Free remaining original allocations and the hole allocation
        shared_memory.free_buffer(addrs[0]).expect("free 0");
        shared_memory.free_buffer(addrs[2]).expect("free 2");
        shared_memory.free_buffer(addrs[4]).expect("free 4");
        shared_memory.free_buffer(addrs[6]).expect("free 6");
        shared_memory.free_buffer(addr_hole).expect("free hole");

        // Everything is coalesced back into a single 2048-byte region
        let full = shared_memory.allocate_buffers(2048, 0, 0, 0).unwrap();
        assert_eq!(full.data_buffer.offset, 0);
    }

    #[fuchsia::test]
    async fn binder_object_enqueues_release_command_when_dropped() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let proc = BinderProcessFixture::new(current_task, &device);

            const LOCAL_BINDER_OBJECT: LocalBinderObject = LocalBinderObject {
                weak_ref_addr: UserAddress::const_from(0x0000000000000010),
                strong_ref_addr: UserAddress::const_from(0x0000000000000100),
            };

            let (object, guard) = register_binder_object(
                &proc.proc,
                LOCAL_BINDER_OBJECT.weak_ref_addr,
                LOCAL_BINDER_OBJECT.strong_ref_addr,
            );

            let mut actions = RefCountActions::default();
            object.ack_acquire(&mut actions).expect("ack_acquire");
            guard.release(&mut actions);
            actions.release(());

            assert_matches!(
                &proc.proc.lock().command_queue.front(),
                Some(QueuedCommand { command: Command::ReleaseRef(LOCAL_BINDER_OBJECT), .. })
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn handle_table_refs() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let proc = BinderProcessFixture::new(current_task, &device);

            let (object, guard) = register_binder_object(
                &proc.proc,
                UserAddress::from(0x0000000000000010),
                UserAddress::from(0x0000000000000100),
            );

            // Simulate another process keeping a strong reference.
            let other_process_guard = object.inc_strong_checked();

            let mut handle_table = HandleTable::default();

            // Starts with one strong reference.
            let handle = handle_table
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            handle_table
                .inc_strong(handle.object_index(), &mut RefCountActions::default_released())
                .expect("inc_strong 1");
            handle_table
                .inc_strong(handle.object_index(), &mut RefCountActions::default_released())
                .expect("inc_strong 2");
            handle_table
                .inc_weak(handle.object_index(), &mut RefCountActions::default_released())
                .expect("inc_weak 0");
            handle_table
                .dec_strong(handle.object_index(), &mut RefCountActions::default_released())
                .expect("dec_strong 2");
            handle_table
                .dec_strong(handle.object_index(), &mut RefCountActions::default_released())
                .expect("dec_strong 1");

            // Remove the initial strong reference.
            handle_table
                .dec_strong(handle.object_index(), &mut RefCountActions::default_released())
                .expect("dec_strong 0");

            // Removing more strong references should fail.
            handle_table
                .dec_strong(handle.object_index(), &mut RefCountActions::default_released())
                .expect_err("dec_strong -1");

            // The object should still take up an entry in the handle table, as there is 1 weak
            // reference and it is maintained alive by another process.
            let (_, guard) = handle_table.get(handle.object_index()).expect("object still exists");
            guard.release(&mut RefCountActions::default_released());

            // Simulate another process droppping its reference.
            other_process_guard.release(&mut RefCountActions::default_released());
            object.apply_deferred_refcounts();
            // Ack the initial acquire.
            object.ack_acquire(&mut RefCountActions::default_released()).expect("ack_acquire");
            // Ack the subsequent incref from the test.
            object.ack_incref(&mut RefCountActions::default_released()).expect("ack_incref");

            // Our weak reference won't keep the object alive.
            assert!(handle_table.get(handle.object_index()).is_none(), "object should be dead");

            // Remove from our table.
            handle_table
                .dec_weak(handle.object_index(), &mut RefCountActions::default_released())
                .expect("dec_weak 0");
            object.apply_deferred_refcounts();

            // Another removal attempt will prove the handle has been removed.
            handle_table
                .dec_weak(handle.object_index(), &mut RefCountActions::default_released())
                .expect_err("handle should no longer exist");
        })
        .await;
    }

    #[fuchsia::test]
    fn serialize_binder_handle() {
        let mut output = [0u8; std::mem::size_of::<flat_binder_object>()];

        SerializedBinderObject::Handle {
            handle: 2.into(),
            flags: BinderObjectFlags::parse(42).expect("parse"),
            cookie: 99,
        }
        .write_to(&mut output)
        .expect("write handle");
        assert_eq!(
            struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_HANDLE,
                flags: 42,
                cookie: 99,
                __bindgen_anon_1.handle: 2,
            }),
            output
        );
    }

    #[fuchsia::test]
    fn serialize_binder_object() {
        let mut output = [0u8; std::mem::size_of::<flat_binder_object>()];

        SerializedBinderObject::Object {
            local: LocalBinderObject {
                weak_ref_addr: UserAddress::from(0xDEADBEEF),
                strong_ref_addr: UserAddress::from(0xDEADDEAD),
            },
            flags: BinderObjectFlags::parse(42).expect("parse"),
        }
        .write_to(&mut output)
        .expect("write object");
        assert_eq!(
            struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_BINDER,
                flags: 42,
                cookie: 0xDEADDEAD,
                __bindgen_anon_1.binder: 0xDEADBEEF,
            }),
            output
        );
    }

    #[fuchsia::test]
    fn serialize_binder_fd() {
        let mut output = [0u8; std::mem::size_of::<flat_binder_object>()];

        SerializedBinderObject::File { fd: FdNumber::from_raw(2), cookie: 99 }
            .write_to(&mut output)
            .expect("write fd");

        let (output_fd_object, _) = binder_fd_object::read_from_prefix(&output)
            .expect("output ought be a binder_fd_object");
        assert_eq!(BINDER_TYPE_FD, output_fd_object.hdr.type_);
        assert_eq!(99, output_fd_object.cookie);
        // SAFETY: Union read.
        assert_eq!(2, unsafe { output_fd_object.__bindgen_anon_1.fd });
    }

    #[fuchsia::test]
    fn serialize_binder_buffer() {
        let mut output = [0u8; std::mem::size_of::<binder_buffer_object>()];

        SerializedBinderObject::Buffer {
            buffer: UserAddress::from(0xDEADBEEF),
            length: 0x100,
            parent: 1,
            parent_offset: 20,
            flags: 42,
        }
        .write_to(&mut output)
        .expect("write buffer");
        assert_eq!(
            binder_buffer_object {
                hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                buffer: 0xDEADBEEF,
                length: 0x100,
                parent: 1,
                parent_offset: 20,
                flags: 42,
            }
            .as_bytes(),
            output
        );
    }

    #[fuchsia::test]
    fn serialize_binder_fd_array() {
        let mut output = [0u8; std::mem::size_of::<binder_fd_array_object>()];

        SerializedBinderObject::FileArray { num_fds: 2, parent: 1, parent_offset: 20 }
            .write_to(&mut output)
            .expect("write fd array");
        assert_eq!(
            binder_fd_array_object {
                hdr: binder_object_header { type_: BINDER_TYPE_FDA },
                num_fds: 2,
                parent: 1,
                parent_offset: 20,
                pad: 0,
            }
            .as_bytes(),
            output
        );
    }

    #[fuchsia::test]
    fn serialize_binder_buffer_too_small() {
        let mut output = [0u8; std::mem::size_of::<binder_uintptr_t>()];
        SerializedBinderObject::Handle {
            handle: 2.into(),
            flags: BinderObjectFlags::parse(42).expect("parse"),
            cookie: 99,
        }
        .write_to(&mut output)
        .expect_err("write handle should not succeed");
        SerializedBinderObject::Object {
            local: LocalBinderObject {
                weak_ref_addr: UserAddress::from(0xDEADBEEF),
                strong_ref_addr: UserAddress::from(0xDEADDEAD),
            },
            flags: BinderObjectFlags::parse(42).expect("parse"),
        }
        .write_to(&mut output)
        .expect_err("write object should not succeed");
        SerializedBinderObject::File { fd: FdNumber::from_raw(2), cookie: 99 }
            .write_to(&mut output)
            .expect_err("write fd should not succeed");
    }

    #[fuchsia::test]
    fn deserialize_binder_handle() {
        let input = struct_with_union_into_bytes!(flat_binder_object {
            hdr.type_: BINDER_TYPE_HANDLE,
            flags: 42,
            cookie: 99,
            __bindgen_anon_1.handle: 2,
        });
        assert_eq!(
            SerializedBinderObject::from_bytes(&input).expect("read handle"),
            SerializedBinderObject::Handle {
                handle: 2.into(),
                flags: BinderObjectFlags::parse(42).expect("parse"),
                cookie: 99
            }
        );
    }

    #[fuchsia::test]
    fn deserialize_binder_object() {
        let input = struct_with_union_into_bytes!(flat_binder_object {
            hdr.type_: BINDER_TYPE_BINDER,
            flags: 42,
            cookie: 0xDEADDEAD,
            __bindgen_anon_1.binder: 0xDEADBEEF,
        });
        assert_eq!(
            SerializedBinderObject::from_bytes(&input).expect("read object"),
            SerializedBinderObject::Object {
                local: LocalBinderObject {
                    weak_ref_addr: UserAddress::from(0xDEADBEEF),
                    strong_ref_addr: UserAddress::from(0xDEADDEAD)
                },
                flags: BinderObjectFlags::parse(42).expect("parse")
            }
        );
    }

    #[fuchsia::test]
    fn deserialize_binder_fd() {
        let input = struct_with_union_into_bytes!(binder_fd_object {
            hdr.type_: BINDER_TYPE_FD,
            pad_flags: 0xdeadbeef,
            cookie: 99,
            __bindgen_anon_1.fd: 2,
        });
        assert_eq!(
            SerializedBinderObject::from_bytes(&input).expect("read handle"),
            SerializedBinderObject::File { fd: FdNumber::from_raw(2), cookie: 99 }
        );
    }

    #[fuchsia::test]
    fn deserialize_binder_buffer() {
        let input = binder_buffer_object {
            hdr: binder_object_header { type_: BINDER_TYPE_PTR },
            buffer: 0xDEADBEEF,
            length: 0x100,
            parent: 1,
            parent_offset: 20,
            flags: 42,
        };
        assert_eq!(
            SerializedBinderObject::from_bytes(input.as_bytes()).expect("read buffer"),
            SerializedBinderObject::Buffer {
                buffer: UserAddress::from(0xDEADBEEF),
                length: 0x100,
                parent: 1,
                parent_offset: 20,
                flags: 42,
            }
        );
    }

    #[fuchsia::test]
    fn deserialize_binder_fd_array() {
        let input = binder_fd_array_object {
            hdr: binder_object_header { type_: BINDER_TYPE_FDA },
            num_fds: 2,
            pad: 0,
            parent: 1,
            parent_offset: 20,
        };
        assert_eq!(
            SerializedBinderObject::from_bytes(input.as_bytes()).expect("read fd array"),
            SerializedBinderObject::FileArray { num_fds: 2, parent: 1, parent_offset: 20 }
        );
    }

    #[fuchsia::test]
    fn deserialize_unknown_object() {
        let input = struct_with_union_into_bytes!(flat_binder_object {
            hdr.type_: 9001,
            flags: 42,
            cookie: 99,
            __bindgen_anon_1.handle: 2,
        });
        SerializedBinderObject::from_bytes(&input).expect_err("read unknown object");
    }

    #[fuchsia::test]
    fn deserialize_input_too_small() {
        let input = struct_with_union_into_bytes!(binder_fd_object {
            hdr.type_: BINDER_TYPE_FD,
            pad_flags: 0xdeadbeef,
            cookie: 99,
            __bindgen_anon_1.fd: 2,
        });
        SerializedBinderObject::from_bytes(&input[..std::mem::size_of::<binder_uintptr_t>()])
            .expect_err("read buffer too small");
    }

    #[fuchsia::test]
    fn deserialize_unaligned() {
        let input = struct_with_union_into_bytes!(flat_binder_object {
            hdr.type_: BINDER_TYPE_HANDLE,
            flags: 42,
            cookie: 99,
            __bindgen_anon_1.handle: 2,
        });
        let mut unaligned_input = vec![];
        unaligned_input.push(0u8);
        unaligned_input.extend(input);
        SerializedBinderObject::from_bytes(&unaligned_input[1..]).expect("read unaligned object");
    }

    #[fuchsia::test]
    async fn copy_transaction_data_between_processes() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Explicitly install a VMO that we can read from later.
            let memory = MemoryObject::from(
                zx::Vmo::create(VMO_LENGTH as u64).expect("failed to create VMO"),
            );
            *receiver.proc.shared_memory.lock() = Some(
                SharedMemory::map(&memory, BASE_ADDR, VMO_LENGTH)
                    .expect("failed to map shared memory"),
            );

            // Map some memory for process 1.
            let data_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);

            // Write transaction data in process 1.
            const BINDER_DATA: &[u8; 8] = b"binder!!";
            let mut transaction_data = vec![];
            transaction_data.extend(BINDER_DATA);
            transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr: binder_object_header { type_: BINDER_TYPE_HANDLE },
                flags: 0,
                __bindgen_anon_1.handle: 0,
                cookie: 0,
            }));

            let offsets_addr = (data_addr
                + current_task
                    .write_memory(data_addr, &transaction_data)
                    .expect("failed to write transaction data"))
            .unwrap();

            // Write the offsets data (where in the data buffer `flat_binder_object`s are).
            let offsets_data: u64 = BINDER_DATA.len() as u64;
            current_task
                .write_object(UserRef::new(offsets_addr), &offsets_data)
                .expect("failed to write offsets buffer");

            // Construct the `binder_transaction_data` struct that contains pointers to the data and
            // offsets buffers.
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: 1,
                    flags: 0,
                    sender_pid: sender.proc.key.id,
                    sender_euid: 0,
                    target: binder_transaction_data__bindgen_ty_1 { handle: 0 },
                    cookie: 0,
                    data_size: transaction_data.len() as u64,
                    offsets_size: std::mem::size_of::<u64>() as u64,
                    data: binder_transaction_data__bindgen_ty_2 {
                        ptr: binder_transaction_data__bindgen_ty_2__bindgen_ty_1 {
                            buffer: data_addr.ptr() as u64,
                            offsets: offsets_addr.ptr() as u64,
                        },
                    },
                },
                buffers_size: 0,
            };

            // Copy the data from process 1 to process 2
            let security_context = "hello\0".into();
            let (buffers, transaction_state) = device
                .copy_transaction_buffers(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    &receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &transaction,
                    Some(security_context),
                )
                .expect("copy data");
            let data_buffer = buffers.data;
            let offsets_buffer = buffers.offsets;
            let security_context_buffer = buffers.security_context.expect("security_context");

            // Check that the returned buffers are in-bounds of process 2's shared memory.
            assert!(data_buffer.address >= BASE_ADDR);
            assert!(data_buffer.address < BASE_ADDR.checked_add(VMO_LENGTH).unwrap());
            assert!(offsets_buffer.address >= BASE_ADDR);
            assert!(offsets_buffer.address < BASE_ADDR.checked_add(VMO_LENGTH).unwrap());
            assert!(security_context_buffer.address >= BASE_ADDR);
            assert!(security_context_buffer.address < BASE_ADDR.checked_add(VMO_LENGTH).unwrap());

            // Verify the contents of the copied data in process 2's shared memory VMO.
            let mut buffer = [0u8; BINDER_DATA.len() + std::mem::size_of::<flat_binder_object>()];
            memory
                .read(&mut buffer, (data_buffer.address - BASE_ADDR) as u64)
                .expect("failed to read data");
            assert_eq!(&buffer[..], &transaction_data);

            let mut buffer = [0u8; std::mem::size_of::<u64>()];
            memory
                .read(&mut buffer, (offsets_buffer.address - BASE_ADDR) as u64)
                .expect("failed to read offsets");
            assert_eq!(&buffer[..], offsets_data.as_bytes());
            let mut buffer = vec![0u8; security_context.len()];
            memory
                .read(&mut buffer[..], (security_context_buffer.address - BASE_ADDR) as u64)
                .expect("failed to read security_context");
            assert_eq!(&buffer[..], security_context);
            transaction_state.release(());
        })
        .await;
    }

    #[fuchsia::test]
    async fn transaction_translate_binder_leaving_process() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);
            let mut receiver_shared_memory = receiver.lock_shared_memory();
            let mut allocations =
                receiver_shared_memory.allocate_buffers(0, 0, 0, 0).expect("allocate buffers");

            const BINDER_OBJECT: LocalBinderObject = LocalBinderObject {
                weak_ref_addr: UserAddress::const_from(0x0000000000000010),
                strong_ref_addr: UserAddress::const_from(0x0000000000000100),
            };

            const DATA_PREAMBLE: &[u8; 5] = b"stuff";

            let mut transaction_data = vec![];
            transaction_data.extend(DATA_PREAMBLE);
            let offsets = [transaction_data.len() as binder_uintptr_t];
            transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_BINDER,
                flags: 0,
                cookie: BINDER_OBJECT.strong_ref_addr.ptr() as u64,
                __bindgen_anon_1.binder: BINDER_OBJECT.weak_ref_addr.ptr() as u64,
            }));

            const EXPECTED_HANDLE: Handle = Handle::from_raw(1);

            let transaction_state = device
                .translate_objects(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &offsets,
                    &mut transaction_data,
                    &mut allocations.scatter_gather_buffer,
                )
                .expect("failed to translate handles");

            // Verify that the new handle was returned in `transaction_state` so that it gets dropped
            // at the end of the transaction.
            assert_eq!(transaction_state.state.as_ref().unwrap().handles[0], EXPECTED_HANDLE);

            // Verify that the transaction data was mutated.
            let mut expected_transaction_data = vec![];
            expected_transaction_data.extend(DATA_PREAMBLE);
            expected_transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_HANDLE,
                flags: 0,
                cookie: 0,
                __bindgen_anon_1.handle: EXPECTED_HANDLE.into(),
            }));
            assert_eq!(&expected_transaction_data, &transaction_data);

            // Verify that a handle was created in the receiver.
            let (object, guard) = receiver
                .proc
                .lock()
                .handles
                .get(EXPECTED_HANDLE.object_index())
                .expect("expected handle not present");
            guard.release(&mut RefCountActions::default_released());
            assert_eq!(object.owner.as_ptr(), OwnedRef::as_ptr(&sender.proc));
            assert_eq!(object.local, BINDER_OBJECT);

            // Verify that a strong acquire command is sent to the sender process (on the same thread
            // that sent the transaction).
            assert_matches!(
                sender.thread.lock().command_queue.commands.front(),
                Some(QueuedCommand { command: Command::AcquireRef(BINDER_OBJECT), .. })
            );
            transaction_state.release(());
        })
        .await;
    }

    #[fuchsia::test]
    async fn transaction_translate_binder_handle_entering_owning_process() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);
            let mut receiver_shared_memory = receiver.lock_shared_memory();
            let mut allocations =
                receiver_shared_memory.allocate_buffers(0, 0, 0, 0).expect("allocate buffers");

            let (binder_object, guard) = register_binder_object(
                &receiver.proc,
                UserAddress::from(0x0000000000000010),
                UserAddress::from(0x0000000000000100),
            );
            scopeguard::defer! {
                binder_object.ack_acquire(&mut RefCountActions::default_released()).expect("ack_acquire");
                binder_object.apply_deferred_refcounts();
            }

            // Pretend the binder object was given to the sender earlier, so it can be sent back.
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());
            // Clear the strong reference.
            scopeguard::defer! {
                sender.proc.lock().handles.dec_strong(handle.object_index(), &mut RefCountActions::default_released()).expect("dec_strong");
            }

            const DATA_PREAMBLE: &[u8; 5] = b"stuff";

            let mut transaction_data = vec![];
            transaction_data.extend(DATA_PREAMBLE);
            let offsets = [transaction_data.len() as binder_uintptr_t];
            transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_HANDLE,
                flags: 0,
                cookie: 0,
                __bindgen_anon_1.handle: handle.into(),
            }));

            device
                .translate_objects(&sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &offsets,
                    &mut transaction_data,
                    &mut allocations.scatter_gather_buffer,
                )
                .expect("failed to translate handles")
                .release(());

            // Verify that the transaction data was mutated.
            let mut expected_transaction_data = vec![];
            expected_transaction_data.extend(DATA_PREAMBLE);
            expected_transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_BINDER,
                flags: 0,
                cookie: binder_object.local.strong_ref_addr.ptr() as u64,
                __bindgen_anon_1.binder: binder_object.local.weak_ref_addr.ptr() as u64,
            }));
            assert_eq!(&expected_transaction_data, &transaction_data);
        }).await;
    }

    #[fuchsia::test]
    async fn transaction_translate_binder_handle_passed_between_non_owning_processes() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);
            let owner = BinderProcessFixture::new(current_task, &device);
            let mut receiver_shared_memory = receiver.lock_shared_memory();
            let mut allocations =
                receiver_shared_memory.allocate_buffers(0, 0, 0, 0).expect("allocate buffers");

            let binder_object = LocalBinderObject {
                weak_ref_addr: UserAddress::from(0x0000000000000010),
                strong_ref_addr: UserAddress::from(0x0000000000000100),
            };

            const SENDING_HANDLE: Handle = Handle::from_raw(1);
            const RECEIVING_HANDLE: Handle = Handle::from_raw(2);

            // Pretend the binder object was given to the sender earlier.
            let (_, guard) =
                BinderObject::new(&owner.proc, binder_object, BinderObjectFlags::empty());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());
            assert_eq!(SENDING_HANDLE, handle);

            // Give the receiver another handle so that the input handle number and output handle
            // number aren't the same.
            let (_, guard) = BinderObject::new(
                &owner.proc,
                LocalBinderObject::default(),
                BinderObjectFlags::empty(),
            );
            receiver
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            const DATA_PREAMBLE: &[u8; 5] = b"stuff";

            let mut transaction_data = vec![];
            transaction_data.extend(DATA_PREAMBLE);
            let offsets = [transaction_data.len() as binder_uintptr_t];
            transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_HANDLE,
                flags: 0,
                cookie: 0,
                __bindgen_anon_1.handle: SENDING_HANDLE.into(),
            }));

            let transaction_state = device
                .translate_objects(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &offsets,
                    &mut transaction_data,
                    &mut allocations.scatter_gather_buffer,
                )
                .expect("failed to translate handles");

            // Verify that the new handle was returned in `transaction_state` so that it gets dropped
            // at the end of the transaction.
            assert_eq!(transaction_state.state.as_ref().unwrap().handles[0], RECEIVING_HANDLE);

            // Verify that the transaction data was mutated.
            let mut expected_transaction_data = vec![];
            expected_transaction_data.extend(DATA_PREAMBLE);
            expected_transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_HANDLE,
                flags: 0,
                cookie: 0,
                __bindgen_anon_1.handle: RECEIVING_HANDLE.into(),
            }));
            assert_eq!(&expected_transaction_data, &transaction_data);

            // Verify that a handle was created in the receiver.
            let (object, guard) = receiver
                .proc
                .lock()
                .handles
                .get(RECEIVING_HANDLE.object_index())
                .expect("expected handle not present");
            guard.release(&mut RefCountActions::default_released());
            assert_eq!(object.owner.as_ptr(), OwnedRef::as_ptr(&owner.proc));
            assert_eq!(object.local, binder_object);
            transaction_state.release(());
        })
        .await;
    }

    #[fuchsia::test]
    async fn transaction_translate_binder_handles_with_same_address() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);
            let other_proc = BinderProcessFixture::new(current_task, &device);
            let mut receiver_shared_memory = receiver.lock_shared_memory();
            let mut allocations =
                receiver_shared_memory.allocate_buffers(0, 0, 0, 0).expect("allocate buffers");

            let binder_object_addr = LocalBinderObject {
                weak_ref_addr: UserAddress::from(0x0000000000000010),
                strong_ref_addr: UserAddress::from(0x0000000000000100),
            };

            const SENDING_HANDLE_SENDER: Handle = Handle::from_raw(1);
            const SENDING_HANDLE_OTHER: Handle = Handle::from_raw(2);
            const RECEIVING_HANDLE_SENDER: Handle = Handle::from_raw(2);
            const RECEIVING_HANDLE_OTHER: Handle = Handle::from_raw(3);

            // Add both objects (sender owned and other owned) to sender handle table.
            let (_, sender_guard) =
                BinderObject::new(&sender.proc, binder_object_addr, BinderObjectFlags::empty());
            let (_, other_guard) =
                BinderObject::new(&other_proc.proc, binder_object_addr, BinderObjectFlags::empty());
            assert_eq!(
                sender
                    .proc
                    .lock()
                    .handles
                    .insert_for_transaction(sender_guard, &mut RefCountActions::default_released()),
                SENDING_HANDLE_SENDER
            );
            assert_eq!(
                sender
                    .proc
                    .lock()
                    .handles
                    .insert_for_transaction(other_guard, &mut RefCountActions::default_released()),
                SENDING_HANDLE_OTHER
            );

            // Give the receiver another handle so that the input handle numbers and output handle
            // numbers aren't the same.
            let (_, guard) = BinderObject::new(
                &other_proc.proc,
                LocalBinderObject::default(),
                BinderObjectFlags::empty(),
            );
            receiver
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            const DATA_PREAMBLE: &[u8; 5] = b"stuff";

            let mut transaction_data = vec![];
            let mut offsets = vec![];
            transaction_data.extend(DATA_PREAMBLE);
            offsets.push(transaction_data.len() as binder_uintptr_t);
            transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_HANDLE,
                flags: 0,
                cookie: 0,
                __bindgen_anon_1.handle: SENDING_HANDLE_SENDER.into(),
            }));
            offsets.push(transaction_data.len() as binder_uintptr_t);
            transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_HANDLE,
                flags: 0,
                cookie: 0,
                __bindgen_anon_1.handle: SENDING_HANDLE_OTHER.into(),
            }));

            let transaction_state = device
                .translate_objects(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &offsets,
                    &mut transaction_data,
                    &mut allocations.scatter_gather_buffer,
                )
                .expect("failed to translate handles");

            // Verify that the new handles were returned in `transaction_state` so that it gets dropped
            // at the end of the transaction.
            assert_eq!(
                transaction_state.state.as_ref().unwrap().handles[0],
                RECEIVING_HANDLE_SENDER
            );
            assert_eq!(
                transaction_state.state.as_ref().unwrap().handles[1],
                RECEIVING_HANDLE_OTHER
            );

            // Verify that the transaction data was mutated.
            let mut expected_transaction_data = vec![];
            expected_transaction_data.extend(DATA_PREAMBLE);
            expected_transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_HANDLE,
                flags: 0,
                cookie: 0,
                __bindgen_anon_1.handle: RECEIVING_HANDLE_SENDER.into(),
            }));
            expected_transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_HANDLE,
                flags: 0,
                cookie: 0,
                __bindgen_anon_1.handle: RECEIVING_HANDLE_OTHER.into(),
            }));
            assert_eq!(&expected_transaction_data, &transaction_data);

            // Verify that two handles were created in the receiver.
            let (object, guard) = receiver
                .proc
                .lock()
                .handles
                .get(RECEIVING_HANDLE_SENDER.object_index())
                .expect("expected handle not present");
            guard.release(&mut RefCountActions::default_released());
            assert_eq!(object.owner.as_ptr(), OwnedRef::as_ptr(&sender.proc));
            assert_eq!(object.local, binder_object_addr);
            let (object, guard) = receiver
                .proc
                .lock()
                .handles
                .get(RECEIVING_HANDLE_OTHER.object_index())
                .expect("expected handle not present");
            guard.release(&mut RefCountActions::default_released());
            assert_eq!(object.owner.as_ptr(), OwnedRef::as_ptr(&other_proc.proc));
            assert_eq!(object.local, binder_object_addr);
            transaction_state.release(());
        })
        .await;
    }

    /// Tests that hwbinder's scatter-gather buffer-fix-up implementation is correct.
    #[fuchsia::test]
    async fn transaction_translate_buffers() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new_current(current_task, &device);

            // Allocate memory in the sender to hold all the buffers that will get submitted to the
            // binder driver.
            let sender_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            let mut writer = UserMemoryWriter::new(&current_task, sender_addr);

            // Serialize a string into memory.
            const FOO_STR_LEN: i32 = 3;
            const FOO_STR_PADDED_LEN: u64 = 8;
            let sender_foo_addr = writer.write(b"foo");

            // Pad the next buffer to ensure 8-byte alignment.
            writer.write(&[0; FOO_STR_PADDED_LEN as usize - FOO_STR_LEN as usize]);

            // Serialize a C struct that points to the above string.
            #[repr(C)]
            #[derive(IntoBytes, Immutable)]
            struct Bar {
                foo_str: UserAddress,
                len: i32,
                _padding: u32,
            }
            let sender_bar_addr = writer.write_object(&Bar {
                foo_str: sender_foo_addr,
                len: FOO_STR_LEN,
                _padding: 0,
            });

            // Mark the start of the transaction data.
            let transaction_data_addr = writer.current_address();

            // Write the buffer object representing the C struct `Bar`.
            let sender_buffer0_addr = writer.write_object(&binder_buffer_object {
                hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                buffer: sender_bar_addr.ptr() as u64,
                length: std::mem::size_of::<Bar>() as u64,
                ..binder_buffer_object::default()
            });

            // Write the buffer object representing the "foo" string. Its parent is the C struct `Bar`,
            // which has a pointer to it.
            let sender_buffer1_addr = writer.write_object(&binder_buffer_object {
                hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                buffer: sender_foo_addr.ptr() as u64,
                length: FOO_STR_LEN as u64,
                // Mark this buffer as having a parent who references it. The driver will then read
                // the next two fields.
                flags: BINDER_BUFFER_FLAG_HAS_PARENT,
                // The index in the offsets array of the parent buffer.
                parent: 0,
                // The location in the parent buffer where a pointer to this object needs to be
                // fixed up.
                parent_offset: offset_of!(Bar, foo_str) as u64,
            });

            // Write the offsets array.
            let offsets_addr = writer.current_address();
            writer.write_object(&((sender_buffer0_addr - transaction_data_addr) as u64));
            writer.write_object(&((sender_buffer1_addr - transaction_data_addr) as u64));

            let end_data_addr = writer.current_address();

            // Construct the input for the binder driver to process.
            let input = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    target: binder_transaction_data__bindgen_ty_1 { handle: 1 },
                    data_size: (offsets_addr - transaction_data_addr) as u64,
                    offsets_size: (end_data_addr - offsets_addr) as u64,
                    data: binder_transaction_data__bindgen_ty_2 {
                        ptr: binder_transaction_data__bindgen_ty_2__bindgen_ty_1 {
                            buffer: transaction_data_addr.ptr() as u64,
                            offsets: offsets_addr.ptr() as u64,
                        },
                    },
                    ..binder_transaction_data::new_zeroed()
                },
                // Each buffer size must be rounded up to a multiple of 8 to ensure enough
                // space in the allocated target for 8-byte alignment.
                buffers_size: std::mem::size_of::<Bar>() as u64 + FOO_STR_PADDED_LEN,
            };

            // Perform the translation and copying.
            let (buffers, transaction_state) = device
                .copy_transaction_buffers(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    current_task,
                    current_task,
                    &receiver.proc,
                    &input,
                    None,
                )
                .expect("copy_transaction_buffers");
            transaction_state.release(());
            let data_buffer = buffers.data;

            // Read back the translated objects from the receiver's memory.
            let translated_objects = current_task
                .read_objects_to_array::<binder_buffer_object, 2>(UserRef::new(data_buffer.address))
                .expect("read output");

            // Check that the second buffer is the string "foo".
            let foo_addr = UserAddress::from(translated_objects[1].buffer);
            let str = current_task.read_memory_to_array::<3>(foo_addr).expect("read buffer 1");
            assert_eq!(&str, b"foo");

            // Check that the first buffer points to the string "foo".
            let foo_ptr: UserAddress = current_task
                .read_object(UserRef::new(UserAddress::from(translated_objects[0].buffer)))
                .expect("read buffer 0");
            assert_eq!(foo_ptr, foo_addr);
        })
        .await;
    }

    /// Tests that when the scatter-gather buffer size reported by userspace is too small, we stop
    /// processing and fail, instead of skipping a buffer object that doesn't fit.
    #[fuchsia::test]
    async fn transaction_fails_when_sg_buffer_size_is_too_small() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Allocate memory in the sender to hold all the buffers that will get submitted to the
            // binder driver.
            let sender_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            let mut writer = UserMemoryWriter::new(&current_task, sender_addr);

            // Serialize a series of buffers that point to empty data. Each successive buffer is smaller
            // than the last.
            let buffer_objects = [8, 7, 6]
                .iter()
                .map(|size| binder_buffer_object {
                    hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                    buffer: writer
                        .write(&{
                            let mut data = vec![];
                            data.resize(*size, 0u8);
                            data
                        })
                        .ptr() as u64,
                    length: *size as u64,
                    ..binder_buffer_object::default()
                })
                .collect::<Vec<_>>();

            // Mark the start of the transaction data.
            let transaction_data_addr = writer.current_address();

            // Write the buffer objects to the transaction payload.
            let offsets = buffer_objects
                .into_iter()
                .map(|buffer_object| {
                    (writer.write_object(&buffer_object) - transaction_data_addr) as u64
                })
                .collect::<Vec<_>>();

            // Write the offsets array.
            let offsets_addr = writer.current_address();
            for offset in offsets {
                writer.write_object(&offset);
            }

            let end_data_addr = writer.current_address();

            // Construct the input for the binder driver to process.
            let input = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    target: binder_transaction_data__bindgen_ty_1 { handle: 1 },
                    data_size: (offsets_addr - transaction_data_addr) as u64,
                    offsets_size: (end_data_addr - offsets_addr) as u64,
                    data: binder_transaction_data__bindgen_ty_2 {
                        ptr: binder_transaction_data__bindgen_ty_2__bindgen_ty_1 {
                            buffer: transaction_data_addr.ptr() as u64,
                            offsets: offsets_addr.ptr() as u64,
                        },
                    },
                    ..binder_transaction_data::new_zeroed()
                },
                // Make the buffers size only fit the first buffer fully (size 8). The remaining space
                // should be 6 bytes, so that the second buffer doesn't fit but the next one does.
                buffers_size: 8 + 6,
            };

            // Perform the translation and copying.
            device
                .copy_transaction_buffers(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &input,
                    None,
                )
                .expect_err("copy_transaction_buffers should fail");
        })
        .await;
    }

    /// Tests that when a scatter-gather buffer refers to a parent that comes *after* it in the
    /// object list, the transaction fails.
    #[fuchsia::test]
    async fn transaction_fails_when_sg_buffer_parent_is_out_of_order() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Allocate memory in the sender to hold all the buffers that will get submitted to the
            // binder driver.
            let sender_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            let mut writer = UserMemoryWriter::new(&current_task, sender_addr);

            // Write the data for two buffer objects.
            const BUFFER_DATA_LEN: usize = 8;
            let buf0_addr = writer.write(&[0; BUFFER_DATA_LEN]);
            let buf1_addr = writer.write(&[0; BUFFER_DATA_LEN]);

            // Mark the start of the transaction data.
            let transaction_data_addr = writer.current_address();

            // Write a buffer object that marks a future buffer as its parent.
            let sender_buffer0_addr = writer.write_object(&binder_buffer_object {
                hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                buffer: buf0_addr.ptr() as u64,
                length: BUFFER_DATA_LEN as u64,
                // Mark this buffer as having a parent who references it. The driver will then read
                // the next two fields.
                flags: BINDER_BUFFER_FLAG_HAS_PARENT,
                parent: 0,
                parent_offset: 0,
            });

            // Write a buffer object that acts as the first buffers parent (contains a pointer to the
            // first buffer).
            let sender_buffer1_addr = writer.write_object(&binder_buffer_object {
                hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                buffer: buf1_addr.ptr() as u64,
                length: BUFFER_DATA_LEN as u64,
                ..binder_buffer_object::default()
            });

            // Write the offsets array.
            let offsets_addr = writer.current_address();
            writer.write_object(&((sender_buffer0_addr - transaction_data_addr) as u64));
            writer.write_object(&((sender_buffer1_addr - transaction_data_addr) as u64));

            let end_data_addr = writer.current_address();

            // Construct the input for the binder driver to process.
            let input = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    target: binder_transaction_data__bindgen_ty_1 { handle: 1 },
                    data_size: (offsets_addr - transaction_data_addr) as u64,
                    offsets_size: (end_data_addr - offsets_addr) as u64,
                    data: binder_transaction_data__bindgen_ty_2 {
                        ptr: binder_transaction_data__bindgen_ty_2__bindgen_ty_1 {
                            buffer: transaction_data_addr.ptr() as u64,
                            offsets: offsets_addr.ptr() as u64,
                        },
                    },
                    ..binder_transaction_data::new_zeroed()
                },
                buffers_size: BUFFER_DATA_LEN as u64 * 2,
            };

            // Perform the translation and copying.
            device
                .copy_transaction_buffers(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &input,
                    None,
                )
                .expect_err("copy_transaction_buffers should fail");
        })
        .await;
    }

    #[fuchsia::test]
    async fn transaction_translate_fd_array() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Insert a binder object for the receiver, and grab a handle to it in the sender.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (_, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Open a file in the sender process that we won't be using. It is there to occupy a file
            // descriptor so that the translation doesn't happen to use the same FDs for receiver and
            // sender, potentially hiding a bug.
            let file = PanickingFile::new_file(&current_task);
            current_task.add_file(file, FdFlags::empty()).unwrap();

            // Open two files in the sender process. These will be sent in the transaction.
            let file1 = PanickingFile::new_file(&current_task);
            let file2 = PanickingFile::new_file(&current_task);
            let files = [file1, file2];
            let sender_fds = files
                .iter()
                .map(|file| {
                    current_task.add_file(file.clone(), FdFlags::CLOEXEC).expect("add file")
                })
                .collect::<Vec<_>>();

            // Ensure that the receiver.task() has no file descriptors.
            assert!(receiver.task().files().get_all_fds().is_empty(), "receiver already has files");

            // Allocate memory in the sender to hold all the buffers that will get submitted to the
            // binder driver.
            let sender_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            let mut writer = UserMemoryWriter::new(&current_task, sender_addr);

            // Serialize a simple buffer. This will ensure that the FD array being translated is not at
            // the beginning of the buffer, exercising the offset math.
            let sender_padding_addr = writer.write(&[0; 8]);

            // Serialize a C struct with an fd array.
            #[repr(C)]
            #[derive(IntoBytes, KnownLayout, FromBytes, Immutable)]
            struct Bar {
                len: u32,
                fds: [u32; 2],
                _padding: u32,
            }
            let sender_bar_addr = writer.write_object(&Bar {
                len: 2,
                fds: [sender_fds[0].raw() as u32, sender_fds[1].raw() as u32],
                _padding: 0,
            });

            // Mark the start of the transaction data.
            let transaction_data_addr = writer.current_address();

            // Write the buffer object representing the padding.
            let sender_padding_buffer_addr = writer.write_object(&binder_buffer_object {
                hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                buffer: sender_padding_addr.ptr() as u64,
                length: 8,
                ..binder_buffer_object::default()
            });

            // Write the buffer object representing the C struct `Bar`.
            let sender_buffer_addr = writer.write_object(&binder_buffer_object {
                hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                buffer: sender_bar_addr.ptr() as u64,
                length: std::mem::size_of::<Bar>() as u64,
                ..binder_buffer_object::default()
            });

            // Write the fd array object that tells the kernel where the file descriptors are in the
            // `Bar` buffer.
            let sender_fd_array_addr = writer.write_object(&binder_fd_array_object {
                hdr: binder_object_header { type_: BINDER_TYPE_FDA },
                pad: 0,
                num_fds: sender_fds.len() as u64,
                // The index in the offsets array of the parent buffer.
                parent: 1,
                // The location in the parent buffer where the FDs are, which need to be duped.
                parent_offset: offset_of!(Bar, fds) as u64,
            });

            // Write the offsets array.
            let offsets_addr = writer.current_address();
            writer.write_object(&((sender_padding_buffer_addr - transaction_data_addr) as u64));
            writer.write_object(&((sender_buffer_addr - transaction_data_addr) as u64));
            writer.write_object(&((sender_fd_array_addr - transaction_data_addr) as u64));

            let end_data_addr = writer.current_address();

            // Construct the input for the binder driver to process.
            let input = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    data_size: (offsets_addr - transaction_data_addr) as u64,
                    offsets_size: (end_data_addr - offsets_addr) as u64,
                    data: binder_transaction_data__bindgen_ty_2 {
                        ptr: binder_transaction_data__bindgen_ty_2__bindgen_ty_1 {
                            buffer: transaction_data_addr.ptr() as u64,
                            offsets: offsets_addr.ptr() as u64,
                        },
                    },
                    ..binder_transaction_data::new_zeroed()
                },
                buffers_size: std::mem::size_of::<Bar>() as u64 + 8,
            };

            // Perform the translation and copying.
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), input)
                .expect("transaction queued");

            // Get the data buffer out of the receiver's queue.
            let data_buffer = match receiver
                .proc
                .lock()
                .command_queue
                .pop_front()
                .expect("the transaction should be queued on the process")
            {
                QueuedCommand {
                    command:
                        Command::Transaction {
                            data: TransactionData { buffers: TransactionBuffers { data, .. }, .. },
                            ..
                        },
                    ..
                } => data,
                _ => panic!("unexpected command in process queue"),
            };

            // Start reading from the receiver's memory, which holds the translated transaction.
            let mut reader = UserMemoryCursor::new(
                &*receiver.task().task,
                data_buffer.address,
                data_buffer.length as u64,
            )
            .expect("create memory cursor");

            // Skip the first object, it was only there to pad the next one.
            reader.read_object::<binder_buffer_object>().expect("read padding buffer");

            // Read back the buffer object representing `Bar`.
            let bar_buffer_object =
                reader.read_object::<binder_buffer_object>().expect("read bar buffer object");
            let translated_bar = receiver
                .task()
                .task
                .read_object::<Bar>(UserRef::new(UserAddress::from(bar_buffer_object.buffer)))
                .expect("read Bar");

            // Verify that the fds have been translated.
            let (receiver_file, receiver_fd_flags) = receiver
                .task()
                .files()
                .get_allowing_opath_with_flags(FdNumber::from_raw(translated_bar.fds[0] as i32))
                .expect("FD not found in receiver");
            assert!(
                Arc::ptr_eq(&receiver_file, &files[0]),
                "FD in receiver does not refer to the same file as sender"
            );
            assert_eq!(receiver_fd_flags, FdFlags::CLOEXEC);
            let (receiver_file, receiver_fd_flags) = receiver
                .task()
                .files()
                .get_allowing_opath_with_flags(FdNumber::from_raw(translated_bar.fds[1] as i32))
                .expect("FD not found in receiver");
            assert!(
                Arc::ptr_eq(&receiver_file, &files[1]),
                "FD in receiver does not refer to the same file as sender"
            );
            assert_eq!(receiver_fd_flags, FdFlags::CLOEXEC);

            // Release the buffer in the receiver and verify that the associated FDs have been closed.
            receiver.proc.handle_free_buffer(data_buffer.address).expect("failed to free buffer");
            assert!(
                receiver
                    .task()
                    .files()
                    .get_allowing_opath(FdNumber::from_raw(translated_bar.fds[0] as i32))
                    .expect_err("file should be closed")
                    == EBADF
            );
            assert!(
                receiver
                    .task()
                    .files()
                    .get_allowing_opath(FdNumber::from_raw(translated_bar.fds[1] as i32))
                    .expect_err("file should be closed")
                    == EBADF
            );
        })
        .await;
    }
    #[fuchsia::test]
    async fn transaction_receiver_exits_after_getting_fd_array() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Insert a binder object for the receiver, and grab a handle to it in the sender.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (_, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Open a file in the sender process that we won't be using. It is there to occupy a file
            // descriptor so that the translation doesn't happen to use the same FDs for receiver and
            // sender, potentially hiding a bug.
            let file = PanickingFile::new_file(&current_task);
            current_task.add_file(file, FdFlags::empty()).unwrap();

            // Open two files in the sender process. These will be sent in the transaction.
            let file1 = PanickingFile::new_file(&current_task);
            let file2 = PanickingFile::new_file(&current_task);
            let files = [file1, file2];
            let sender_fds = files
                .into_iter()
                .map(|file| current_task.add_file(file, FdFlags::CLOEXEC).expect("add file"))
                .collect::<Vec<_>>();

            // Ensure that the receiver.task() has no file descriptors.
            assert!(receiver.task().files().get_all_fds().is_empty(), "receiver already has files");

            // Allocate memory in the sender to hold all the buffers that will get submitted to the
            // binder driver.
            let sender_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            let mut writer = UserMemoryWriter::new(&current_task, sender_addr);

            // Serialize a simple buffer. This will ensure that the FD array being translated is not at
            // the beginning of the buffer, exercising the offset math.
            let sender_padding_addr = writer.write(&[0; 8]);

            // Serialize a C struct with an fd array.
            #[repr(C)]
            #[derive(IntoBytes, KnownLayout, FromBytes, Immutable)]
            struct Bar {
                len: u32,
                fds: [u32; 2],
                _padding: u32,
            }
            let sender_bar_addr = writer.write_object(&Bar {
                len: 2,
                fds: [sender_fds[0].raw() as u32, sender_fds[1].raw() as u32],
                _padding: 0,
            });

            // Mark the start of the transaction data.
            let transaction_data_addr = writer.current_address();

            // Write the buffer object representing the padding.
            let sender_padding_buffer_addr = writer.write_object(&binder_buffer_object {
                hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                buffer: sender_padding_addr.ptr() as u64,
                length: 8,
                ..binder_buffer_object::default()
            });

            // Write the buffer object representing the C struct `Bar`.
            let sender_buffer_addr = writer.write_object(&binder_buffer_object {
                hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                buffer: sender_bar_addr.ptr() as u64,
                length: std::mem::size_of::<Bar>() as u64,
                ..binder_buffer_object::default()
            });

            // Write the fd array object that tells the kernel where the file descriptors are in the
            // `Bar` buffer.
            let sender_fd_array_addr = writer.write_object(&binder_fd_array_object {
                hdr: binder_object_header { type_: BINDER_TYPE_FDA },
                pad: 0,
                num_fds: sender_fds.len() as u64,
                // The index in the offsets array of the parent buffer.
                parent: 1,
                // The location in the parent buffer where the FDs are, which need to be duped.
                parent_offset: offset_of!(Bar, fds) as u64,
            });

            // Write the offsets array.
            let offsets_addr = writer.current_address();
            writer.write_object(&((sender_padding_buffer_addr - transaction_data_addr) as u64));
            writer.write_object(&((sender_buffer_addr - transaction_data_addr) as u64));
            writer.write_object(&((sender_fd_array_addr - transaction_data_addr) as u64));

            let end_data_addr = writer.current_address();

            // Construct the input for the binder driver to process.
            let input = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    data_size: (offsets_addr - transaction_data_addr) as u64,
                    offsets_size: (end_data_addr - offsets_addr) as u64,
                    data: binder_transaction_data__bindgen_ty_2 {
                        ptr: binder_transaction_data__bindgen_ty_2__bindgen_ty_1 {
                            buffer: transaction_data_addr.ptr() as u64,
                            offsets: offsets_addr.ptr() as u64,
                        },
                    },
                    ..binder_transaction_data::new_zeroed()
                },
                buffers_size: std::mem::size_of::<Bar>() as u64 + 8,
            };

            // Perform the translation and copying.
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), input)
                .expect("transaction queued");
        })
        .await;
    }

    #[fuchsia::test]
    async fn transaction_fd_array_sender_cancels() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Insert a binder object for the receiver, and grab a handle to it in the sender.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (_, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Open a file in the sender process that we won't be using. It is there to occupy a file
            // descriptor so that the translation doesn't happen to use the same FDs for receiver and
            // sender, potentially hiding a bug.
            let file1 = PanickingFile::new_file(&current_task);
            current_task.add_file(file1, FdFlags::empty()).unwrap();

            // Open two files in the sender process. These will be sent in the transaction.
            let file1 = PanickingFile::new_file(&current_task);
            let file2 = PanickingFile::new_file(&current_task);
            let files = [file1, file2];
            let sender_fds = files
                .into_iter()
                .map(|file| current_task.add_file(file, FdFlags::CLOEXEC).expect("add file"))
                .collect::<Vec<_>>();

            // Ensure that the receiver.task() has no file descriptors.
            assert!(receiver.task().files().get_all_fds().is_empty(), "receiver already has files");

            // Allocate memory in the sender to hold all the buffers that will get submitted to the
            // binder driver.
            let sender_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            let mut writer = UserMemoryWriter::new(&current_task, sender_addr);

            // Serialize a simple buffer. This will ensure that the FD array being translated is not at
            // the beginning of the buffer, exercising the offset math.
            let sender_padding_addr = writer.write(&[0; 8]);

            // Serialize a C struct with an fd array.
            #[repr(C)]
            #[derive(IntoBytes, KnownLayout, FromBytes, Immutable)]
            struct Bar {
                len: u32,
                fds: [u32; 2],
                _padding: u32,
            }
            let sender_bar_addr = writer.write_object(&Bar {
                len: 2,
                fds: [sender_fds[0].raw() as u32, sender_fds[1].raw() as u32],
                _padding: 0,
            });

            // Mark the start of the transaction data.
            let transaction_data_addr = writer.current_address();

            // Write the buffer object representing the padding.
            let sender_padding_buffer_addr = writer.write_object(&binder_buffer_object {
                hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                buffer: sender_padding_addr.ptr() as u64,
                length: 8,
                ..binder_buffer_object::default()
            });

            // Write the buffer object representing the C struct `Bar`.
            let sender_buffer_addr = writer.write_object(&binder_buffer_object {
                hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                buffer: sender_bar_addr.ptr() as u64,
                length: std::mem::size_of::<Bar>() as u64,
                ..binder_buffer_object::default()
            });

            // Write the fd array object that tells the kernel where the file descriptors are in the
            // `Bar` buffer.
            let sender_fd_array_addr = writer.write_object(&binder_fd_array_object {
                hdr: binder_object_header { type_: BINDER_TYPE_FDA },
                pad: 0,
                num_fds: sender_fds.len() as u64,
                // The index in the offsets array of the parent buffer.
                parent: 1,
                // The location in the parent buffer where the FDs are, which need to be duped.
                parent_offset: offset_of!(Bar, fds) as u64,
            });

            // Write the offsets array.
            let offsets_addr = writer.current_address();
            writer.write_object(&((sender_padding_buffer_addr - transaction_data_addr) as u64));
            writer.write_object(&((sender_buffer_addr - transaction_data_addr) as u64));
            writer.write_object(&((sender_fd_array_addr - transaction_data_addr) as u64));

            let end_data_addr = writer.current_address();

            // Construct the input for the binder driver to process.
            let input = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    data_size: (offsets_addr - transaction_data_addr) as u64,
                    offsets_size: (end_data_addr - offsets_addr) as u64,
                    data: binder_transaction_data__bindgen_ty_2 {
                        ptr: binder_transaction_data__bindgen_ty_2__bindgen_ty_1 {
                            buffer: transaction_data_addr.ptr() as u64,
                            offsets: offsets_addr.ptr() as u64,
                        },
                    },
                    ..binder_transaction_data::new_zeroed()
                },
                buffers_size: std::mem::size_of::<Bar>() as u64 + 8,
            };

            // Perform the translation and copying.
            let (_, transient_state) = device
                .copy_transaction_buffers(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &input,
                    None,
                )
                .expect("copy_transaction_buffers");

            // The receiver should have the fd.
            let fd = transient_state.state.as_ref().unwrap().owned_fds[0];
            assert!(
                receiver.task().files().get_allowing_opath(fd).is_ok(),
                "file should be translated"
            );

            // Release the result, which should close the fds in the receiver.
            transient_state.release(());
            assert!(
                receiver.task().files().get_allowing_opath(fd).expect_err("file should be closed")
                    == EBADF
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn transaction_translation_fails_on_invalid_handle() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);
            let mut receiver_shared_memory = receiver.lock_shared_memory();
            let mut allocations =
                receiver_shared_memory.allocate_buffers(0, 0, 0, 0).expect("allocate buffers");

            let mut transaction_data = vec![];
            transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_HANDLE,
                flags: 0,
                cookie: 0,
                __bindgen_anon_1.handle: 42,
            }));

            let transaction_ref_error = device
                .translate_objects(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &[0 as binder_uintptr_t],
                    &mut transaction_data,
                    &mut allocations.scatter_gather_buffer,
                )
                .expect_err("translate handles unexpectedly succeeded");

            assert_eq!(transaction_ref_error, TransactionError::Failure);
        })
        .await;
    }

    #[fuchsia::test]
    async fn transaction_translation_fails_on_invalid_object_type() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);
            let mut receiver_shared_memory = receiver.lock_shared_memory();
            let mut allocations =
                receiver_shared_memory.allocate_buffers(0, 0, 0, 0).expect("allocate buffers");

            let mut transaction_data = vec![];
            transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_WEAK_HANDLE,
                flags: 0,
                cookie: 0,
                __bindgen_anon_1.handle: 42,
            }));

            let transaction_ref_error = device
                .translate_objects(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &[0 as binder_uintptr_t],
                    &mut transaction_data,
                    &mut allocations.scatter_gather_buffer,
                )
                .expect_err("translate handles unexpectedly succeeded");

            assert_eq!(transaction_ref_error, TransactionError::Malformed(errno!(EINVAL)));
        })
        .await;
    }

    #[fuchsia::test]
    async fn transaction_drop_references_on_failed_transaction() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);
            let mut receiver_shared_memory = receiver.lock_shared_memory();
            let mut allocations =
                receiver_shared_memory.allocate_buffers(0, 0, 0, 0).expect("allocate buffers");

            let binder_object = LocalBinderObject {
                weak_ref_addr: UserAddress::from(0x0000000000000010),
                strong_ref_addr: UserAddress::from(0x0000000000000100),
            };

            let mut transaction_data = vec![];
            transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_BINDER,
                flags: 0,
                cookie: binder_object.strong_ref_addr.ptr() as u64,
                __bindgen_anon_1.binder: binder_object.weak_ref_addr.ptr() as u64,
            }));
            transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_HANDLE,
                flags: 0,
                cookie: 0,
                __bindgen_anon_1.handle: 42,
            }));

            device
                .translate_objects(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &[
                        0 as binder_uintptr_t,
                        std::mem::size_of::<flat_binder_object>() as binder_uintptr_t,
                    ],
                    &mut transaction_data,
                    &mut allocations.scatter_gather_buffer,
                )
                .expect_err("translate handles unexpectedly succeeded");

            // Ensure that the handle created in the receiving process is not present.
            assert!(
                receiver.proc.lock().handles.get(0).is_none(),
                "handle present when it should have been dropped"
            );
        })
        .await;
    }

    // Open the binder device, which creates an instance of the binder device associated with
    // the process.
    fn open_binder_fd(current_task: &CurrentTask, binder_driver: &BinderDevice) -> FileHandle {
        // `open()` requires an `FsNode` so create one in `AnonFs`.
        let fs = anon_fs(current_task.kernel());
        let node = create_namespace_node_for_testing(&fs, Anon::new_for_binder_device());

        let binder = binder_driver
            .open(&current_task, DeviceId::NONE, &node, OpenFlags::RDWR)
            .expect("binder dev open failed");
        FileObject::new_anonymous(current_task, binder, node.entry.node.clone(), OpenFlags::RDWR)
    }

    #[fuchsia::test]
    async fn close_binder() {
        spawn_kernel_and_run(async |current_task| {
            let binder_driver = BinderDevice::default();

            let binder_fd = open_binder_fd(&current_task, &binder_driver);
            let binder_connection =
                binder_fd.downcast_file::<BinderConnection>().expect("must be a BinderConnection");
            let identifier = binder_connection.identifier;

            // Ensure that the binder driver has created process state.
            binder_driver
                .find_process(identifier)
                .expect("failed to find process")
                .release(current_task.kernel());

            // Close the file descriptor.
            std::mem::drop(binder_fd);
            current_task.trigger_delayed_releaser();

            // Verify that the process state no longer exists.
            binder_driver.find_process(identifier).expect_err("process was not cleaned up");
        })
        .await;
    }

    #[fuchsia::test]
    async fn flush_kicks_threads() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            // Open the binder device, which creates an instance of the binder device associated
            // with the process.
            let binder_fd = open_binder_fd(&current_task, &device);
            let binder_connection =
                binder_fd.downcast_file::<BinderConnection>().expect("must be a BinderConnection");
            let binder_proc = binder_connection.proc(current_task).unwrap();
            let binder_thread =
                binder_proc.lock().find_or_register_thread(&current_task.task).unwrap();

            let thread = std::thread::spawn({
                let task = current_task.weak_task();
                let binder_proc = WeakRef::<BinderProcess>::from(&binder_proc);

                move || {
                    let task = if let Some(task) = task.upgrade() {
                        task
                    } else {
                        return;
                    };
                    let binder_proc = if let Some(binder_proc) = binder_proc.upgrade() {
                        binder_proc
                    } else {
                        return;
                    };
                    // Wait for the task to start waiting.
                    while !task.read().is_blocked() {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    // Do the kick.
                    binder_proc.kick_all_threads();
                }
            });

            let context = OperationContext {
                current_task,
                connection_security_state: &security::binder_connection_alloc(current_task),
                binder_proc: &binder_proc,
                binder_thread: &binder_thread,
                memory_accessor: binder_proc.get_memory_accessor(current_task, None),
            };
            let read_buffer_addr = map_memory(current_task, UserAddress::default(), *PAGE_SIZE);
            let bytes_read = device
                .handle_thread_read(
                    &context,
                    &UserBuffer { address: read_buffer_addr, length: *PAGE_SIZE as usize },
                )
                .unwrap();
            assert_eq!(bytes_read, 0);
            thread.join().expect("join");
            binder_thread.release(current_task.kernel());
            binder_proc.release(current_task.kernel());
        })
        .await;
    }

    #[fuchsia::test]
    async fn decrementing_refs_on_dead_binder_succeeds() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let owner = BinderProcessFixture::new(current_task, &device);
            let client = BinderProcessFixture::new(current_task, &device);

            // Register an object with the owner.
            let guard = owner.proc.lock().find_or_register_object(
                &owner.thread,
                LocalBinderObject {
                    weak_ref_addr: UserAddress::from(0x0000000000000001),
                    strong_ref_addr: UserAddress::from(0x0000000000000002),
                },
                BinderObjectFlags::empty(),
            );

            // Keep a weak reference to the object.
            let weak_object = Arc::downgrade(&guard.binder_object);

            // Insert a handle to the object in the client. This also retains a strong reference.
            let handle = client
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Grab a weak reference.
            client
                .proc
                .lock()
                .handles
                .inc_weak(handle.object_index(), &mut RefCountActions::default_released())
                .expect("inc_weak");

            // Now the owner process dies.
            std::mem::drop(owner);

            // Confirm that the object is considered dead. The representation is still alive, but the
            // owner is dead.
            let (object, guard) = client
                .proc
                .lock()
                .handles
                .get(handle.object_index())
                .expect("expected handle not present");
            guard.release(&mut RefCountActions::default_released());
            assert!(object.owner.upgrade().is_none(), "owner should be dead");
            std::mem::drop(object);

            // Decrement the weak reference. This should prove that the handle is still occupied.
            client
                .proc
                .lock()
                .handles
                .dec_weak(handle.object_index(), &mut RefCountActions::default_released())
                .expect("dec_weak");

            // Decrement the last strong reference.
            client
                .proc
                .lock()
                .handles
                .dec_strong(handle.object_index(), &mut RefCountActions::default_released())
                .expect("dec_strong");

            // Confirm that now the handle has been removed from the table.
            assert!(
                client.proc.lock().handles.get(handle.object_index()).is_none(),
                "handle should have been dropped"
            );

            // Now the binder object representation should also be gone.
            assert!(weak_object.upgrade().is_none(), "object should be dead");
        })
        .await;
    }

    #[fuchsia::test]
    async fn death_notification_fires_when_process_dies() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Register an object with the owner.
            let guard = sender.proc.lock().find_or_register_object(
                &sender.thread,
                LocalBinderObject {
                    weak_ref_addr: UserAddress::from(0x0000000000000001),
                    strong_ref_addr: UserAddress::from(0x0000000000000002),
                },
                BinderObjectFlags::empty(),
            );

            // Insert a handle to the object in the client. This also retains a strong reference.
            let handle = receiver
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            const DEATH_NOTIFICATION_COOKIE: binder_uintptr_t = 0xDEADBEEF;

            // Register a death notification handler.
            receiver
                .proc
                .handle_request_death_notification(handle, DEATH_NOTIFICATION_COOKIE)
                .expect("request death notification");

            // Now the owner process dies.
            std::mem::drop(sender);

            // The client process should have a notification waiting.
            assert_matches!(
                receiver.proc.lock().command_queue.front(),
                Some(QueuedCommand { command: Command::DeadBinder(DEATH_NOTIFICATION_COOKIE), .. })
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn death_notification_fires_when_request_for_death_notification_is_made_on_dead_binder() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Register an object with the sender.
            let guard = sender.proc.lock().find_or_register_object(
                &sender.thread,
                LocalBinderObject {
                    weak_ref_addr: UserAddress::from(0x0000000000000001),
                    strong_ref_addr: UserAddress::from(0x0000000000000002),
                },
                BinderObjectFlags::empty(),
            );

            // Insert a handle to the object in the receiver. This also retains a strong reference.
            let handle = receiver
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Now the sender process dies.
            std::mem::drop(sender);

            const DEATH_NOTIFICATION_COOKIE: binder_uintptr_t = 0xDEADBEEF;

            // Register a death notification handler.
            receiver
                .proc
                .handle_request_death_notification(handle, DEATH_NOTIFICATION_COOKIE)
                .expect("request death notification");

            // The receiver thread should not have a notification, as the calling thread is not allowed
            // to receive it, or else a deadlock may occur if the thread is in the middle of a
            // transaction. Since there is only one thread, check the process command queue.
            assert_matches!(
                receiver.proc.lock().command_queue.front(),
                Some(QueuedCommand { command: Command::DeadBinder(DEATH_NOTIFICATION_COOKIE), .. })
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn death_notification_is_cleared_before_process_dies() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let owner = BinderProcessFixture::new(current_task, &device);
            let client = BinderProcessFixture::new(current_task, &device);

            // Register an object with the owner.
            let guard = owner.proc.lock().find_or_register_object(
                &owner.thread,
                LocalBinderObject {
                    weak_ref_addr: UserAddress::from(0x0000000000000001),
                    strong_ref_addr: UserAddress::from(0x0000000000000002),
                },
                BinderObjectFlags::empty(),
            );

            // Insert a handle to the object in the client. This also retains a strong reference.
            let handle = client
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            let death_notification_cookie = 0xDEADBEEF;

            // Register a death notification handler.
            client
                .proc
                .handle_request_death_notification(handle, death_notification_cookie)
                .expect("request death notification");

            // Now clear the death notification handler.
            client
                .proc
                .handle_clear_death_notification(handle, death_notification_cookie)
                .expect("clear death notification");

            // Check that the client received an acknowlgement
            {
                let queue = &mut client.proc.lock().command_queue;
                assert_eq!(queue.len(), 1);
                assert_matches!(
                    queue.front().unwrap(),
                    QueuedCommand { command: Command::ClearDeathNotificationDone(_), .. }
                );

                // Clear the command queue.
                queue.clear();
            }

            // Pretend the client thread is waiting for commands, so that it can be scheduled commands.
            let fake_waiter = Waiter::new();
            {
                let mut state = client.thread.lock();
                state.registration = RegistrationState::Main;
                state.command_queue.waiters.wait_async(&fake_waiter);
            }

            // Now the owner process dies.
            std::mem::drop(owner);

            // The client thread should have no notification.
            assert!(client.thread.lock().command_queue.is_empty());

            // The client process should have no notification.
            assert!(client.proc.lock().command_queue.is_empty());
        })
        .await;
    }

    #[fuchsia::test]
    async fn send_fd_in_transaction() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);
            let mut receiver_shared_memory = receiver.lock_shared_memory();
            let mut allocations =
                receiver_shared_memory.allocate_buffers(0, 0, 0, 0).expect("allocate buffers");

            // Open a file in the sender process.
            let file = PanickingFile::new_file(&current_task);
            let sender_fd =
                current_task.add_file(file.clone(), FdFlags::CLOEXEC).expect("add file");

            // Send the fd in a transaction. `cookie` is set so that we can ensure binder
            // driver doesn't touch them/passes them through.
            let mut transaction_data = struct_with_union_into_bytes!(binder_fd_object {
                hdr.type_: BINDER_TYPE_FD,
                pad_flags: 0xdeadbeef,
                cookie: 51,
                __bindgen_anon_1.fd: sender_fd.raw() as u32,
            });
            let offsets = [0];

            let transient_transaction_state = device
                .translate_objects(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &offsets,
                    &mut transaction_data,
                    &mut allocations.scatter_gather_buffer,
                )
                .expect("failed to translate handles");

            // Simulate success by converting the transient state.
            let transaction_state = transient_transaction_state.into_state();

            // The receiver should now have a file.
            let receiver_fd = receiver
                .task()
                .files()
                .get_all_fds()
                .first()
                .cloned()
                .expect("receiver should have FD");

            // The FD should have the same flags.
            assert_eq!(
                receiver
                    .task()
                    .files()
                    .get_fd_flags_allowing_opath(receiver_fd)
                    .expect("get flags"),
                FdFlags::CLOEXEC
            );

            // The FD should point to the same file.
            assert!(
                Arc::ptr_eq(
                    &receiver
                        .task()
                        .files()
                        .get_allowing_opath(receiver_fd)
                        .expect("receiver should have FD"),
                    &file
                ),
                "FDs from sender and receiver don't point to the same file"
            );

            let (transaction_data_fd_object, _) =
                binder_fd_object::read_from_prefix(&transaction_data)
                    .expect("transaction_data ought be a binder_fd_object");
            assert_eq!(BINDER_TYPE_FD, transaction_data_fd_object.hdr.type_);
            assert_eq!(51, transaction_data_fd_object.cookie);
            // SAFETY: Union read.
            assert_eq!(receiver_fd.raw() as u32, unsafe {
                transaction_data_fd_object.__bindgen_anon_1.fd
            });
            transaction_state.release(());
        })
        .await;
    }

    #[fuchsia::test]
    async fn send_fd_in_transaction_with_prefetched_files() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);
            let mut receiver_shared_memory = receiver.lock_shared_memory();
            let mut allocations =
                receiver_shared_memory.allocate_buffers(0, 0, 0, 0).expect("allocate buffers");

            // Open a file in the sender process.
            let file = new_null_file(&current_task, OpenFlags::empty());
            let sender_fd =
                current_task.add_file(file.clone(), FdFlags::empty()).expect("add file");

            let mut source_files = vec![fbinder::FileHandle {
                handle: file.to_handle(current_task).expect("to_handle"),
                flags: Some(FileFlags::empty()),
                fd: Some(sender_fd.raw()),
                ..Default::default()
            }];

            // Send the fd in a transaction. `cookie` is set so that we can ensure binder
            // driver doesn't touch them/passes them through.
            let mut transaction_data = struct_with_union_into_bytes!(binder_fd_object {
                hdr.type_: BINDER_TYPE_FD,
                pad_flags: 0xdeadbeef,
                cookie: 51,
                __bindgen_anon_1.fd: sender_fd.raw() as u32,
            });
            let offsets = [0];

            let transient_transaction_state = device
                .translate_objects(
                    &sender.context(current_task),
                    &mut source_files,
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &offsets,
                    &mut transaction_data,
                    &mut allocations.scatter_gather_buffer,
                )
                .expect("failed to translate handles");

            // Simulate success by converting the transient state.
            let transaction_state = transient_transaction_state.into_state();

            // The receiver should now have a file.
            let receiver_fd = receiver
                .task()
                .files()
                .get_all_fds()
                .first()
                .cloned()
                .expect("receiver should have FD");

            let (transaction_data_fd_object, _) =
                binder_fd_object::read_from_prefix(&transaction_data)
                    .expect("transaction_data ought be a binder_fd_object");
            assert_eq!(BINDER_TYPE_FD, transaction_data_fd_object.hdr.type_);
            assert_eq!(51, transaction_data_fd_object.cookie);
            // SAFETY: Union read.
            assert_eq!(receiver_fd.raw() as u32, unsafe {
                transaction_data_fd_object.__bindgen_anon_1.fd
            });
            transaction_state.release(());
        })
        .await;
    }

    #[fuchsia::test]
    async fn cleanup_fd_in_failed_transaction() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);
            let mut receiver_shared_memory = receiver.lock_shared_memory();
            let mut allocations =
                receiver_shared_memory.allocate_buffers(0, 0, 0, 0).expect("allocate buffers");

            // Open a file in the sender process.
            let file = PanickingFile::new_file(&current_task);
            let sender_fd = current_task.add_file(file, FdFlags::CLOEXEC).expect("add file");

            // Send the fd in a transaction.
            let mut transaction_data = struct_with_union_into_bytes!(binder_fd_object {
                hdr.type_: BINDER_TYPE_FD,
                pad_flags: 0xdeadbeef,
                cookie: 0,
                __bindgen_anon_1.fd: sender_fd.raw() as u32,
            });
            let offsets = [0];

            let transaction_state = device
                .translate_objects(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &offsets,
                    &mut transaction_data,
                    &mut allocations.scatter_gather_buffer,
                )
                .expect("failed to translate handles");

            assert!(
                !receiver.task().files().get_all_fds().is_empty(),
                "receiver should have a file"
            );

            // Simulate an error, which will release the transaction state.
            transaction_state.release(());

            assert!(
                receiver.task().files().get_all_fds().is_empty(),
                "receiver should not have any files"
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn cleanup_refs_in_successful_transaction() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);
            let mut receiver_shared_memory = receiver.lock_shared_memory();
            let mut allocations =
                receiver_shared_memory.allocate_buffers(0, 0, 0, 0).expect("allocate buffers");

            const BINDER_OBJECT: LocalBinderObject = LocalBinderObject {
                weak_ref_addr: UserAddress::const_from(0x0000000000000010),
                strong_ref_addr: UserAddress::const_from(0x0000000000000100),
            };

            const DATA_PREAMBLE: &[u8; 5] = b"stuff";

            let mut transaction_data = vec![];
            transaction_data.extend(DATA_PREAMBLE);
            let offsets = [transaction_data.len() as binder_uintptr_t];
            transaction_data.extend(struct_with_union_into_bytes!(flat_binder_object {
                hdr.type_: BINDER_TYPE_BINDER,
                flags: 0,
                cookie: BINDER_OBJECT.strong_ref_addr.ptr() as u64,
                __bindgen_anon_1.binder: BINDER_OBJECT.weak_ref_addr.ptr() as u64,
            }));

            const EXPECTED_HANDLE: Handle = Handle::from_raw(1);

            let transaction_state = device
                .translate_objects(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    receiver.task(),
                    receiver.task(),
                    &receiver.proc,
                    &offsets,
                    &mut transaction_data,
                    &mut allocations.scatter_gather_buffer,
                )
                .expect("failed to translate handles");

            let (object, guard) = receiver
                .proc
                .lock()
                .handles
                .get(EXPECTED_HANDLE.object_index())
                .expect("expected handle not present");
            guard.release(&mut RefCountActions::default_released());
            object.ack_acquire(&mut RefCountActions::default_released()).expect("ack_acquire");

            // Verify that a strong acquire command is sent to the sender process (on the same thread
            // that sent the transaction).
            assert_matches!(
                sender.thread.lock().command_queue.commands.front(),
                Some(QueuedCommand { command: Command::AcquireRef(BINDER_OBJECT), .. })
            );
            sender.thread.lock().command_queue.pop_front().unwrap();

            // Simulate a successful transaction by converting the transient state.
            let transaction_state = transaction_state.into_state();
            transaction_state.release(());

            // Verify that a strong release command is sent to the sender process.
            assert_matches!(
                &sender.proc.lock().command_queue.front(),
                Some(QueuedCommand { command: Command::ReleaseRef(BINDER_OBJECT), .. })
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn transaction_error_dispatch() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let proc = BinderProcessFixture::new(current_task, &device);

            TransactionError::Malformed(errno!(EINVAL))
                .dispatch(&proc.thread)
                .expect("no error");
            assert_matches!(
                proc.thread.lock().command_queue.pop_front(),
                Some(QueuedCommand { command: Command::Error(val), .. }) if val == EINVAL.return_value() as i32
            );

            TransactionError::Failure
                .dispatch(&proc.thread)
                .expect("no error");
            assert_matches!(
                proc.thread.lock().command_queue.pop_front(),
                Some(QueuedCommand { command: Command::FailedReply, .. })
            );

            TransactionError::Dead
                .dispatch(&proc.thread)
                .expect("no error");
            assert_matches!(
                proc.thread.lock().command_queue.pop_front(),
                Some(QueuedCommand { command: Command::DeadReply, .. })
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn next_oneway_transaction_scheduled_after_buffer_freed() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Insert a binder object for the receiver, and grab a handle to it in the sender.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (object, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Construct a oneway transaction to send from the sender to the receiver.
            const FIRST_TRANSACTION_CODE: u32 = 42;
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: FIRST_TRANSACTION_CODE,
                    flags: transaction_flags_TF_ONE_WAY,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Submit the transaction.
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), transaction)
                .expect("failed to handle the transaction");

            // The thread is ineligible to take the command (not sleeping) so check the process queue.
            assert_matches!(
                receiver.proc.lock().command_queue.front(),
                Some(QueuedCommand {
                    command: Command::OnewayTransaction(TransactionData {
                        code: FIRST_TRANSACTION_CODE,
                        peer_pid: 0,
                        ..
                    }),
                    ..
                })
            );

            // The object should not have the transaction queued on it, as it was immediately scheduled.
            // But it should be marked as handling a oneway.
            assert!(
                object.lock().handling_oneway_transaction,
                "object oneway queue should be marked as being handled"
            );

            // Queue another transaction.
            const SECOND_TRANSACTION_CODE: u32 = 43;
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: SECOND_TRANSACTION_CODE,
                    flags: transaction_flags_TF_ONE_WAY,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), transaction)
                .expect("transaction queued");

            // There should now be an entry in the queue.
            assert_eq!(object.lock().oneway_transactions.len(), 1);

            // The process queue should be unchanged. Simulate dispatching the command.
            let buffer_addr = match receiver
                .proc
                .lock()
                .command_queue
                .pop_front()
                .expect("the first oneway transaction should be queued on the process")
            {
                QueuedCommand {
                    command:
                        Command::OnewayTransaction(TransactionData {
                            code: FIRST_TRANSACTION_CODE,
                            buffers: TransactionBuffers { data, .. },
                            ..
                        }),
                    ..
                } => data.address,
                _ => panic!("unexpected command in process queue"),
            };

            // Now the receiver issues the `BC_FREE_BUFFER` command, which should queue up the next
            // oneway transaction, guaranteeing sequential execution.
            receiver.proc.handle_free_buffer(buffer_addr).expect("failed to free buffer");

            assert!(
                object.lock().oneway_transactions.is_empty(),
                "oneway queue should now be empty"
            );
            assert!(
                object.lock().handling_oneway_transaction,
                "object oneway queue should still be marked as being handled"
            );

            // The process queue should have a new transaction. Simulate dispatching the command.
            let buffer_addr = match receiver
                .proc
                .lock()
                .command_queue
                .pop_front()
                .expect("the second oneway transaction should be queued on the process")
            {
                QueuedCommand {
                    command:
                        Command::OnewayTransaction(TransactionData {
                            code: SECOND_TRANSACTION_CODE,
                            buffers: TransactionBuffers { data, .. },
                            ..
                        }),
                    ..
                } => data.address,
                _ => panic!("unexpected command in process queue"),
            };

            // Now the receiver issues the `BC_FREE_BUFFER` command, which should end oneway handling.
            receiver.proc.handle_free_buffer(buffer_addr).expect("failed to free buffer");

            assert!(
                object.lock().oneway_transactions.is_empty(),
                "oneway queue should still be empty"
            );
            assert!(
                !object.lock().handling_oneway_transaction,
                "object oneway queue should no longer be marked as being handled"
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn synchronous_transactions_bypass_oneway_transaction_queue() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Insert a binder object for the receiver, and grab a handle to it in the sender.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (object, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Construct a oneway transaction to send from the sender to the receiver.
            const ONEWAY_TRANSACTION_CODE: u32 = 42;
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: ONEWAY_TRANSACTION_CODE,
                    flags: transaction_flags_TF_ONE_WAY,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Submit the transaction twice so that the queue is populated.
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), transaction)
                .expect("failed to handle the transaction");
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), transaction)
                .expect("failed to handle the transaction");

            // The thread is ineligible to take the command (not sleeping) so check (and dequeue)
            // the process queue.
            assert_matches!(
                receiver.proc.lock().command_queue.pop_front(),
                Some(QueuedCommand {
                    command: Command::OnewayTransaction(TransactionData {
                        code: ONEWAY_TRANSACTION_CODE,
                        ..
                    }),
                    ..
                })
            );

            // The object should also have the second transaction queued on it.
            assert!(
                object.lock().handling_oneway_transaction,
                "object oneway queue should be marked as being handled"
            );
            assert_eq!(
                object.lock().oneway_transactions.len(),
                1,
                "object oneway queue should have second transaction queued"
            );

            // Queue a synchronous (request/response) transaction.
            const SYNC_TRANSACTION_CODE: u32 = 43;
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: SYNC_TRANSACTION_CODE,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), transaction)
                .expect("sync transaction queued");

            assert_eq!(
                object.lock().oneway_transactions.len(),
                1,
                "oneway queue should not have grown"
            );

            // The process queue should now have the synchronous transaction queued.
            assert_matches!(
                receiver.proc.lock().command_queue.pop_front(),
                Some(QueuedCommand {
                    command: Command::Transaction {
                        data: TransactionData { code: SYNC_TRANSACTION_CODE, .. },
                        ..
                    },
                    ..
                })
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn dead_reply_when_transaction_recipient_proc_dies() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Insert a binder object for the receiver, and grab a handle to it in the sender.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (_, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Construct a synchronous transaction to send from the sender to the receiver.
            const FIRST_TRANSACTION_CODE: u32 = 42;
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: FIRST_TRANSACTION_CODE,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Submit the transaction.
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), transaction)
                .expect("failed to handle the transaction");

            // Check that there are no commands waiting for the sending thread.
            assert!(sender.thread.lock().command_queue.is_empty());

            // Check that the receiving process has a transaction scheduled.
            assert_matches!(
                receiver.proc.lock().command_queue.front(),
                Some(QueuedCommand { command: Command::Transaction { .. }, .. })
            );

            // Drop the receiving process.
            std::mem::drop(receiver);

            // Check that there is a dead reply command for the sending thread.
            assert_matches!(
                sender.thread.lock().command_queue.commands.front(),
                Some(QueuedCommand { command: Command::DeadReply, .. })
            );
            // Check that the transaction has been popped.
            assert_matches!(sender.thread.lock().transactions.pop(), None);
        })
        .await;
    }

    #[fuchsia::test]
    async fn dead_reply_when_transaction_recipient_proc_dies_not_top_transaction() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);
            let second_receiver = BinderProcessFixture::new(current_task, &device);

            // Create the first transaction, which will be sent from `sender` to `receiver`.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (_, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());
            const FIRST_TRANSACTION_CODE: u32 = 42;
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: FIRST_TRANSACTION_CODE,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Create the second transaction, which will be sent from `sender` to
            // `second_receiver`.
            const OBJECT_ADDR_2: UserAddress = UserAddress::const_from(0x20);
            let (_, guard) = register_binder_object(
                &second_receiver.proc,
                OBJECT_ADDR_2,
                (OBJECT_ADDR_2 + 1u64).unwrap(),
            );
            let second_handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());
            const SECOND_TRANSACTION_CODE: u32 = 43;
            let second_transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: SECOND_TRANSACTION_CODE,
                    target: binder_transaction_data__bindgen_ty_1 { handle: second_handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Submit the transactions, creating a transaction stack where the transaction
            // targeting `second_receiver` is on top.
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), transaction)
                .expect("failed to handle the transaction");
            device
                .handle_transaction(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    second_transaction,
                )
                .expect("failed to handle the transaction");

            // Check that both receivers have a transaction scheduled.
            assert_matches!(
                receiver.proc.lock().command_queue.front(),
                Some(QueuedCommand { command: Command::Transaction { .. }, .. })
            );
            assert_matches!(
                second_receiver.proc.lock().command_queue.front(),
                Some(QueuedCommand { command: Command::Transaction { .. }, .. })
            );

            // Drop the receiving process for the bottom transaction.
            std::mem::drop(receiver);

            // Check that there are no dead replies waiting for the thread, and that no
            // transactions have been popped, since `receiver` was not the target of the top
            // transaction.
            assert_matches!(sender.thread.lock().command_queue.commands.front(), None);
            assert_eq!(sender.thread.lock().transactions.len(), 2);

            // Drop the second receiver.
            std::mem::drop(second_receiver);

            // Check that there is one dead reply now, and that one transaction has been popped.
            assert_matches!(
                sender.thread.lock().command_queue.commands.front(),
                Some(QueuedCommand { command: Command::DeadReply, .. })
            );
            assert_eq!(sender.thread.lock().transactions.len(), 1);

            // Read out the first dead reply and verify that there is still one pending transaction
            // left (the bottom one, targeting `receiver`).
            let read_buffer_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            device
                .handle_thread_read(
                    &sender.context(current_task),
                    &UserBuffer { address: read_buffer_addr, length: *PAGE_SIZE as usize },
                )
                .expect("read command");
            assert_eq!(sender.thread.lock().transactions.len(), 1);

            // Make sure that there is no command left to be processed, but that the next time the
            // thread handles a read, it detects that the top transaction is dead, generates a dead
            // reply, and pops the transaction.
            assert_matches!(sender.thread.lock().command_queue.commands.front(), None);
            device
                .handle_thread_read(
                    &sender.context(current_task),
                    &UserBuffer { address: read_buffer_addr, length: *PAGE_SIZE as usize },
                )
                .expect("read command");
            // Verify that the transaction was popped by the dead reply.
            assert_eq!(sender.thread.lock().transactions.len(), 0);
        })
        .await;
    }

    #[fuchsia::test]
    async fn dead_reply_when_transaction_recipient_thread_dies() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Insert a binder object for the receiver, and grab a handle to it in the sender.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (_, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Construct a synchronous transaction to send from the sender to the receiver.
            const FIRST_TRANSACTION_CODE: u32 = 42;
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: FIRST_TRANSACTION_CODE,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Submit the transaction.
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), transaction)
                .expect("failed to handle the transaction");

            // Check that there are no commands waiting for the sending thread.
            assert!(sender.thread.lock().command_queue.is_empty());

            // Check that the receiving process has a transaction scheduled.
            assert_matches!(
                receiver.proc.lock().command_queue.front(),
                Some(QueuedCommand { command: Command::Transaction { .. }, .. })
            );

            // Drop the receiving process.
            std::mem::drop(receiver);

            // Check that there is a dead reply command for the sending thread.
            assert_matches!(
                sender.thread.lock().command_queue.commands.front(),
                Some(QueuedCommand { command: Command::DeadReply, .. })
            );
            assert_matches!(sender.thread.lock().transactions.pop(), None);
        })
        .await;
    }

    #[fuchsia::test]
    async fn dead_reply_when_transaction_recipient_thread_dies_while_processing_reply() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new_current(current_task, &device);

            // Insert a binder object for the receiver, and grab a handle to it in the sender.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (_, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Construct a synchronous transaction to send from the sender to the receiver.
            const FIRST_TRANSACTION_CODE: u32 = 42;
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: FIRST_TRANSACTION_CODE,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Make the receiver thread look eligible for transactions.
            // Pretend the client thread is waiting for commands, so that it can be scheduled commands.
            let fake_waiter = Waiter::new();
            {
                let mut thread_state = receiver.thread.lock();
                thread_state.registration = RegistrationState::Main;
                thread_state.command_queue.waiters.wait_async(&fake_waiter);
            }

            // Submit the transaction.
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), transaction)
                .expect("failed to handle the transaction");

            // Check that there are no commands waiting for the sending thread.
            assert!(sender.thread.lock().command_queue.is_empty());

            // Check that the receiving process has a transaction scheduled. Because the thread is
            // available, the command ends up directly on the thread's command queue.
            assert_matches!(
                receiver.thread.lock().command_queue.commands.front(),
                Some(QueuedCommand { command: Command::Transaction { .. }, .. })
            );

            // Have the thread dequeue the command.
            let read_buffer_addr = map_memory(current_task, UserAddress::default(), *PAGE_SIZE);
            device
                .handle_thread_read(
                    &receiver.context(current_task),
                    &UserBuffer { address: read_buffer_addr, length: *PAGE_SIZE as usize },
                )
                .expect("read command");

            // The thread should now have an empty command list and an ongoing transaction.
            assert!(receiver.thread.lock().command_queue.is_empty());
            assert!(!receiver.thread.lock().transactions.is_empty());

            // Drop the receiving process and thread.
            std::mem::drop(receiver);

            // Check that there is a dead reply command for the sending thread.
            assert_matches!(
                sender.thread.lock().command_queue.commands.front(),
                Some(QueuedCommand { command: Command::DeadReply, .. })
            );
            assert_matches!(sender.thread.lock().transactions.pop(), None);
        })
        .await;
    }

    #[fuchsia::test]
    async fn failed_reply_when_transaction_reply_is_too_big() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new_current(current_task, &device);

            // Insert a binder object for the receiver, and grab a handle to it in the sender.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (_, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Construct a synchronous transaction to send from the sender to the receiver.
            const FIRST_TRANSACTION_CODE: u32 = 42;
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: FIRST_TRANSACTION_CODE,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Make the receiver thread look eligible for transactions.
            // Pretend the client thread is waiting for commands, so that it can be scheduled commands.
            let fake_waiter = Waiter::new();
            {
                let mut thread_state = receiver.thread.lock();
                thread_state.registration = RegistrationState::Main;
                thread_state.command_queue.waiters.wait_async(&fake_waiter);
            }

            // Submit the transaction.
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), transaction)
                .expect("failed to handle the transaction");

            // Check that there are no commands waiting for the sending thread.
            assert!(sender.thread.lock().command_queue.is_empty());

            // Check that the receiving process' thread has a transaction scheduled.
            assert_matches!(
                receiver.thread.lock().command_queue.commands.front(),
                Some(QueuedCommand { command: Command::Transaction { .. }, .. })
            );

            // Have the thread dequeue the command.
            let read_buffer_addr = map_memory(current_task, UserAddress::default(), *PAGE_SIZE);
            device
                .handle_thread_read(
                    &receiver.context(current_task),
                    &UserBuffer { address: read_buffer_addr, length: *PAGE_SIZE as usize },
                )
                .expect("read command");

            // The thread should now have an empty command list and an ongoing transaction.
            assert!(receiver.thread.lock().command_queue.is_empty());
            assert!(!receiver.thread.lock().transactions.is_empty());

            // Respond to the transaction with too much data.
            let reply = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: FIRST_TRANSACTION_CODE,
                    ..binder_transaction_data::default()
                },
                buffers_size: (u64::MAX / 2) & !7,
            };

            // Submit the reply.
            assert_eq!(
                device
                    .handle_reply(&receiver.context(current_task), &mut Vec::new(), reply)
                    .expect_err("transaction should have failed"),
                TransactionError::Failure
            );

            assert!(receiver.thread.lock().transactions.is_empty());
            assert_matches!(
                sender.thread.lock().command_queue.pop_front(),
                Some(QueuedCommand { command: Command::FailedReply, .. })
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_process_queue_unhandled_command_stuck_during_synchronous_call() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let proc_a = BinderProcessFixture::new_current(current_task, &device);
            let proc_b = BinderProcessFixture::new_current(current_task, &device);

            // Register binder object on proc_b so proc_a can call proc_b synchronously.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (_, guard) =
                register_binder_object(&proc_b.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle_b = proc_a
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Create a second thread (looper_thread) in proc_a representing a binder looper thread.
            let looper_task = current_task.clone_task_for_test(
                (starnix_uapi::CLONE_VM | starnix_uapi::CLONE_THREAD | starnix_uapi::CLONE_SIGHAND)
                    as u64,
                None,
            );
            let looper_thread = proc_a
                .proc
                .lock()
                .find_or_register_thread(&looper_task.task)
                .expect("register looper thread");

            // Make looper_thread registered as Auxiliary looper and waiting for commands.
            let looper_waiter = Waiter::new();
            {
                let mut looper_state = looper_thread.lock();
                looper_state.registration = RegistrationState::Auxilliary;
                looper_state.command_queue.waiters.wait_async(&looper_waiter);
            }

            // 1. proc_a.thread (main thread) sends synchronous transaction to proc_b.
            let transaction_to_b = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: 100,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle_b.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };
            device
                .handle_transaction(
                    &proc_a.context(current_task),
                    &mut Vec::new(),
                    transaction_to_b,
                )
                .expect("handle_transaction to proc_b");

            // 2. A process-directed command is enqueued on proc_a.
            let oneway_to_a = Command::OnewayTransaction(TransactionData {
                peer_pid: proc_b.proc.key.id,
                peer_tid: proc_b.thread.tid,
                peer_euid: current_task.current_creds().euid,
                object: FlatBinderObject::Remote { handle: Handle::ContextManager },
                code: 200,
                flags: transaction_flags_TF_ONE_WAY,
                buffers: TransactionBuffers::default(),
            });
            proc_a.proc.enqueue_command(oneway_to_a.into());

            // 3. proc_b sends reply back to proc_a.thread.
            let reply = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: 100,
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };
            {
                let mut proc_b_thread_state = proc_b.thread.lock();
                proc_b_thread_state.transactions.push(TransactionRole::Receiver {
                    peer: WeakBinderPeer::new(&proc_a.proc, &proc_a.thread),
                    trace_id: fuchsia_trace::Id::new(),
                });
            }
            device
                .handle_reply(&proc_b.context(current_task), &mut Vec::new(), reply)
                .expect("handle_reply to proc_a");

            // 4. proc_a.thread reads from driver once and receives BR_REPLY.
            let read_buffer_addr = map_memory(current_task, UserAddress::default(), *PAGE_SIZE);
            device
                .handle_thread_read(
                    &proc_a.context(current_task),
                    &UserBuffer { address: read_buffer_addr, length: *PAGE_SIZE as usize },
                )
                .expect("proc_a.thread handle_thread_read");

            // proc_a.thread returns to userspace. It will NOT call handle_thread_read again!
            assert!(proc_a.thread.lock().transactions.is_empty());

            // 5. Verify that looper_thread (which was waiting in available_threads) handles
            // the process queue command without requiring proc_a.thread to re-enter handle_thread_read.
            let looper_context = OperationContext {
                current_task: &looper_task,
                connection_security_state: &proc_a.connection_security_state,
                binder_proc: &proc_a.proc,
                binder_thread: &looper_thread,
                memory_accessor: looper_task.as_memory_accessor().expect("as_memory_accessor"),
            };
            device
                .handle_thread_read(
                    &looper_context,
                    &UserBuffer { address: read_buffer_addr, length: *PAGE_SIZE as usize },
                )
                .expect("looper_thread handle_thread_read");

            // Process command queue should now be empty and processed by looper_thread!
            assert!(
                proc_a.proc.lock().command_queue.is_empty(),
                "The process queue command should be successfully handled and drained by the looper thread!"
            );

            looper_thread.release(current_task.kernel());
        })
        .await;
    }

    #[fuchsia::test]
    async fn connect_to_multiple_binder() {
        spawn_kernel_and_run(async |current_task| {
            let driver = BinderDevice::default();

            // Opening the driver twice from the same task must succeed.
            let _d1 = open_binder_fd(&current_task, &driver);
            let _d2 = open_binder_fd(&current_task, &driver);
        })
        .await;
    }

    pub type TestFdTable = BTreeMap<i32, fbinder::FileHandle>;
    /// Run a test implementation of the ProcessAccessor protocol.
    /// The test implementation starts with an empty fd table, and updates it depending on the
    /// client calls. The future will resolve when the client is disconnected and return the
    /// current fd table at that point.
    pub async fn run_process_accessor(
        server_end: ServerEnd<fbinder::ProcessAccessorMarker>,
    ) -> Result<TestFdTable, anyhow::Error> {
        let mut stream = fbinder::ProcessAccessorRequestStream::from_channel(
            fasync::Channel::from_channel(server_end.into_channel()),
        );
        // The fd table is per connection.
        let mut next_fd = 0;
        let mut fds: TestFdTable = Default::default();
        'event_loop: while let Some(event) = stream.try_next().await? {
            match event {
                fbinder::ProcessAccessorRequest::WriteMemory { address, content, responder } => {
                    let size = content.get_content_size()?;
                    // SAFETY: This is not safe and rely on the client being correct.
                    let buffer = unsafe {
                        std::slice::from_raw_parts_mut(address as *mut u8, size as usize)
                    };
                    content.read(buffer, 0)?;
                    responder.send(Ok(()))?;
                }
                fbinder::ProcessAccessorRequest::WriteBytes { address, bytes, responder } => {
                    // SAFETY: This is not safe and rely on the client being correct.
                    let buffer =
                        unsafe { std::slice::from_raw_parts_mut(address as *mut u8, bytes.len()) };
                    buffer.copy_from_slice(bytes.as_slice());
                    responder.send(Ok(()))?;
                }
                fbinder::ProcessAccessorRequest::FileRequest { payload, responder } => {
                    let mut response = fbinder::FileResponse::default();
                    for fd in payload.close_requests.unwrap_or(vec![]) {
                        if fds.remove(&fd).is_none() {
                            responder.send(Err(fposix::Errno::Ebadf))?;
                            continue 'event_loop;
                        }
                    }
                    for fd in payload.get_requests.unwrap_or(vec![]) {
                        if let Some(file) = fds.remove(&fd) {
                            response.get_responses.get_or_insert_with(Vec::new).push(file);
                        } else {
                            responder.send(Err(fposix::Errno::Ebadf))?;
                            continue 'event_loop;
                        }
                    }
                    for mut file in payload.add_requests.unwrap_or(vec![]) {
                        let fd = next_fd;
                        next_fd += 1;
                        // NOTE: For tests, we fake a flag. As we add, then get. In production, the
                        // flags for a get would come from the underlying handle.s
                        file.flags = Some(FileFlags::RIGHT_READABLE);
                        fds.insert(fd, file);
                        response.add_responses.get_or_insert_with(Vec::new).push(fd);
                    }
                    responder.send(Ok(response))?;
                }
                fbinder::ProcessAccessorRequest::_UnknownMethod { ordinal, .. } => {
                    log_warn!("Unknown ProcessAccessor ordinal: {}", ordinal);
                }
            }
        }
        Ok(fds)
    }

    /// Spawn a new thread that will run a test implementation of the ProcessAccessor
    /// protocol.
    /// The test implementation starts with an empty fd table, and updates it depending
    /// on the client calls. The thread will stop when the client is disconnected.
    /// This function will then return the current fd table at that point.
    fn spawn_new_process_accessor_thread(
        server_end: ServerEnd<fbinder::ProcessAccessorMarker>,
    ) -> std::thread::JoinHandle<Result<TestFdTable, anyhow::Error>> {
        std::thread::spawn(move || {
            let mut executor = LocalExecutor::default();
            executor.run_singlethreaded(run_process_accessor(server_end))
        })
    }

    #[allow(dead_code)]
    fn apply_writes(ioctl_writes: Vec<fbinder::IoctlReadWrite>, vmo: &zx::Vmo) {
        for ioctl_write in ioctl_writes.iter() {
            // SAFETY This is required to emulate the scattered writes for tests.
            #[allow(
                clippy::undocumented_unsafe_blocks,
                reason = "Force documented unsafe blocks in Starnix"
            )]
            unsafe {
                vmo.read_raw(
                    ioctl_write.address as *mut u8,
                    ioctl_write.length as usize,
                    ioctl_write.offset,
                )
                .expect("read_raw")
            }
        }
    }

    #[::fuchsia::test]
    async fn remote_binder_task() {
        const VECTOR_SIZE: usize = 128 * 1024 * 1024;
        const_assert!(VECTOR_SIZE > fbinder::MAX_WRITE_BYTES as usize);
        const SMALL_SIZE: usize = 128;
        const_assert!(SMALL_SIZE <= fbinder::MAX_WRITE_BYTES as usize);
        let (process_accessor_client_end, process_accessor_server_end) =
            create_endpoints::<fbinder::ProcessAccessorMarker>();

        let process_accessor_thread =
            spawn_new_process_accessor_thread(process_accessor_server_end);

        let process_accessor = fbinder::ProcessAccessorSynchronousProxy::new(
            process_accessor_client_end.into_channel(),
        );

        spawn_kernel_and_run(async |task| {
            let process = fuchsia_runtime::process_self()
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .expect("process");
            let remote_creds = Credentials::root();
            let remote_binder_task =
                Arc::new(RemoteResourceAccessor { process_accessor, process, remote_creds });
            let mut vector = Vec::with_capacity(VECTOR_SIZE);
            for i in 0..VECTOR_SIZE {
                vector.push((i & 255) as u8);
            }
            let remote_ioctl = RemoteIoctl {
                ioctl_writes: Cell::new(Vec::new()),
                ioctl_reads: Vec::new(),
                vmo: zx::Vmo::create(VECTOR_SIZE as u64).expect("Vmo::create"),
            };
            let remote_memory_accessor = RemoteMemoryAccessor {
                remote_resource_accessor: remote_binder_task.clone(),
                remote_ioctl: &remote_ioctl,
            };
            let other_vector = remote_memory_accessor
                .read_memory_to_vec((vector.as_ptr() as u64).into(), VECTOR_SIZE)
                .expect("read_memory");
            assert_eq!(vector[1], 1);
            assert_eq!(vector, other_vector);
            vector.fill(0);
            remote_memory_accessor
                .write_memory((vector.as_ptr() as u64).into(), &other_vector)
                .expect("write_memory");
            apply_writes(remote_ioctl.ioctl_writes.take(), &remote_ioctl.vmo);
            assert_eq!(vector[1], 1);
            assert_eq!(vector, other_vector);
            vector.fill(0);
            remote_memory_accessor
                .write_memory((vector.as_ptr() as u64).into(), &other_vector[..SMALL_SIZE])
                .expect("write_memory");
            apply_writes(remote_ioctl.ioctl_writes.take(), &remote_ioctl.vmo);
            assert_eq!(vector[1], 1);
            assert_eq!(vector[..SMALL_SIZE], other_vector[..SMALL_SIZE]);

            // Do one more write than there is space for. The last write should then use the
            // ProcessAccessor to write to the address.
            vector.fill(0);
            for _ in 0..=fbinder::MAX_IOCTL_WRITE_COUNT {
                remote_memory_accessor
                    .write_memory((vector.as_ptr() as u64).into(), &other_vector[..SMALL_SIZE])
                    .expect("write_memory");
            }
            assert_eq!(
                remote_ioctl.ioctl_writes.take().len(),
                fbinder::MAX_IOCTL_WRITE_COUNT as usize
            );
            assert_eq!(vector[1], 1);
            assert_eq!(vector[..SMALL_SIZE], other_vector[..SMALL_SIZE]);

            let mut files = vec![
                (new_null_file(&task, OpenFlags::RDONLY), FdFlags::empty()),
                (new_null_file(&task, OpenFlags::WRONLY), FdFlags::empty()),
            ];
            // Add more files to force chunking of requests.
            for _ in 0..fbinder::MAX_REQUEST_COUNT {
                files.push((new_null_file(&task, OpenFlags::RDWR), FdFlags::empty()));
            }
            let fds = remote_binder_task
                .add_files_with_flags(&task, files, &mut |_| {})
                .expect("add_files_with_flags");
            for (i, fd) in fds.iter().enumerate() {
                assert_eq!(fd.raw(), i as i32);
            }

            assert_eq!(remote_binder_task.close_files(vec![fds[1]]), Ok(()));
            let (handle, flags) = remote_binder_task
                .get_files_with_flags(&task, vec![fds[0]])
                .expect("get_files_with_flags")
                .pop()
                .expect("pop");
            assert_eq!(flags, FdFlags::empty());
            assert_eq!(handle.flags(), OpenFlags::RDONLY);

            assert_eq!(
                remote_binder_task
                    .get_files_with_flags(&task, vec![FdNumber::from_raw(1000)])
                    .expect_err("bad fd"),
                errno!(EBADF)
            );

            std::mem::drop(remote_memory_accessor);
            std::mem::drop(remote_binder_task);
            let fds = process_accessor_thread.join().expect("join").expect("fds");
            // Close and get requests both remove file descriptors.
            assert_eq!(fds.len(), fbinder::MAX_REQUEST_COUNT as usize);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn remote_binder_composite_file_descriptors() {
        let (process_accessor_client_end, process_accessor_server_end) =
            create_endpoints::<fbinder::ProcessAccessorMarker>();

        let process_accessor_thread =
            spawn_new_process_accessor_thread(process_accessor_server_end);

        let process_accessor = fbinder::ProcessAccessorSynchronousProxy::new(
            process_accessor_client_end.into_channel(),
        );

        spawn_kernel_and_run(async |task| {
            let process = fuchsia_runtime::process_self()
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .expect("process");
            let remote_creds = Credentials::root();
            let remote_binder_task =
                Arc::new(RemoteResourceAccessor { process_accessor, process, remote_creds });

            let sync_file = SyncFile::new_file(
                &task,
                [0; 32],
                SyncFence {
                    sync_points: vec![
                        SyncPoint::new(Timeline::Hwc, zx::Counter::create()),
                        SyncPoint::new(Timeline::Hwc, zx::Counter::create()),
                    ],
                },
            )
            .expect("new_file");

            let files = vec![(sync_file, FdFlags::empty())];
            let fds = remote_binder_task
                .add_files_with_flags(&task, files, &mut |_| {})
                .expect("add_files_with_flags");
            assert_eq!(fds.len(), 1);

            // TODO(https://fxbug.dev/481167098): Support composite file descriptors.
            assert_eq!(
                remote_binder_task
                    .get_files_with_flags(&task, vec![fds[0]])
                    .expect_err("get_files_with_flags should fail for composite file descriptors"),
                errno!(EBADFD)
            );

            std::mem::drop(remote_binder_task);
            let fds = process_accessor_thread.join().expect("join").expect("fds");
            // Close and get requests both remove file descriptors.
            assert_eq!(fds.len(), 0);
        })
        .await;
    }

    #[fuchsia::test]
    async fn no_reply_when_transaction_before_process_frozen() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Insert a binder object for the receiver, and grab a handle to it in the sender.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (_, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Construct a synchronous transaction to send from the sender to the receiver.
            const FIRST_TRANSACTION_CODE: u32 = 42;
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: FIRST_TRANSACTION_CODE,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Make the receiver thread look eligible for transactions.
            // Pretend the client thread is waiting for commands, so that it can be scheduled
            // commands.
            let fake_waiter = Waiter::new();
            {
                let mut thread_state = receiver.thread.lock();
                thread_state.registration = RegistrationState::Main;
                thread_state.command_queue.waiters.wait_async(&fake_waiter);
            }

            // Submit the transaction.
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), transaction)
                .expect("failed to handle the transaction");

            // Check that there are no commands waiting for the sending thread.
            assert!(sender.thread.lock().command_queue.is_empty());

            // Check that the receiving process has a transaction scheduled.
            assert_matches!(
                receiver.thread.lock().command_queue.commands.front(),
                Some(QueuedCommand { command: Command::Transaction { .. }, .. })
            );

            // Freeze the receiver process.
            receiver.proc.freeze_state.lock().freeze();

            // Check that there is a frozen reply command for the sending thread.
            assert!(sender.thread.lock().command_queue.commands.is_empty());
            assert_matches!(
                sender.thread.lock().transactions.pop(),
                Some(TransactionRole::Sender(_))
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn frozen_reply_when_process_frozen() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            // Insert a binder object for the receiver, and grab a handle to it in the sender.
            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (_, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Freeze the receiver process.
            let freeze_info_address = map_object_anywhere(
                &current_task,
                &binder_freeze_info {
                    pid: receiver.proc.key.id as u32,
                    enable: 1,
                    timeout_ms: 1000,
                },
            );
            device
                .ioctl(
                    current_task,
                    &security::binder_connection_alloc(current_task),
                    &receiver.proc,
                    None,
                    uapi::BINDER_FREEZE,
                    freeze_info_address.into(),
                    Vec::new(),
                )
                .expect("BINDER_FREEZE ioctl");

            // Construct a synchronous transaction to send from the sender to the receiver.
            const FIRST_TRANSACTION_CODE: u32 = 42;
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: FIRST_TRANSACTION_CODE,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Submit the transaction.
            assert_matches!(
                device.handle_transaction(
                    &sender.context(current_task),
                    &mut Vec::new(),
                    transaction,
                ),
                Err(TransactionError::Frozen)
            );

            // Check that there are no commands waiting for the sending thread.
            assert!(sender.thread.lock().command_queue.is_empty());
            assert!(sender.thread.lock().transactions.is_empty());

            // Check the frozen info
            let frozen_status_info_address = map_object_anywhere(
                &current_task,
                &binder_frozen_status_info {
                    pid: receiver.proc.key.id as u32,
                    sync_recv: 0,
                    async_recv: 0,
                },
            );
            device
                .ioctl(
                    current_task,
                    &security::binder_connection_alloc(current_task),
                    &receiver.proc,
                    None,
                    uapi::BINDER_GET_FROZEN_INFO,
                    frozen_status_info_address.into(),
                    Vec::new(),
                )
                .expect("BINDER_GET_FROZEN_INFO ioctl");
            let read_frozen_status_info = receiver
                .proc
                .get_memory_accessor(current_task, None)
                .read_object(UserRef::<binder_frozen_status_info>::new(frozen_status_info_address))
                .expect("read returned binder frozen status");
            assert_eq!(read_frozen_status_info.sync_recv, 1);
            assert_eq!(read_frozen_status_info.async_recv, 0)
        })
        .await;
    }

    #[fuchsia::test]
    async fn freeze_notification_fires_when_process_frozen() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let owner = BinderProcessFixture::new(current_task, &device);
            let client = BinderProcessFixture::new(current_task, &device);

            // Register an object with the owner.
            let guard = owner.proc.lock().find_or_register_object(
                &owner.thread,
                LocalBinderObject {
                    weak_ref_addr: UserAddress::from(0x0000000000000001),
                    strong_ref_addr: UserAddress::from(0x0000000000000002),
                },
                BinderObjectFlags::empty(),
            );

            // Insert a handle to the object in the client. This also retains a strong reference.
            let handle = client
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            const FREEZE_NOTIFICATION_COOKIE: binder_uintptr_t = 0xAAAAAAAA;

            // Register a death notification handler.
            client
                .proc
                .handle_request_freeze_notification(handle, FREEZE_NOTIFICATION_COOKIE)
                .expect("request freeze notification");

            // The client process should acknowledge the request.
            assert_matches!(
                client.proc.lock().command_queue.pop_front(),
                Some(QueuedCommand {
                    command: Command::FrozenBinder(binder_frozen_state_info { is_frozen: 0, .. }),
                    ..
                })
            );

            let pending_notifications = owner.proc.freeze_state.lock().freeze();
            for (proc, cmd) in pending_notifications {
                proc.enqueue_command(cmd.into());
                proc.release(current_task.kernel());
            }

            // The client process should have a notification waiting.
            assert_matches!(
                client.proc.lock().command_queue.front(),
                Some(QueuedCommand {
                    command: Command::FrozenBinder(binder_frozen_state_info { is_frozen: 1, .. }),
                    ..
                })
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn freeze_notification_is_cleared_before_process_frozen() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let owner = BinderProcessFixture::new(current_task, &device);
            let client = BinderProcessFixture::new(current_task, &device);

            // Register an object with the owner.
            let guard = owner.proc.lock().find_or_register_object(
                &owner.thread,
                LocalBinderObject {
                    weak_ref_addr: UserAddress::from(0x0000000000000001),
                    strong_ref_addr: UserAddress::from(0x0000000000000002),
                },
                BinderObjectFlags::empty(),
            );

            // Insert a handle to the object in the receiver. This also retains a strong reference.
            let handle = client
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            const FREEZE_NOTIFICATION_COOKIE: binder_uintptr_t = 0xAAAAAAAA;

            // Register a freeze notification handler.
            client
                .proc
                .handle_request_freeze_notification(handle, FREEZE_NOTIFICATION_COOKIE)
                .expect("request freeze notification");

            // Now clear the freeze notification handler.
            client
                .proc
                .handle_clear_freeze_notification(handle, FREEZE_NOTIFICATION_COOKIE)
                .expect("clear freeze notification");

            // Check that the pending FrozenBinder was cancelled, leaving only ClearFreezeNotificationDone.
            {
                let queue = &mut client.proc.lock().command_queue;
                assert_eq!(queue.len(), 1);
                assert!(matches!(
                    queue.pop_front(),
                    Some(QueuedCommand {
                        command: Command::ClearFreezeNotificationDone(FREEZE_NOTIFICATION_COOKIE),
                        ..
                    })
                ));
            }

            // Pretend the client thread is waiting for commands, so that it can be scheduled commands.
            let fake_waiter = Waiter::new();
            {
                let mut state = client.thread.lock();
                state.registration = RegistrationState::Main;
                state.command_queue.waiters.wait_async(&fake_waiter);
            }

            owner.proc.freeze_state.lock().freeze();

            // The client thread should have no notification.
            assert!(client.thread.lock().command_queue.is_empty());
            // The client process should have no notification.
            assert!(client.proc.lock().command_queue.is_empty());
        })
        .await;
    }

    #[fuchsia::test]
    async fn freeze_notification_is_cleared_after_process_dead() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let owner = BinderProcessFixture::new(current_task, &device);
            let client = BinderProcessFixture::new(current_task, &device);

            // Register an object with the owner.
            let guard = owner.proc.lock().find_or_register_object(
                &owner.thread,
                LocalBinderObject {
                    weak_ref_addr: UserAddress::from(0x0000000000000001),
                    strong_ref_addr: UserAddress::from(0x0000000000000002),
                },
                BinderObjectFlags::empty(),
            );

            // Insert a handle to the object in the receiver. This also retains a strong reference.
            let handle = client
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            const FREEZE_NOTIFICATION_COOKIE: binder_uintptr_t = 0xAAAAAAAA;

            // Register a freeze notification handler.
            client
                .proc
                .handle_request_freeze_notification(handle, FREEZE_NOTIFICATION_COOKIE)
                .expect("request freeze notification");

            // The client process should acknowledge the request.
            assert_matches!(
                client.proc.lock().command_queue.pop_front(),
                Some(QueuedCommand {
                    command: Command::FrozenBinder(binder_frozen_state_info { is_frozen: 0, .. }),
                    ..
                })
            );

            // Now let the owner process die!
            drop(owner);

            // Clear the freeze notification handler. Since the owner is dead,
            // this should quietly succeed and return Ok(()) under our fix.
            client
                .proc
                .handle_clear_freeze_notification(handle, FREEZE_NOTIFICATION_COOKIE)
                .expect("clear freeze notification on dead process should succeed");

            // Check that the client received the ClearFreezeNotificationDone acknowledgement.
            assert_matches!(
                client.proc.lock().command_queue.pop_front(),
                Some(QueuedCommand {
                    command: Command::ClearFreezeNotificationDone(FREEZE_NOTIFICATION_COOKIE),
                    ..
                })
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn freeze_notification_cleared_while_in_flight_holds_until_done() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let owner = BinderProcessFixture::new(current_task, &device);
            let client = BinderProcessFixture::new(current_task, &device);

            let guard = owner.proc.lock().find_or_register_object(
                &owner.thread,
                LocalBinderObject {
                    weak_ref_addr: UserAddress::from(0x0000000000000001),
                    strong_ref_addr: UserAddress::from(0x0000000000000002),
                },
                BinderObjectFlags::empty(),
            );

            let handle = client
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            const FREEZE_NOTIFICATION_COOKIE: binder_uintptr_t = 0xBBBBBBBB;

            // Register a freeze notification handler.
            client
                .proc
                .handle_request_freeze_notification(handle, FREEZE_NOTIFICATION_COOKIE)
                .expect("request freeze notification");

            // Dequeue the notification, simulating it being dispatched to userspace.
            assert_matches!(
                client.proc.lock().command_queue.pop_front(),
                Some(QueuedCommand {
                    command: Command::FrozenBinder(binder_frozen_state_info { is_frozen: 0, .. }),
                    ..
                })
            );

            // Mark this notification as in-flight.
            client.proc.lock().in_flight_freeze_notifications.insert(FREEZE_NOTIFICATION_COOKIE);

            // Clear the freeze notification while it is in flight.
            client
                .proc
                .handle_clear_freeze_notification(handle, FREEZE_NOTIFICATION_COOKIE)
                .expect("clear freeze notification while in flight");

            // The ClearFreezeNotificationDone command should NOT be enqueued yet.
            assert!(client.proc.lock().command_queue.is_empty());

            // Acknowledge the freeze notification.
            client.proc.handle_freeze_notification_done(FREEZE_NOTIFICATION_COOKIE);

            // Now the ClearFreezeNotificationDone command should be in the queue.
            assert_matches!(
                client.proc.lock().command_queue.pop_front(),
                Some(QueuedCommand {
                    command: Command::ClearFreezeNotificationDone(FREEZE_NOTIFICATION_COOKIE),
                    ..
                })
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_transaction_known_receiver_from_thread_pool() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let sender = BinderProcessFixture::new_current(current_task, &device);
            let receiver = BinderProcessFixture::new(current_task, &device);

            const OBJECT_ADDR: UserAddress = UserAddress::const_from(0x01);
            let (_, guard) =
                register_binder_object(&receiver.proc, OBJECT_ADDR, (OBJECT_ADDR + 1u64).unwrap());
            let handle = sender
                .proc
                .lock()
                .handles
                .insert_for_transaction(guard, &mut RefCountActions::default_released());

            // Make the receiver thread available for transactions.
            let event = InterruptibleEvent::new();
            let (mut fake_waiter, _guard) = SimpleWaiter::new(&event);
            {
                let mut thread_state = receiver.thread.lock();
                thread_state.registration = RegistrationState::Main;
                thread_state.command_queue.waiters.wait_async_simple(&mut fake_waiter);
            }

            // Construct a synchronous transaction to send from the sender to the receiver.
            const FIRST_TRANSACTION_CODE: u32 = 42;
            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    code: FIRST_TRANSACTION_CODE,
                    target: binder_transaction_data__bindgen_ty_1 { handle: handle.into() },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Process the transaction
            device
                .handle_transaction(&sender.context(current_task), &mut Vec::new(), transaction)
                .expect("failed to handle the transaction");

            // We're not responding to a request, but we should still pull a thread from the
            // pool and thus know who we are sending to.
            let sender_thread_state = sender.thread.lock();
            let transaction_role =
                sender_thread_state.transactions.last().expect("should have a transaction");
            if let TransactionRole::Sender(sender_info) = transaction_role {
                assert_eq!(sender_info.target_thread_handle, Some(receiver.thread.thread.clone()));
            } else {
                panic!("Expected Sender role, got {:?}", transaction_role);
            }
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_transaction_priority_inheritance_response_propagation() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let proc_a = BinderProcessFixture::new_current(current_task, &device);

            let proc_b = BinderProcessFixture::new_current(current_task, &device);
            let event = InterruptibleEvent::new();
            let event_clone = event.clone();

            let proc_a_thread_handle =
                proc_a.thread.thread.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
            let (tx, rx) = std::sync::mpsc::channel();

            let blocked_thread = std::thread::spawn(move || {
                let (fake_waiter, guard) = SimpleWaiter::new(&event_clone);
                tx.send(fake_waiter).unwrap();
                guard
                    .block_until(Some(&proc_a_thread_handle), zx::MonotonicInstant::INFINITE)
                    .unwrap();
            });

            let mut fake_waiter = rx.recv().unwrap();
            {
                let mut thread_state = proc_b.thread.lock();
                thread_state.registration = RegistrationState::Main;
                thread_state.command_queue.waiters.wait_async_simple(&mut fake_waiter);
            }

            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    target: binder_transaction_data__bindgen_ty_1 { handle: 0 },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Set proc_b as context manager so we can send a transaction to it.
            let context_manager =
                BinderObject::new_context_manager_marker(&proc_b.proc, BinderObjectFlags::empty());
            *device.context_manager.lock() = Some(context_manager);

            device
                .handle_transaction(&proc_a.context(current_task), &mut Vec::new(), transaction)
                .expect("A sends to B");

            let read_buffer_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            device
                .handle_thread_read(
                    &proc_b.context(current_task),
                    &UserBuffer { address: read_buffer_addr, length: *PAGE_SIZE as usize },
                )
                .expect("B reads transaction");

            let transaction_b_to_a = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    target: binder_transaction_data__bindgen_ty_1 { handle: 0 },
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Set proc_a as context manager so we can send a transaction to it.
            let context_manager_a =
                BinderObject::new_context_manager_marker(&proc_a.proc, BinderObjectFlags::empty());
            *device.context_manager.lock() = Some(context_manager_a);

            device
                .handle_transaction(
                    &proc_b.context(current_task),
                    &mut Vec::new(),
                    transaction_b_to_a,
                )
                .expect("B sends to A");

            // proc_b should see that it's replying to proc_a and include this in the Sender
            // information.
            let thread_state = proc_b.thread.lock();
            let role = thread_state.transactions.last().unwrap();
            if let TransactionRole::Sender(sender_info) = role {
                assert_eq!(sender_info.target_thread, Some(proc_a.thread.tid));
                assert!(sender_info.target_thread_handle.is_some());

                let a_thread_handle = &proc_a.thread.thread;
                let target_handle = sender_info
                    .target_thread_handle
                    .as_ref()
                    .expect("We should have a target thread set");
                assert_eq!(a_thread_handle.koid().unwrap(), target_handle.koid().unwrap());
            } else {
                panic!("Expected Sender role");
            }

            // Clean up the blocked thread.
            event.notify();
            blocked_thread.join().unwrap();
        })
        .await;
    }

    #[fuchsia::test]
    async fn binder_object_multiple_registration() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let owner = BinderProcessFixture::new(current_task, &device);

            let local = LocalBinderObject {
                weak_ref_addr: UserAddress::from(0x1),
                strong_ref_addr: UserAddress::from(0x2),
            };

            // Register the object once.
            let _guard1 = owner.proc.lock().find_or_register_object(
                &owner.thread,
                local,
                BinderObjectFlags::empty(),
            );

            // Register the same object again. This will find the existing object
            // and call `inc_strong_unchecked`, which triggered the deadlock.
            let _guard2 = owner.proc.lock().find_or_register_object(
                &owner.thread,
                local,
                BinderObjectFlags::empty(),
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn unscheduled_sync_transaction_requeues_pi_to_worker_thread() {
        spawn_kernel_and_run(async |current_task| {
            let device = BinderDevice::default();
            let proc_a = BinderProcessFixture::new(current_task, &device);
            let proc_b = BinderProcessFixture::new_current(current_task, &device);

            let transaction = binder_transaction_data_sg {
                transaction_data: binder_transaction_data {
                    target: binder_transaction_data__bindgen_ty_1 { handle: 0 },
                    code: 42,
                    ..binder_transaction_data::default()
                },
                buffers_size: 0,
            };

            // Set proc_b as context manager so proc_a can send a transaction to it.
            let context_manager =
                BinderObject::new_context_manager_marker(&proc_b.proc, BinderObjectFlags::empty());
            *device.context_manager.lock() = Some(context_manager);

            // 1. proc_a sends synchronous transaction to proc_b (queued on process queue).
            device
                .handle_transaction(&proc_a.context(current_task), &mut Vec::new(), transaction)
                .expect("A sends transaction to B");

            // 2. proc_a.thread sets its requeue_event as it prepares to wait for the reply.
            let event = InterruptibleEvent::new();
            let _requeue_registration =
                RequeueEventRegistration::new(&proc_a.thread, event.clone());

            // Simulate proc_a starting to wait.
            let (tx, rx) = std::sync::mpsc::channel();
            let event_clone = event.clone();
            let blocked_thread = std::thread::spawn(move || {
                let (thread_handle, _) = fuchsia_runtime::with_thread_self(|thread| {
                    (
                        thread.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                        thread.koid().unwrap(),
                    )
                });
                tx.send(thread_handle).unwrap();
                let guard = event_clone.begin_wait();
                let _ = guard.block_until(None, zx::MonotonicInstant::INFINITE);
            });

            // Wait until blocked_thread is actively sleeping in the kernel on zx_futex_wait.
            let blocked_thread_handle = rx.recv().unwrap();
            while blocked_thread_handle.info().unwrap().state
                != zx::ThreadState::Blocked(zx::ThreadBlockType::Futex)
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }

            // Verify before requeue that the event has no owner.
            assert_eq!(event.get_owner(), None);

            // 3. proc_b.thread (the worker thread) calls handle_thread_read to dequeue the transaction from process queue.
            let read_buffer_addr = map_memory(current_task, UserAddress::default(), *PAGE_SIZE);
            device
                .handle_thread_read(
                    &proc_b.context(current_task),
                    &UserBuffer { address: read_buffer_addr, length: *PAGE_SIZE as usize },
                )
                .expect("B reads transaction");

            // 4. Verify that Zircon Futex Priority Inheritance ownership was transferred synchronously to proc_b.thread!
            let worker_koid = proc_b.thread.thread.koid().unwrap();
            assert_eq!(event.get_owner(), Some(worker_koid));

            // Clean up: notify event and join blocked thread.
            event.notify();
            blocked_thread.join().unwrap();
        })
        .await;
    }
}
