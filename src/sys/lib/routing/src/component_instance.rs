// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bedrock::sandbox_construction::ComponentSandbox;
use crate::error::ComponentInstanceError;
use crate::policy::GlobalPolicyChecker;
use crate::resolving::{ComponentAddress, ComponentResolutionContext, ResolverError};
use async_trait::async_trait;
use capability_source::{BuiltinCapabilities, NamespaceCapabilities};
use cm_rust::offer::{OfferDecl, OfferSource};
use cm_rust::{CapabilityDecl, CollectionDecl, ExposeDecl, UseDecl};
use cm_types::{Name, Url};
use derivative::Derivative;
use moniker::{BorrowedChildName, ChildName, ExtendedMoniker, Moniker};
use runtime_capabilities::{WeakInstanceToken, WeakInstanceTokenAny};
use std::clone::Clone;
use std::sync::{Arc, Weak};

/// A trait providing a representation of a component instance.
#[async_trait]
pub trait ComponentInstanceInterface: Sized + Send + Sync {
    type TopInstance: TopInstanceInterface + Send + Sync;

    /// Returns a new `WeakComponentInstanceInterface<Self>` pointing to `self`.
    fn as_weak(self: &Arc<Self>) -> WeakComponentInstanceInterface<Self> {
        WeakComponentInstanceInterface::new(self)
    }

    /// Returns this `ComponentInstanceInterface`'s child moniker, if it is
    /// not the root instance.
    fn child_moniker(&self) -> Option<&BorrowedChildName> {
        self.moniker().leaf()
    }

    /// Returns this `ComponentInstanceInterface`'s moniker.
    fn moniker(&self) -> &Moniker;

    /// Returns this `ComponentInstanceInterface`'s component URL.
    fn url(&self) -> &Url;

    /// Returns configuration overrides applied to this component by its parent.
    fn config_parent_overrides(&self) -> Option<&[cm_rust::ConfigOverride]>;

    /// Returns the `GlobalPolicyChecker` for this component instance.
    fn policy_checker(&self) -> &GlobalPolicyChecker;

    /// Returns the component ID index for this component instance.
    fn component_id_index(&self) -> &component_id_index::Index;

    /// Gets the parent, if it still exists, or returns an `InstanceNotFound` error.
    fn try_get_parent(&self) -> Result<ExtendedInstanceInterface<Self>, ComponentInstanceError>;

    /// Locks and returns a lazily-resolved and populated
    /// `ResolvedInstanceInterface`.  Returns an `InstanceNotFound` error if the
    /// instance is destroyed. The instance will remain locked until the result
    /// is dropped.
    ///
    /// NOTE: The `Box<dyn>` in the return type is necessary, because the type
    /// of the result depends on the lifetime of the `self` reference. The
    /// proposed "generic associated types" feature would let us define this
    /// statically.
    async fn lock_resolved_state<'a>(
        self: &'a Arc<Self>,
    ) -> Result<Box<dyn ResolvedInstanceInterface<Component = Self> + 'a>, ComponentInstanceError>;

    /// Returns a clone of this component's sandbox. This may resolve the component if necessary.
    async fn component_sandbox(
        self: &Arc<Self>,
    ) -> Result<ComponentSandbox, ComponentInstanceError>;

    /// Attempts to walk the component tree (up and/or down) from the current component to find the
    /// extended instance represented by the given extended moniker. Intermediate components will
    /// be resolved as needed. Functionally this calls into `find_absolute` or `find_above_root`
    /// depending on the extended moniker.
    async fn find_extended_instance(
        self: &Arc<Self>,
        moniker: &ExtendedMoniker,
    ) -> Result<ExtendedInstanceInterface<Self>, ComponentInstanceError> {
        match moniker {
            ExtendedMoniker::ComponentInstance(moniker) => {
                Ok(ExtendedInstanceInterface::Component(self.find_absolute(moniker).await?))
            }
            ExtendedMoniker::ComponentManager => {
                Ok(ExtendedInstanceInterface::AboveRoot(self.find_above_root()?))
            }
        }
    }

    /// Attempts to walk the component tree (up and/or down) from the current component to find the
    /// component instance represented by the given moniker. Intermediate components will be
    /// resolved as needed.
    async fn find_absolute(
        self: &Arc<Self>,
        target_moniker: &Moniker,
    ) -> Result<Arc<Self>, ComponentInstanceError> {
        let mut current = self.clone();
        while !target_moniker.has_prefix(current.moniker()) {
            match current.try_get_parent()? {
                ExtendedInstanceInterface::AboveRoot(_) => panic!(
                    "the current component ({}) must be root, but it's not a prefix for {}",
                    current.moniker(),
                    target_moniker
                ),
                ExtendedInstanceInterface::Component(parent) => current = parent,
            }
        }
        while current.moniker() != target_moniker {
            let remaining_path = target_moniker.strip_prefix(current.moniker()).expect(
                "previous loop will only exit when current.moniker() is a prefix of target_moniker",
            );
            for moniker_part in remaining_path.path() {
                let child = current.lock_resolved_state().await?.get_child(moniker_part).ok_or(
                    ComponentInstanceError::InstanceNotFound {
                        moniker: current.moniker().child(moniker_part.into()),
                    },
                )?;
                current = child;
            }
        }
        Ok(current)
    }

    /// Attempts to walk the component tree up to the above root instance. Intermediate components
    /// will be resolved as needed.
    fn find_above_root(self: &Arc<Self>) -> Result<Arc<Self::TopInstance>, ComponentInstanceError> {
        let mut current = self.clone();
        loop {
            match current.try_get_parent()? {
                ExtendedInstanceInterface::AboveRoot(top_instance) => return Ok(top_instance),
                ExtendedInstanceInterface::Component(parent) => current = parent,
            }
        }
    }
}

