// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::DictExt;
use cm_types::{BorrowedName, IterablePath, Name};
use runtime_capabilities::{Capability, Data, Dictionary};
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, LazyLock};

/// This trait is implemented by types that wrap a [Dictionary] and wish to present an abstracted
/// interface over the [Dictionary].
///
/// All such types are defined in this module, so this trait is private.
///
/// See also: [StructuredDictMap]
trait StructuredDict: Into<Arc<Dictionary>> + Default + Clone + fmt::Debug {
    /// Converts from [Dictionary] to `Self`.
    ///
    /// REQUIRES: [Dictionary] is a valid representation of `Self`.
    ///
    /// IMPORTANT: The caller should know that [Dictionary] is a valid representation of [Self].
    /// This function is not guaranteed to perform any validation.
    fn from_dict(dict: Arc<Dictionary>) -> Self;
}

/// A collection type for mapping [Name] to [StructuredDict], using [Dictionary] as the underlying
/// representation.
///
/// For example, this can be used to store a map of child or collection names to [ComponentInput]s
/// (where [ComponentInput] is the type that implements [StructuredDict]).
///
/// Because the representation of this type is [Dictionary], this type itself implements
/// [StructuredDict].
#[derive(Clone, Debug, Default)]
#[allow(private_bounds)]
pub struct StructuredDictMap<T: StructuredDict> {
    inner: Arc<Dictionary>,
    phantom: PhantomData<T>,
}

impl<T: StructuredDict> StructuredDict for StructuredDictMap<T> {
    fn from_dict(dict: Arc<Dictionary>) -> Self {
        Self { inner: dict, phantom: Default::default() }
    }
}

#[allow(private_bounds)]
impl<T: StructuredDict> StructuredDictMap<T> {
    pub fn insert(&self, key: Name, value: T) -> Option<Capability> {
        self.inner.insert(key, Capability::Dictionary(value.into()))
    }

    pub fn get(&self, key: &BorrowedName) -> Option<T> {
        self.inner.get(key).map(|cap| {
            let Capability::Dictionary(dict) = cap else {
                unreachable!("structured map entry must be a dict: {cap:?}");
            };
            T::from_dict(dict)
        })
    }

    pub fn remove(&self, key: &Name) -> Option<T> {
        self.inner.remove(&*key).map(|cap| {
            let Capability::Dictionary(dict) = cap else {
                unreachable!("structured map entry must be a dict: {cap:?}");
            };
            T::from_dict(dict)
        })
    }

    pub fn append(&self, other: &Self) -> Result<(), ()> {
        self.inner.append(&other.inner)
    }

    pub fn enumerate(&self) -> impl Iterator<Item = (Name, T)> {
        self.inner.enumerate().map(|(key, capability_res)| match capability_res {
            Capability::Dictionary(dict) => (key, T::from_dict(dict)),
            cap => unreachable!("structured map entry must be a dict: {cap:?}"),
        })
    }
}

impl<T: StructuredDict> From<StructuredDictMap<T>> for Arc<Dictionary> {
    fn from(m: StructuredDictMap<T>) -> Self {
        m.inner
    }
}

// Dictionary keys for different kinds of sandboxes.
/// Dictionary of capabilities from or to the parent.
static PARENT: LazyLock<Name> = LazyLock::new(|| "parent".parse().unwrap());

/// Dictionary of capabilities from a component's environment.
static ENVIRONMENT: LazyLock<Name> = LazyLock::new(|| "environment".parse().unwrap());

/// Dictionary of debug capabilities in a component's environment.
static DEBUG: LazyLock<Name> = LazyLock::new(|| "debug".parse().unwrap());

/// Dictionary of runner capabilities in a component's environment.
static RUNNERS: LazyLock<Name> = LazyLock::new(|| "runners".parse().unwrap());

/// Dictionary of resolver capabilities in a component's environment.
static RESOLVERS: LazyLock<Name> = LazyLock::new(|| "resolvers".parse().unwrap());

/// Dictionary of capabilities the component exposes to the framework.
static FRAMEWORK: LazyLock<Name> = LazyLock::new(|| "framework".parse().unwrap());

/// The stop timeout for a component environment.
static STOP_TIMEOUT: LazyLock<Name> = LazyLock::new(|| "stop_timeout".parse().unwrap());

/// The name of a component environment.
static NAME: LazyLock<Name> = LazyLock::new(|| "name".parse().unwrap());

/// Contains the capabilities component receives from its parent and environment. Stored as a
/// [Dictionary] containing two nested [Dictionary]s for the parent and environment.
#[derive(Clone, Debug)]
pub struct ComponentInput(Arc<Dictionary>);

