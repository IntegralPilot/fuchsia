// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Chaining branch combinators.
//!
//! This module provides functions for chaining branching [`Handler`] combinators like `try_and`
//! and `or`. These chaining combinators are often more ergonomic when combining more than two
//! handlers and can also improve syntax and readability by more naturally grouping together
//! branches in complex expressions.
//!
//! Chaining is either _static_ or _dynamic_. Static chaining refers to chaining a fixed number of
//! handlers of any types. Dynamic chaining refers to chaining an unknown number of handlers of the
//! same type. The [`boxed`] function can be used to allow different handler types with dynamic
//! chaining via type erasure.
//!
//! Functions in this module accept tuples of handlers for static chaining and `Vec`s of handlers
//! for dynamic chaining. Handlers are always executed in order.
//!
//! [`boxed`]: crate::event::boxed
//! [`Handler`]: crate::event::Handler

use crate::event::convert::Try;
use crate::event::{And, Handled, Handler, Or, TryAnd, TryOr, TryOrUnmatched};

/// A handler and its inputs (state and event) that can be called once.
#[derive(Debug)]
struct BoundHandler<'h, H, S, E> {
    handler: &'h mut H,
    state: &'h mut S,
    event: &'h E,
}

impl<'h, H, S, E> BoundHandler<'h, H, S, E>
where
    H: Handler<S, E>,
{
    pub fn call(self) -> Handled<H::Output> {
        self.handler.call(self.state, self.event)
    }
}

