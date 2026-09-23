// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Networking types and operations.
//!
//! This crate defines types and operations useful for operating with various
//! network protocols. Some general utilities are defined in the crate root,
//! while protocol-specific operations are defined in their own modules.
//!
//! # Witness types
//!
//! This crate makes heavy use of the "witness type" pattern. A witness type is
//! one whose existence "bears witness" to a particular property. For example,
//! the [`UnicastAddr`] type wraps an existing address and guarantees that it
//! is unicast.
//!
//! There are a few components to a witness type.
//!
//! First, each property is encoded in a trait. For example, the
//! [`UnicastAddress`] trait is implemented by any address type which can be
//! unicast. The [`is_unicast`] method is used to determine whether a given
//! instance is unicast.
//!
//! Second, a witness type wraps an address. For example, `UnicastAddr<A>` can
//! be used with any `A: UnicastAddress`. There are two ways to obtain an
//! instance of a witness type. Some constants are constructed as witness types
//! at compile time, and so provide a static guarantee of the witnessed property
//! (e.g., [`Ipv6::LOOPBACK_IPV6_ADDRESS`] is a `UnicastAddr`). Otherwise, an
//! instance can be constructed fallibly at runtime. For example,
//! [`UnicastAddr::new`] accepts an `A` and returns an `Option<UnicastAddr<A>>`,
//! returning `Some` if the address passes the `is_unicast` check, and `None`
//! otherwise.
//!
//! Finally, each witness type implements the [`Witness`] trait, which allows
//! code to be written which is generic over which witness type is used.
//!
//! Witness types enable a variety of operations which are only valid on certain
//! types of addresses. For example, a multicast MAC address can be derived from
//! a multicast IPv6 address, so the `MulticastAddr<Mac>` type implements
//! `From<MulticastAddr<Ipv6Addr>>`. Similarly, given an [`Ipv6Addr`], the
//! [`to_solicited_node_address`] method can be used to construct the address's
//! solicited-node address, which is a `MulticastAddr<Ipv6Addr>`. Combining
//! these, it's possible to take an `Ipv6Addr` and compute the solicited node
//! address's multicast MAC address without performing any runtime validation:
//!
//! ```rust
//! # use net_types::ethernet::Mac;
//! # use net_types::ip::Ipv6Addr;
//! # use net_types::MulticastAddr;
//! fn to_solicited_node_multicast_mac(addr: &Ipv6Addr) -> MulticastAddr<Mac> {
//!     addr.to_solicited_node_address().into()
//! }
//! ```
//!
//! # Naming Conventions
//!
//! When both types and traits exist which represent the same concept, the
//! traits will be given a full name - such as [`IpAddress`] or
//! [`UnicastAddress`] - while the types will be given an abbreviated name -
//! such as [`IpAddr`], [`Ipv4Addr`], [`Ipv6Addr`], or [`UnicastAddr`].
//!
//! [`is_unicast`]: crate::UnicastAddress::is_unicast
//! [`Ipv6::LOOPBACK_IPV6_ADDRESS`]: crate::ip::Ipv6::LOOPBACK_IPV6_ADDRESS
//! [`to_solicited_node_address`]: crate::ip::Ipv6Addr::to_solicited_node_address
//! [`IpAddress`]: crate::ip::IpAddress
//! [`IpAddr`]: crate::ip::IpAddr
//! [`Ipv4Addr`]: crate::ip::Ipv4Addr
//! [`Ipv6Addr`]: crate::ip::Ipv6Addr

#![deny(missing_docs)]
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]

pub mod ethernet;
pub mod ip;

use core::fmt::{self, Debug, Display, Formatter};
use core::ops::Deref;

use crate::ip::{GenericOverIp, Ip, IpAddress, IpInvariant, IpVersionMarker};

mod sealed {
    // Used to ensure that certain traits cannot be implemented by anyone
    // outside this crate, such as the Ip and IpAddress traits.
    pub trait Sealed {}
}

/// A type which is a witness to some property about an address.
///
/// A type which implements `Witness<A>` wraps an address of type `A` and
/// guarantees some property about the wrapped address. It is implemented by
/// [`SpecifiedAddr`], [`UnicastAddr`], [`MulticastAddr`], [`LinkLocalAddr`],
/// and [`NonMappedAddr`].
pub trait Witness<A>: AsRef<A> + Sized + sealed::Sealed {
    /// Constructs a new witness type.
    ///
    /// `new` returns `None` if `addr` does not satisfy the property guaranteed
    /// by `Self`.
    fn new(addr: A) -> Option<Self>;

    /// Constructs a new witness type without checking to see if `addr` actually
    /// satisfies the required property.
    ///
    /// # Safety
    ///
    /// It is up to the caller to make sure that `addr` satisfies the required
    /// property in order to avoid breaking the guarantees of this trait.
    unsafe fn new_unchecked(addr: A) -> Self;

    /// Constructs a new witness type from an existing witness type.
    ///
    /// `from_witness(witness)` is equivalent to `new(witness.into_addr())`.
    fn from_witness<W: Witness<A>>(addr: W) -> Option<Self> {
        Self::new(addr.into_addr())
    }

    // In a previous version of this code, we did `fn get(self) -> A where Self:
    // Copy` (taking `self` by value and using `where Self: Copy`). That felt
    // marginally cleaner, but it turns out that there are cases in which the
    // user only has access to a reference and still wants to be able to call
    // `get` without having to do the ugly `(*addr).get()`.

    /// Gets a copy of the address.
    #[inline]
    fn get(&self) -> A
    where
        A: Copy,
    {
        *self.as_ref()
    }

    /// Consumes this witness and returns the contained `A`.
    ///
    /// If `A: Copy`, prefer [`get`] instead of `into_addr`. `get` is idiomatic
    /// for wrapper types which which wrap `Copy` types (e.g., see
    /// [`NonZeroUsize::get`] or [`Cell::get`]). `into_xxx` methods are
    /// idiomatic only when `self` must be consumed by value because the wrapped
    /// value is not `Copy` (e.g., see [`Cell::into_inner`]).
    ///
    /// [`get`]: Witness::get
    /// [`NonZeroUsize::get`]: core::num::NonZeroUsize::get
    /// [`Cell::get`]: core::cell::Cell::get
    /// [`Cell::into_inner`]: core::cell::Cell::into_inner
    fn into_addr(self) -> A;

    /// Transposes this witness type with another witness type layered inside of
    /// it.
    /// (e.g. UnicastAddr<SpecifiedAddr<T>> -> SpecifiedAddr<UnicastAddr<T>>)
    fn transpose<T>(self) -> A::Map<Self::Map<T>>
    where
        Self: TransposableWitness<A>,
        A: TransposableWitness<T>,
        Self::Map<T>: Witness<T>,
        A::Map<Self::Map<T>>: Witness<Self::Map<T>>,
    {
        let middle = self.into_addr();
        let innermost = middle.into_addr();
        unsafe {
            // SAFETY: We're transposing two witness layers, so we know that the
            // inner address upheld both invariants that are witnessed.
            let new_middle = Self::Map::<T>::new_unchecked(innermost);
            A::Map::<Self::Map<T>>::new_unchecked(new_middle)
        }
    }
}

/// Witness types that can be transposed with other witness wrapper types.
// Technically, this could be merged directly into the `Witness` trait rather
// than exist as a separate trait. However, this ends up impeding type
// inference, as the trait solver gets confused by the existence of `Witness`
// impls both for single wrapper layers (`SpecifiedAddr<T>` impls `Witness<T>`)
// and for nested wrappers (`UnicastAddr<SpecifiedAddr<T>>` also impls
// `Witness<T>`). Since `transpose` is most useful for swapping single-layer
// `Witness` impls nested within each other, we only want to impl
// `TransposableWitness` for one wrapper layer at a time, which allows type
// inference to work properly.
pub trait TransposableWitness<A>: Witness<A> {
    /// Maps the type wrapped by this witness.
    type Map<T>;
}

// NOTE: The "witness" types UnicastAddr, MulticastAddr, and LinkLocalAddr -
// which provide the invariant that the value they contain is a unicast,
// multicast, or link-local address, respectively - cannot actually guarantee
// this property without certain promises from the implementations of the
// UnicastAddress, MulticastAddress, and LinkLocalAddress traits that they rely
// on. In particular, the values must be "immutable" in the sense that, given
// only immutable references to the values, nothing about the values can change
// such that the "unicast-ness", "multicast-ness" or "link-local-ness" of the
// values change. Since the UnicastAddress, MulticastAddress, and
// LinkLocalAddress traits are not unsafe traits, it would be unsound for unsafe
// code to rely for its soundness on this behavior. For a more in-depth
// discussion of why this isn't possible without an explicit opt-in on the part
// of the trait implementor, see this forum thread:
// https://users.rust-lang.org/t/prevent-interior-mutability/29403

