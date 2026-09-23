// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_wlan_tap as fidl_tap;
use std::fmt::{self, Debug, Formatter};
use std::marker::PhantomData;
use wlan_common::channel::Channel;

use crate::event::{Handled, Handler};

pub trait FromEvent<E>: Sized {
    fn from_event(event: &E) -> Option<Self>;
}

impl FromEvent<fidl_tap::SetChannelArgs> for Channel {
    fn from_event(event: &fidl_tap::SetChannelArgs) -> Option<Self> {
        let cbw = wlan_common::channel::Bandwidth::from_fidl(
            event.bandwidth,
            event.vht_secondary_80_channel.number,
        )
        .ok()?;
        Some(Channel { primary: event.primary.number, bandwidth: cbw, band: event.primary.band })
    }
}

impl FromEvent<fidl_tap::SetCountryArgs> for [u8; 2] {
    fn from_event(event: &fidl_tap::SetCountryArgs) -> Option<Self> {
        Some(event.alpha2)
    }
}

impl FromEvent<fidl_tap::TxArgs> for fidl_tap::WlanTxPacket {
    fn from_event(event: &fidl_tap::TxArgs) -> Option<Self> {
        Some(event.packet.clone())
    }
}

/// An event handler that is implemented over combinations of adapters and arbitrary function
/// parameters that are extracted from the given event.
#[repr(transparent)]
pub struct Extract<X, F> {
    f: F,
    phantom: PhantomData<fn() -> X>,
}

impl<X, F> Extract<X, F> {
    fn new(f: F) -> Self {
        Extract { f, phantom: PhantomData }
    }
}

impl<X, F> Clone for Extract<X, F>
where
    F: Clone,
{
    fn clone(&self) -> Self {
        Extract { f: self.f.clone(), phantom: PhantomData }
    }
}

impl<X, F> Debug for Extract<X, F> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extract").field("f", &"{unknown}").finish()
    }
}

impl<X, F> Copy for Extract<X, F> where F: Copy {}

// Unless adapted, handlers normally return a `Handled` that indicates whether or not they have
// matched a given event. However, extractors default to the more common case and match if
// extraction is successful (the composed function does **not** return a `Handled`). For the less
// common case where the matching predicate is more complex, the `extract_and_match` function must
// be used.
#[derive(Clone, Copy, Debug)]
#[repr(transparent)]
pub struct AndMatch<E>(pub E);

// Unless adapted, handlers normally receive state. However, extractors (and most handlers) rarely
// need state. Because extractors take the form of functions with various parameters, they default
// to the more common case and ignore state unless this marker type is used.
/// A marker type that indicates that an extractor's first parameter accepts handler state.
///
/// When using state, the type of the state must be fully qualified and cannot be inferred. For
/// more complex state types, consider using a type definition.
///
/// # Examples
///
/// This type must be used to interact with handler state within an extractor. The following
/// constructs an event handler that runs and matches when a management frame can be extracted from
/// an event and has exclusive access to state when it runs.
///
/// ```rust,ignore
/// let mut handler = event::extract(Stateful(|state: &mut State, frame: Buffered<MgmtFrame>| {
///     /* ... */
/// }));
/// ```
#[derive(Clone, Copy, Debug)]
#[repr(transparent)]
pub struct Stateful<F>(pub F);

/// Constructs an extractor event handler that runs and matches when its parameters can be
/// extracted from an event.
///
/// An extractor executes when the arguments of its composed function can be constructed from an
/// event. This both routes events within an event handler and extracts the necessary data for a
/// handler declaratively without the need for the handler to destructure, convert, nor reference
/// data in an ad-hoc manner.
///
/// # Examples
///
/// The following constructs an event handler that runs and matches when an authentication
/// management frame can be extracted from a transmission event.
///
/// ```rust,ignore
/// let mut handler = event::on_transmit(event::extract(|frame: Buffered<AuthFrame>| {
///     let frame = frame.get();
///     assert_eq!(
///         { frame.auth_hdr.status_code },
///         StatusCode::Success.into(),
///     );
/// }));
/// ```
pub fn extract<S, E, Z, F>(
    f: F,
) -> impl Handler<S, E, Output = <Extract<Z, F> as Handler<S, E>>::Output>
where
    Extract<Z, F>: Handler<S, E>,
{
    let mut extract = Extract::new(f);
    move |state: &mut S, event: &E| extract.call(state, event)
}