/// Provides a dynamic chaining implementation that is agnostic to the specific combinator used in
/// the chain.
trait DynamicChain<H> {
    fn chain<S, E, F>(&mut self, state: &mut S, event: &E, f: F) -> Handled<H::Output>
    where
        H: Handler<S, E>,
        F: FnMut(Handled<H::Output>, BoundHandler<'_, H, S, E>) -> Handled<H::Output>;
}

impl<H> DynamicChain<H> for [H] {
    /// Chains (folds) the handlers in the slice.
    ///
    /// The given function receives the aggregated `Handled` and a bound handler and can choose to
    /// call the handler or not. This function determines which `Handled` combinator to use, such
    /// as `Handled::try_and_then`, etc.
    fn chain<S, E, F>(&mut self, state: &mut S, event: &E, mut f: F) -> Handled<H::Output>
    where
        H: Handler<S, E>,
        F: FnMut(Handled<H::Output>, BoundHandler<'_, H, S, E>) -> Handled<H::Output>,
    {
        self.iter_mut()
            .fold(None, |chain: Option<Handled<_>>, handler| {
                if let Some(handled) = chain {
                    Some(f(handled, BoundHandler { handler, state, event }))
                } else {
                    Some(handler.call(state, event))
                }
            })
            .unwrap_or(Handled::Unmatched)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DynamicAnd<T>(T);

impl<S, E, H> Handler<S, E> for DynamicAnd<Vec<H>>
where
    H: Handler<S, E>,
{
    type Output = H::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.0
            .as_mut_slice()
            .chain(state, event, |previous, next| previous.and_then(|_| next.call()))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DynamicTryAnd<T>(T);

impl<S, E, H> Handler<S, E> for DynamicTryAnd<Vec<H>>
where
    H: Handler<S, E>,
    H::Output: Try,
{
    type Output = H::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.0
            .as_mut_slice()
            .chain(state, event, |previous, next| previous.try_and_then(|_| next.call()))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DynamicOr<T>(T);

impl<S, E, H> Handler<S, E> for DynamicOr<Vec<H>>
where
    H: Handler<S, E>,
{
    type Output = H::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.0.as_mut_slice().chain(state, event, |previous, next| previous.or_else(|| next.call()))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DynamicTryOr<T>(T);

impl<S, E, H> Handler<S, E> for DynamicTryOr<Vec<H>>
where
    H: Handler<S, E>,
    H::Output: Try,
{
    type Output = H::Output;

    fn call(&mut self, state: &mut S, event: &E) -> Handled<Self::Output> {
        self.0
            .as_mut_slice()
            .chain(state, event, |previous, next| previous.try_or_else(|| next.call()))
            // NOTE: Terminate the chain. This prevents the last handler in the chain from
            //       returning `Matched` with an error output. Instead, the last handler is tried
            //       and `Unmatched` is returned if the output is an error.
            .try_or_else(|| Handled::Unmatched)
    }
}

/// Types that describe a sequence of event handlers that can be combined via `and`.
pub trait AndChain<S, E> {
    type Combinator: Handler<S, E>;

    fn chain(self) -> Self::Combinator;
}

impl<S, E, H> AndChain<S, E> for Vec<H>
where
    H: Handler<S, E>,
{
    type Combinator = DynamicAnd<Vec<H>>;

    fn chain(self) -> Self::Combinator {
        DynamicAnd(self)
    }
}

/// Types that describe a sequence of event handlers that can be combined via `try_and`.
pub trait TryAndChain<S, E> {
    type Combinator: Handler<S, E>;

    fn chain(self) -> Self::Combinator;
}

impl<S, E, H> TryAndChain<S, E> for Vec<H>
where
    H: Handler<S, E>,
    H::Output: Try,
{
    type Combinator = DynamicTryAnd<Vec<H>>;

    fn chain(self) -> Self::Combinator {
        DynamicTryAnd(self)
    }
}

/// Types that describe a sequence of event handlers that can be combined via `or`.
pub trait OrChain<S, E> {
    type Combinator: Handler<S, E>;

    fn chain(self) -> Self::Combinator;
}

impl<S, E, H> OrChain<S, E> for Vec<H>
where
    H: Handler<S, E>,
{
    type Combinator = DynamicOr<Vec<H>>;

    fn chain(self) -> Self::Combinator {
        DynamicOr(self)
    }
}

/// Types that describe a sequence of event handlers that can be combined via `try_or`.
pub trait TryOrChain<S, E> {
    type Combinator: Handler<S, E>;

    fn chain(self) -> Self::Combinator;
}

impl<S, E, H> TryOrChain<S, E> for Vec<H>
where
    H: Handler<S, E>,
    H::Output: Try,
{
    type Combinator = DynamicTryOr<Vec<H>>;

    fn chain(self) -> Self::Combinator {
        DynamicTryOr(self)
    }
}

/// Executes the handlers in a tuple or `Vec` in order until a handler does **not** match the
/// event. If all handlers match, then the output of the last handler in the sequence is returned,
/// otherwise `Handled::Unmatched`.
pub fn and<S, E, T>(
    handlers: T,
) -> impl Handler<S, E, Output = <T::Combinator as Handler<S, E>>::Output>
where
    T: AndChain<S, E>,
{
    handlers.chain()
}

/// Executes the handlers in a tuple or `Vec` in order until a handler does **not** match the event
/// or returns an error. If a handler matches but returns an error, the error is returned. If all
/// handlers match and no handlers return an error, then the output of the last handler in the
/// sequence is returned, otherwise `Handled::Unmatched`.
///
/// The output type must implement `Try` and the residual of the output must be the same for all
/// handlers (the error types must be compatible).
pub fn try_and<S, E, T>(
    handlers: T,
) -> impl Handler<S, E, Output = <T::Combinator as Handler<S, E>>::Output>
where
    T: TryAndChain<S, E>,
{
    handlers.chain()
}

/// Executes the handlers in a tuple or `Vec` in order until a handler matches the event. If any
/// handler matches, then the output of that handler is returned, otherwise `Handled::Unmatched`.
///
/// The output type must be the same for all handlers.
pub fn or<S, E, T>(
    handlers: T,
) -> impl Handler<S, E, Output = <T::Combinator as Handler<S, E>>::Output>
where
    T: OrChain<S, E>,
{
    handlers.chain()
}

/// Executes the handlers in a tuple or `Vec` in order until a handler matches the event and does
/// **not** return an error. If any handler matches and returns a **non**-error, then the output of
/// that handler is returned, otherwise `Handled::Unmatched`.
///
/// **This is subtly different from chaining calls to [`Handler::try_or`]**: the chain is
/// terminated by [`Handler:try_or_unmatched`] (or an equivalent adapter), which tries the output
/// of the last handler in the combinator chain and returns `Unmatched` if the output is an error.
///
/// The output type must implement `Try` and be the same for all handlers.
pub fn try_or<S, E, T>(
    handlers: T,
) -> impl Handler<S, E, Output = <T::Combinator as Handler<S, E>>::Output>
where
    T: TryOrChain<S, E>,
{
    handlers.chain()
}

// Invokes another macro with the non-unary subsequences of a single tuple parameter (down to a
// binary tuple). That is, given a macro `f` and the starting tuple `(T1, T2, T3)`, this macro
// invokes `f!((T1, T2, T3))` and `f!((T2, T3))`. Note that in this example `f!((T3,))` is **not**
// invoked, as `(T3,)` is a unary tuple.
macro_rules! with_nonunary_tuples {
    ($f:ident, ( $head:ident,$tail:ident$(,)? )$(,)?) => {
        $f!(($head, $tail));
    };
    ($f:ident, ( $head:ident,$body:ident,$($tail:ident),+$(,)? )$(,)?) => {
        $f!(($head,$body,$($tail,)*));
        with_nonunary_tuples!($f, ( $body,$($tail,)+ ));
    };
}
// Reverses the input tuple and then generates the nested type names of combinators. Note that it
// is not possible for a macro to match and pop off elements from the end of a repeated sequence,
// so the input tuple must first be reversed (to effectively pop elements from the end rather than
// the front).
macro_rules! forward_static_combinator_chain_output {
    ($combinator:ident, ( $($forward:ident),*$(,)? )) => {
        forward_static_combinator_chain_output!($combinator, ($($forward,)*); ())
    };
    ($combinator:ident, ( $head:ident,$($tail:ident),*$(,)? ); ( $($reverse:ident),*$(,)? )) => {
        forward_static_combinator_chain_output!($combinator, ($($tail,)*); ($head,$($reverse,)*))
    };
    // Base case: the tuple is reversed and can be used to invoke
    // `reverse_static_combinator_chain_output`.
    ($combinator:ident, ( $(,)? ); ( $($reverse:ident),*$(,)? )) => {
        reverse_static_combinator_chain_output!($combinator, ( $($reverse,)* ))
    };
}
// Generates the nested type names of combinators. The input handler types must be in reverse
// order. Given the combinator `Or` and the tuple `(C, B, A)`, this macro outputs the identifier
// `Or<Or<A, B>, C>`, which is the name of the combinator when combining the handlers
// `A.or(B).or(C)`.
macro_rules! reverse_static_combinator_chain_output {
    ($combinator:ident, ( $head:ident,$body:ident,$($tail:ident,)+ )$(,)?) => {
        $combinator<reverse_static_combinator_chain_output!($combinator, ($body,$($tail,)+)), $head>
    };
    ($combinator:ident, ( $head:ident,$tail:ident$(,)? )$(,)?) => {
        $combinator<$tail, $head>
    };
}
// Implements the traits for static combinator chains. These traits are implemented for tuples of
// handler types and effectively chain calls to their corresponding combinator. For example,
// `AndChain` calls the `and` combinator against its handler tuple elements in order such that
// given the tuple `(A, B, C)` it returns `A.and(B).and(C)`.
macro_rules! impl_static_combinator_chain {
    (( $head:ident,$($tail:ident),+$(,)? )$(,)?) => {
        impl<S, E, $head, $($tail,)+> AndChain<S, E> for ($head, $($tail,)+)
        where
            $head: Handler<S, E>,
            $(
                $tail: Handler<S, E>,
            )+
        {
            type Combinator = forward_static_combinator_chain_output!(And, ($head, $($tail,)+));

            #[allow(non_snake_case)]
            fn chain(self) -> Self::Combinator {
                let ($head, $($tail,)+) = self;
                $head
                $(
                    .and($tail)
                )+
            }
        }

        impl<S, E, R, $head, $($tail,)+> TryAndChain<S, E> for ($head, $($tail,)+)
        where
            $head: Handler<S, E>,
            $head::Output: Try<Residual = R>,
            $(
                $tail: Handler<S, E>,
                $tail::Output: Try<Residual = R>,
            )+
        {
            type Combinator = forward_static_combinator_chain_output!(TryAnd, ($head, $($tail,)+));

            #[allow(non_snake_case)]
            fn chain(self) -> Self::Combinator {
                let ($head, $($tail,)+) = self;
                $head
                $(
                    .try_and($tail)
                )+
            }
        }

        impl<S, E, O, $head, $($tail,)+> OrChain<S, E> for ($head, $($tail,)+)
        where
            $head: Handler<S, E, Output = O>,
            $(
                $tail: Handler<S, E, Output = O>,
            )+
        {
            type Combinator = forward_static_combinator_chain_output!(Or, ($head, $($tail,)+));

            #[allow(non_snake_case)]
            fn chain(self) -> Self::Combinator {
                let ($head, $($tail,)+) = self;
                $head
                $(
                    .or($tail)
                )+
            }
        }

        impl<S, E, O, $head, $($tail,)+> TryOrChain<S, E> for ($head, $($tail,)+)
        where
            O: Try,
            $head: Handler<S, E, Output = O>,
            $(
                $tail: Handler<S, E, Output = O>,
            )+
        {
            type Combinator = TryOrUnmatched<
                forward_static_combinator_chain_output!(TryOr, ($head, $($tail,)+))
            >;

            #[allow(non_snake_case)]
            fn chain(self) -> Self::Combinator {
                let ($head, $($tail,)+) = self;
                $head
                $(
                    .try_or($tail)
                )+
                // NOTE: Terminate the chain. This prevents the last handler in the chain from
                //       returning `Matched` with an error output. Instead, the last handler is
                //       tried and `Unmatched` is returned if the output is an error.
                .try_or_unmatched()
            }
        }
    };
}
with_nonunary_tuples!(impl_static_combinator_chain, (T1, T2, T3, T4, T5, T6, T7, T8));

#[cfg(test)]
mod tests {
    use crate::event::{self, branch, Handled, Handler};

    /// Constructs a handler that discards output and never matches.
    fn unmatched<S, E, T, F>(mut f: F) -> impl Handler<S, E, Output = T>
    where
        F: FnMut(&mut S, &E) -> T,
    {
        move |state: &mut S, event: &E| {
            let _ = f(state, event);
            Handled::Unmatched
        }
    }

    /// Curries a function that is compatible with handler adapters from a parameterless function
    /// that ignores state and events and provides only output.
    fn output<S, E, T, F>(mut f: F) -> impl FnMut(&mut S, &E) -> T
    where
        F: FnMut() -> T,
    {
        move |_, _| f()
    }

    /// Curries a function that is compatible with handler adapters from a parameterless function
    /// that ignores state and events and provides only output. The curried function pushes a
    /// breadcrumb into its `Vec` state each time it is called.
    fn breadcrumb<E, T, B, F>(crumb: B, mut f: F) -> impl FnMut(&mut Vec<B>, &E) -> T
    where
        B: Copy,
        F: FnMut() -> T,
    {
        move |breadcrumbs: &mut Vec<_>, _| {
            breadcrumbs.push(crumb);
            f()
        }
    }

    const fn unit() {}

    const fn some() -> Option<()> {
        Some(())
    }

    const fn none() -> Option<()> {
        None
    }

    #[test]
    fn static_and_executes_in_order() {
        let mut breadcrumbs = vec![];
        branch::and((
            event::matched(breadcrumb(0, unit)),
            event::matched(breadcrumb(1, unit)),
            event::matched(breadcrumb(2, unit)),
            event::matched(breadcrumb(3, unit)),
            event::matched(breadcrumb(4, unit)),
        ))
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn static_and_executes_until_unmatched() {
        let mut breadcrumbs = vec![];
        branch::and((
            event::matched(breadcrumb(0, unit)),
            unmatched(breadcrumb(1, unit)),
            event::matched(breadcrumb(2, unit)),
        ))
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1]);
    }

    #[test]
    fn static_and_matched_if_all_matched() {
        let handled = branch::and((
            event::matched(output(unit)),
            event::matched(output(unit)),
            event::matched(output(unit)),
        ))
        .call(&mut (), &());
        assert!(handled.is_matched());
    }

    #[test]
    fn static_and_unmatched_if_any_unmatched() {
        let handled = branch::and((
            event::matched(output(unit)),
            unmatched(output(unit)),
            event::matched(output(unit)),
        ))
        .call(&mut (), &());
        assert!(handled.is_unmatched());
    }

    #[test]
    fn dynamic_and_executes_in_order() {
        let mut breadcrumbs = vec![];
        branch::and(vec![
            event::boxed(event::matched(breadcrumb(0, unit))),
            event::boxed(event::matched(breadcrumb(1, unit))),
            event::boxed(event::matched(breadcrumb(2, unit))),
            event::boxed(event::matched(breadcrumb(3, unit))),
            event::boxed(event::matched(breadcrumb(4, unit))),
        ])
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn dynamic_and_executes_until_unmatched() {
        let mut breadcrumbs = vec![];
        branch::and(vec![
            event::boxed(event::matched(breadcrumb(0, unit))),
            event::boxed(unmatched(breadcrumb(1, unit))),
            event::boxed(event::matched(breadcrumb(2, unit))),
        ])
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1]);
    }

    #[test]
    fn dynamic_and_matched_if_all_matched() {
        let handled = branch::and(vec![
            event::boxed(event::matched(output(unit))),
            event::boxed(event::matched(output(unit))),
            event::boxed(event::matched(output(unit))),
        ])
        .call(&mut (), &());
        assert!(handled.is_matched());
    }

    #[test]
    fn dynamic_and_unmatched_if_any_unmatched() {
        let handled = branch::and(vec![
            event::boxed(event::matched(output(unit))),
            event::boxed(unmatched(output(unit))),
            event::boxed(event::matched(output(unit))),
        ])
        .call(&mut (), &());
        assert!(handled.is_unmatched());
    }

    #[test]
    fn static_try_and_executes_in_order() {
        let mut breadcrumbs = vec![];
        branch::try_and((
            event::matched(breadcrumb(0, some)),
            event::matched(breadcrumb(1, some)),
            event::matched(breadcrumb(2, some)),
            event::matched(breadcrumb(3, some)),
            event::matched(breadcrumb(4, some)),
        ))
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn static_try_and_executes_until_unmatched() {
        let mut breadcrumbs = vec![];
        branch::try_and((
            event::matched(breadcrumb(0, some)),
            unmatched(breadcrumb(1, some)),
            event::matched(breadcrumb(2, some)),
        ))
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1]);
    }

    #[test]
    fn static_try_and_executes_until_matched_residual() {
        let mut breadcrumbs = vec![];
        branch::try_and((
            event::matched(breadcrumb(0, some)),
            event::matched(breadcrumb(1, none)),
            event::matched(breadcrumb(2, some)),
        ))
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1]);
    }

    #[test]
    fn static_try_and_matched_if_all_matched_output() {
        let handled = branch::try_and((
            event::matched(output(some)),
            event::matched(output(some)),
            event::matched(output(some)),
        ))
        .call(&mut (), &());
        assert!(handled.is_matched());
    }

    #[test]
    fn static_try_and_unmatched_if_any_unmatched() {
        let handled = branch::try_and((
            event::matched(output(some)),
            unmatched(output(some)),
            event::matched(output(some)),
        ))
        .call(&mut (), &());
        assert!(handled.is_unmatched());
    }

    #[test]
    fn static_try_and_unmatched_if_any_matched_residual() {
        let handled = branch::try_and((
            event::matched(output(some)),
            event::matched(output(none)),
            event::matched(output(some)),
        ))
        .call(&mut (), &());
        assert!(handled.is_unmatched());
    }

    #[test]
    fn dynamic_try_and_executes_in_order() {
        let mut breadcrumbs = vec![];
        branch::try_and(vec![
            event::boxed(event::matched(breadcrumb(0, some))),
            event::boxed(event::matched(breadcrumb(1, some))),
            event::boxed(event::matched(breadcrumb(2, some))),
            event::boxed(event::matched(breadcrumb(3, some))),
            event::boxed(event::matched(breadcrumb(4, some))),
        ])
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn dynamic_try_and_executes_until_unmatched() {
        let mut breadcrumbs = vec![];
        branch::try_and(vec![
            event::boxed(event::matched(breadcrumb(0, some))),
            event::boxed(unmatched(breadcrumb(1, some))),
            event::boxed(event::matched(breadcrumb(2, some))),
        ])
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1]);
    }