/// Implements a trait for a witness type.
///
/// `impl_trait_for_witness` implements `$trait` for `$witness<A>` if `A:
/// $trait`.
macro_rules! impl_trait_for_witness {
    ($trait:ident, $method:ident, $witness:ident) => {
        impl<A: $trait> $trait for $witness<A> {
            fn $method(&self) -> bool {
                self.0.$method()
            }
        }
    };
}

/// Implements a trait with an associated type for a witness type.
///
/// `impl_trait_with_associated_type_for_witness` implements `$trait` with
/// associated type `$type` for `$witness<A>` if `A: $trait`.
macro_rules! impl_trait_with_associated_type_for_witness {
    ($trait:ident, $method:ident, $type:ident, $witness:ident) => {
        impl<A: $trait> $trait for $witness<A> {
            type $type = A::$type;
            fn $method(&self) -> Self::$type {
                self.0.$method()
            }
        }
    };
}

/// Addresses that can be specified.
///
/// `SpecifiedAddress` is implemented by address types for which some values are
/// considered [unspecified] addresses. Unspecified addresses are usually not
/// legal to be used in actual network traffic, and are only meant to represent
/// the lack of any defined address. The exact meaning of the unspecified
/// address often varies by context. For example, the IPv4 address 0.0.0.0 and
/// the IPv6 address :: can be used, in the context of creating a listening
/// socket on systems that use the BSD sockets API, to listen on all local IP
/// addresses rather than a particular one.
///
/// [unspecified]: https://en.wikipedia.org/wiki/0.0.0.0
pub trait SpecifiedAddress {
    /// Is this a specified address?
    ///
    /// `is_specified` must maintain the invariant that, if it is called twice
    /// on the same object, and in between those two calls, no code has operated
    /// on a mutable reference to that object, both calls will return the same
    /// value. This property is required in order to implement
    /// [`SpecifiedAddr`]. Note that, since this is not an `unsafe` trait,
    /// `unsafe` code may NOT rely on this property for its soundness. However,
    /// code MAY rely on this property for its correctness.
    fn is_specified(&self) -> bool;
}

impl_trait_for_witness!(SpecifiedAddress, is_specified, UnicastAddr);
impl_trait_for_witness!(SpecifiedAddress, is_specified, MulticastAddr);
impl_trait_for_witness!(SpecifiedAddress, is_specified, BroadcastAddr);
impl_trait_for_witness!(SpecifiedAddress, is_specified, LinkLocalAddr);
impl_trait_for_witness!(SpecifiedAddress, is_specified, NonMappedAddr);

/// Addresses that can be unicast.
///
/// `UnicastAddress` is implemented by address types for which some values are
/// considered [unicast] addresses. Unicast addresses are used to identify a
/// single network node, as opposed to broadcast and multicast addresses, which
/// identify a group of nodes.
///
/// `UnicastAddress` is only implemented for addresses whose unicast-ness can be
/// determined by looking only at the address itself (this is notably not true
/// for IPv4 addresses, which can be considered broadcast addresses depending on
/// the subnet in which they are used).
///
/// [unicast]: https://en.wikipedia.org/wiki/Unicast
pub trait UnicastAddress {
    /// Is this a unicast address?
    ///
    /// `is_unicast` must maintain the invariant that, if it is called twice on
    /// the same object, and in between those two calls, no code has operated on
    /// a mutable reference to that object, both calls will return the same
    /// value. This property is required in order to implement [`UnicastAddr`].
    /// Note that, since this is not an `unsafe` trait, `unsafe` code may NOT
    /// rely on this property for its soundness. However, code MAY rely on this
    /// property for its correctness.
    ///
    /// If this type also implements [`SpecifiedAddress`], then `a.is_unicast()`
    /// implies `a.is_specified()`.
    fn is_unicast(&self) -> bool;
}

impl_trait_for_witness!(UnicastAddress, is_unicast, SpecifiedAddr);
impl_trait_for_witness!(UnicastAddress, is_unicast, MulticastAddr);
impl_trait_for_witness!(UnicastAddress, is_unicast, BroadcastAddr);
impl_trait_for_witness!(UnicastAddress, is_unicast, LinkLocalAddr);
impl_trait_for_witness!(UnicastAddress, is_unicast, NonMappedAddr);

/// Addresses that can be multicast.
///
/// `MulticastAddress` is implemented by address types for which some values are
/// considered [multicast] addresses. Multicast addresses are used to identify a
/// group of multiple network nodes, as opposed to unicast addresses, which
/// identify a single node, or broadcast addresses, which identify all the nodes
/// in some region of a network.
///
/// [multicast]: https://en.wikipedia.org/wiki/Multicast
pub trait MulticastAddress {
    /// Is this a multicast address?
    ///
    /// `is_multicast` must maintain the invariant that, if it is called twice
    /// on the same object, and in between those two calls, no code has operated
    /// on a mutable reference to that object, both calls will return the same
    /// value. This property is required in order to implement
    /// [`MulticastAddr`]. Note that, since this is not an `unsafe` trait,
    /// `unsafe` code may NOT rely on this property for its soundness. However,
    /// code MAY rely on this property for its correctness.
    ///
    /// If this type also implements [`SpecifiedAddress`], then
    /// `a.is_multicast()` implies `a.is_specified()`.
    fn is_multicast(&self) -> bool;

    /// Is this a non-multicast address? The inverse of `is_multicast()`.
    fn is_non_multicast(&self) -> bool {
        !self.is_multicast()
    }
}

impl_trait_for_witness!(MulticastAddress, is_multicast, SpecifiedAddr);
impl_trait_for_witness!(MulticastAddress, is_multicast, UnicastAddr);
impl_trait_for_witness!(MulticastAddress, is_multicast, BroadcastAddr);
impl_trait_for_witness!(MulticastAddress, is_multicast, LinkLocalAddr);
impl_trait_for_witness!(MulticastAddress, is_multicast, NonMappedAddr);

/// Addresses that can be broadcast.
///
/// `BroadcastAddress` is implemented by address types for which some values are
/// considered [broadcast] addresses. Broadcast addresses are used to identify
/// all the nodes in some region of a network, as opposed to unicast addresses,
/// which identify a single node, or multicast addresses, which identify a group
/// of nodes (not necessarily all of them).
///
/// [broadcast]: https://en.wikipedia.org/wiki/Broadcasting_(networking)
pub trait BroadcastAddress {
    /// Is this a broadcast address?
    ///
    /// If this type also implements [`SpecifiedAddress`], then
    /// `a.is_broadcast()` implies `a.is_specified()`.
    fn is_broadcast(&self) -> bool;
}

impl_trait_for_witness!(BroadcastAddress, is_broadcast, SpecifiedAddr);
impl_trait_for_witness!(BroadcastAddress, is_broadcast, UnicastAddr);
impl_trait_for_witness!(BroadcastAddress, is_broadcast, MulticastAddr);
impl_trait_for_witness!(BroadcastAddress, is_broadcast, LinkLocalAddr);
impl_trait_for_witness!(BroadcastAddress, is_broadcast, NonMappedAddr);

/// Addresses that can be a link-local.
///
/// `LinkLocalAddress` is implemented by address types for which some values are
/// considered [link-local] addresses. Link-local addresses are used for
/// communication within a network segment, as opposed to global/public
/// addresses which may be used for communication across networks.
///
/// `LinkLocalAddress` is only implemented for addresses whose link-local-ness
/// can be determined by looking only at the address itself.
///
/// [link-local]: https://en.wikipedia.org/wiki/Link-local_address
pub trait LinkLocalAddress {
    /// Is this a link-local address?
    ///
    /// `is_link_local` must maintain the invariant that, if it is called twice
    /// on the same object, and in between those two calls, no code has operated
    /// on a mutable reference to that object, both calls will return the same
    /// value. This property is required in order to implement
    /// [`LinkLocalAddr`]. Note that, since this is not an `unsafe` trait,
    /// `unsafe` code may NOT rely on this property for its soundness. However,
    /// code MAY rely on this property for its correctness.
    ///
    /// If this type also implements [`SpecifiedAddress`], then
    /// `a.is_link_local()` implies `a.is_specified()`.
    fn is_link_local(&self) -> bool;
}

impl_trait_for_witness!(LinkLocalAddress, is_link_local, SpecifiedAddr);
impl_trait_for_witness!(LinkLocalAddress, is_link_local, UnicastAddr);
impl_trait_for_witness!(LinkLocalAddress, is_link_local, MulticastAddr);
impl_trait_for_witness!(LinkLocalAddress, is_link_local, BroadcastAddr);
impl_trait_for_witness!(LinkLocalAddress, is_link_local, NonMappedAddr);

/// A scope used by [`ScopeableAddress`]. See that trait's documentation for
/// more information.
///
/// `Scope` is implemented for `()`. No addresses with the `()` scope can ever
/// have an associated zone (in other words, `().can_have_zone()` always returns
/// `false`).
pub trait Scope {
    /// Can addresses in this scope have an associated zone?
    fn can_have_zone(&self) -> bool;
}

