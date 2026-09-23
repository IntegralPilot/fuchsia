// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Event handling and composition for interacting with the WLAN tap driver.
//!
//! This module provides APIs for writing tests that must handle and interact with events from the
//! WLAN tap driver. The primary mechanism for this are event handlers, which can be composed in
//! sophisticated ways to route and act upon events. The hardware simulator test harness forwards
//! events into these handlers.
//!
//! Event handlers are described by the [`Handler`] trait. Much like the standard [`Iterator`]
//! trait, [`Handler`] is composable. Event handlers are constructed by composing [`Handler`]s
//! together via combinators. [`Handler`] has many associated functions that are analogous to those
//! found in [`Iterator`], [`Option`], and [`Result`] and follows many of the same patterns. The
//! execution of handlers and routing of events is largely controlled by whether or not handlers
//! match an event: see [`Handled`], which resembles [`Option`].
//!
//! The primary method of the [`Handler`] trait is [`call`][`Handler::call`], which accepts an
//! exclusive reference to state and a shared reference to an event and must react to this event
//! and return a [`Handled`] that describes whether or not the event was matched and, if so, what
//! the output is.
//!
//! # Examples
//!
//! [`Handler`] combinators can be used to construct complex event handlers in a declarative way.
//! The following non-trivial example constructs an event handler that examines the status code of
//! an association response frame. The handler also enforces ordering: action frames may arrive at
//! any time, but any other management frame must follow the association response frame.
//!
//! ```rust,ignore
//! let mut handler = event::on_transmit(branch::or(( // Only handle transmit events.
//!     event::extract(|_: Buffered<ActionFrame<false>>| {}), // Allow (ignore) action frames...
//!     event::until_first_match(branch::or(( // ...or match only once.
//!         event::extract(|frame: Buffered<AssocRespFrame>| { // Examine response frames...
//!             let frame = frame.get();
//!             assert_eq!(
//!                 { frame.assoc_resp_hdr.status_code },
//!                 fidl_ieee80211::StatusCode::Success.into(),
//!             );
//!         })
//!         .and(event::once(|_, _| sender.send(()).unwrap())), // ...and send a completion signal...
//!         event::extract(|_: Buffered<MgmtFrame>| {
//!             panic!("unexpected management frame"); // ...or panic if a management frame arrives instead.
//!         }),
//!     ))),
//! )));
//! ```
//!
//! See existing tests for more examples of constructing event handlers.
//!
//! [`Handled`]: crate::event::Handled
//! [`Handler`]: crate::event::Handler
//! [`Handler::call`]: crate::event::Handler::call
//! [`Iterator`]: core::iter::Iterator
//! [`Option`]: core::option::Option

mod convert;
mod extract;
mod filter;

pub mod action;
pub mod branch;
pub mod buffered;

use anyhow::Context as _;
use std::convert::Infallible;
use std::fmt::{Debug, Display};
use std::marker::PhantomData;
use std::ops::{ControlFlow, RangeBounds};
use std::thread;

pub use crate::event::convert::Try;
pub use crate::event::extract::{extract, extract_and_match, Stateful};
pub use crate::event::filter::{
    on_join_bss, on_scan, on_set_channel, on_set_country, on_start_mac, on_transmit,
};
pub use crate::event::Handled::{Matched, Unmatched};

// The WLAN tap `WlanSoftmacStart` function has no parameters and so, unlike other events, there is
// no corresponding FIDL type to import. This zero-sized type represents this event. For example,
// the bound `Handler<(), StartMacArgs>` represents a handler type that reacts to this event.
#[derive(Clone, Copy, Debug)]
pub struct StartMacArgs;

/// A composable event handler.
///
/// This trait describes types that react to WLAN tap driver events. Such types can be composed in
/// a similar manner to the standard [`Iterator`] trait using adapter and combinator functions.
/// Most `Handler` functions resemble those found in [`Iterator`], [`Option`], and [`Result`].
///
/// The primary method implemented by event handlers is [`call`][`Handler::call`], which receives
/// an exclusive reference to some state (of the input type `S`) and a shared reference to some
/// event (of the input type `E`) and returns a [`Handled`] that indicates whether or not the
/// handler matched the event and, if so, provides some output (of the type
/// [`Output`][`Handler::Output`]).
///
/// The matching of a handler with an event controls the routing and behavior of that handler. In
/// particular, branching combinators like [`and`], [`or`], [`try_and_then`], etc. execute handlers
/// based on whether or not the composed handlers have matched a given event.
///
/// [`and`]: crate::event::Handler::and
/// [`Handled`]: crate::event::Handled
/// [`Handler::call`]: crate::event::Handler::call
/// [`Handler::Output`]: crate::event::Handler::Output
/// [`Iterator`]: core::iter::Iterator
/// [`Option`]: core::option::Option
/// [`or`]: crate::event::Handler::or
/// [`Result`]: core::option::Result
/// [`try_and_then`]: crate::event::Handler::try_and_then
pub trait Handler<S, E> {
    /// The output of the event handler when it is matched.
    type Output;

