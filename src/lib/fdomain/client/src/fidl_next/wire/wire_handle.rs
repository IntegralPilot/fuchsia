// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_next::ValidationError;
use fuchsia_sync::RwLock;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicPtr, Ordering};

use fidl_next_codec::{
    Constrained, Decode, DecodeError, Encode, EncodeError, EncodeOption, FromWire, FromWireOption,
    Slot, Wire, munge, wire,
};

use crate::Client;
use crate::fidl_next::{HandleDecoder, HandleEncoder};

struct HandleAssoc {
    hid: UnsafeCell<u32>,
    client: AtomicPtr<Client>,
}

// SAFETY: We use the atomic pointer to synchronize access to the hid field.
unsafe impl Send for HandleAssoc {}
unsafe impl Sync for HandleAssoc {}

const HANDLE_CLIENT_ASSOC_START_SIZE: usize = 32;
static HANDLE_CLIENT_ASSOC: RwLock<&'static [HandleAssoc]> = RwLock::new(&[]);

/// An FDomain handle.
#[repr(C, align(4))]
pub union Handle {
    encoded: wire::Uint32,
    decoded: u32,
}

impl From<crate::Handle> for Handle {
    fn from(mut handle: crate::Handle) -> Handle {
        let id = handle.id;
        let client = std::mem::replace(&mut handle.client, std::sync::Weak::new());
        let ptr = client.into_raw() as *mut Client;

        loop {
            let table = HANDLE_CLIENT_ASSOC.read();

            for (got_id, entry) in table.iter().enumerate() {
                let got_id: u32 = got_id.try_into().expect("Handle table overflowed u32");
                if entry
                    .client
                    .compare_exchange(
                        std::ptr::null_mut(),
                        ptr,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    // SAFETY: If we were able to populate the client field then
                    // we own this slot and it is ours to write.
                    unsafe {
                        *entry.hid.get() = id;
                        return Handle { decoded: got_id + 1 };
                    }
                }
            }

            std::mem::drop(table);
            let mut table = HANDLE_CLIENT_ASSOC.write();
            let new_len = std::cmp::max(table.len() * 2, HANDLE_CLIENT_ASSOC_START_SIZE);

            let mut new_vec = Vec::with_capacity(new_len);
            for entry in table.iter() {
                // SAFETY: We are holding a write lock on `HANDLE_CLIENT_ASSOC`,
                // ensuring exclusive access to the table. The `hid` field is
                // written once when the client pointer is successfully
                // aclaimed, and by the time we are resizing, any `hid` values
                // associated with a non-null client pointer are stable.
                let hid = unsafe { *entry.hid.get() };
                let client_ptr = entry.client.load(Ordering::Relaxed);
                new_vec.push(HandleAssoc {
                    hid: UnsafeCell::new(hid),
                    client: AtomicPtr::new(client_ptr),
                });
            }
            new_vec.resize_with(new_len, || HandleAssoc {
                hid: UnsafeCell::new(0),
                client: AtomicPtr::new(std::ptr::null_mut()),
            });

            let new = new_vec.into_boxed_slice();
            let new = Box::leak(new);
            let old = std::mem::replace(&mut *table, new);

            if old.len() > 0 {
                // SAFETY: If this isn't the zero-length starting slice then it was
                // leaked just above in a previous call/iteration.
                unsafe { drop(Box::from_raw(old as *const [HandleAssoc] as *mut [HandleAssoc])) }
            }
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        drop(self.take_handle());
    }
}

impl Constrained for Handle {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

unsafe impl Wire for Handle {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(_: &mut MaybeUninit<Self>) {
        // Wire handles have no padding
    }
}

impl Handle {
    /// Encodes a handle as present in an output.
    pub fn set_encoded_present(out: &mut MaybeUninit<Self>) {
        munge!(let Self { encoded } = out);
        encoded.write(wire::Uint32(u32::MAX));
    }

    /// Returns whether the underlying u32 is invalid.
    pub fn is_invalid(&self) -> bool {
        self.as_raw_handle() == 0
    }

    pub fn invalidate(&mut self) {
        self.decoded = 0;
    }

    /// Returns the underlying `u1`.
    #[inline]
    pub fn as_raw_handle(&self) -> u32 {
        unsafe { self.decoded }
    }

