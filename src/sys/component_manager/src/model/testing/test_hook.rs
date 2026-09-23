// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use errors::ModelError;
use hooks::{Event, EventPayload, EventType, Hook, HooksRegistration};
use moniker::Moniker;
use std::cmp::Eq;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Weak};
use std::{fmt, sync};

struct ComponentInstance {
    pub moniker: Moniker,
    pub children: sync::Mutex<Vec<Arc<ComponentInstance>>>,
}

impl Clone for ComponentInstance {
    // This is used by TestHook. ComponentInstance is immutable so when a change
    // needs to be made, TestHook clones ComponentInstance and makes the change
    // in the new copy.
    fn clone(&self) -> Self {
        let children = self.children.lock().unwrap();
        return ComponentInstance {
            moniker: self.moniker.clone(),
            children: sync::Mutex::new(children.clone()),
        };
    }
}

impl PartialEq for ComponentInstance {
    fn eq(&self, other: &Self) -> bool {
        self.moniker == other.moniker
    }
}

impl Eq for ComponentInstance {}

impl ComponentInstance {
    pub fn print(&self) -> String {
        let mut s: String = self.moniker.leaf().map_or(String::new(), |m| m.to_string());
        let mut children = self.children.lock().unwrap();
        if children.is_empty() {
            return s;
        }

        // The position of a child in the children vector is a function of timing.
        // In order to produce stable topology strings across runs, we sort the set
        // of children here by moniker.
        children.sort_by(|a, b| a.moniker.cmp(&b.moniker));

        s.push('(');
        let mut count = 0;
        for child in children.iter() {
            // If we've seen a previous child, then add a comma to separate children.
            if count > 0 {
                s.push(',');
            }
            s.push_str(&child.print());
            count += 1;
        }
        s.push(')');
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lifecycle {
    Start(Moniker),
    Stop(Moniker),
    Destroy(Moniker),
}

impl fmt::Display for Lifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Lifecycle::Start(m) => write!(f, "bind({})", m),
            Lifecycle::Stop(m) => write!(f, "stop({})", m),
            Lifecycle::Destroy(m) => write!(f, "destroy({})", m),
        }
    }
}

/// TestHook is a Hook that generates a strings representing the component
/// topology.
pub struct TestHook {
    instances: sync::Mutex<HashMap<Moniker, Arc<ComponentInstance>>>,
    lifecycle_events: sync::Mutex<Vec<Lifecycle>>,
}

impl fmt::Display for TestHook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.print())?;
        Ok(())
    }
}

impl TestHook {
    pub fn new() -> TestHook {
        let this = Self {
            instances: sync::Mutex::new(HashMap::new()),
            lifecycle_events: sync::Mutex::new(vec![]),
        };
        this.create_instance_if_necessary(&Moniker::root()).unwrap();
        this
    }

    /// Returns the set of hooks into the component manager that TestHook is interested in.
    pub fn hooks(self: &Arc<Self>) -> Vec<HooksRegistration> {
        vec![HooksRegistration::new(
            "TestHook",
            vec![EventType::Destroyed, EventType::Started, EventType::Stopped],
            Arc::downgrade(self) as Weak<dyn Hook>,
        )]
    }

    /// Recursively traverse the Instance tree to generate a string representing the component
    /// topology.
    pub fn print(&self) -> String {
        let instances = self.instances.lock().unwrap();
        let moniker = Moniker::root();
        let root_instance =
            instances.get(&moniker).map(|x| x.clone()).expect("Unable to find root instance.");
        root_instance.print()
    }

    /// Return the sequence of lifecycle events.
    pub fn lifecycle(&self) -> Vec<Lifecycle> {
        self.lifecycle_events.lock().unwrap().clone()
    }