/// A trait providing a representation of a resolved component instance.
#[async_trait]
pub trait ResolvedInstanceInterface: Send + Sync {
    /// Type representing a (unlocked and potentially unresolved) component instance.
    type Component;

    /// Current view of this component's `uses` declarations. Implementers are
    /// not required to retain their component declaration and in that case
    /// should return `None`.
    fn try_uses(&self) -> Option<Box<[UseDecl]>>;

    /// Current view of this component's `exposes` declarations. Implementers
    /// are not required to retain their component declaration and in that case
    /// should return `None`.
    fn try_exposes(&self) -> Option<Box<[ExposeDecl]>>;

    /// Current view of this component's `offers` declarations. Does not include
    /// any dynamic offers between children of this component. Implementers are
    /// not required to retain their component declaration and in that case
    /// should return `None`.
    fn try_offers(&self) -> Option<Box<[OfferDecl]>>;

    /// Current view of this component's `capabilities` declarations.
    /// Implementers are not required to retain their component declaration and
    /// in that case should return `None`.
    fn try_capabilities(&self) -> Option<Box<[CapabilityDecl]>>;

    /// Current view of this component's `collections` declarations.
    /// Implementers are not required to retain their component declaration and
    /// in that case should return `None`.
    fn try_collections(&self) -> Option<Box<[CollectionDecl]>>;

    /// Returns a live child of this instance.
    fn get_child(&self, moniker: &BorrowedChildName) -> Option<Arc<Self::Component>>;

    /// Returns a vector of the live children in `collection`.
    fn children_in_collection(&self, collection: &Name) -> Vec<(ChildName, Arc<Self::Component>)>;

    /// Returns the resolver-ready location of the component, which is either
    /// an absolute component URL or a relative path URL with context.
    async fn address(&self) -> Result<ComponentAddress, ResolverError>;

    /// Returns the context to be used to resolve a component from a path
    /// relative to this component (for example, a component in a subpackage).
    /// If `None`, the resolver cannot resolve relative path component URLs.
    fn context_to_resolve_children(&self) -> Option<ComponentResolutionContext>;
}

/// An extension trait providing functionality for any model of a resolved
/// component.
pub trait ResolvedInstanceInterfaceExt: ResolvedInstanceInterface {
    /// Returns true if the given offer source refers to a valid entity, e.g., a
    /// child that exists, a declared collection, etc. However, implementers are
    /// not required to retain their component decl, in which case they should
    /// return `None` if they lack sufficient information to determine if the
    /// offer source is a valid entity.
    fn try_offer_source_exists(&self, source: &OfferSource) -> Option<bool> {
        match source {
            OfferSource::Framework
            | OfferSource::Self_
            | OfferSource::Parent
            | OfferSource::Void => Some(true),
            OfferSource::Child(cm_rust::ChildRef { name, collection }) => {
                let child_moniker = match ChildName::try_new(
                    name.as_str(),
                    collection.as_ref().map(|c| c.as_str()),
                ) {
                    Ok(m) => m,
                    Err(_) => return Some(false),
                };
                Some(self.get_child(&child_moniker).is_some())
            }
            OfferSource::Collection(collection_name) => self
                .try_collections()
                .map(|c| c.iter().any(|collection| collection.name == *collection_name)),
            OfferSource::Capability(capability_name) => self
                .try_capabilities()
                .map(|c| c.iter().any(|capability| capability.name() == capability_name)),
        }
    }
}

impl<T: ResolvedInstanceInterface> ResolvedInstanceInterfaceExt for T {}