    /// Reacts to an event with some state.
    ///
    /// Returns a [`Handled`] that indicates whether or not the handler matched the given event.
    ///
    /// [`Handled`]: crate::event::Handled
    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output>;

    /// Maps the output of the event handler using the given function.
    fn map<U, F>(self, f: F) -> Map<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Output) -> U,
    {
        Map { handler: self, f }
    }

    /// Executes the handler followed by the given handler if and only if the event is matched by
    /// the first.
    fn and<H2>(self, handler: H2) -> And<Self, H2>
    where
        Self: Sized,
        H2: Handler<S, E>,
    {
        And { handler: self, and: handler }
    }

    /// Executes the handler followed by the given handler if and only if the event is matched by
    /// the first and the output is not an error.
    ///
    /// As with other `try` functions, this function considers whether or not the handler matched
    /// the event (`Handled::Matched` vs. `Handled::Unmatched`) **and** the output when matched
    /// (output vs. error in `Handled::Matched`). Both `Handled::Unmatched` and `Handled::Matched`
    /// **with an error** value are considered failures to handle an event.
    fn try_and<H2>(self, handler: H2) -> TryAnd<Self, H2>
    where
        Self: Sized,
        Self::Output: Try,
        H2: Handler<S, E>,
        H2::Output: Try<Residual = <Self::Output as Try>::Residual>,
    {
        TryAnd { handler: self, and: handler }
    }

    /// Executes the handler followed by the given function if and only if the event is matched by
    /// the first. The function is given the output of the handler and must return a compatible
    /// handler to execute next.
    fn and_then<H2, F>(self, f: F) -> AndThen<Self, F>
    where
        Self: Sized,
        H2: Handler<S, E>,
        F: FnMut(Self::Output) -> H2,
    {
        AndThen { handler: self, f }
    }

    /// Executes the handler followed by the given function if and only if the event is matched by
    /// the first and the output is not an error. The function is given the output of the handler
    /// and must return a compatible handler to execute next.
    ///
    /// As with other `try` functions, this function considers whether or not the handler matched
    /// the event (`Handled::Matched` vs. `Handled::Unmatched`) **and** the output when matched
    /// (output vs. error in `Handled::Matched`). Both `Handled::Unmatched` and `Handled::Matched`
    /// **with an error** value are considered failures to handle an event.
    fn try_and_then<H2, F>(self, f: F) -> TryAndThen<Self, F>
    where
        Self: Sized,
        Self::Output: Try,
        H2: Handler<S, E>,
        H2::Output: Try<Residual = <Self::Output as Try>::Residual>,
        F: FnMut(<Self::Output as Try>::Output) -> H2,
    {
        TryAndThen { handler: self, f }
    }

    /// Executes the handler followed by the given handler if and only if the event is **not**
    /// matched by the first.
    fn or<H2>(self, handler: H2) -> Or<Self, H2>
    where
        Self: Sized,
        H2: Handler<S, E, Output = Self::Output>,
    {
        Or { handler: self, or: handler }
    }

    /// Executes the handler followed by the given handler if the event is **not** matched by the
    /// first or the output is an error. That is, the given handler is executed only if the first
    /// fails: it does not match the event or it matches **but returns an error**.
    ///
    /// As with other `try` functions, this function considers whether or not the handler matched
    /// the event (`Handled::Matched` vs. `Handled::Unmatched`) **and** the output when matched
    /// (output vs. error in `Handled::Matched`). Both `Handled::Unmatched` and `Handled::Matched`
    /// **with an error** value are considered failures to handle an event.
    fn try_or<H2>(self, handler: H2) -> TryOr<Self, H2>
    where
        Self: Sized,
        Self::Output: Try,
        H2: Handler<S, E, Output = Self::Output>,
    {
        TryOr { handler: self, or: handler }
    }

    /// Combines the handler's output with [`try_or`] over `Unmatched`.
    ///
    /// This adapter terminates a chain of `try` combinators such that the last handler's output is
    /// tried and, if it is unmatched or an error, `Unmatched` is returned.
    fn try_or_unmatched(self) -> TryOrUnmatched<Self>
    where
        Self: Sized,
        Self::Output: Try,
    {
        TryOrUnmatched { handler: self }
    }

    /// Provides context for fallible outputs (like `Result`s).
    fn context<C, O, D>(self, context: C) -> Context<Self, C, O, D>
    where
        Self: Sized,
        Self::Output: anyhow::Context<O, D>,
        C: Clone + Display + Send + Sync + 'static,
    {
        Context { handler: self, context, phantom: PhantomData }
    }

    /// Panics with the given message if a fallible output indicates an error.
    fn expect<M>(self, message: M) -> Expect<Self, M>
    where
        Self: Sized,
        Self::Output: Try,
        M: Display,
    {
        Expect { handler: self, message: message.into() }
    }

    /// Panics if the event handler does not match an event a number of times within the specified
    /// range.
    ///
    /// The panic occurs as eagerly as possible, but may be deferred until the event handler is
    /// dropped if the number of matches is less than the lower bound of the range. Such a panic is
    /// disabled if the thread is already panicking.
    fn expect_matches_times<R>(self, expected: R) -> ExpectMatchesTimes<Self, R>
    where
        Self: Sized,
        R: RangeBounds<usize>,
    {
        ExpectMatchesTimes { handler: self, n: 0, expected }
    }

    /// Borrows the event handler (rather than consuming it).
    ///
    /// This function is analogous to [`Iterator::by_ref`] and can be used to apply adapters and
    /// compositions to an event handler without consuming (moving) it. In particular, this is
    /// useful when code cannot relinquish ownership of a handler and yet must adapt it, such as
    /// methods that operate on a handler field through a reference.
    ///
    /// [`Iterator::by_ref`]: core::iter::Iterator::by_ref
    fn by_ref(&mut self) -> ByRef<'_, Self>
    where
        Self: Sized,
    {
        ByRef { handler: self }
    }
}

