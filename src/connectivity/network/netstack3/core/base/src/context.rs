// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common context abstractions.

/// A pair of core and bindings contexts.
///
/// This trait exists so implementers can be agnostic on the storage of the
/// contexts, since all we need from a context pair is mutable references from
/// both contexts.
pub trait ContextPair {
    /// The core context type held by this pair.
    type CoreContext;
    /// The bindings context type held by this pair.
    type BindingsContext;

    /// Gets a mutable reference to both contexts.
    fn contexts(&mut self) -> (&mut Self::CoreContext, &mut Self::BindingsContext);

    /// Gets a mutable reference to the core context.
    fn core_ctx(&mut self) -> &mut Self::CoreContext {
        let (core_ctx, _) = self.contexts();
        core_ctx
    }

    /// Gets a mutable reference to the bindings context.
    fn bindings_ctx(&mut self) -> &mut Self::BindingsContext {
        let (_, bindings_ctx) = self.contexts();
        bindings_ctx
    }
}

impl<'a, C> ContextPair for &'a mut C
where
    C: ContextPair,
{
    type CoreContext = C::CoreContext;
    type BindingsContext = C::BindingsContext;
    fn contexts(&mut self) -> (&mut Self::CoreContext, &mut Self::BindingsContext) {
        C::contexts(self)
    }
}

/// A type that provides a context implementation.
///
/// This trait allows for [`CtxPair`] to hold context implementations
/// agnostically of the storage method and how they're implemented. For example,
/// tests usually create API structs with a mutable borrow to contexts, while
/// the core context exposed to bindings is implemented on an owned [`CoreCtx`]
/// type.
///
/// The shape of this trait is equivalent to [`core::ops::DerefMut`] but we
/// decide against using that because of the automatic dereferencing semantics
/// the compiler provides around implementers of `DerefMut`.
pub trait ContextProvider {
    /// The context provided by this `ContextProvider`.
    type Context: Sized;

    /// Gets a mutable borrow to this context.
    fn context(&mut self) -> &mut Self::Context;
}

impl<'a, T: Sized> ContextProvider for &'a mut T {
    type Context = T;

    fn context(&mut self) -> &mut Self::Context {
        &mut *self
    }
}

/// A concrete implementation of [`ContextPair`].
///
///
/// `CtxPair` provides a [`ContextPair`] implementation when `CC` and `BC` are
/// [`ContextProvider`] and using their respective targets as the `CoreContext`
/// and `BindingsContext` associated types.
#[derive(Clone)]
pub struct CtxPair<CC, BC> {
    /// The core context.
    pub core_ctx: CC,
    /// The bindings context.
    // We put `bindings_ctx` after `core_ctx` to make sure that `core_ctx` is
    // dropped before `bindings_ctx` so that the existence of
    // strongly-referenced device IDs in `bindings_ctx` causes test failures,
    // forcing proper cleanup of device IDs in our unit tests.
    //
    // Note that if strongly-referenced (device) IDs exist when dropping the
    // primary reference, the primary reference's drop impl will panic. See
    // `crate::sync::PrimaryRc::drop` for details.
    // TODO(https://fxbug.dev/320021524): disallow destructuring to actually
    // uphold the intent above.
    pub bindings_ctx: BC,
}

impl<CC, BC> CtxPair<CC, BC> {
    /// Creates a new `CtxPair` with `core_ctx` and a default bindings context.
    pub fn with_core_ctx(core_ctx: CC) -> Self
    where
        BC: Default,
    {
        Self { core_ctx, bindings_ctx: BC::default() }
    }

    /// Creates a new `CtxPair` with a default bindings context and a core
    /// context resulting from `builder`.
    pub fn with_default_bindings_ctx<F: FnOnce(&mut BC) -> CC>(builder: F) -> Self
    where
        BC: Default,
    {
        let mut bindings_ctx = BC::default();
        let core_ctx = builder(&mut bindings_ctx);
        Self { core_ctx, bindings_ctx }
    }

    /// Creates a new `CtxPair` with a mutable borrow of the contexts.
    pub fn as_mut(&mut self) -> CtxPair<&mut CC, &mut BC> {
        let Self { core_ctx, bindings_ctx } = self;
        CtxPair { core_ctx, bindings_ctx }
    }

    /// Creates a new `CtxPair` with a default bindings context and the provided
    /// core context builder.
    pub fn new_with_builder(builder: CC::Builder) -> Self
    where
        CC: BuildableCoreContext<BC>,
        BC: Default,
    {
        Self::with_default_bindings_ctx(|bindings_ctx| CC::build(bindings_ctx, builder))
    }
}

impl<CC, BC> Default for CtxPair<CC, BC>
where
    CC: BuildableCoreContext<BC>,
    CC::Builder: Default,
    BC: Default,
{
    fn default() -> Self {
        Self::new_with_builder(Default::default())
    }
}

impl<CC, BC> ContextPair for CtxPair<CC, BC>
where
    CC: ContextProvider,
    BC: ContextProvider,
{
    type CoreContext = CC::Context;
    type BindingsContext = BC::Context;

    fn contexts(&mut self) -> (&mut Self::CoreContext, &mut Self::BindingsContext) {
        let Self { core_ctx, bindings_ctx } = self;
        (core_ctx.context(), bindings_ctx.context())
    }
}

/// A core context that can be created from some builder type.
///
/// This allows `CtxPair` to define its construction methods away from where
/// core contexts are actually defined.
pub trait BuildableCoreContext<BC> {
    /// The builder type that can build this core context.
    type Builder;

    /// Consumes this builder and returns the context.
    fn build(bindings_ctx: &mut BC, builder: Self::Builder) -> Self;
}

impl<O, BC> BuildableCoreContext<BC> for O
where
    O: Default,
{
    type Builder = ();
    fn build(_bindings_ctx: &mut BC, (): ()) -> Self {
        O::default()
    }
}