impl Default for ComponentInput {
    fn default() -> Self {
        Self::new(ComponentEnvironment::new())
    }
}

impl StructuredDict for ComponentInput {
    fn from_dict(dict: Arc<Dictionary>) -> Self {
        Self(dict)
    }
}

impl ComponentInput {
    pub fn new(environment: ComponentEnvironment) -> Self {
        let dict = Dictionary::new();

        if !environment.0.is_empty() {
            dict.insert(ENVIRONMENT.clone(), Capability::Dictionary(environment.into()));
        }
        Self(dict)
    }

    /// Creates a new ComponentInput with entries cloned from this ComponentInput.
    ///
    /// This is a shallow copy. Values are cloned, not copied, so are new references to the same
    /// underlying data.
    pub fn shallow_copy(&self) -> Self {
        // Note: We call [Dictionary::copy] on the nested [Dictionary]s, not the root [Dictionary],
        // because [Dictionary::copy] only goes one level deep and we want to copy the contents of
        // the inner sandboxes.
        let dest = Dictionary::new();
        shallow_copy(&self.0, &dest, &*PARENT);
        shallow_copy(&self.0, &dest, &*ENVIRONMENT);
        Self(dest)
    }

    /// Returns the sub-dictionary containing capabilities routed by the component's parent.
    pub fn capabilities(&self) -> Arc<Dictionary> {
        get_or_insert(&self.0, &*PARENT)
    }

    /// Returns the sub-dictionary containing capabilities routed by the component's environment.
    pub fn environment(&self) -> ComponentEnvironment {
        ComponentEnvironment(get_or_insert(&self.0, &*ENVIRONMENT))
    }

    pub fn insert_capability(
        &self,
        path: &impl IterablePath,
        capability: Capability,
    ) -> Option<Capability> {
        self.capabilities().insert_capability(path, capability.into())
    }
}

impl From<ComponentInput> for Arc<Dictionary> {
    fn from(e: ComponentInput) -> Self {
        e.0
    }
}

/// The capabilities a component has in its environment. Stored as a [Dictionary] containing a
/// nested [Dictionary] holding the environment's debug capabilities.
#[derive(Clone, Debug)]
pub struct ComponentEnvironment(Arc<Dictionary>);

impl Default for ComponentEnvironment {
    fn default() -> Self {
        Self(Dictionary::new())
    }
}

impl StructuredDict for ComponentEnvironment {
    fn from_dict(dict: Arc<Dictionary>) -> Self {
        Self(dict)
    }
}

impl ComponentEnvironment {
    pub fn new() -> Self {
        Self::default()
    }

    /// Capabilities listed in the `debug_capabilities` portion of its environment.
    pub fn debug(&self) -> Arc<Dictionary> {
        get_or_insert(&self.0, &*DEBUG)
    }

    /// Capabilities listed in the `runners` portion of its environment.
    pub fn runners(&self) -> Arc<Dictionary> {
        get_or_insert(&self.0, &*RUNNERS)
    }

    /// Capabilities listed in the `resolvers` portion of its environment.
    pub fn resolvers(&self) -> Arc<Dictionary> {
        get_or_insert(&self.0, &*RESOLVERS)
    }

    /// Sets the stop timeout (in milliseconds) for this environment.
    pub fn set_stop_timeout(&self, timeout: i64) {
        let _ = self.0.insert(STOP_TIMEOUT.clone(), Capability::Data(Data::Int64(timeout)));
    }

    /// Returns the stop timeout (in milliseconds) for this environment.
    pub fn stop_timeout(&self) -> Option<i64> {
        let Some(Capability::Data(data_cap)) = self.0.get(&*STOP_TIMEOUT) else {
            return None;
        };
        let Data::Int64(timeout) = &data_cap else {
            return None;
        };
        Some(*timeout)
    }

    /// Sets the name for this environment.
    pub fn set_name(&self, name: &Name) {
        let _ = self.0.insert(NAME.clone(), Capability::Data(Data::String(name.as_str().into())));
    }

    /// Returns the name for this environment.
    pub fn name(&self) -> Option<Name> {
        let Some(Capability::Data(data_cap)) = self.0.get(&*NAME) else {
            return None;
        };
        let Data::String(name) = &data_cap else {
            return None;
        };
        Some(Name::new(name).unwrap())
    }

    pub fn shallow_copy(&self) -> Self {
        // Note: We call [Dictionary::shallow_copy] on the nested [Dictionary]s, not the root
        // [Dictionary], because [Dictionary::shallow_copy] only goes one level deep and we want to
        // copy the contents of the inner sandboxes.
        let dest = Dictionary::new();
        shallow_copy(&self.0, &dest, &*DEBUG);
        shallow_copy(&self.0, &dest, &*RUNNERS);
        shallow_copy(&self.0, &dest, &*RESOLVERS);
        Self(dest)
    }
}