    #[test]
    fn dynamic_try_and_executes_until_matched_residual() {
        let mut breadcrumbs = vec![];
        branch::try_and(vec![
            event::boxed(event::matched(breadcrumb(0, some))),
            event::boxed(event::matched(breadcrumb(1, none))),
            event::boxed(event::matched(breadcrumb(2, some))),
        ])
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1]);
    }

    #[test]
    fn dynamic_try_and_matched_if_all_matched_output() {
        let handled = branch::try_and(vec![
            event::boxed(event::matched(output(some))),
            event::boxed(event::matched(output(some))),
            event::boxed(event::matched(output(some))),
        ])
        .call(&mut (), &());
        assert!(handled.is_matched());
    }

    #[test]
    fn dynamic_try_and_unmatched_if_any_unmatched() {
        let handled = branch::try_and(vec![
            event::boxed(event::matched(output(some))),
            event::boxed(unmatched(output(some))),
            event::boxed(event::matched(output(some))),
        ])
        .call(&mut (), &());
        assert!(handled.is_unmatched());
    }

    #[test]
    fn dynamic_try_and_unmatched_if_any_matched_residual() {
        let handled = branch::try_and(vec![
            event::boxed(event::matched(output(some))),
            event::boxed(event::matched(output(none))),
            event::boxed(event::matched(output(some))),
        ])
        .call(&mut (), &());
        assert!(handled.is_unmatched());
    }

    #[test]
    fn static_or_executes_in_order() {
        let mut breadcrumbs = vec![];
        branch::or((
            unmatched(breadcrumb(0, unit)),
            unmatched(breadcrumb(1, unit)),
            unmatched(breadcrumb(2, unit)),
            unmatched(breadcrumb(3, unit)),
            unmatched(breadcrumb(4, unit)),
        ))
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn static_or_executes_until_matched() {
        let mut breadcrumbs = vec![];
        branch::or((
            unmatched(breadcrumb(0, unit)),
            event::matched(breadcrumb(1, unit)),
            unmatched(breadcrumb(2, unit)),
        ))
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1]);
    }

    #[test]
    fn static_or_matched_if_any_matched() {
        let handled = branch::or((
            unmatched(output(unit)),
            unmatched(output(unit)),
            event::matched(output(unit)),
        ))
        .call(&mut (), &());
        assert!(handled.is_matched());
    }

    #[test]
    fn static_or_unmatched_if_all_unmatched() {
        let handled =
            branch::or((unmatched(output(unit)), unmatched(output(unit)), unmatched(output(unit))))
                .call(&mut (), &());
        assert!(handled.is_unmatched());
    }

    #[test]
    fn dynamic_or_executes_in_order() {
        let mut breadcrumbs = vec![];
        branch::or(vec![
            event::boxed(unmatched(breadcrumb(0, unit))),
            event::boxed(unmatched(breadcrumb(1, unit))),
            event::boxed(unmatched(breadcrumb(2, unit))),
            event::boxed(unmatched(breadcrumb(3, unit))),
            event::boxed(unmatched(breadcrumb(4, unit))),
        ])
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn dynamic_or_executes_until_matched() {
        let mut breadcrumbs = vec![];
        branch::or(vec![
            event::boxed(unmatched(breadcrumb(0, unit))),
            event::boxed(event::matched(breadcrumb(1, unit))),
            event::boxed(unmatched(breadcrumb(2, unit))),
        ])
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1]);
    }

    #[test]
    fn dynamic_or_matched_if_any_matched() {
        let handled = branch::or(vec![
            event::boxed(unmatched(output(unit))),
            event::boxed(unmatched(output(unit))),
            event::boxed(event::matched(output(unit))),
        ])
        .call(&mut (), &());
        assert!(handled.is_matched());
    }

    #[test]
    fn dynamic_or_unmatched_if_all_unmatched() {
        let handled = branch::or(vec![
            event::boxed(unmatched(output(unit))),
            event::boxed(unmatched(output(unit))),
            event::boxed(unmatched(output(unit))),
        ])
        .call(&mut (), &());
        assert!(handled.is_unmatched());
    }

    #[test]
    fn static_try_or_executes_in_order() {
        let mut breadcrumbs = vec![];
        branch::try_or((
            unmatched(breadcrumb(0, some)),
            unmatched(breadcrumb(1, some)),
            unmatched(breadcrumb(2, some)),
        ))
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1, 2]);
    }

    #[test]
    fn static_try_or_executes_until_matched_output() {
        let mut breadcrumbs = vec![];
        branch::try_or((
            unmatched(breadcrumb(0, some)),
            event::matched(breadcrumb(1, some)),
            unmatched(breadcrumb(2, some)),
        ))
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1]);
    }

    #[test]
    fn static_try_or_executes_through_matched_residual() {
        let mut breadcrumbs = vec![];
        branch::try_or((
            unmatched(breadcrumb(0, some)),
            event::matched(breadcrumb(1, none)),
            event::matched(breadcrumb(2, none)),
        ))
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1, 2]);
    }

    #[test]
    fn static_try_or_matched_if_any_matched_output() {
        let handled = branch::try_or((
            unmatched(output(some)),
            unmatched(output(some)),
            event::matched(output(some)),
        ))
        .call(&mut (), &());
        assert!(handled.is_matched());
    }

    #[test]
    fn static_try_or_unmatched_if_all_unmatched_or_matched_residual() {
        let handled = branch::try_or((
            unmatched(output(some)),
            unmatched(output(some)),
            unmatched(output(some)),
        ))
        .call(&mut (), &());
        assert!(handled.is_unmatched());

        let handled = branch::try_or((
            event::matched(output(none)),
            event::matched(output(none)),
            event::matched(output(none)),
        ))
        .call(&mut (), &());
        assert!(handled.is_unmatched());

        let handled = branch::try_or((
            event::matched(output(none)),
            unmatched(output(some)),
            unmatched(output(some)),
        ))
        .call(&mut (), &());
        assert!(handled.is_unmatched());
    }

    #[test]
    fn dynamic_try_or_executes_in_order() {
        let mut breadcrumbs = vec![];
        branch::try_or(vec![
            event::boxed(unmatched(breadcrumb(0, some))),
            event::boxed(unmatched(breadcrumb(1, some))),
            event::boxed(unmatched(breadcrumb(2, some))),
            event::boxed(unmatched(breadcrumb(3, some))),
            event::boxed(unmatched(breadcrumb(4, some))),
        ])
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn dynamic_try_or_executes_until_matched_output() {
        let mut breadcrumbs = vec![];
        branch::try_or(vec![
            event::boxed(unmatched(breadcrumb(0, some))),
            event::boxed(event::matched(breadcrumb(1, some))),
            event::boxed(unmatched(breadcrumb(2, some))),
        ])
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1]);
    }

    #[test]
    fn dynamic_try_or_executes_through_matched_residual() {
        let mut breadcrumbs = vec![];
        branch::try_or(vec![
            event::boxed(unmatched(breadcrumb(0, some))),
            event::boxed(event::matched(breadcrumb(1, none))),
            event::boxed(event::matched(breadcrumb(2, none))),
        ])
        .call(&mut breadcrumbs, &());
        assert_eq!(breadcrumbs, &[0, 1, 2]);
    }

    #[test]
    fn dynamic_try_or_matched_if_any_matched_output() {
        let handled = branch::try_or(vec![
            event::boxed(unmatched(output(some))),
            event::boxed(unmatched(output(some))),
            event::boxed(event::matched(output(some))),
        ])
        .call(&mut (), &());
        assert!(handled.is_matched());
    }

    #[test]
    fn dynamic_try_or_unmatched_if_all_unmatched_or_matched_residual() {
        let handled = branch::try_or(vec![
            event::boxed(unmatched(output(some))),
            event::boxed(unmatched(output(some))),
            event::boxed(unmatched(output(some))),
        ])
        .call(&mut (), &());
        assert!(handled.is_unmatched());

        let handled = branch::try_or(vec![
            event::boxed(event::matched(output(none))),
            event::boxed(event::matched(output(none))),
            event::boxed(event::matched(output(none))),
        ])
        .call(&mut (), &());
        assert!(handled.is_unmatched());

        let handled = branch::try_or(vec![
            event::boxed(event::matched(output(none))),
            event::boxed(unmatched(output(some))),
            event::boxed(unmatched(output(some))),
        ])
        .call(&mut (), &());
        assert!(handled.is_unmatched());
    }
}
