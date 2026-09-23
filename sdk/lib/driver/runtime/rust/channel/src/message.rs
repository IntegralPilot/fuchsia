// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A helper for managing a self-contained arena-allocated buffer along with its arena handle.

use crate::arena::{Arena, ArenaBox};
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use fdf_core::handle::MixedHandle;

use fdf_sys::*;

/// A struct that holds both an arena along with a data buffer that is allocated within that arena.
#[derive(Debug)]
pub struct Message<T: ?Sized + 'static> {
    data: Option<ArenaBox<'static, T>>,
    // note: `[Option<MixedHandle>]` is byte-equivalent to a C array of `fdf_handle_t`.
    handles: Option<ArenaBox<'static, [Option<MixedHandle>]>>,
    // note: this must maintain its position as the last item of the struct
    // to ensure that it is freed after the data and handle pointers.
    arena: Arena,
    _p: PhantomData<T>,
}

impl<T: ?Sized> Message<T> {
    /// Consumes the given arena, data buffer, and handles buffers and returns a message that holds
    /// all three.
    ///
    /// # Panics
    ///
    /// This function panics if either of the [`ArenaBox`]s are not allocated by [`Arena`].
    pub fn new<'a>(
        arena: &'a Arena,
        data: Option<ArenaBox<'a, T>>,
        handles: Option<ArenaBox<'a, [Option<MixedHandle>]>>,
    ) -> Self {
        let data = data.map(|data| {
            assert!(
                arena.contains(&data),
                "Data buffer pointer is not in the arena being included in the message"
            );
            // SAFETY: we will store this ArenaBox with a clone of the Arena that
            // owns it.
            unsafe { ArenaBox::erase_lifetime(data) }
        });
        let handles = handles.map(|handles| {
            assert!(
                arena.contains(&handles),
                "Handle buffer pointer is not in the arena being included in the message"
            );
            // SAFETY: we will store this ArenaBox with a clone of the Arena that
            // owns it.
            unsafe { ArenaBox::erase_lifetime(handles) }
        });
        // SAFETY: We just checked that both boxes were allocated from the arena.
        unsafe { Self::new_unchecked(arena.clone(), data, handles) }
    }

    /// Given the [`Arena`], allocates a new [`Message`] and runs the given `f` to allow the caller
    /// to allocate data and handles into the [`Message`] without requiring a check that the
    /// correct [`Arena`] was used to allocate the data.
    ///
    /// Note that it may be possible to sneak an `ArenaBox<'static, T>` into this [`Message`]
    /// object with this function by using an [`Arena`] with static lifetime to allocate it. This
    /// will not cause any unsoundness, but if you try to send that message through a [`Channel`]
    /// it will cause a runtime error when the arenas don't match.
    pub fn new_with<F>(arena: Arena, f: F) -> Self
    where
        F: for<'a> FnOnce(
            &'a Arena,
        )
            -> (Option<ArenaBox<'a, T>>, Option<ArenaBox<'a, [Option<MixedHandle>]>>),
    {
        let (data, handles) = f(&arena);
        // SAFETY: The `for<'a>` in the callback definition makes it so that the caller must
        // (without resorting to unsafe themselves) allocate the [`ArenaBox`] from the given
        // [`Arena`].
        Self {
            data: data.map(|data| unsafe { ArenaBox::erase_lifetime(data) }),
            handles: handles.map(|handles| unsafe { ArenaBox::erase_lifetime(handles) }),
            arena,
            _p: PhantomData,
        }
    }

    /// A shorthand for [`Self::new_with`] when there's definitely a data body and nothing else.
    pub fn new_with_data<F>(arena: Arena, f: F) -> Self
    where
        F: for<'a> FnOnce(&'a Arena) -> ArenaBox<'a, T>,
    {
        // SAFETY: The `for<'a>` in the callback definition makes it so that the caller must
        // (without resorting to unsafe themselves) allocate the [`ArenaBox`] from the given
        // [`Arena`].
        let data = Some(unsafe { ArenaBox::erase_lifetime(f(&arena)) });
        Self { data, handles: None, arena, _p: PhantomData }
    }

    /// As with [`Self::new`], this consumes the arguments to produce a message object that holds
    /// all of them together to be extracted again later, but does not validate that the pointers
    /// came from the same arena.
    ///
    /// # Safety
    ///
    /// The caller is responsible for:
    /// - ensuring that the [`ArenaBox`]es came from the same arena as is being passed in to this
    ///   function, or the erased lifetime of the arena boxes might cause use-after-free.
    /// - the bytes in `data` are actually of type `T`, and are properly aligned for type `T`.
    pub(crate) unsafe fn new_unchecked(
        arena: Arena,
        data_ptr: Option<ArenaBox<'static, T>>,
        handles_ptr: Option<ArenaBox<'static, [Option<MixedHandle>]>>,
    ) -> Self {
        Self { arena, data: data_ptr, handles: handles_ptr, _p: PhantomData }
    }

    /// Gets a reference to the arena this message was allocated with.
    pub fn arena(&self) -> &Arena {
        &self.arena
    }

    /// Takes the arena and drops any data or handle bodies held in this message
    pub fn take_arena(self) -> Arena {
        self.arena
    }

    /// Gets a reference to the data in this message, if there is any
    pub fn data(&self) -> Option<&T> {
        self.data.as_deref()
    }

    /// Gets a mutable reference to the data in this message, if there is any
    pub fn data_mut(&mut self) -> Option<&mut T> {
        self.data.as_deref_mut()
    }

    /// Maps the message data to a new [`ArenaBox`] based on the arena and the old data.
    pub fn map_data<F, R: ?Sized>(self, f: F) -> Message<R>
    where
        F: for<'a> FnOnce(&'a Arena, ArenaBox<'a, T>) -> ArenaBox<'a, R>,
    {
        let Self { arena, data: data_ptr, handles: handles_ptr, .. } = self;
        let data_ptr = data_ptr.map(|data_ptr| {
            // SAFETY: The `ArenaBox` being returned is tied to the lifetime
            // of the arena we gave the closure, and we will now be moving
            // into the new `Message`. So just like the old one,
            // the new box is tied to the life of the message and the arena
            // within it.
            unsafe { ArenaBox::erase_lifetime(f(&arena, data_ptr)) }
        });
        Message { arena, data: data_ptr, handles: handles_ptr, _p: PhantomData }
    }

    /// Gets a reference to the handles array in this message, if there is one.
    pub fn handles(&self) -> Option<&[Option<MixedHandle>]> {
        self.handles.as_deref()
    }

    /// Gets a mutable reference to the handles array in this message, if there is one.
    pub fn handles_mut(&mut self) -> Option<&mut [Option<MixedHandle>]> {
        self.handles.as_deref_mut()
    }

    /// Gets a reference to all three of the arena, data, and handles of the message
    pub fn as_refs(&self) -> (&Arena, Option<&T>, Option<&[Option<MixedHandle>]>) {
        (&self.arena, self.data.as_deref(), self.handles.as_deref())
    }

    /// Gets a reference to the arena and mutable references to the data handles of the message
    pub fn as_mut_refs(&mut self) -> (&Arena, Option<&mut T>, Option<&mut [Option<MixedHandle>]>) {
        (&self.arena, self.data.as_deref_mut(), self.handles.as_deref_mut())
    }

    /// Unpacks the arena and buffers in this message to the caller.
    ///
    /// The `arena` argument provides a place to put the [`Arena`] from this message
    /// in the local lifetime of the caller so that the [`ArenaBox`]es can be tied to
    /// its lifetime.
    #[expect(clippy::type_complexity)]
    pub fn into_arena_boxes<'a>(
        self,
        arena: &'a mut Option<Arena>,
    ) -> (Option<ArenaBox<'a, T>>, Option<ArenaBox<'a, [Option<MixedHandle>]>>) {
        arena.replace(self.arena);
        // SAFETY: the lifetime we're giving these [`ArenaBox`]es is the same one
        // as the lifetime of the place we're putting the [`Arena`] they belong to.
        let data = self.data.map(|ptr| unsafe { ArenaBox::erase_lifetime(ptr) });
        let handles = self.handles.map(|ptr| unsafe { ArenaBox::erase_lifetime(ptr) });
        (data, handles)
    }

    /// Takes the `ArenaBox`es for the data and handles from this [`Message`], but leaves
    /// the [`Arena`] in the [`Message`] to act as a holder of the arena lifetime.
    #[expect(clippy::type_complexity)]
    pub fn take_arena_boxes(
        &mut self,
    ) -> (&'_ Arena, Option<ArenaBox<'_, T>>, Option<ArenaBox<'_, [Option<MixedHandle>]>>) {
        (&self.arena, self.data.take(), self.handles.take())
    }

    /// Unpacks the arena and buffers into raw pointers
    ///
    /// Care must be taken to ensure that the data and handle pointers are not used
    /// if the arena is freed. If they are never reconstituted into a [`Message`]
    /// or an [`Arena`] and [`ArenaBox`]es, they will be leaked.
    #[expect(clippy::type_complexity)]
    pub fn into_raw(
        self,
    ) -> (NonNull<fdf_arena_t>, Option<NonNull<T>>, Option<NonNull<[Option<MixedHandle>]>>) {
        let arena = self.arena.into_raw();
        // SAFETY: the arena and the pointers we're returning will all have the same
        // effectively 'static lifetime, and it is up to the caller to make sure that
        // they free them in the correct order.
        let data = self.data.map(|data| unsafe { ArenaBox::into_ptr(data) });
        let handles = self.handles.map(|handles| unsafe { ArenaBox::into_ptr(handles) });
        (arena, data, handles)
    }
}

