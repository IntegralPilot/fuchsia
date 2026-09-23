// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl_fuchsia_power_broker::{DependencyToken, Permissions};
use std::collections::HashMap;

use crate::topology::ElementID;

/// Use kernel object IDs to uniquely represent Credentials. They are
/// guaranteed not to be reused over the lifetime of the system:
/// https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts#kernel_object_ids
type CredentialID = zx::Koid;

/// A Credential that has been created and registered with the system.
#[derive(Clone, Debug, PartialEq)]
pub struct Credential {
    id: CredentialID,
    element: ElementID,
    permissions: Permissions,
}

impl Credential {
    pub fn get_element(&self) -> ElementID {
        self.element
    }

    pub fn contains(&self, permissions: Permissions) -> bool {
        self.permissions.contains(permissions)
    }

    /// Returns true iff self includes these permissions for the given element.
    #[cfg(test)]
    pub fn authorizes(&self, element: ElementID, permissions: Permissions) -> bool {
        self.element == element && self.permissions.contains(permissions)
    }
}

#[derive(Debug)]
pub struct Token {
    pub token: DependencyToken,
}

impl From<DependencyToken> for Token {
    fn from(token: DependencyToken) -> Self {
        Token { token }
    }
}

impl From<&DependencyToken> for Token {
    fn from(token: &DependencyToken) -> Self {
        Token { token: token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap() }
    }
}

impl Into<DependencyToken> for Token {
    fn into(self) -> DependencyToken {
        self.token
    }
}

impl Token {
    fn koid(&self) -> Option<zx::Koid> {
        let Ok(info) = self.token.as_handle_ref().basic_info() else {
            return None;
        };
        Some(info.koid)
    }
}

/// CredentialToRegister holds the necessary information for a Credential
/// to be created and registered. It is meant for clients to use to specify
/// new Credentials to be registered.
#[derive(Debug)]
pub struct CredentialToRegister {
    pub broker_token: Token,
    pub permissions: Permissions,
}

#[derive(Debug)]
pub struct Registry {
    credentials: HashMap<CredentialID, Credential>,
    tokens: HashMap<CredentialID, Token>,
    credential_ids_by_element: HashMap<ElementID, Vec<CredentialID>>,
}

impl Registry {
    pub fn new() -> Self {
        Registry {
            credentials: HashMap::new(),
            tokens: HashMap::new(),
            credential_ids_by_element: HashMap::new(),
        }
    }

    pub fn get_credential_by_koid(&self, koid: &zx::Koid) -> Option<Credential> {
        self.credentials.get(koid).cloned()
    }

    pub fn lookup(&self, token: &Token) -> Option<Credential> {
        let koid = token.koid()?;
        self.get_credential_by_koid(&koid)
    }

    pub fn register(
        &mut self,
        element: ElementID,
        credential_to_register: CredentialToRegister,
    ) -> Result<(), RegisterCredentialsError> {
        let Some(id) = credential_to_register.broker_token.koid() else {
            log::error!("could not get koid for {:?}", credential_to_register.broker_token);
            return Err(RegisterCredentialsError::Internal);
        };
        if self.tokens.contains_key(&id) {
            log::debug!("register_dependency_token: token already in use");
            return Err(RegisterCredentialsError::AlreadyInUse);
        };
        self.tokens.insert(id, credential_to_register.broker_token);
        self.credential_ids_by_element.entry(element.clone()).or_insert(Vec::new()).push(id);
        let credential = Credential {
            id: id,
            element: element.clone(),
            permissions: credential_to_register.permissions,
        };
        log::debug!("registered credential: {:?}", credential);
        self.credentials.insert(id, credential);
        Ok(())
    }

    pub fn unregister(&mut self, credential: &Credential) -> Option<Credential> {
        self.unregister_id(&credential.id)
    }

    fn unregister_id(&mut self, id: &CredentialID) -> Option<Credential> {
        let cred = {
            if let Some(credential) = self.credentials.remove(id) {
                if let Some(by_element_entry) =
                    self.credential_ids_by_element.get_mut(&credential.element)
                {
                    by_element_entry.retain(|cid| cid != id);
                } else {
                    log::error!("missing {:?} in credential_ids_by_element", credential.element);
                };
                Some(credential)
            } else {
                None
            }
        };
        self.tokens.remove(id);
        cred
    }

    fn for_element(&self, element: ElementID) -> Vec<CredentialID> {
        let Some(credential_ids) = self.credential_ids_by_element.get(&element) else {
            return Vec::new();
        };
        credential_ids.iter().cloned().collect()
    }

    pub fn unregister_all_for_element(&mut self, element: ElementID) {
        let credential_ids = self.for_element(element);
        for id in credential_ids {
            self.unregister_id(&id);
        }
    }
}

#[derive(Debug)]
pub enum RegisterCredentialsError {
    AlreadyInUse,
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_no_permissions() {
        let none = Permissions::empty();
        assert_eq!(none.contains(Permissions::MODIFY_DEPENDENT), false);
        assert_eq!(none.contains(Permissions::MODIFY_DEPENDENCY), false);
    }

    #[fuchsia::test]
    fn test_all_permissions() {
        let all = Permissions::all();
        assert_eq!(all.contains(Permissions::MODIFY_DEPENDENT), true);
        assert_eq!(all.contains(Permissions::MODIFY_DEPENDENCY), true);
    }

    #[fuchsia::test]
    fn test_some_permissions() {
        let some = Permissions::MODIFY_DEPENDENT | Permissions::MODIFY_DEPENDENCY;
        assert_eq!(some.contains(Permissions::MODIFY_DEPENDENT), true);
        assert_eq!(some.contains(Permissions::MODIFY_DEPENDENCY), true);
    }

    #[fuchsia::test]
    fn test_credential_authorizes() {
        let element_id = ElementID::new(1);
        let gold_credential = Credential {
            element: element_id,
            id: DependencyToken::create().as_handle_ref().koid().expect("get_koid failed"),
            permissions: Permissions::MODIFY_DEPENDENT,
        };
        assert_eq!(gold_credential.authorizes(element_id, Permissions::MODIFY_DEPENDENT), true);
        assert_eq!(gold_credential.authorizes(element_id, Permissions::MODIFY_DEPENDENCY), false);
    }

    #[fuchsia::test]
    fn test_register_unregister() {
        let mut registry = Registry::new();
        let token_kryptonite = DependencyToken::create();
        let element_kryptonite: ElementID = ElementID::new(1);
        let credential_to_register = CredentialToRegister {
            broker_token: token_kryptonite
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .expect("dup failed")
                .into(),
            permissions: Permissions::MODIFY_DEPENDENT,
        };
        registry.register(element_kryptonite, credential_to_register).expect("register failed");
        let token_kryptonite_dup =
            token_kryptonite.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed");
        let credential = registry.lookup(&Token::from(&token_kryptonite_dup)).unwrap();
        assert_eq!(
            credential.id,
            token_kryptonite.as_handle_ref().basic_info().expect("basic_info failed").koid
        );
        assert_eq!(credential.element, element_kryptonite);
        assert_eq!(credential.permissions.contains(Permissions::MODIFY_DEPENDENT), true);
        assert_eq!(credential.permissions.contains(Permissions::MODIFY_DEPENDENCY), false);

        let unregistered = registry.unregister(&credential);
        assert_eq!(unregistered, Some(credential.clone()));
        let lookup_not_found = registry.lookup(&Token::from(&token_kryptonite));
        assert_eq!(lookup_not_found, None);

        let extra_unregister = registry.unregister(&credential);
        assert_eq!(extra_unregister, None);
    }
}