impl<F, S, E, U> Handler<S, E> for F
where
    F: FnMut(&mut S, &E) -> Handled<U>,
{
    type Output = U;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        (self)(state, event)
    }
}

impl<'h, S, E, O> Handler<S, E> for &'h mut dyn Handler<S, E, Output = O> {
    type Output = O;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        (**self).call(state, event)
    }
}

impl<'h, S, E, O> Handler<S, E> for Box<dyn Handler<S, E, Output = O> + 'h> {
    type Output = O;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.as_mut().call(state, event)
    }
}

#[derive(Debug)]
pub struct Map<H, F> {
    handler: H,
    f: F,
}

impl<H, S, E, U, F> Handler<S, E> for Map<H, F>
where
    H: Handler<S, E>,
    F: FnMut(H::Output) -> U,
{
    type Output = U;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.handler.call(state, event).map(&mut self.f)
    }
}

#[derive(Debug)]
pub struct And<H1, H2> {
    handler: H1,
    and: H2,
}

impl<H1, S, E, H2> Handler<S, E> for And<H1, H2>
where
    H1: Handler<S, E>,
    H2: Handler<S, E>,
{
    type Output = H2::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.handler.call(state, event).and_then(|_| self.and.call(state, event))
    }
}

#[derive(Debug)]
pub struct TryAnd<H1, H2> {
    handler: H1,
    and: H2,
}

impl<H1, S, E, H2> Handler<S, E> for TryAnd<H1, H2>
where
    H1: Handler<S, E>,
    H1::Output: Try,
    H2: Handler<S, E>,
    H2::Output: Try<Residual = <H1::Output as Try>::Residual>,
{
    type Output = H2::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.handler.call(state, event).try_and_then(|_| self.and.call(state, event))
    }
}

#[derive(Debug)]
pub struct AndThen<H, F> {
    handler: H,
    f: F,
}

