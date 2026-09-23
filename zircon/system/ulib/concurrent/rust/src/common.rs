// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// An enumeration of various synchronization options to use when performing
/// memory transfer operations with `well_defined_copy_to` / `well_defined_copy_from`.
///
/// # AcqRelOps
/// Use either `Ordering::Acquire` (`CopyFrom`) or `Ordering::Release` (`CopyTo`)
/// on every atomic load/store operation during the transfer to/from the shared
/// buffer.
///
/// # Fence
/// Use either an `Ordering::Acquire` thread fence (`CopyFrom`) after the transfer
/// operation, or an `Ordering::Release` (`CopyTo`) thread fence before the
/// operation, and `Ordering::Relaxed` for each of the atomic load/store
/// operations during the transfer.
///
/// # None
/// Simply use `Ordering::Relaxed` for each of the atomic load/store
/// operations during the transfer. Do not actually introduce any
/// explicit synchronization behavior.
///
/// WARNING: Use cases for this transfer mode tend to be unusual. Users will almost
/// always want some form of synchronization to take place during their
/// transfers. One example of where it may be appropriate to use `SyncOpt::None`
/// might be a situation where users are attempting to observe the state of more
/// than one object while inside of a sequence lock read transaction, and the
/// user has decided that it is better to use a thread fence than to use acquire
/// semantics on each element transferred. Such a sequence might look something
/// like this:
///
/// ```
/// # use concurrent::{WellDefinedCopyable, SYNC_OPT_FENCE, SYNC_OPT_NONE};
/// # use zerocopy::{FromBytes, Immutable, IntoBytes};
/// # #[derive(Copy, Clone, Default, FromBytes, IntoBytes, Immutable)]
/// # #[repr(C, align(8))]
/// # struct Foo(u64);
/// # #[derive(Copy, Clone, Default, FromBytes, IntoBytes, Immutable)]
/// # #[repr(C, align(8))]
/// # struct Bar(u64);
/// # let src_foo1 = WellDefinedCopyable::new(Foo::default());
/// # let src_foo2 = WellDefinedCopyable::new(Foo::default());
/// # let src_bar1 = WellDefinedCopyable::new(Bar::default());
/// # let src_bar2 = WellDefinedCopyable::new(Bar::default());
/// let mut foo1 = Foo::default();
/// let mut foo2 = Foo::default();
/// let mut bar1 = Bar::default();
/// let mut bar2 = Bar::default();
/// // ...
/// src_foo1.read::<SYNC_OPT_NONE>(&mut foo1);
/// src_foo2.read::<SYNC_OPT_NONE>(&mut foo2);
/// src_bar1.read::<SYNC_OPT_NONE>(&mut bar1);
/// src_bar2.read::<SYNC_OPT_FENCE>(&mut bar2);
/// ```
///
/// Note that it is the _last_ transfer operation which includes the fence. In
/// the case of a `CopyTo` operation (when publishing data) it would be the _first_
/// operation which included the fence, not the last.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncOpt {
    AcqRelOps,
    Fence,
    None,
}

impl SyncOpt {
    pub const fn from_u8(val: u8) -> Self {
        match val {
            SYNC_OPT_ACQ_REL_OPS => SyncOpt::AcqRelOps,
            SYNC_OPT_FENCE => SyncOpt::Fence,
            SYNC_OPT_NONE => SyncOpt::None,
            _ => panic!("invalid SyncOpt discriminant"),
        }
    }
}

pub const SYNC_OPT_ACQ_REL_OPS: u8 = SyncOpt::AcqRelOps as u8;
pub const SYNC_OPT_FENCE: u8 = SyncOpt::Fence as u8;
pub const SYNC_OPT_NONE: u8 = SyncOpt::None as u8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_u8() {
        assert_eq!(SyncOpt::from_u8(SYNC_OPT_ACQ_REL_OPS), SyncOpt::AcqRelOps);
        assert_eq!(SyncOpt::from_u8(SYNC_OPT_FENCE), SyncOpt::Fence);
        assert_eq!(SyncOpt::from_u8(SYNC_OPT_NONE), SyncOpt::None);
    }

    #[test]
    #[should_panic(expected = "invalid SyncOpt discriminant")]
    fn test_from_u8_invalid() {
        SyncOpt::from_u8(42);
    }
}