impl Scope for () {
    fn can_have_zone(&self) -> bool {
        false
    }
}

/// An address that can be tied to some scope identifier.
///
/// `ScopeableAddress` is implemented by address types for which some values can
/// have extra scoping information attached. Notably, some IPv6 addresses
/// belonging to a particular scope class require extra metadata to identify the
/// scope identifier or "zone". The zone is typically the networking interface
/// identifier.
///
/// Address types which are never in any identified scope may still implement
/// `ScopeableAddress` by setting the associated `Scope` type to `()`, which has
/// the effect of ensuring that a zone can never be associated with an address
/// (since the implementation of [`Scope::can_have_zone`] for `()` always
/// returns `false`).
pub trait ScopeableAddress {
    /// The type of all non-global scopes.
    type Scope: Scope;

    /// The scope of this address.
    ///
    /// `scope` must maintain the invariant that, if it is called twice on the
    /// same object, and in between those two calls, no code has operated on a
    /// mutable reference to that object, both calls will return the same value.
    /// This property is required in order to implement [`AddrAndZone`]. Note
    /// that, since this is not an `unsafe` trait, `unsafe` code may NOT rely on
    /// this property for its soundness. However, code MAY rely on this property
    /// for its correctness.
    ///
    /// If this type also implements [`SpecifiedAddress`] then
    /// `a.scope().can_have_zone()` implies `a.is_specified()`, since
    /// unspecified addresses are always global, and the global scope cannot
    /// have a zone.
    fn scope(&self) -> Self::Scope;
}

impl_trait_with_associated_type_for_witness!(ScopeableAddress, scope, Scope, SpecifiedAddr);
impl_trait_with_associated_type_for_witness!(ScopeableAddress, scope, Scope, UnicastAddr);
impl_trait_with_associated_type_for_witness!(ScopeableAddress, scope, Scope, MulticastAddr);
impl_trait_with_associated_type_for_witness!(ScopeableAddress, scope, Scope, BroadcastAddr);
impl_trait_with_associated_type_for_witness!(ScopeableAddress, scope, Scope, LinkLocalAddr);
impl_trait_with_associated_type_for_witness!(ScopeableAddress, scope, Scope, NonMappedAddr);

/// An address that may represent an address from another addressing scheme.
///
/// `MappedAddress` is implemented by address types that can map another
/// addressing scheme. Notably, IPv6 addresses, which may represent an IPv4
/// address using the IPv4-mapped-Ipv6 subnet (e.g. ::FFFF:0:0/96).
///
/// Address types which cannot be used to represent another addressing scheme
/// can still implement `MappedAddress` by treating all addresses as
/// non-mapped.
pub trait MappedAddress {
    /// Is this a non-mapped address?
    fn is_non_mapped(&self) -> bool;
}

impl_trait_for_witness!(MappedAddress, is_non_mapped, SpecifiedAddr);
impl_trait_for_witness!(MappedAddress, is_non_mapped, UnicastAddr);
impl_trait_for_witness!(MappedAddress, is_non_mapped, MulticastAddr);
impl_trait_for_witness!(MappedAddress, is_non_mapped, BroadcastAddr);
impl_trait_for_witness!(MappedAddress, is_non_mapped, LinkLocalAddr);

macro_rules! doc_comment {
    ($x:expr, $($tt:tt)*) => {
        #[doc = $x]
        $($tt)*
    };
}