impl<H1, S, E, H2, F> Handler<S, E> for AndThen<H1, F>
where
    H1: Handler<S, E>,
    H2: Handler<S, E>,
    F: FnMut(H1::Output) -> H2,
{
    type Output = H2::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.handler.call(state, event).and_then(|output| (self.f)(output).call(state, event))
    }
}

#[derive(Debug)]
pub struct TryAndThen<H, F> {
    handler: H,
    f: F,
}

impl<H1, S, E, H2, F> Handler<S, E> for TryAndThen<H1, F>
where
    H1: Handler<S, E>,
    H1::Output: Try,
    H2: Handler<S, E>,
    H2::Output: Try<Residual = <H1::Output as Try>::Residual>,
    F: FnMut(<H1::Output as Try>::Output) -> H2,
{
    type Output = H2::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.handler.call(state, event).try_and_then(|output| (self.f)(output).call(state, event))
    }
}

#[derive(Debug)]
pub struct Or<H1, H2> {
    handler: H1,
    or: H2,
}

impl<H1, S, E, H2> Handler<S, E> for Or<H1, H2>
where
    H1: Handler<S, E>,
    H2: Handler<S, E, Output = H1::Output>,
{
    type Output = H2::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.handler.call(state, event).or_else(|| self.or.call(state, event))
    }
}

#[derive(Debug)]
pub struct TryOr<H1, H2> {
    handler: H1,
    or: H2,
}

impl<H1, S, E, H2> Handler<S, E> for TryOr<H1, H2>
where
    H1: Handler<S, E>,
    H1::Output: Try,
    H2: Handler<S, E, Output = H1::Output>,
{
    type Output = H2::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.handler.call(state, event).try_or_else(|| self.or.call(state, event))
    }
}

#[derive(Debug)]
pub struct TryOrUnmatched<H> {
    handler: H,
}

impl<H, S, E> Handler<S, E> for TryOrUnmatched<H>
where
    H: Handler<S, E>,
    H::Output: Try,
{
    type Output = H::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.handler.call(state, event).try_or_else(|| Handled::Unmatched)
    }
}

#[derive(Debug)]
pub struct Context<H, C, O, D> {
    handler: H,
    context: C,
    phantom: PhantomData<fn() -> (O, D)>,
}

impl<H, S, E, C, O, D> Handler<S, E> for Context<H, C, O, D>
where
    H: Handler<S, E>,
    H::Output: anyhow::Context<O, D>,
    C: Clone + Display + Send + Sync + 'static,
{
    type Output = Result<O, anyhow::Error>;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        match self.handler.call(state, event) {
            Handled::Matched(output) => Handled::Matched(output.context(self.context.clone())),
            _ => Handled::Unmatched,
        }
    }
}

#[derive(Debug)]
pub struct Expect<H, M> {
    handler: H,
    message: M,
}

impl<H, S, E, M> Handler<S, E> for Expect<H, M>
where
    H: Handler<S, E>,
    H::Output: Try,
    M: Display,
{
    type Output = <H::Output as Try>::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        match self.handler.call(state, event) {
            Handled::Matched(output) => Handled::Matched(output.expect(&self.message)),
            Handled::Unmatched => Handled::Unmatched,
        }
    }
}

#[derive(Debug)]
pub struct ExpectMatchesTimes<H, R>
where
    R: RangeBounds<usize>,
{
    handler: H,
    n: usize,
    expected: R,
}

impl<H, R> ExpectMatchesTimes<H, R>
where
    R: RangeBounds<usize>,
{
    fn assert(&self) {
        assert!(
            self.expected.contains(&self.n),
            "handler called {} time(s); expected {:?}-{:?} time(s)",
            self.n,
            self.expected.start_bound(),
            self.expected.end_bound(),
        );
    }
}

impl<H, R> Drop for ExpectMatchesTimes<H, R>
where
    R: RangeBounds<usize>,
{
    fn drop(&mut self) {
        if !thread::panicking() {
            self.assert();
        }
    }
}

impl<H, S, E, R> Handler<S, E> for ExpectMatchesTimes<H, R>
where
    H: Handler<S, E>,
    R: RangeBounds<usize>,
{
    type Output = H::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        let output = self.handler.call(state, event);
        if output.is_matched() {
            self.n = self.n.saturating_add(1);
            self.assert();
        }
        output
    }
}

pub struct ByRef<'h, H> {
    handler: &'h mut H,
}

impl<'h, H, S, E> Handler<S, E> for ByRef<'h, H>
where
    H: Handler<S, E>,
{
    type Output = H::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.handler.call(state, event)
    }
}