impl<T> Message<T> {
    /// Takes the data from the message, dropping the [`Arena`] and handles
    /// array in the process.
    pub fn take_data(self) -> Option<T> {
        self.data.map(ArenaBox::take)
    }

    /// Takes the data from the message, dropping the [`Arena`] and handles
    /// array in the process.
    pub fn take_data_boxed(self) -> Option<Box<T>> {
        self.data.map(ArenaBox::take_boxed)
    }
}

impl<T> Message<[T]> {
    /// Takes the data from the message, dropping the [`Arena`] and handles
    /// array in the process.
    pub fn take_data_boxed_slice(self) -> Option<Box<[T]>> {
        self.data.map(ArenaBox::take_boxed_slice)
    }
}

impl<T> Message<MaybeUninit<T>> {
    /// Assumes the contents of the data payload of this message are initialized.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the value is initialized
    /// properly. See [`MaybeUninit::assume_init`] for more details on the
    /// safety requirements of this.
    pub unsafe fn assume_init(self) -> Message<T> {
        // SAFETY: the caller is responsible for ensuring the contents
        // of the data pointer are initialized.
        self.map_data(|_, data_ptr| unsafe { ArenaBox::assume_init(data_ptr) })
    }
}

impl<T> Message<[MaybeUninit<T>]> {
    /// Assumes the contents of the data payload of this message are initialized.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the value is initialized
    /// properly. See [`MaybeUninit::assume_init`] for more details on the
    /// safety requirements of this.
    pub unsafe fn assume_init(self) -> Message<[T]> {
        // SAFETY: the caller is responsible for ensuring the contents
        // of the data pointer are initialized.
        self.map_data(|_, data_ptr| unsafe { ArenaBox::assume_init_slice(data_ptr) })
    }
}