// Elsewhere we need to implement `ResolvedInstanceInterface` for `&T` and
// `MappedMutexGuard<_, _, T>`, where `T : ResolvedComponentInstance`. We can't
// implement the latter outside of this crate because of the "orphan rule". So
// here we implement it for all `Deref`s.
#[async_trait]
impl<T> ResolvedInstanceInterface for T
where
    T: std::ops::Deref + Send + Sync,
    T::Target: ResolvedInstanceInterface,
{
    type Component = <T::Target as ResolvedInstanceInterface>::Component;

    fn try_uses(&self) -> Option<Box<[UseDecl]>> {
        T::Target::try_uses(&*self)
    }

    fn try_exposes(&self) -> Option<Box<[ExposeDecl]>> {
        T::Target::try_exposes(&*self)
    }

    fn try_offers(&self) -> Option<Box<[cm_rust::offer::OfferDecl]>> {
        T::Target::try_offers(&*self)
    }

    fn try_capabilities(&self) -> Option<Box<[cm_rust::CapabilityDecl]>> {
        T::Target::try_capabilities(&*self)
    }

    fn try_collections(&self) -> Option<Box<[cm_rust::CollectionDecl]>> {
        T::Target::try_collections(&*self)
    }

    fn get_child(&self, moniker: &BorrowedChildName) -> Option<Arc<Self::Component>> {
        T::Target::get_child(&*self, moniker)
    }

    fn children_in_collection(&self, collection: &Name) -> Vec<(ChildName, Arc<Self::Component>)> {
        T::Target::children_in_collection(&*self, collection)
    }

    async fn address(&self) -> Result<ComponentAddress, ResolverError> {
        T::Target::address(&*self).await
    }

    fn context_to_resolve_children(&self) -> Option<ComponentResolutionContext> {
        T::Target::context_to_resolve_children(&*self)
    }
}

/// A wrapper for a weak reference to a type implementing `ComponentInstanceInterface`. Provides the
/// moniker of the component instance, which is useful for error reporting if the original
/// component instance has been destroyed.
#[derive(Derivative)]
#[derivative(Clone(bound = ""), Default(bound = ""), Debug)]
pub struct WeakComponentInstanceInterface<C: ComponentInstanceInterface> {
    #[derivative(Debug = "ignore")]
    inner: Weak<C>,
    pub moniker: Moniker,
}

impl<C: ComponentInstanceInterface> WeakComponentInstanceInterface<C> {
    pub fn new(component: &Arc<C>) -> Self {
        Self { inner: Arc::downgrade(component), moniker: component.moniker().clone() }
    }

    /// Returns a new weak component instance that will always fail to upgrade.
    pub fn invalid() -> Self {
        Self { inner: Weak::new(), moniker: Moniker::new(&[]) }
    }

    /// Attempts to upgrade this `WeakComponentInterface<C>` into an `Arc<C>`, if the
    /// original component instance interface `C` has not been destroyed.
    pub fn upgrade(&self) -> Result<Arc<C>, ComponentInstanceError> {
        self.inner
            .upgrade()
            .ok_or_else(|| ComponentInstanceError::instance_not_found(self.moniker.clone()))
    }
}

impl<C: ComponentInstanceInterface> From<&Arc<C>> for WeakComponentInstanceInterface<C> {
    fn from(component: &Arc<C>) -> Self {
        Self { inner: Arc::downgrade(component), moniker: component.moniker().clone() }
    }
}

impl<C: ComponentInstanceInterface + 'static> TryFrom<Arc<WeakInstanceToken>>
    for WeakComponentInstanceInterface<C>
{
    type Error = ();

    fn try_from(
        weak_component_token: Arc<WeakInstanceToken>,
    ) -> Result<WeakComponentInstanceInterface<C>, Self::Error> {
        let weak_extended: WeakExtendedInstanceInterface<C> = weak_component_token.try_into()?;
        match weak_extended {
            WeakExtendedInstanceInterface::Component(weak_component) => Ok(weak_component),
            WeakExtendedInstanceInterface::AboveRoot(_) => Err(()),
        }
    }
}

impl<C: ComponentInstanceInterface + 'static> PartialEq for WeakComponentInstanceInterface<C> {
    fn eq(&self, other: &Self) -> bool {
        self.inner.ptr_eq(&other.inner) && self.moniker == other.moniker
    }
}

/// Either a type implementing `ComponentInstanceInterface` or its `TopInstance`.
#[derive(Debug, Clone)]
pub enum ExtendedInstanceInterface<C: ComponentInstanceInterface> {
    Component(Arc<C>),
    AboveRoot(Arc<C::TopInstance>),
}