/// The reaction of an event handler to a particular event.
///
/// `Handled` describes whether or not a handler has matched a particular event. When matched, a
/// handler may include an arbitrary output. When unmatched, there is no further output. Whether or
/// not a handler has matched an event can affect the execution of a composite handler and the test
/// harness.
///
/// Note that matching an event has no precise definition: handlers may react arbitrarily to
/// events. In practice, there is little ambiguity, but handlers and adapters can match in
/// particular ways to enable certain handler behaviors. See the [`until_first_match`] adapter for
/// an example.
///
/// [`until_first_match`]: crate::event::until_first_match
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Handled<T> {
    /// An event handler has matched and handled an event.
    Matched(T),
    /// An event handler has **not** matched nor handled an event.
    Unmatched,
}

impl<T> Handled<T> {
    pub fn map<U, F>(self, f: F) -> Handled<U>
    where
        F: FnOnce(T) -> U,
    {
        match self {
            Handled::Matched(inner) => Handled::Matched(f(inner)),
            Handled::Unmatched => Handled::Unmatched,
        }
    }

    pub fn and_then<U, F>(self, f: F) -> Handled<U>
    where
        F: FnOnce(T) -> Handled<U>,
    {
        match self {
            Handled::Matched(inner) => f(inner),
            Handled::Unmatched => Handled::Unmatched,
        }
    }

    pub fn and<U>(self, right: Handled<U>) -> Handled<U> {
        self.and_then(|_| right)
    }

    pub fn try_and_then<U, F>(self, f: F) -> Handled<U>
    where
        T: Try,
        F: FnOnce(T::Output) -> Handled<U>,
    {
        self.and_then(|output| match output.branch() {
            ControlFlow::Continue(output) => f(output),
            ControlFlow::Break(_) => Handled::Unmatched,
        })
    }

    pub fn or_else<F>(self, f: F) -> Self
    where
        F: FnOnce() -> Self,
    {
        match self {
            Handled::Matched(_) => self,
            Handled::Unmatched => f(),
        }
    }

    pub fn or(self, right: Self) -> Self {
        self.or_else(move || right)
    }

    pub fn try_or_else<F>(self, f: F) -> Self
    where
        T: Try,
        F: FnOnce() -> Self,
    {
        match self {
            Handled::Matched(inner) => match inner.branch() {
                ControlFlow::Continue(output) => Handled::Matched(Try::from_output(output)),
                ControlFlow::Break(_) => f(),
            },
            Handled::Unmatched => f(),
        }
    }

    pub fn flatten(self) -> Handled<T::Output>
    where
        T: Try,
    {
        match self {
            Handled::Matched(inner) => match inner.branch() {
                ControlFlow::Continue(output) => Handled::Matched(output),
                ControlFlow::Break(_) => Handled::Unmatched,
            },
            Handled::Unmatched => Handled::Unmatched,
        }
    }

    pub fn matched(self) -> Option<T> {
        match self {
            Handled::Matched(inner) => Some(inner),
            Handled::Unmatched => None,
        }
    }

    pub fn is_matched(&self) -> bool {
        matches!(self, Handled::Matched(_))
    }

    pub fn is_unmatched(&self) -> bool {
        matches!(self, Handled::Unmatched)
    }
}

impl<T> From<T> for Handled<T> {
    fn from(matched: T) -> Self {
        Handled::Matched(matched)
    }
}

impl<T> Try for Handled<T> {
    type Output = T;
    type Residual = Handled<Infallible>;

    fn from_output(output: Self::Output) -> Self {
        Handled::Matched(output)
    }

    fn from_residual(_: Self::Residual) -> Self {
        Handled::Unmatched
    }

    fn branch(self) -> ControlFlow<Self::Residual, Self::Output> {
        match self {
            Handled::Matched(output) => ControlFlow::Continue(output),
            Handled::Unmatched => ControlFlow::Break(Handled::Unmatched),
        }
    }
}

/// Boxes an event handler as a trait object.
///
/// This function can be used for dynamic dispatch and conditionals wherein the `Handler` type
/// parameters and associated types are the same yet the implementing handler types differ. That
/// is, with the exception of the output type, this function erases the handler's type.
pub fn boxed<'h, H, S, E>(handler: H) -> Box<dyn Handler<S, E, Output = H::Output> + 'h>
where
    H: Handler<S, E> + 'h,
{
    Box::new(handler)
}