impl Message<[MaybeUninit<u8>]> {
    /// Transforms the message body into a message of type `T`
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the data portion of this
    /// message originated from a source with a properly allocated `T` with correct
    /// alignment
    pub unsafe fn cast_unchecked<T>(self) -> Message<T> {
        // SAFETY: the caller is responsible for ensuring the contents
        // of the data pointer are an initialized value of `T` and are
        // correctly aligned.
        self.map_data(|_, data_ptr| unsafe { ArenaBox::cast_unchecked(data_ptr) })
    }
}

#[cfg(test)]
mod test {
    use crate::channel::Channel;
    use fdf_core::handle::MixedHandleType;

    use super::*;

    #[test]
    #[should_panic]
    fn bad_data_pointer() {
        let arena = Arena::new();
        let other_arena = Arena::new();
        Message::new(&arena, Some(other_arena.insert(1)), None);
    }

    #[test]
    #[should_panic]
    fn bad_handle_pointer() {
        let arena = Arena::new();
        let other_arena = Arena::new();
        Message::<()>::new(&arena, None, Some(other_arena.insert_boxed_slice(Box::new([]))));
    }

    #[test]
    fn round_trip_data() {
        let arena = Arena::new();
        let data = arena.insert(1);
        let message = Message::new(&arena, Some(data), None);
        let mut arena = None;
        let (data, _) = message.into_arena_boxes(&mut arena);
        assert_eq!(*data.unwrap(), 1);
    }

    #[test]
    fn round_trip_handles() {
        let arena = Arena::new();
        let zircon_handle = MixedHandle::from_zircon_handle(zx::Port::create().into_handle());
        let (driver_handle1, driver_handle2) = Channel::create();
        driver_handle2
            .write(Message::new_with_data(arena.clone(), |arena| arena.insert(1)))
            .unwrap();

        let handles = arena
            .insert_boxed_slice(Box::new([zircon_handle, Some(MixedHandle::from(driver_handle1))]));
        let message = Message::<()>::new(&arena, None, Some(handles));

        let mut arena = None;
        let (_, Some(mut handles)) = message.into_arena_boxes(&mut arena) else {
            panic!("didn't get handles back");
        };
        assert_eq!(handles.len(), 2);
        let MixedHandleType::Zircon(_zircon_handle) = handles[0].take().unwrap().resolve() else {
            panic!("first handle in the handle set wasn't a zircon handle");
        };
        let MixedHandleType::Driver(driver_handle1) = handles[1].take().unwrap().resolve() else {
            panic!("second handle in the handle set wasn't a driver handle");
        };
        let driver_handle1 = unsafe { Channel::<i32>::from_driver_handle(driver_handle1) };
        assert_eq!(driver_handle1.try_read().unwrap().unwrap().data().unwrap(), &1);
    }

    #[test]
    fn map_data() {
        let arena = Arena::new();
        let data = arena.insert(1);
        let message = Message::new(&arena, Some(data), None);
        let message = message.map_data(|arena, i| arena.insert(*i + 1));
        assert_eq!(message.data().unwrap(), &2);
    }
}