    pub async fn on_started_async<'a>(
        &'a self,
        target_moniker: &Moniker,
    ) -> Result<(), ModelError> {
        self.create_instance_if_necessary(target_moniker)?;
        let mut events = self.lifecycle_events.lock().unwrap();
        events.push(Lifecycle::Start(target_moniker.clone()));
        Ok(())
    }

    pub async fn on_stopped_async<'a>(
        &'a self,
        target_moniker: &Moniker,
    ) -> Result<(), ModelError> {
        let mut events = self.lifecycle_events.lock().unwrap();
        events.push(Lifecycle::Stop(target_moniker.clone()));
        Ok(())
    }

    pub async fn on_destroyed_async<'a>(
        &'a self,
        target_moniker: &Moniker,
    ) -> Result<(), ModelError> {
        // TODO: Can this be changed not restrict to dynamic instances? Static instances can be
        // deleted too.
        if let Some(child_moniker) = target_moniker.leaf() {
            if child_moniker.collection().is_some() {
                self.remove_instance(target_moniker)?;
            }
        }

        let mut events = self.lifecycle_events.lock().unwrap();
        events.push(Lifecycle::Destroy(target_moniker.clone()));
        Ok(())
    }

    pub fn create_instance_if_necessary(&self, moniker: &Moniker) -> Result<(), ModelError> {
        let mut instances = self.instances.lock().unwrap();
        let new_instance = match instances.get(moniker) {
            Some(old_instance) => Arc::new((old_instance.deref()).clone()),
            None => Arc::new(ComponentInstance {
                moniker: moniker.clone(),
                children: sync::Mutex::new(vec![]),
            }),
        };
        instances.insert(moniker.clone(), new_instance.clone());
        if let Some(parent_moniker) = moniker.parent() {
            // If the parent isn't in the list of instances (this can happen if the child was
            // started before the parent), add it in.
            let entry = instances.entry(parent_moniker.clone());
            let parent_instance = match entry {
                Entry::Occupied(ref e) => e.get(),
                Entry::Vacant(e) => e.insert(Arc::new(ComponentInstance {
                    moniker: parent_moniker,
                    children: sync::Mutex::new(vec![]),
                })),
            };
            let mut children = parent_instance.children.lock().unwrap();
            let opt_index = children.iter().position(|c| c.moniker == new_instance.moniker);
            if let Some(index) = opt_index {
                children.remove(index);
            }
            children.push(new_instance.clone());
        }
        Ok(())
    }

    pub fn remove_instance(&self, moniker: &Moniker) -> Result<(), ModelError> {
        let mut instances = self.instances.lock().unwrap();
        if let Some(parent_moniker) = moniker.parent() {
            instances.remove(moniker);
            let parent_instance = instances
                .get(&parent_moniker)
                .unwrap_or_else(|| panic!("parent instance {} not found", parent_moniker));
            let mut children = parent_instance.children.lock().unwrap();
            let opt_index = children.iter().position(|c| c.moniker == *moniker);
            if let Some(index) = opt_index {
                children.remove(index);
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Hook for TestHook {
    async fn on(self: Arc<Self>, event: &Event) -> Result<(), ModelError> {
        let target_moniker = event
            .target_moniker
            .unwrap_instance_moniker_or(ModelError::UnexpectedComponentManagerMoniker)?;
        match &event.payload {
            EventPayload::Destroyed => {
                self.on_destroyed_async(&target_moniker).await?;
            }
            EventPayload::Started { .. } => {
                self.on_started_async(&target_moniker).await?;
            }
            EventPayload::Stopped { .. } => {
                self.on_stopped_async(&target_moniker).await?;
            }
            _ => (),
        };
        Ok(())
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn test_hook_test() {
        let root = Moniker::root();
        let a: Moniker = ["a"].try_into().unwrap();
        let ab: Moniker = ["a", "b"].try_into().unwrap();
        let ac: Moniker = ["a", "c"].try_into().unwrap();
        let abd: Moniker = ["a", "b", "d"].try_into().unwrap();
        let abe: Moniker = ["a", "b", "e"].try_into().unwrap();
        let acf: Moniker = ["a", "c", "f"].try_into().unwrap();

        // Try adding parent followed by children then verify the topology string
        // is correct.
        {
            let test_hook = TestHook::new();
            assert!(test_hook.create_instance_if_necessary(&root).is_ok());
            assert!(test_hook.create_instance_if_necessary(&a).is_ok());
            assert!(test_hook.create_instance_if_necessary(&ab).is_ok());
            assert!(test_hook.create_instance_if_necessary(&ac).is_ok());
            assert!(test_hook.create_instance_if_necessary(&abd).is_ok());
            assert!(test_hook.create_instance_if_necessary(&abe).is_ok());
            assert!(test_hook.create_instance_if_necessary(&acf).is_ok());
            assert_eq!("(a(b(d,e),c(f)))", test_hook.print());
        }

        // Changing the order of monikers should not affect the output string.
        {
            let test_hook = TestHook::new();
            assert!(test_hook.create_instance_if_necessary(&root).is_ok());
            assert!(test_hook.create_instance_if_necessary(&a).is_ok());
            assert!(test_hook.create_instance_if_necessary(&ac).is_ok());
            assert!(test_hook.create_instance_if_necessary(&ab).is_ok());
            assert!(test_hook.create_instance_if_necessary(&abd).is_ok());
            assert!(test_hook.create_instance_if_necessary(&abe).is_ok());
            assert!(test_hook.create_instance_if_necessary(&acf).is_ok());
            assert_eq!("(a(b(d,e),c(f)))", test_hook.print());
        }

        // Submitting children before parents should still succeed.
        {
            let test_hook = TestHook::new();
            assert!(test_hook.create_instance_if_necessary(&root).is_ok());
            assert!(test_hook.create_instance_if_necessary(&acf).is_ok());
            assert!(test_hook.create_instance_if_necessary(&abe).is_ok());
            assert!(test_hook.create_instance_if_necessary(&abd).is_ok());
            assert!(test_hook.create_instance_if_necessary(&ab).is_ok());
            // Model will call create_instance_if_necessary for ab's children again
            // after the call to bind_instance for ab.
            assert!(test_hook.create_instance_if_necessary(&abe).is_ok());
            assert!(test_hook.create_instance_if_necessary(&abd).is_ok());
            assert!(test_hook.create_instance_if_necessary(&ac).is_ok());
            // Model will call create_instance_if_necessary for ac's children again
            // after the call to bind_instance for ac.
            assert!(test_hook.create_instance_if_necessary(&acf).is_ok());
            assert!(test_hook.create_instance_if_necessary(&a).is_ok());
            // Model will call create_instance_if_necessary for a's children again
            // after the call to bind_instance for a.
            assert!(test_hook.create_instance_if_necessary(&ab).is_ok());
            assert!(test_hook.create_instance_if_necessary(&ac).is_ok());
            assert_eq!("(a(b(d,e),c(f)))", test_hook.print());
        }
    }
}