/// Constructs an event handler that always matches.
///
/// This function can be used to adapt functions into handlers. The output of the accepted function
/// is always wrapped by [`Handled::Matched`], meaning that the framework will always interpret the
/// handler as having matched any event that it receives.
///
/// # Examples
///
/// ```rust,ignore
/// let mut handler = event::on_transmit(event::matched(|_state, _event| {
///     0 // This handler always return `Handled::Matched(0)`.
/// }));
/// ```
///
/// [`Handled::Matched`]: crate::event::Handled::Matched
/// [`Handler`]: crate::event::Handler
pub fn matched<S, E, T, F>(mut f: F) -> impl Handler<S, E, Output = T>
where
    F: FnMut(&mut S, &E) -> T,
{
    move |state: &mut S, event: &E| Handled::Matched(f(state, event))
}

/// Constructs an event handler from a `FnOnce` that only executes once.
///
/// The event handler executes the given function upon receiving its first call and always matches
/// the event. After this occurs, the function has been consumed and the adapter always returns
/// [`Handled::Unmatched`].
///
/// [`Handled::Unmatched`]: crate::event::Handled::Unmatched
/// [`Handler`]: crate::event::Handler
pub fn once<S, E, T, F>(f: F) -> impl Handler<S, E, Output = T>
where
    F: FnOnce(&mut S, &E) -> T,
{
    let mut f = Some(f);
    move |state: &mut S, event: &E| {
        f.take().map_or(Handled::Unmatched, |f| Handled::Matched(f(state, event)))
    }
}

/// Stops executing its composed handler after its first match.
///
/// This function constructs a handler that forwards events to the composed handler until it
/// returns [`Handled::Matched`]. After this occurs, the composed handler is no longer executed and
/// the adapter always returns [`Handled::Unmatched`].
///
/// [`Handled::Matched`]: crate::event::Handled::Matched
/// [`Handled::Unmatched`]: crate::event::Handled::Unmatched
/// [`Handler`]: crate::event::Handler
pub fn until_first_match<H, S, E>(handler: H) -> impl Handler<S, E, Output = H::Output>
where
    H: Handler<S, E>,
{
    let mut handler = Some(handler);
    move |state: &mut S, event: &E| {
        let output = handler.as_mut().map(|handler| handler.call(state, event));
        match output {
            Some(output) => {
                if matches!(output, Handled::Matched(_)) {
                    let _ = handler.take();
                }
                output
            }
            _ => Handled::Unmatched,
        }
    }
}

/// Forwards the given state to the composed event handler.
///
/// This state overrides any state passed to the constructed event handler; only the state given to
/// this function is visible to the composed event handler.
///
/// # Examples
///
/// The following examples constructs an event handler where branches of an event handler have
/// exclusive access to an `isize`. Note that it would not be possible for the closures to capture
/// this state from the environment mutably, as the closures would need to mutably alias the data.
///
/// ```rust,ignore
/// let mut handler = event::with_state(
///     0isize,
///     branch::or((
///         event::extract(Stateful(|count: &mut isize, _: Buffered<MgmtFrame>| {
///             *count += 1;
///             *count
///         })),
///         event::extract(Stateful(|count: &mut isize, _: Buffered<DataFrame>| {
///             *count -= 1;
///             *count
///         })),
///     )),
/// );
/// ```
pub fn with_state<H, S1, S2, E>(
    mut state: S2,
    mut handler: H,
) -> impl Handler<S1, E, Output = H::Output>
where
    H: Handler<S2, E>,
{
    move |_state: &mut S1, event: &E| handler.call(&mut state, event)
}

/// Maps the state passed to the constructed handler and forwards the mapped state to its composed
/// handler.
///
/// This function constructs a handler that maps the state it receives and passes the resulting
/// state to its composed handler. The mapping function can produce arbitrary state for the
/// composed handler.
pub fn map_state<H, S1, S2, E, F>(
    mut f: F,
    mut handler: H,
) -> impl Handler<S1, E, Output = H::Output>
where
    H: Handler<S2, E>,
    F: FnMut(&mut S1) -> S2,
{
    move |s1: &mut S1, event: &E| {
        let mut s2 = f(s1);
        handler.call(&mut s2, event)
    }
}