    /// Takes the raw handle out of the handle table.
    pub(crate) fn take_handle(&mut self) -> crate::Handle {
        // SAFETY: `WireHandle` is always a valid index into the association table,
        // and the handle value is always in the association table.
        unsafe {
            let pos = self.decoded as usize;
            self.decoded = 0;
            let Some(pos) = pos.checked_sub(1) else {
                return crate::Handle::invalid();
            };
            let (id, ptr) = {
                let table = HANDLE_CLIENT_ASSOC.read();
                let entry = &table[pos];
                // We have to read the hid first as when we swap out the client
                // that is when we mark the slot free.
                let hid = *entry.hid.get();
                let ptr = entry.client.swap(std::ptr::null_mut(), Ordering::Release);
                (hid, ptr)
            };

            // The pointer should never be null here, as a non-null client was
            // stored when the Handle was created.
            assert!(!ptr.is_null(), "Attempted to take an invalid or already taken handle slot");
            let client = std::sync::Weak::from_raw(ptr);

            crate::Handle { id, client }
        }
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_raw_handle().fmt(f)
    }
}

unsafe impl<D: HandleDecoder + ?Sized> Decode<D> for Handle {
    fn decode(
        mut slot: Slot<'_, Self>,
        decoder: &mut D,
        _: <Self as Constrained>::Constraint,
    ) -> Result<(), DecodeError> {
        munge!(let Self { encoded } = slot.as_mut());

        match **encoded {
            0 => (),
            u32::MAX => {
                let handle = decoder.take_raw_handle()?;
                munge!(let Self { mut decoded } = slot);
                decoded.write(handle);
            }
            e => return Err(DecodeError::InvalidHandlePresence(e)),
        }
        Ok(())
    }
}

/// An optional Zircon handle.
#[derive(Debug)]
#[repr(transparent)]
pub struct OptionalHandle {
    pub(crate) handle: Handle,
}

impl Constrained for OptionalHandle {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

unsafe impl Wire for OptionalHandle {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(out: &mut MaybeUninit<Self>) {
        munge!(let Self { handle } = out);
        Handle::zero_padding(handle);
    }
}

impl OptionalHandle {
    /// Encodes a handle as present in a slot.
    pub fn set_encoded_present(out: &mut MaybeUninit<Self>) {
        munge!(let Self { handle } = out);
        Handle::set_encoded_present(handle);
    }

    /// Encodes a handle as absent in an output.
    pub fn set_encoded_absent(out: &mut MaybeUninit<Self>) {
        munge!(let Self { handle: Handle { encoded } } = out);
        encoded.write(wire::Uint32(0));
    }

    /// Returns whether a handle is present.
    pub fn is_some(&self) -> bool {
        !self.handle.is_invalid()
    }

    /// Returns whether a handle is absent.
    pub fn is_none(&self) -> bool {
        self.handle.is_invalid()
    }

    /// Returns the underlying [`zx_handle_t`], if any.
    #[inline]
    pub fn as_raw_handle(&self) -> Option<u32> {
        self.is_some().then(|| self.handle.as_raw_handle())
    }
}

unsafe impl<D: HandleDecoder + ?Sized> Decode<D> for OptionalHandle {
    fn decode(
        mut slot: Slot<'_, Self>,
        decoder: &mut D,
        constraint: <Self as Constrained>::Constraint,
    ) -> Result<(), DecodeError> {
        munge!(let Self { handle } = slot.as_mut());
        Handle::decode(handle, decoder, constraint)
    }
}

unsafe impl<E: HandleEncoder + ?Sized> Encode<Handle, E> for crate::Handle {
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Handle>,
        _: (),
    ) -> Result<(), EncodeError> {
        if self.client.upgrade().is_none() {
            Err(EncodeError::InvalidRequiredHandle)
        } else {
            encoder.push_handle(self)?;
            Handle::set_encoded_present(out);
            Ok(())
        }
    }
}

impl FromWire<Handle> for crate::Handle {
    fn from_wire(mut wire: Handle) -> Self {
        wire.take_handle()
    }
}

unsafe impl<E: HandleEncoder + ?Sized> EncodeOption<OptionalHandle, E> for crate::Handle {
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<OptionalHandle>,
        _: (),
    ) -> Result<(), EncodeError> {
        if let Some(handle) = this {
            encoder.push_handle(handle)?;
            OptionalHandle::set_encoded_present(out);
        } else {
            OptionalHandle::set_encoded_absent(out);
        }
        Ok(())
    }
}

impl FromWireOption<OptionalHandle> for crate::Handle {
    fn from_wire_option(mut wire: OptionalHandle) -> Option<Self> {
        if wire.handle.is_invalid() { None } else { Some(wire.handle.take_handle()) }
    }
}