/// Define a witness type and implement methods and traits for it.
///
/// - `$type` is the type's name
/// - `$adj` is a string literal representing the adjective used to describe
///   addresses of this type for documentation purposes (e.g., "specified",
///   "unicast", etc)
/// - `$trait` is the name of the trait associated with the property to be
///   witnessed
/// - `$method` is the method on `$trait` which determines whether the property
///   holds (e.g., `is_specified`)
macro_rules! impl_witness {
    ($type:ident, $adj:literal, $trait:ident, $method:ident) => {
        doc_comment! {
        concat!("An address which is guaranteed to be ", $adj, ".

`", stringify!($type), "` wraps an address of type `A` and guarantees that it is
a ", $adj, " address. Note that this guarantee is contingent on a correct
implementation of the [`", stringify!($trait), "`] trait. Since that trait is
not `unsafe`, `unsafe` code may NOT rely on this guarantee for its soundness."),
            #[derive(Copy, Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
            pub struct $type<A>(A);
        }

        impl<A: $trait> $type<A> {
            // NOTE(joshlf): It may seem odd to include `new` and `from_witness`
            // constructors here when they already exists on the `Witness`
            // trait, which this type implements. The reason we do this is that,
            // since many of these types implement the `Witness` trait multiple
            // times (e.g., `Witness<A> for LinkLocalAddr<A>` and `Witness<A>
            // for LinkLocalAddr<MulticastAddr<A>`), if we didn't provide these
            // constructors, callers invoking `Foo::new` or `Foo::from_witness`
            // would need to `use` the `Witness` trait, and the compiler often
            // doesn't have enough information to figure out which `Witness`
            // implementation is meant in a given situation. This, in turn,
            // requires a lot of boilerplate type annotations on the part of
            // users. Providing these constructors helps alleviate this problem.

            doc_comment! {
                concat!("Constructs a new `", stringify!($type), "`.

`new` returns `None` if `!addr.", stringify!($method), "()`."),
                #[inline]
                pub fn new(addr: A) -> Option<$type<A>> {
                    if !addr.$method() {
                        return None;
                    }
                    Some($type(addr))
                }
            }

            doc_comment! {
                concat!("Constructs a new `", stringify!($type), "` from a
witness type.

`from_witness(witness)` is equivalent to `new(witness.into_addr())`."),
                pub fn from_witness<W: Witness<A>>(addr: W) -> Option<$type<A>> {
                    $type::new(addr.into_addr())
                }
            }
        }

        // TODO(https://github.com/rust-lang/rust/issues/57563): Once traits
        // other than `Sized` are supported for const fns, move this into the
        // block with the `A: $trait` bound.
        impl<A> $type<A> {
            doc_comment! {
                concat!("Constructs a new `", stringify!($type), "` without
checking to see if `addr` is actually ", $adj, ".

# Safety

It is up to the caller to make sure that `addr` is ", $adj, " to avoid breaking
the guarantees of `", stringify!($type), "`. See [`", stringify!($type), "`] for
more details."),
                pub const unsafe fn new_unchecked(addr: A) -> $type<A> {
                    $type(addr)
                }
            }
        }

        impl<A> sealed::Sealed for $type<A> {}
        impl<A: $trait> Witness<A> for $type<A> {
            fn new(addr: A) -> Option<$type<A>> {
                $type::new(addr)
            }

            unsafe fn new_unchecked(addr: A) -> $type<A> {
                $type(addr)
            }

            #[inline]
            fn into_addr(self) -> A {
                self.0
            }
        }

        impl<A: $trait> TransposableWitness<A> for $type<A> {
            type Map<T> = $type<T>;
        }

        impl<A: $trait> AsRef<$type<A>> for $type<A> {
            fn as_ref(&self) -> &$type<A> {
                self
            }
        }

        impl<A: $trait> AsRef<A> for $type<A> {
            fn as_ref(&self) -> &A {
                &self.0
            }
        }

        impl<A: $trait> Deref for $type<A> {
            type Target = A;

            #[inline]
            fn deref(&self) -> &A {
                &self.0
            }
        }

        impl<A: Display> Display for $type<A> {
            #[inline]
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        // Witness types help provide type safety for the compiler. The things
        // they witness should be evident from seeing the contained type so we
        // save some characters and offer a passthrough Debug impl.
        impl<A: Debug> Debug for $type<A> {
            #[inline]
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

/// Implements an `into_specified` method on the witness type `$type`.
///
/// - `$trait` is the name of the trait associated with the witnessed property
/// - `$method` is the method on `$trait` which determines whether the property
///   holds (e.g., `is_unicast`)
///
/// An `into_specified` method is predicated on the witnessed property implying
/// that the address is also specified (e.g., `UnicastAddress::is_unicast`
/// implies `SpecifiedAddress::is_specified`).
macro_rules! impl_into_specified {
    ($type:ident, $trait:ident, $method:ident) => {
        impl<A: $trait + SpecifiedAddress> $type<A> {
            doc_comment! {
                concat!("Converts this `", stringify!($type), "` into a
[`SpecifiedAddr`].

[`", stringify!($trait), "::", stringify!($method), "`] implies
[`SpecifiedAddress::is_specified`], so all `", stringify!($type), "`s are
guaranteed to be specified, so this conversion is infallible."),
                #[inline]
                pub fn into_specified(self) -> SpecifiedAddr<A> {
                    SpecifiedAddr(self.0)
                }
            }
        }

        impl<A: $trait + SpecifiedAddress> From<$type<A>> for SpecifiedAddr<A> {
            fn from(addr: $type<A>) -> SpecifiedAddr<A> {
                addr.into_specified()
            }
        }
    };
}

/// Implements [`Witness`] for a nested witness type.
///
/// Accepted Formats:
/// * `impl_nested_witness!(trait1, type1, trait2, type2)`
///     Implements `Witness<A>` for `type1<type2<A>>`.
/// * `impl_nested_witness!(trait1, type1, trait2, type2, trait3, type3)`
///     Implements `Witness<A>` for `type1<type2<type3<A>>>`.
///
/// Due to the nature of combinatorix, it is not advised to use this macro
/// for all possible combinations of nested witnesses, only those that are
/// actually instantiated in code.
macro_rules! impl_nested_witness {
    ($trait1:ident, $type1:ident, $trait2:ident, $type2:ident) => {
        impl<A: $trait1 + $trait2> Witness<A> for $type1<$type2<A>> {
            #[inline]
            fn new(addr: A) -> Option<$type1<$type2<A>>> {
                $type2::new(addr).and_then(Witness::<$type2<A>>::new)
            }

            unsafe fn new_unchecked(addr: A) -> $type1<$type2<A>> {
                $type1($type2(addr))
            }

            #[inline]
            fn into_addr(self) -> A {
                self.0.into_addr()
            }
        }

        impl<A: $trait1 + $trait2> AsRef<A> for $type1<$type2<A>> {
            fn as_ref(&self) -> &A {
                &self.0.0
            }
        }
    };
    ($trait1:ident, $type1:ident, $trait2:ident, $type2:ident, $trait3:ident, $type3:ident) => {
        impl<A: $trait1 + $trait2 + $trait3> Witness<A> for $type1<$type2<$type3<A>>> {
            #[inline]
            fn new(addr: A) -> Option<$type1<$type2<$type3<A>>>> {
                $type3::new(addr).and_then(Witness::<$type3<A>>::new)
            }

            unsafe fn new_unchecked(addr: A) -> $type1<$type2<$type3<A>>> {
                $type1($type2($type3(addr)))
            }

            #[inline]
            fn into_addr(self) -> A {
                self.0.into_addr()
            }
        }

        impl<A: $trait1 + $trait2 + $trait3> AsRef<A> for $type1<$type2<$type3<A>>> {
            fn as_ref(&self) -> &A {
                &self.0.0
            }
        }
    };
}

/// Implements `From<T> or SpecifiedAddr<A>` where `T` is the nested witness.
///
/// Accepted Formats:
/// * `impl_into_specified_for_nested_witness!(trait1, type1, trait2, type2)`
///     Implements `From<type1<type2<A>>> for SpecifiedAddr<A>`.
/// * `impl_nested_witness!(trait1, type1, trait2, type2, trait3, type3)`
///     Implements `From<type1<type2<type3<<A>>>> for SpecifiedAddr<A>`.
///
/// Due to the nature of combinatorix, it is not advised to use this macro
/// for all possible combinations of nested witnesses, only those that are
/// actually instantiated in code.
macro_rules! impl_into_specified_for_nested_witness {
    ($trait1:ident, $type1:ident, $trait2:ident, $type2:ident) => {
        impl<A: $trait1 + $trait2 + SpecifiedAddress> From<$type1<$type2<A>>> for SpecifiedAddr<A> {
            fn from(addr: $type1<$type2<A>>) -> SpecifiedAddr<A> {
                SpecifiedAddr(addr.into_addr())
            }
        }
    };
    ($trait1:ident, $type1:ident, $trait2:ident, $type2:ident, $trait3:ident, $type3:ident) => {
        impl<A: $trait1 + $trait2 + $trait3 + SpecifiedAddress> From<$type1<$type2<$type3<A>>>>
            for SpecifiedAddr<A>
        {
            fn from(addr: $type1<$type2<$type3<A>>>) -> SpecifiedAddr<A> {
                SpecifiedAddr(addr.into_addr())
            }
        }
    };
}

/// Implements `TryFrom<$from_ty<A>> for $into_ty<A>`
macro_rules! impl_try_from_witness {
    (@inner [$from_ty:ident: $from_trait:ident], [$into_ty:ident: $into_trait:ident]) => {
        impl<A: $from_trait + $into_trait> TryFrom<$from_ty<A>> for $into_ty<A> {
            type Error = ();
            fn try_from(addr: $from_ty<A>) -> Result<$into_ty<A>, ()> {
                Witness::<A>::from_witness(addr).ok_or(())
            }
        }
    };
    ([$from_ty:ident: $from_trait:ident], $([$into_ty:ident: $into_trait:ident]),*) => {
        $(
            impl_try_from_witness!(@inner [$from_ty: $from_trait], [$into_ty: $into_trait]);
        )*
    }
}

// SpecifiedAddr
impl_witness!(SpecifiedAddr, "specified", SpecifiedAddress, is_specified);
impl_try_from_witness!(
    [SpecifiedAddr: SpecifiedAddress],
    [UnicastAddr: UnicastAddress],
    [MulticastAddr: MulticastAddress],
    [BroadcastAddr: BroadcastAddress],
    [LinkLocalAddr: LinkLocalAddress],
    [LinkLocalUnicastAddr: LinkLocalUnicastAddress],
    [LinkLocalMulticastAddr: LinkLocalMulticastAddress],
    [LinkLocalBroadcastAddr: LinkLocalBroadcastAddress],
    [NonMappedAddr: MappedAddress]
);

// UnicastAddr
impl_witness!(UnicastAddr, "unicast", UnicastAddress, is_unicast);
impl_into_specified!(UnicastAddr, UnicastAddress, is_unicast);
impl_nested_witness!(UnicastAddress, UnicastAddr, LinkLocalAddress, LinkLocalAddr);
impl_nested_witness!(UnicastAddress, UnicastAddr, MappedAddress, NonMappedAddr);
impl_into_specified_for_nested_witness!(
    UnicastAddress,
    UnicastAddr,
    LinkLocalAddress,
    LinkLocalAddr
);
impl_into_specified_for_nested_witness!(UnicastAddress, UnicastAddr, MappedAddress, NonMappedAddr);
impl_try_from_witness!(
    [UnicastAddr: UnicastAddress],
    [MulticastAddr: MulticastAddress],
    [BroadcastAddr: BroadcastAddress],
    [LinkLocalAddr: LinkLocalAddress],
    [LinkLocalMulticastAddr: LinkLocalMulticastAddress],
    [LinkLocalBroadcastAddr: LinkLocalBroadcastAddress],
    [NonMappedAddr: MappedAddress]
);

// MulticastAddr
impl_witness!(MulticastAddr, "multicast", MulticastAddress, is_multicast);
impl_into_specified!(MulticastAddr, MulticastAddress, is_multicast);
impl_nested_witness!(MulticastAddress, MulticastAddr, LinkLocalAddress, LinkLocalAddr);
impl_nested_witness!(MulticastAddress, MulticastAddr, MappedAddress, NonMappedAddr);
impl_into_specified_for_nested_witness!(
    MulticastAddress,
    MulticastAddr,
    LinkLocalAddress,
    LinkLocalAddr
);
impl_into_specified_for_nested_witness!(
    MulticastAddress,
    MulticastAddr,
    MappedAddress,
    NonMappedAddr
);
impl_try_from_witness!(
    [MulticastAddr: MulticastAddress],
    [UnicastAddr: UnicastAddress],
    [BroadcastAddr: BroadcastAddress],
    [LinkLocalAddr: LinkLocalAddress],
    [LinkLocalUnicastAddr: LinkLocalUnicastAddress],
    [LinkLocalBroadcastAddr: LinkLocalBroadcastAddress],
    [NonMappedAddr: MappedAddress]
);

impl<A: MulticastAddress + MappedAddress> MulticastAddr<A> {
    /// Wraps `self` in the [`NonMappedAddr`] witness type.
    pub fn non_mapped(self) -> NonMappedAddr<MulticastAddr<A>> {
        // Safety: IPv4 addresses cannot be mapped. For IPv6 addresses, the
        // multicast subnet (FF00::/8) and the ipv4-mapped-ipv6 address space
        // (::FFFF:0000:0000/96) are disjoint: presence in the multicast subnet
        // implies absence from the ipv4-mapped-ipv6 address space.
        unsafe { NonMappedAddr::new_unchecked(self) }
    }
}

// NonMulticastAddr - An address known to not be multicast.
//
// Note this type is similar to `UnicastAddr`, but not identical: all
// `UnicastAddr' can also be `NonMulticastAddr`, but not all `NonMulticastAddr`
// can be `UnicastAddr`. E.g. an IPv4 Broadcast Addr is non-multicast but not
// unicast.
impl_witness!(NonMulticastAddr, "non-multicast", MulticastAddress, is_non_multicast);
impl_nested_witness!(MulticastAddress, NonMulticastAddr, SpecifiedAddress, SpecifiedAddr);
impl_nested_witness!(MulticastAddress, NonMulticastAddr, UnicastAddress, UnicastAddr);
impl_nested_witness!(MulticastAddress, NonMulticastAddr, BroadcastAddress, BroadcastAddr);
impl_nested_witness!(MulticastAddress, NonMulticastAddr, MappedAddress, NonMappedAddr);

impl<A: MulticastAddress + UnicastAddress> From<UnicastAddr<A>> for NonMulticastAddr<A> {
    #[inline]
    fn from(addr: UnicastAddr<A>) -> Self {
        // Safe because a unicast address is guaranteed to not be multicast.
        unsafe { NonMulticastAddr::new_unchecked(addr.into_addr()) }
    }
}
// NB: Implement nested witness to a depth of three, only for the types that are
// actually used by consumers of this library.
impl_nested_witness!(
    MulticastAddress,
    NonMulticastAddr,
    MappedAddress,
    NonMappedAddr,
    SpecifiedAddress,
    SpecifiedAddr
);
impl_into_specified_for_nested_witness!(
    MulticastAddress,
    NonMulticastAddr,
    MappedAddress,
    NonMappedAddr,
    SpecifiedAddress,
    SpecifiedAddr
);

// BroadcastAddr
impl_witness!(BroadcastAddr, "broadcast", BroadcastAddress, is_broadcast);
impl_into_specified!(BroadcastAddr, BroadcastAddress, is_broadcast);
impl_nested_witness!(BroadcastAddress, BroadcastAddr, LinkLocalAddress, LinkLocalAddr);
impl_nested_witness!(BroadcastAddress, BroadcastAddr, MappedAddress, NonMappedAddr);
impl_into_specified_for_nested_witness!(
    BroadcastAddress,
    BroadcastAddr,
    LinkLocalAddress,
    LinkLocalAddr
);
impl_into_specified_for_nested_witness!(
    BroadcastAddress,
    BroadcastAddr,
    MappedAddress,
    NonMappedAddr
);
impl_try_from_witness!(
    [BroadcastAddr: BroadcastAddress],
    [UnicastAddr: UnicastAddress],
    [MulticastAddr: MulticastAddress],
    [LinkLocalAddr: LinkLocalAddress],
    [LinkLocalUnicastAddr: LinkLocalUnicastAddress],
    [LinkLocalMulticastAddr: LinkLocalMulticastAddress],
    [NonMappedAddr: MappedAddress]
);

// LinkLocalAddr
impl_witness!(LinkLocalAddr, "link-local", LinkLocalAddress, is_link_local);
impl_into_specified!(LinkLocalAddr, LinkLocalAddress, is_link_local);
impl_nested_witness!(LinkLocalAddress, LinkLocalAddr, UnicastAddress, UnicastAddr);
impl_nested_witness!(LinkLocalAddress, LinkLocalAddr, MulticastAddress, MulticastAddr);
impl_nested_witness!(LinkLocalAddress, LinkLocalAddr, BroadcastAddress, BroadcastAddr);
impl_nested_witness!(LinkLocalAddress, LinkLocalAddr, MappedAddress, NonMappedAddr);
impl_into_specified_for_nested_witness!(
    LinkLocalAddress,
    LinkLocalAddr,
    UnicastAddress,
    UnicastAddr
);
impl_into_specified_for_nested_witness!(
    LinkLocalAddress,
    LinkLocalAddr,
    MulticastAddress,
    MulticastAddr
);
impl_into_specified_for_nested_witness!(
    LinkLocalAddress,
    LinkLocalAddr,
    BroadcastAddress,
    BroadcastAddr
);
impl_into_specified_for_nested_witness!(
    LinkLocalAddress,
    LinkLocalAddr,
    MappedAddress,
    NonMappedAddr
);
impl_try_from_witness!(
    [LinkLocalAddr: LinkLocalAddress],
    [UnicastAddr: UnicastAddress],
    [MulticastAddr: MulticastAddress],
    [BroadcastAddr: BroadcastAddress],
    [NonMappedAddr: MappedAddress]
);

// NonMappedAddr
impl_witness!(NonMappedAddr, "non_mapped", MappedAddress, is_non_mapped);
impl_nested_witness!(MappedAddress, NonMappedAddr, SpecifiedAddress, SpecifiedAddr);
impl_nested_witness!(MappedAddress, NonMappedAddr, UnicastAddress, UnicastAddr);
impl_nested_witness!(MappedAddress, NonMappedAddr, MulticastAddress, MulticastAddr);
impl_nested_witness!(MappedAddress, NonMappedAddr, BroadcastAddress, BroadcastAddr);
impl_nested_witness!(MappedAddress, NonMappedAddr, LinkLocalAddress, LinkLocalAddr);
impl_into_specified_for_nested_witness!(
    MappedAddress,
    NonMappedAddr,
    SpecifiedAddress,
    SpecifiedAddr
);
impl_into_specified_for_nested_witness!(MappedAddress, NonMappedAddr, UnicastAddress, UnicastAddr);
impl_into_specified_for_nested_witness!(
    MappedAddress,
    NonMappedAddr,
    MulticastAddress,
    MulticastAddr
);
impl_into_specified_for_nested_witness!(
    MappedAddress,
    NonMappedAddr,
    BroadcastAddress,
    BroadcastAddr
);
impl_into_specified_for_nested_witness!(
    MappedAddress,
    NonMappedAddr,
    LinkLocalAddress,
    LinkLocalAddr
);
impl_try_from_witness!(
    [NonMappedAddr: MappedAddress],
    [SpecifiedAddr: SpecifiedAddress],
    [UnicastAddr: UnicastAddress],
    [MulticastAddr: MulticastAddress],
    [BroadcastAddr: BroadcastAddress],
    [LinkLocalAddr: LinkLocalAddress],
    [LinkLocalUnicastAddr: LinkLocalUnicastAddress],
    [LinkLocalMulticastAddr: LinkLocalMulticastAddress],
    [LinkLocalBroadcastAddr: LinkLocalBroadcastAddress]
);

// NOTE(joshlf): We provide these type aliases both for convenience and also to
// steer users towards these types and away from `UnicastAddr<LinkLocalAddr<A>>`
// and `MulticastAddr<LinkLocalAddr<A>>`, which are also valid. The reason we
// still implement `Witness<A>` for those types is that user code may contain
// generic contexts (e.g., some code with `UnicastAddr<A>`, and other code which
// wishes to supply `A = LinkLocalAddr<AA>`), and we want to support that use
// case.

/// An address that can be link-local and unicast.
///
/// `LinkLocalUnicastAddress` is a shorthand for `LinkLocalAddress +
/// UnicastAddress`.
pub trait LinkLocalUnicastAddress: LinkLocalAddress + UnicastAddress {}
impl<A: LinkLocalAddress + UnicastAddress> LinkLocalUnicastAddress for A {}

/// An address that can be link-local and multicast.
///
/// `LinkLocalMulticastAddress` is a shorthand for `LinkLocalAddress +
/// MulticastAddress`.
pub trait LinkLocalMulticastAddress: LinkLocalAddress + MulticastAddress {}
impl<A: LinkLocalAddress + MulticastAddress> LinkLocalMulticastAddress for A {}

/// An address that can be link-local and broadcast.
///
/// `LinkLocalBroadcastAddress` is a shorthand for `LinkLocalAddress +
/// BroadcastAddress`.
pub trait LinkLocalBroadcastAddress: LinkLocalAddress + BroadcastAddress {}
impl<A: LinkLocalAddress + BroadcastAddress> LinkLocalBroadcastAddress for A {}

/// A link-local unicast address.
pub type LinkLocalUnicastAddr<A> = LinkLocalAddr<UnicastAddr<A>>;

/// A link-local multicast address.
pub type LinkLocalMulticastAddr<A> = LinkLocalAddr<MulticastAddr<A>>;

/// A link-local broadcast address.
pub type LinkLocalBroadcastAddr<A> = LinkLocalAddr<BroadcastAddr<A>>;

impl_try_from_witness!(
    [LinkLocalUnicastAddr: LinkLocalUnicastAddress],
    [UnicastAddr: UnicastAddress],
    [MulticastAddr: MulticastAddress],
    [LinkLocalAddr: LinkLocalAddress],
    [LinkLocalMulticastAddr: LinkLocalMulticastAddress],
    [LinkLocalBroadcastAddr: LinkLocalBroadcastAddress]
);
impl_try_from_witness!(
    [LinkLocalMulticastAddr: LinkLocalMulticastAddress],
    [UnicastAddr: UnicastAddress],
    [MulticastAddr: MulticastAddress],
    [LinkLocalAddr: LinkLocalAddress],
    [LinkLocalUnicastAddr: LinkLocalUnicastAddress],
    [LinkLocalBroadcastAddr: LinkLocalBroadcastAddress]
);
impl_try_from_witness!(
    [LinkLocalBroadcastAddr: LinkLocalBroadcastAddress],
    [UnicastAddr: UnicastAddress],
    [MulticastAddr: MulticastAddress],
    [LinkLocalAddr: LinkLocalAddress],
    [LinkLocalUnicastAddr: LinkLocalUnicastAddress],
    [LinkLocalMulticastAddr: LinkLocalMulticastAddress]
);

/// A witness type for an address and a scope zone.
///
/// `AddrAndZone` carries an address that *may* have a scope, alongside the
/// particular zone of that scope. The zone is also referred to as a "scope
/// identifier" in some systems (such as Linux).
///
/// Note that although `AddrAndZone` acts as a witness type, it does not
/// implement [`Witness`] since it carries both the address and scoping
/// information, and not only the witnessed address.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct AddrAndZone<A, Z>(A, Z);

impl<A: ScopeableAddress, Z> AddrAndZone<A, Z> {
    /// Constructs a new `AddrAndZone`, returning `Some` only if the provided
    /// `addr`'s scope can have a zone (`addr.scope().can_have_zone()`).
    pub fn new(addr: A, zone: Z) -> Option<Self> {
        if addr.scope().can_have_zone() { Some(Self(addr, zone)) } else { None }
    }
}

impl<A: ScopeableAddress + IpAddress, Z> AddrAndZone<A, Z> {
    /// Constructs a new `AddrAndZone`, returning `Some` only if the provided
    /// `addr`'s scope can have a zone (`addr.scope().can_have_zone()`) and
    /// `addr` is not a loopback address.
    pub fn new_not_loopback(addr: A, zone: Z) -> Option<Self> {
        if addr.scope().can_have_zone() && !addr.is_loopback() {
            Some(Self(addr, zone))
        } else {
            None
        }
    }
}

impl<A, Z> AddrAndZone<A, Z> {
    /// Constructs a new `AddrAndZone` without checking to see if `addr`'s scope
    /// can have a zone.
    ///
    /// # Safety
    ///
    /// It is up to the caller to make sure that `addr`'s scope can have a zone
    /// to avoid breaking the guarantees of `AddrAndZone`.
    #[inline]
    pub const unsafe fn new_unchecked(addr: A, zone: Z) -> Self {
        Self(addr, zone)
    }

    /// Consumes this `AddrAndZone`, returning the address and zone separately.
    pub fn into_addr_scope_id(self) -> (A, Z) {
        let AddrAndZone(addr, zone) = self;
        (addr, zone)
    }

    /// Translates the zone identifier using the provided function.
    pub fn map_zone<Y>(self, f: impl FnOnce(Z) -> Y) -> AddrAndZone<A, Y> {
        let AddrAndZone(addr, zone) = self;
        AddrAndZone(addr, f(zone))
    }

    /// Translates the address using `f`.
    pub fn map_addr<B>(self, f: impl FnOnce(A) -> B) -> AddrAndZone<B, Z> {
        let Self(addr, zone) = self;
        AddrAndZone(f(addr), zone)
    }

    /// Attempts to translate the zone identifier using the provided function.
    pub fn try_map_zone<Y, E>(
        self,
        f: impl FnOnce(Z) -> Result<Y, E>,
    ) -> Result<AddrAndZone<A, Y>, E> {
        let AddrAndZone(addr, zone) = self;
        f(zone).map(|zone| AddrAndZone(addr, zone))
    }

    /// Accesses the addr for this `AddrAndZone`.
    pub fn addr(&self) -> A
    where
        A: Copy,
    {
        let AddrAndZone(addr, _zone) = self;
        *addr
    }

    /// Converts from `AddrAndZone<A, Z>` to `AddrAndZone<&A, &Z>`.
    pub fn as_ref(&self) -> AddrAndZone<&A, &Z> {
        let Self(addr, zone) = self;
        AddrAndZone(addr, zone)
    }
}

impl<A: Display, Z: Display> Display for AddrAndZone<A, Z> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}%{}", self.0, self.1)
    }
}

impl<A, Z> sealed::Sealed for AddrAndZone<A, Z> {}

impl<A: SpecifiedAddress, Z> From<AddrAndZone<SpecifiedAddr<A>, Z>> for AddrAndZone<A, Z> {
    fn from(AddrAndZone(addr, zone): AddrAndZone<SpecifiedAddr<A>, Z>) -> Self {
        Self(addr.into_addr(), zone)
    }
}

/// An address that may have an associated scope zone.
#[allow(missing_docs)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ZonedAddr<A, Z> {
    Unzoned(A),
    Zoned(AddrAndZone<A, Z>),
}

impl<A: Display, Z: Display> Display for ZonedAddr<A, Z> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unzoned(addr) => write!(f, "{addr}"),
            Self::Zoned(addr_and_zone) => write!(f, "{addr_and_zone}"),
        }
    }
}

impl<A, Z> ZonedAddr<A, Z> {
    /// Decomposes this `ZonedAddr` into an addr and an optional scope zone.
    pub fn into_addr_zone(self) -> (A, Option<Z>) {
        match self {
            ZonedAddr::Unzoned(addr) => (addr, None),
            ZonedAddr::Zoned(scope_and_zone) => {
                let (addr, zone) = scope_and_zone.into_addr_scope_id();
                (addr, Some(zone))
            }
        }
    }

    /// Accesses the addr for this `ZonedAddr`.
    pub fn addr(&self) -> A
    where
        A: Copy,
    {
        match self {
            ZonedAddr::Unzoned(addr) => *addr,
            ZonedAddr::Zoned(addr_and_zone) => addr_and_zone.addr(),
        }
    }

    /// Translates the zone identifier using the provided function.
    pub fn map_zone<Y>(self, f: impl FnOnce(Z) -> Y) -> ZonedAddr<A, Y> {
        match self {
            ZonedAddr::Unzoned(u) => ZonedAddr::Unzoned(u),
            ZonedAddr::Zoned(z) => ZonedAddr::Zoned(z.map_zone(f)),
        }
    }

    /// Translates the address using `f`.
    pub fn map_addr<B>(self, f: impl FnOnce(A) -> B) -> ZonedAddr<B, Z> {
        match self {
            Self::Unzoned(u) => ZonedAddr::Unzoned(f(u)),
            Self::Zoned(z) => ZonedAddr::Zoned(z.map_addr(f)),
        }
    }

    /// Converts from `&ZonedAddr<A, Z>` to `ZonedAddr<&A, &Z>`.
    pub fn as_ref(&self) -> ZonedAddr<&A, &Z> {
        match self {
            Self::Unzoned(u) => ZonedAddr::Unzoned(u),
            Self::Zoned(z) => ZonedAddr::Zoned(z.as_ref()),
        }
    }
}

impl<A: ScopeableAddress, Z> ZonedAddr<A, Z> {
    /// Creates a new `ZonedAddr` with the provided optional scope zone.
    ///
    /// If `zone` is `None`, [`ZonedAddr::Unzoned`] is returned. Otherwise, a
    /// [`ZonedAddr::Zoned`] is returned only if the provided `addr`'s scope can
    /// have a zone (`addr.scope().can_have_zone()`).
    pub fn new(addr: A, zone: Option<Z>) -> Option<Self> {
        match zone {
            Some(zone) => AddrAndZone::new(addr, zone).map(ZonedAddr::Zoned),
            None => Some(ZonedAddr::Unzoned(addr)),
        }
    }
}

impl<A: IpAddress + ScopeableAddress, Z: Clone> ZonedAddr<A, Z> {
    /// Creates a [`ZonedAddr::Zoned`] iff `addr` can have a zone and is not
    /// loopback.
    ///
    /// `get_zone` is only called if the address needs a zone.
    pub fn new_zoned_if_necessary(addr: A, get_zone: impl FnOnce() -> Z) -> Self {
        match AddrAndZone::new_not_loopback(addr, ()) {
            Some(addr_and_zone) => Self::Zoned(addr_and_zone.map_zone(move |()| get_zone())),
            None => Self::Unzoned(addr),
        }
    }
}

impl<A: ScopeableAddress<Scope = ()>, Z> ZonedAddr<A, Z> {
    /// Retrieves the addr for this `ZonedAddr` when the `Scope` is `()`.
    ///
    /// `()` is a known implementation that never allows `AddrAndZone` to be
    /// constructed so we can safely drop the zone information.
    pub fn into_unzoned(self) -> A {
        match self {
            ZonedAddr::Unzoned(u) => u,
            ZonedAddr::Zoned(_z) => unreachable!(),
        }
    }
}

impl<A, Z> From<AddrAndZone<A, Z>> for ZonedAddr<A, Z> {
    fn from(a: AddrAndZone<A, Z>) -> Self {
        Self::Zoned(a)
    }
}

impl<A: SpecifiedAddress, Z> From<ZonedAddr<SpecifiedAddr<A>, Z>> for ZonedAddr<A, Z> {
    fn from(zoned_addr: ZonedAddr<SpecifiedAddr<A>, Z>) -> Self {
        match zoned_addr {
            ZonedAddr::Unzoned(a) => Self::Unzoned(a.into_addr()),
            ZonedAddr::Zoned(z) => Self::Zoned(z.into()),
        }
    }
}

impl<A, I: Ip> GenericOverIp<I> for SpecifiedAddr<A> {
    type Type = SpecifiedAddr<I::Addr>;
}

impl<A: IpAddress, I: Ip> GenericOverIp<I> for MulticastAddr<A> {
    type Type = MulticastAddr<I::Addr>;
}

impl<A: GenericOverIp<I>, I: Ip, Z> GenericOverIp<I> for ZonedAddr<A, Z> {
    type Type = ZonedAddr<A::Type, Z>;
}

impl<A: GenericOverIp<I>, I: Ip, Z> GenericOverIp<I> for AddrAndZone<A, Z> {
    type Type = AddrAndZone<A::Type, Z>;
}

/// Provides a `Display` implementation for printing an address and a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AddrAndPortFormatter<A, P, I: Ip> {
    addr: A,
    port: P,
    _marker: IpVersionMarker<I>,
}

impl<A, P, I: Ip> AddrAndPortFormatter<A, P, I> {
    /// Construct a new `AddrAndPortFormatter`.
    pub fn new(addr: A, port: P) -> Self {
        Self { addr, port, _marker: IpVersionMarker::new() }
    }
}

impl<A: Display, P: Display, I: Ip> Display for AddrAndPortFormatter<A, P, I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let Self { addr, port, _marker } = self;
        let IpInvariant(result) = I::map_ip(
            IpInvariant((addr, port, f)),
            |IpInvariant((addr, port, f))| IpInvariant(write!(f, "{}:{}", addr, port)),
            |IpInvariant((addr, port, f))| IpInvariant(write!(f, "[{}]:{}", addr, port)),
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    enum Address {
        Unspecified,
        GlobalUnicast,
        GlobalMulticast,
        GlobalBroadcast,
        LinkLocalUnicast,
        LinkLocalMulticast,
        LinkLocalBroadcast,
        MappedUnicast,
        MappedMulticast,
        MappedBroadcast,
    }

    impl SpecifiedAddress for Address {
        fn is_specified(&self) -> bool {
            *self != Address::Unspecified
        }
    }

    impl UnicastAddress for Address {
        fn is_unicast(&self) -> bool {
            use Address::*;
            match self {
                GlobalUnicast | LinkLocalUnicast | MappedUnicast => true,
                Unspecified | GlobalMulticast | GlobalBroadcast | LinkLocalMulticast
                | LinkLocalBroadcast | MappedMulticast | MappedBroadcast => false,
            }
        }
    }

    impl MulticastAddress for Address {
        fn is_multicast(&self) -> bool {
            use Address::*;
            match self {
                GlobalMulticast | LinkLocalMulticast | MappedMulticast => true,
                Unspecified | GlobalUnicast | GlobalBroadcast | LinkLocalUnicast
                | MappedUnicast | MappedBroadcast | LinkLocalBroadcast => false,
            }
        }
    }

    impl BroadcastAddress for Address {
        fn is_broadcast(&self) -> bool {
            use Address::*;
            match self {
                GlobalBroadcast | LinkLocalBroadcast | MappedBroadcast => true,
                Unspecified | GlobalUnicast | GlobalMulticast | LinkLocalUnicast
                | MappedUnicast | MappedMulticast | LinkLocalMulticast => false,
            }
        }
    }

    impl LinkLocalAddress for Address {
        fn is_link_local(&self) -> bool {
            use Address::*;
            match self {
                LinkLocalUnicast | LinkLocalMulticast | LinkLocalBroadcast => true,
                Unspecified | GlobalUnicast | GlobalMulticast | GlobalBroadcast | MappedUnicast
                | MappedBroadcast | MappedMulticast => false,
            }
        }
    }

    impl MappedAddress for Address {
        fn is_non_mapped(&self) -> bool {
            use Address::*;
            match self {
                MappedUnicast | MappedBroadcast | MappedMulticast => false,
                Unspecified | GlobalUnicast | GlobalMulticast | GlobalBroadcast
                | LinkLocalUnicast | LinkLocalMulticast | LinkLocalBroadcast => true,
            }
        }
    }

    #[derive(Copy, Clone, Eq, PartialEq)]
    enum AddressScope {
        LinkLocal,
        Global,
    }

    impl Scope for AddressScope {
        fn can_have_zone(&self) -> bool {
            matches!(self, AddressScope::LinkLocal)
        }
    }

    impl ScopeableAddress for Address {
        type Scope = AddressScope;

        fn scope(&self) -> AddressScope {
            if self.is_link_local() { AddressScope::LinkLocal } else { AddressScope::Global }
        }
    }

    #[test]
    fn specified_addr() {
        assert_eq!(
            SpecifiedAddr::new(Address::GlobalUnicast),
            Some(SpecifiedAddr(Address::GlobalUnicast))
        );
        assert_eq!(SpecifiedAddr::new(Address::Unspecified), None);
    }

    #[test]
    fn unicast_addr() {
        assert_eq!(
            UnicastAddr::new(Address::GlobalUnicast),
            Some(UnicastAddr(Address::GlobalUnicast))
        );
        assert_eq!(UnicastAddr::new(Address::GlobalMulticast), None);
        assert_eq!(
            unsafe { UnicastAddr::new_unchecked(Address::GlobalUnicast) },
            UnicastAddr(Address::GlobalUnicast)
        );
    }

    #[test]
    fn multicast_addr() {
        assert_eq!(
            MulticastAddr::new(Address::GlobalMulticast),
            Some(MulticastAddr(Address::GlobalMulticast))
        );
        assert_eq!(MulticastAddr::new(Address::GlobalUnicast), None);
        assert_eq!(
            unsafe { MulticastAddr::new_unchecked(Address::GlobalMulticast) },
            MulticastAddr(Address::GlobalMulticast)
        );
    }

    #[test]
    fn broadcast_addr() {
        assert_eq!(
            BroadcastAddr::new(Address::GlobalBroadcast),
            Some(BroadcastAddr(Address::GlobalBroadcast))
        );
        assert_eq!(BroadcastAddr::new(Address::GlobalUnicast), None);
        assert_eq!(
            unsafe { BroadcastAddr::new_unchecked(Address::GlobalBroadcast) },
            BroadcastAddr(Address::GlobalBroadcast)
        );
    }

    #[test]
    fn link_local_addr() {
        assert_eq!(
            LinkLocalAddr::new(Address::LinkLocalUnicast),
            Some(LinkLocalAddr(Address::LinkLocalUnicast))
        );
        assert_eq!(LinkLocalAddr::new(Address::GlobalMulticast), None);
        assert_eq!(
            unsafe { LinkLocalAddr::new_unchecked(Address::LinkLocalUnicast) },
            LinkLocalAddr(Address::LinkLocalUnicast)
        );
    }

    #[test]
    fn non_mapped_addr() {
        assert_eq!(
            NonMappedAddr::new(Address::LinkLocalUnicast),
            Some(NonMappedAddr(Address::LinkLocalUnicast))
        );
        assert_eq!(NonMappedAddr::new(Address::MappedUnicast), None);
        assert_eq!(
            NonMappedAddr::new(Address::LinkLocalMulticast),
            Some(NonMappedAddr(Address::LinkLocalMulticast))
        );
        assert_eq!(NonMappedAddr::new(Address::MappedMulticast), None);
        assert_eq!(
            NonMappedAddr::new(Address::LinkLocalBroadcast),
            Some(NonMappedAddr(Address::LinkLocalBroadcast))
        );
        assert_eq!(NonMappedAddr::new(Address::MappedBroadcast), None);
    }

    macro_rules! test_nested {
        ($outer:ident, $inner:ident, $($input:ident => $output:expr,)*) => {
            $(
                assert_eq!($inner::new(Address::$input).and_then($outer::new), $output);
            )*
        };
    }

    #[test]
    fn nested_link_local() {
        // Test UnicastAddr<LinkLocalAddr>, MulticastAddr<LinkLocalAddr>,
        // BroadcastAddr<LinkLocalAddr>, LinkLocalAddr<UnicastAddr>,
        // LinkLocalAddr<MulticastAddr>, LinkLocalAddr<BroadcastAddr>.

        // Unicast
        test_nested!(
            UnicastAddr,
            LinkLocalAddr,
            Unspecified => None,
            GlobalUnicast => None,
            GlobalMulticast => None,
            LinkLocalUnicast => Some(UnicastAddr(LinkLocalAddr(Address::LinkLocalUnicast))),
            LinkLocalMulticast => None,
            LinkLocalBroadcast => None,
        );

        // Multicast
        test_nested!(
            MulticastAddr,
            LinkLocalAddr,
            Unspecified => None,
            GlobalUnicast => None,
            GlobalMulticast => None,
            LinkLocalUnicast => None,
            LinkLocalMulticast => Some(MulticastAddr(LinkLocalAddr(Address::LinkLocalMulticast))),
            LinkLocalBroadcast => None,
        );

        // Broadcast
        test_nested!(
            BroadcastAddr,
            LinkLocalAddr,
            Unspecified => None,
            GlobalUnicast => None,
            GlobalMulticast => None,
            LinkLocalUnicast => None,
            LinkLocalMulticast => None,
            LinkLocalBroadcast => Some(BroadcastAddr(LinkLocalAddr(Address::LinkLocalBroadcast))),
        );

        // Link-local
        test_nested!(
            LinkLocalAddr,
            UnicastAddr,
            Unspecified => None,
            GlobalUnicast => None,
            GlobalMulticast => None,
            LinkLocalUnicast => Some(LinkLocalAddr(UnicastAddr(Address::LinkLocalUnicast))),
            LinkLocalMulticast => None,
            LinkLocalBroadcast => None,
        );
        test_nested!(
            LinkLocalAddr,
            MulticastAddr,
            Unspecified => None,
            GlobalUnicast => None,
            GlobalMulticast => None,
            LinkLocalUnicast => None,
            LinkLocalMulticast => Some(LinkLocalAddr(MulticastAddr(Address::LinkLocalMulticast))),
            LinkLocalBroadcast => None,
        );
        test_nested!(
            LinkLocalAddr,
            BroadcastAddr,
            Unspecified => None,
            GlobalUnicast => None,
            GlobalMulticast => None,
            LinkLocalUnicast => None,
            LinkLocalMulticast => None,
            LinkLocalBroadcast => Some(LinkLocalAddr(BroadcastAddr(Address::LinkLocalBroadcast))),
        );
    }

    #[test]
    fn nested_non_mapped() {
        // Test:
        //   UnicastAddr<NonMappedAddr>, NonMappedAddr<UnicastAddr>,
        //   MulticastAddr<NonMappedAddr>, NonMappedAddr<MulticastAddr>,
        //   BroadcastAddr<NonMappedAddr>, NonMappedAddr<BroadcastAddr>,

        // Unicast
        test_nested!(
            UnicastAddr,
            NonMappedAddr,
            Unspecified => None,
            LinkLocalUnicast => Some(UnicastAddr(NonMappedAddr(Address::LinkLocalUnicast))),
            LinkLocalMulticast => None,
            LinkLocalBroadcast => None,
            MappedUnicast => None,
            MappedMulticast => None,
            MappedBroadcast => None,
        );

        // Multicast
        test_nested!(
            MulticastAddr,
            NonMappedAddr,
            Unspecified => None,
            LinkLocalUnicast => None,
            LinkLocalMulticast => Some(MulticastAddr(NonMappedAddr(Address::LinkLocalMulticast))),
            LinkLocalBroadcast => None,
            MappedUnicast => None,
            MappedMulticast => None,
            MappedBroadcast => None,
        );

        // Broadcast
        test_nested!(
            BroadcastAddr,
            NonMappedAddr,
            Unspecified => None,
            LinkLocalUnicast => None,
            LinkLocalMulticast => None,
            LinkLocalBroadcast => Some(BroadcastAddr(NonMappedAddr(Address::LinkLocalBroadcast))),
            MappedUnicast => None,
            MappedMulticast => None,
            MappedBroadcast => None,
        );

        // non-mapped
        test_nested!(
            NonMappedAddr,
            UnicastAddr,
            Unspecified => None,
            LinkLocalUnicast => Some(NonMappedAddr(UnicastAddr(Address::LinkLocalUnicast))),
            LinkLocalMulticast => None,
            LinkLocalBroadcast => None,
            MappedUnicast => None,
            MappedMulticast => None,
            MappedBroadcast => None,
        );
        test_nested!(
            NonMappedAddr,
            MulticastAddr,
            Unspecified => None,
            LinkLocalUnicast => None,
            LinkLocalMulticast => Some(NonMappedAddr(MulticastAddr(Address::LinkLocalMulticast))),
            LinkLocalBroadcast => None,
            MappedUnicast => None,
            MappedMulticast => None,
            MappedBroadcast => None,
        );
        test_nested!(
            NonMappedAddr,
            BroadcastAddr,
            Unspecified => None,
            LinkLocalUnicast => None,
            LinkLocalMulticast => None,
            LinkLocalBroadcast => Some(NonMappedAddr(BroadcastAddr(Address::LinkLocalBroadcast))),
            MappedUnicast => None,
            MappedMulticast => None,
            MappedBroadcast => None,
        );
    }

    #[test]
    fn addr_and_zone() {
        let addr_and_zone = AddrAndZone::new(Address::LinkLocalUnicast, ());
        assert_eq!(addr_and_zone, Some(AddrAndZone(Address::LinkLocalUnicast, ())));
        assert_eq!(addr_and_zone.unwrap().into_addr_scope_id(), (Address::LinkLocalUnicast, ()));
        assert_eq!(AddrAndZone::new(Address::GlobalUnicast, ()), None);
        assert_eq!(
            unsafe { AddrAndZone::new_unchecked(Address::LinkLocalUnicast, ()) },
            AddrAndZone(Address::LinkLocalUnicast, ())
        );
    }

    #[test]
    fn addr_and_zone_map_zone() {
        let addr_and_zone = AddrAndZone::new(Address::LinkLocalUnicast, 65).unwrap();
        assert_eq!(
            addr_and_zone.map_zone(|x| char::from_u32(x).unwrap()),
            AddrAndZone::new(Address::LinkLocalUnicast, 'A').unwrap()
        );
    }

    #[test]
    fn addr_and_zone_try_map_zone() {
        let addr_and_zone = AddrAndZone::new(Address::LinkLocalUnicast, 32).unwrap();
        assert_eq!(
            addr_and_zone.try_map_zone(|x| Ok::<_, ()>(x + 1)),
            Ok(AddrAndZone::new(Address::LinkLocalUnicast, 33).unwrap())
        );

        let addr_and_zone = AddrAndZone::new(Address::LinkLocalUnicast, 32).unwrap();
        assert_eq!(addr_and_zone.try_map_zone(|x| Err::<i32, _>(x - 1)), Err(31),);
    }

    #[test]
    fn scoped_address() {
        // Type alias to help the compiler when the scope type can't be
        // inferred.
        type ZonedAddr = crate::ZonedAddr<Address, ()>;
        assert_eq!(
            ZonedAddr::new(Address::GlobalUnicast, None),
            Some(ZonedAddr::Unzoned(Address::GlobalUnicast))
        );
        assert_eq!(
            ZonedAddr::new(Address::Unspecified, None).unwrap().into_addr_zone(),
            (Address::Unspecified, None)
        );
        assert_eq!(
            ZonedAddr::new(Address::LinkLocalUnicast, None),
            Some(ZonedAddr::Unzoned(Address::LinkLocalUnicast))
        );
        assert_eq!(ZonedAddr::new(Address::GlobalUnicast, Some(())), None);
        assert_eq!(ZonedAddr::new(Address::Unspecified, Some(())), None);
        assert_eq!(
            ZonedAddr::new(Address::LinkLocalUnicast, Some(())),
            Some(ZonedAddr::Zoned(AddrAndZone(Address::LinkLocalUnicast, ())))
        );

        assert_eq!(
            ZonedAddr::new(Address::GlobalUnicast, None).unwrap().into_addr_zone(),
            (Address::GlobalUnicast, None)
        );
        assert_eq!(
            ZonedAddr::new(Address::LinkLocalUnicast, Some(())).unwrap().into_addr_zone(),
            (Address::LinkLocalUnicast, Some(()))
        );
    }

    #[test]
    fn transpose_with_fully_qualified_types() {
        let addr: SpecifiedAddr<NonMappedAddr<Address>> =
            <NonMappedAddr<SpecifiedAddr<Address>> as Witness<SpecifiedAddr<Address>>>::transpose::<
                Address,
            >(
                NonMappedAddr::new(
                    SpecifiedAddr::new(Address::LinkLocalUnicast)
                        .expect("should be specified addr"),
                )
                .expect("should be non-mapped addr"),
            );
        assert_eq!(
            addr,
            SpecifiedAddr::new(
                NonMappedAddr::new(Address::LinkLocalUnicast).expect("should be non-mapped addr")
            )
            .expect("should be specified addr")
        )
    }

    #[test]
    fn transpose_with_inferred_types() {
        assert_eq!(
            NonMappedAddr::new(
                SpecifiedAddr::new(Address::LinkLocalUnicast).expect("should be specified addr")
            )
            .expect("should be non-mapped addr")
            .transpose(),
            SpecifiedAddr::new(
                NonMappedAddr::new(Address::LinkLocalUnicast).expect("should be non-mapped addr")
            )
            .expect("should be specified addr")
        )
    }
}