/// A type implementing `ComponentInstanceInterface` or its `TopInstance`, as a weak pointer.
#[derive(Derivative)]
#[derivative(Clone(bound = ""), Debug(bound = ""))]
pub enum WeakExtendedInstanceInterface<C: ComponentInstanceInterface> {
    Component(WeakComponentInstanceInterface<C>),
    AboveRoot(Weak<C::TopInstance>),
}

impl<C: ComponentInstanceInterface + 'static> WeakInstanceTokenAny
    for WeakExtendedInstanceInterface<C>
{
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl<C: ComponentInstanceInterface> WeakExtendedInstanceInterface<C> {
    /// Attempts to upgrade this `WeakExtendedInstanceInterface<C>` into an
    /// `ExtendedInstanceInterface<C>`, if the original extended instance has not been destroyed.
    pub fn upgrade(&self) -> Result<ExtendedInstanceInterface<C>, ComponentInstanceError> {
        match self {
            WeakExtendedInstanceInterface::Component(p) => {
                Ok(ExtendedInstanceInterface::Component(p.upgrade()?))
            }
            WeakExtendedInstanceInterface::AboveRoot(p) => {
                Ok(ExtendedInstanceInterface::AboveRoot(
                    p.upgrade().ok_or_else(ComponentInstanceError::cm_instance_unavailable)?,
                ))
            }
        }
    }

    pub fn extended_moniker(&self) -> ExtendedMoniker {
        match self {
            Self::Component(p) => ExtendedMoniker::ComponentInstance(p.moniker.clone()),
            Self::AboveRoot(_) => ExtendedMoniker::ComponentManager,
        }
    }
}

impl<C: ComponentInstanceInterface> From<&ExtendedInstanceInterface<C>>
    for WeakExtendedInstanceInterface<C>
{
    fn from(extended: &ExtendedInstanceInterface<C>) -> Self {
        match extended {
            ExtendedInstanceInterface::Component(component) => {
                WeakExtendedInstanceInterface::Component(WeakComponentInstanceInterface::new(
                    component,
                ))
            }
            ExtendedInstanceInterface::AboveRoot(top_instance) => {
                WeakExtendedInstanceInterface::AboveRoot(Arc::downgrade(top_instance))
            }
        }
    }
}

impl<C: ComponentInstanceInterface + 'static> TryFrom<Arc<WeakInstanceToken>>
    for WeakExtendedInstanceInterface<C>
{
    type Error = ();

    fn try_from(
        weak_component_token: Arc<WeakInstanceToken>,
    ) -> Result<WeakExtendedInstanceInterface<C>, Self::Error> {
        weak_component_token
            .inner
            .as_any()
            .downcast_ref::<WeakExtendedInstanceInterface<C>>()
            .cloned()
            .ok_or(())
    }
}

/// A special instance identified with the top of the tree, i.e. component manager's instance.
pub trait TopInstanceInterface: Sized + std::fmt::Debug {
    fn namespace_capabilities(&self) -> &NamespaceCapabilities;

    fn builtin_capabilities(&self) -> &BuiltinCapabilities;
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::bedrock::sandbox_construction::ComponentSandbox;

    #[derive(Debug)]
    pub struct TestTopInstance {}

    impl TopInstanceInterface for TestTopInstance {
        fn namespace_capabilities(&self) -> &NamespaceCapabilities {
            todo!()
        }

        fn builtin_capabilities(&self) -> &BuiltinCapabilities {
            todo!()
        }
    }

    pub struct TestComponent {}

    #[async_trait]
    impl ComponentInstanceInterface for TestComponent {
        type TopInstance = TestTopInstance;

        fn child_moniker(&self) -> Option<&BorrowedChildName> {
            todo!()
        }

        fn moniker(&self) -> &Moniker {
            todo!()
        }

        fn url(&self) -> &Url {
            todo!()
        }

        fn config_parent_overrides(&self) -> Option<&[cm_rust::ConfigOverride]> {
            todo!()
        }

        fn policy_checker(&self) -> &GlobalPolicyChecker {
            todo!()
        }

        fn component_id_index(&self) -> &component_id_index::Index {
            todo!()
        }

        fn try_get_parent(
            &self,
        ) -> Result<ExtendedInstanceInterface<Self>, ComponentInstanceError> {
            todo!()
        }

        async fn lock_resolved_state<'a>(
            self: &'a Arc<Self>,
        ) -> Result<Box<dyn ResolvedInstanceInterface<Component = Self> + 'a>, ComponentInstanceError>
        {
            todo!()
        }

        async fn component_sandbox(
            self: &Arc<Self>,
        ) -> Result<ComponentSandbox, ComponentInstanceError> {
            todo!()
        }
    }
}
