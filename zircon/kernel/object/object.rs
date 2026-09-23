// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

mod bti;
pub mod buffer_chain;
mod bus_transaction_initiator_dispatcher;
mod bus_transaction_initiator_dispatcher_ffi;
mod counter_dispatcher;
mod counter_dispatcher_ffi;
mod dispatcher;
mod dispatcher_ffi;
mod event_dispatcher;
mod event_dispatcher_ffi;
mod event_pair_dispatcher;
mod event_pair_dispatcher_ffi;
mod fifo_dispatcher;
mod fifo_dispatcher_ffi;
mod handle;
mod io_buffer_dispatcher;
mod io_buffer_dispatcher_ffi;
mod io_buffer_shared_region_dispatcher;
mod iommu;
mod iommu_dispatcher;
mod iommu_dispatcher_ffi;
mod job_dispatcher;
mod job_dispatcher_ffi;
mod job_policy;
mod job_policy_ffi;
mod log_dispatcher;
mod log_dispatcher_ffi;
mod mbuf;
pub mod message_packet;
mod msi_allocation;
mod msi_dispatcher;
mod msi_dispatcher_ffi;
mod msi_interrupt_dispatcher;
mod msi_interrupt_dispatcher_ffi;
mod pinned_memory_token_dispatcher;
mod pinned_memory_token_dispatcher_ffi;
mod pmt;
mod process_dispatcher;
pub(crate) mod process_dispatcher_ffi;
mod profile_dispatcher;
mod profile_dispatcher_ffi;
mod resource;
mod resource_dispatcher;
mod resource_dispatcher_ffi;
mod resource_ffi;
mod sampler_dispatcher;
mod sampler_dispatcher_ffi;
mod socket_dispatcher;
mod socket_dispatcher_ffi;
mod stream_dispatcher;
mod stream_dispatcher_ffi;
mod suspend_token_dispatcher;
mod suspend_token_dispatcher_ffi;
mod thread_dispatcher;
mod thread_dispatcher_ffi;
mod timer_dispatcher;
mod timer_dispatcher_ffi;
mod vm_address_region_dispatcher;
mod vm_address_region_dispatcher_ffi;
mod vm_object_dispatcher;
mod vm_object_dispatcher_ffi;
mod wait_signal_observer;

pub use bti::{IOMMU_FLAG_PERM_EXECUTE, IOMMU_FLAG_PERM_READ, IOMMU_FLAG_PERM_WRITE};
pub use buffer_chain::BufferChain;
pub use bus_transaction_initiator_dispatcher::BusTransactionInitiatorDispatcher;
pub use counter_dispatcher::CounterDispatcher;
pub use dispatcher::{Dispatcher, DispatcherOps};
pub use event_dispatcher::EventDispatcher;
pub use event_pair_dispatcher::EventPairDispatcher;
pub use fifo_dispatcher::FifoDispatcher;
pub use handle::{HandleRef, HandleValue, KernelHandle};
pub use io_buffer_dispatcher::IoBufferDispatcher;
pub use io_buffer_shared_region_dispatcher::IoBufferSharedRegionDispatcher;
pub use iommu_dispatcher::IommuDispatcher;
pub use job_dispatcher::*;
pub use job_policy::{JobPolicy, JobPolicyCollection, Policy};
pub use log_dispatcher::*;
pub use message_packet::{MessagePacket, MessagePacketPtr};
pub use msi_allocation::MsiAllocation;
pub use msi_dispatcher::MsiDispatcher;
pub use msi_interrupt_dispatcher::MsiInterruptDispatcher;
pub use pinned_memory_token_dispatcher::{PinnedMemoryTokenDispatcher, dev_vaddr_t};
pub use process_dispatcher::{HandleTableReadGuard, ProcessDispatcher};
pub use profile_dispatcher::ProfileDispatcher;
pub use resource::{
    StrictValidation, validate_ranged_resource, validate_ranged_resource_dispatcher,
    validate_resource_kind_base, validate_system_resource,
};
pub use resource_dispatcher::{ResourceDispatcher, ZX_RSRC_FLAGS_MASK, is_valid_kind};
pub use sampler_dispatcher::SamplerDispatcher;
pub use socket_dispatcher::{Disposition, ReadType, SocketDispatcher};
pub use stream_dispatcher::*;
pub use suspend_token_dispatcher::SuspendTokenDispatcher;
pub use thread_dispatcher::{AutoBlocked, Blocked, ThreadDispatcher};
pub use timer_dispatcher::TimerDispatcher;
pub use vm_address_region_dispatcher::VmAddressRegionDispatcher;
pub use vm_object_dispatcher::{InitialMutability, VmObjectDispatcher};
pub use wait_signal_observer::WaitSignalObserver;