impl From<ComponentEnvironment> for Arc<Dictionary> {
    fn from(e: ComponentEnvironment) -> Self {
        e.0
    }
}

/// Contains the capabilities a component makes available to its parent or the framework. Stored as
/// a [Dictionary] containing two nested [Dictionary]s for the capabilities made available to the
/// parent and to the framework.
#[derive(Clone, Debug)]
pub struct ComponentOutput(Arc<Dictionary>);

impl Default for ComponentOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl StructuredDict for ComponentOutput {
    fn from_dict(dict: Arc<Dictionary>) -> Self {
        Self(dict)
    }
}

impl ComponentOutput {
    pub fn new() -> Self {
        Self(Dictionary::new())
    }

    /// Creates a new ComponentOutput with entries cloned from this ComponentOutput.
    ///
    /// This is a shallow copy. Values are cloned, not copied, so are new references to the same
    /// underlying data.
    pub fn shallow_copy(&self) -> Self {
        // Note: We call [Dictionary::copy] on the nested [Dictionary]s, not the root [Dictionary],
        // because [Dictionary::copy] only goes one level deep and we want to copy the contents of
        // the inner sandboxes.
        let dest = Dictionary::new();
        shallow_copy(&self.0, &dest, &*PARENT);
        shallow_copy(&self.0, &dest, &*FRAMEWORK);
        Self(dest)
    }

    /// Returns the sub-dictionary containing capabilities routed to the component's parent.
    /// framework. Lazily adds the dictionary if it does not exist yet.
    pub fn capabilities(&self) -> Arc<Dictionary> {
        get_or_insert(&self.0, &*PARENT)
    }

    /// Returns the sub-dictionary containing capabilities exposed by the component to the
    /// framework. Lazily adds the dictionary if it does not exist yet.
    pub fn framework(&self) -> Arc<Dictionary> {
        get_or_insert(&self.0, &*FRAMEWORK)
    }
}

impl From<ComponentOutput> for Arc<Dictionary> {
    fn from(e: ComponentOutput) -> Self {
        e.0
    }
}

fn shallow_copy(src: &Arc<Dictionary>, dest: &Arc<Dictionary>, key: &Name) {
    if let Some(d) = src.get(key) {
        let Capability::Dictionary(d) = d else {
            unreachable!("{key} entry must be a dictionary: {d:?}");
        };
        dest.insert(key.clone(), Capability::Dictionary(d.shallow_copy()));
    }
}

fn get_or_insert(this: &Dictionary, key: &Name) -> Arc<Dictionary> {
    let cap = this.get_or_insert(&key, || Capability::Dictionary(Dictionary::new()));
    let Capability::Dictionary(dict) = cap else {
        unreachable!("{key} entry must be a dict: {cap:?}");
    };
    dict
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use runtime_capabilities::DictKey;

    impl StructuredDict for Arc<Dictionary> {
        fn from_dict(dict: Arc<Dictionary>) -> Self {
            dict
        }
    }

    #[fuchsia::test]
    async fn structured_dict_map() {
        let dict1 = {
            let dict = Dictionary::new();
            dict.insert("a".parse().unwrap(), Dictionary::new().into());
            dict
        };
        let dict2 = {
            let dict = Dictionary::new();
            dict.insert("b".parse().unwrap(), Dictionary::new().into());
            dict
        };
        let dict2_alt = {
            let dict = Dictionary::new();
            dict.insert("c".parse().unwrap(), Dictionary::new().into());
            dict
        };
        let name1 = Name::new("1").unwrap();
        let name2 = Name::new("2").unwrap();

        let map: StructuredDictMap<Arc<Dictionary>> = Default::default();
        assert_matches!(map.get(&name1), None);
        assert!(map.insert(name1.clone(), dict1).is_none());
        let d = map.get(&name1).unwrap();
        let key = DictKey::new("a").unwrap();
        assert_matches!(d.get(&key), Some(_));

        assert!(map.insert(name2.clone(), dict2).is_none());
        let d = map.remove(&name2).unwrap();
        assert_matches!(map.remove(&name2), None);
        let key = DictKey::new("b").unwrap();
        assert_matches!(d.get(&key), Some(_));

        assert!(map.insert(name2.clone(), dict2_alt).is_none());
        let d = map.get(&name2).unwrap();
        let key = DictKey::new("c").unwrap();
        assert_matches!(d.get(&key), Some(_));
    }
}