/// Constructs an extractor event handler that runs when its parameters can be extracted from an
/// event.
///
/// This function behaves much like [`extract`], but its composed function must return a
/// [`Handled`] and the constructed event handler does not match unless the extraction is
/// successful **and** the composed function indicates a match.
///
/// # Examples
///
/// The following constructs an event handler that runs when a management frame can be extracted
/// from a transmission event and only matches when the management frame subtype is supported.
/// (Note that this differs from extracting `Buffered<Supported<MgmtFrame>>`, as this handler
/// executes for any management frame while that handler would only execute if the frame is
/// supported.)
///
/// ```rust,ignore
/// use Handled::{Matched, Unmatched};
///
/// let mut handler = event::on_transmit(event::extract_and_match(|frame: Buffered<MgmtFrame>| {
///     let frame = frame.get();
///     // ...
///     if MgmtFrame::tag(frame).is_supported() { Matched(()) } else { Unmatched }
/// }));
/// ```
///
/// [`extract`]: crate::event::extract
/// [`Handled`]: crate::event::Handled
pub fn extract_and_match<S, E, Z, F>(
    f: F,
) -> impl Handler<S, E, Output = <AndMatch<Extract<Z, F>> as Handler<S, E>>::Output>
where
    AndMatch<Extract<Z, F>>: Handler<S, E>,
{
    let mut extract = AndMatch(Extract::new(f));
    move |state: &mut S, event: &E| extract.call(state, event)
}

// Invokes another macro with the subsequences of a single tuple parameter (down to a unary tuple).
// That is, given a macro `f` and the starting tuple `(T1, T2)`, this macro invokes `f!((T1, T2))`
// and `f!((T2,))`.
macro_rules! with_tuples {
    ($f:ident$(,)?) => {};
    ($f:ident, ( $head:ident$(,)? )$(,)?) => {
        $f!(($head));
        with_tuples!($f);
    };
    ($f:ident, ( $head:ident,$($tail:ident),* $(,)? )$(,)?) => {
        $f!(($head,$($tail,)*));
        with_tuples!($f, ($($tail,)*));
    };
}
// Implements the `Handler` trait for `Extract` and related types in this module. Matching
// parameters is accomplished via implementations over tuples of the parameter types. Each type
// must implement `FromEvent` and `from_event` is called against the event and forwarded to the
// parameters of the function.
macro_rules! impl_handler_for_extract {
    (( $($t:ident),* $(,)?)$(,)?) => {
        #[allow(non_snake_case)]
        impl<S, E, T, F, $($t,)*> Handler<S, E> for Extract<($($t,)*), F>
        where
            F: FnMut($($t,)*) -> T,
            $(
                $t: FromEvent<E>,
            )*
        {
            type Output = T;

            fn call(&mut self, _state: &mut S, event: &E) -> Handled<Self::Output> {
                match (move || {
                    Some(($(
                        $t::from_event(event)?,
                    )*))
                })() {
                    Some(($($t,)*)) => Handled::Matched((self.f)($($t,)*)),
                    _ => Handled::Unmatched,
                }
            }
        }

        #[allow(non_snake_case)]
        impl<S, E, T, F, $($t,)*> Handler<S, E> for Extract<Stateful<($($t,)*)>, Stateful<F>>
        where
            F: FnMut(&mut S, $($t,)*) -> T,
            $(
                $t: FromEvent<E>,
            )*
        {
            type Output = T;

            fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
                match (move || {
                    Some(($(
                        $t::from_event(event)?,
                    )*))
                })() {
                    Some(($($t,)*)) => Handled::Matched((self.f.0)(state, $($t,)*)),
                    _ => Handled::Unmatched,
                }
            }
        }
        #[allow(non_snake_case)]
        impl<S, E, T, F, $($t,)*> Handler<S, E> for AndMatch<Extract<($($t,)*), F>>
        where
            F: FnMut($($t,)*) -> Handled<T>,
            $(
                $t: FromEvent<E>,
            )*
        {
            type Output = T;

            fn call(&mut self, _state: &mut S, event: &E) -> Handled<Self::Output> {
                match (move || {
                    Some(($(
                        $t::from_event(event)?,
                    )*))
                })() {
                    Some(($($t,)*)) => (self.0.f)($($t,)*),
                    _ => Handled::Unmatched,
                }
            }
        }

        #[allow(non_snake_case)]
        impl<S, E, T, F, $($t,)*> Handler<S, E> for AndMatch<
            Extract<Stateful<($($t,)*)>, Stateful<F>>
        >
        where
            F: FnMut(&mut S, $($t,)*) -> Handled<T>,
            $(
                $t: FromEvent<E>,
            )*
        {
            type Output = T;

            fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
                match (move || {
                    Some(($(
                        $t::from_event(event)?,
                    )*))
                })() {
                    Some(($($t,)*)) => (self.0.f.0)(state, $($t,)*),
                    _ => Handled::Unmatched,
                }
            }
        }
    };
}
with_tuples!(impl_handler_for_extract, (T1, T2, T3, T4, T5, T6));
