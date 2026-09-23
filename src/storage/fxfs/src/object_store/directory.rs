// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::errors::FxfsError;
use crate::filesystem::TruncateGuard;
use crate::lsm_tree::Query;
use crate::lsm_tree::merge::{Merger, MergerIterator};
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::object_handle::{INVALID_OBJECT_ID, ObjectHandle, ObjectProperties};
use crate::object_store::object_record::{
    ChildValue, DirType, EncryptedCasefoldChild, EncryptedChild, ObjectAttributes,
    ObjectDescriptor, ObjectKey, ObjectKeyData, ObjectKind, ObjectValue, Timestamp,
};
use crate::object_store::transaction::{
    LockKey, LockKeys, Mutation, Options, Transaction, lock_keys,
};
use crate::object_store::{
    AttributeId, DataObjectHandle, HandleOptions, HandleOwner, ObjectStore,
    SetExtendedAttributeMode, StoreObjectHandle, TombstoneMode, TrimMode, TrimResult,
};
use anyhow::{Error, anyhow, bail, ensure};
use fidl_fuchsia_io as fio;
use fscrypt::proxy_filename::ProxyFilename;
use fuchsia_sync::Mutex;
use fxfs_crypto::{Cipher, CipherHolder, ObjectType, WrappingKeyId, key_to_cipher};
use std::fmt;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use zerocopy::IntoBytes;

use super::FSCRYPT_KEY_ID;

type BoxPredicate<'a> = Box<dyn Fn(&ObjectKey) -> ControlFlow<bool> + Send + 'a>;

/// This contains the transaction with the appropriate locks to replace src with dst, and also the
/// ID and type of the src and dst.
pub struct ReplaceContext<'a> {
    pub transaction: Transaction<'a>,
    pub truncate_guard: Option<TruncateGuard<'static>>,
    pub src_id_and_descriptor: Option<(u64, ObjectDescriptor)>,
    pub dst_id_and_descriptor: Option<(u64, ObjectDescriptor)>,
    pub src_name: Option<String>,
    pub dst_name: Option<String>,
}

pub struct LookupEntry {
    pub object_id: u64,
    pub descriptor: ObjectDescriptor,
    pub key: ObjectKey,
    pub locked: bool,
}

/// A directory stores name to child object mappings.
pub struct Directory<S: HandleOwner> {
    handle: StoreObjectHandle<S>,
    /// True if the directory has been deleted and is no longer accessible.
    is_deleted: AtomicBool,
    /// The type of directory (encryption, casefolding, etc.)
    dir_type: Mutex<DirType>,
}

#[derive(Clone, Default)]
pub struct MutableAttributesInternal {
    sub_dirs: i64,
    change_time: Option<Timestamp>,
    modification_time: Option<u64>,
    creation_time: Option<u64>,
}

impl MutableAttributesInternal {
    pub fn new(
        sub_dirs: i64,
        change_time: Option<Timestamp>,
        modification_time: Option<u64>,
        creation_time: Option<u64>,
    ) -> Self {
        Self { sub_dirs, change_time, modification_time, creation_time }
    }
}

/// Encrypts a unicode `name` into a sequence of bytes using the fscrypt key.
pub(crate) fn encrypt_filename(
    key: &dyn Cipher,
    object_id: u64,
    name: &str,
) -> Result<Vec<u8>, Error> {
    let mut name_bytes = name.as_bytes().to_vec();
    key.encrypt_filename(object_id, &mut name_bytes)?;
    Ok(name_bytes)
}

/// Decrypts a unicode `name` from a sequence of bytes using the fscrypt key.
pub(crate) fn decrypt_filename(
    key: &dyn Cipher,
    object_id: u64,
    data: &[u8],
) -> Result<String, Error> {
    let mut raw = data.to_vec();
    key.decrypt_filename(object_id, &mut raw)?;
    Ok(String::from_utf8(raw)?)
}

#[fxfs_trace::trace]
impl<S: HandleOwner> Directory<S> {
    fn new(owner: Arc<S>, object_id: u64, dir_type: DirType) -> Self {
        Directory {
            handle: StoreObjectHandle::new(
                owner,
                object_id,
                /* permanent_keys: */ false,
                HandleOptions::default(),
                /* trace: */ false,
            ),
            is_deleted: AtomicBool::new(false),
            dir_type: Mutex::new(dir_type),
        }
    }

    /// Returns `Some(name)` for a given object (assumed to be child object of Directory).
    /// If the object is encrypted and is not unlocked, we will return `None`.
    /// The caller should ensure that `None` is handled correctly -- for example by using the
    /// `ProxyFilename` for things like `did_remove()` and readdir entry fields.
    pub async fn get_case_preserved_name(&self, key: ObjectKey) -> Result<Option<String>, Error> {
        match key.data {
            ObjectKeyData::Child { name } => Ok(Some(name)),
            ObjectKeyData::CasefoldChild { name, .. } => Ok(Some(name)),
            ObjectKeyData::LegacyCasefoldChild(name) => Ok(Some(name.to_string())),
            ObjectKeyData::EncryptedChild(crate::object_store::object_record::EncryptedChild(
                name,
            )) => {
                if let CipherHolder::Cipher(cipher) = self.get_fscrypt_key().await? {
                    Ok(Some(decrypt_filename(cipher.as_ref(), self.object_id(), &name)?))
                } else {
                    Ok(None)
                }
            }
            ObjectKeyData::EncryptedCasefoldChild(
                crate::object_store::object_record::EncryptedCasefoldChild { name, .. },
            ) => {
                if let CipherHolder::Cipher(cipher) = self.get_fscrypt_key().await? {
                    Ok(Some(decrypt_filename(cipher.as_ref(), self.object_id(), &name)?))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    pub fn object_id(&self) -> u64 {
        self.handle.object_id()
    }

    pub fn wrapping_key_id(&self) -> Option<WrappingKeyId> {
        self.dir_type.lock().wrapping_key_id()
    }

    /// Retrieves keys from the key manager or unwraps the wrapped keys in the directory's key
    /// record.  Returns None if the key is currently unavailable due to the wrapping key being
    /// unavailable.
    pub async fn get_fscrypt_key(&self) -> Result<CipherHolder, Error> {
        let object_id = self.object_id();
        let store = self.store();
        store
            .key_manager()
            .get_fscrypt_key(object_id, store.crypt().unwrap().as_ref(), async || {
                store.get_keys(object_id).await
            })
            .await
    }

    pub fn owner(&self) -> &Arc<S> {
        self.handle.owner()
    }

    pub fn store(&self) -> &ObjectStore {
        self.handle.store()
    }

    pub fn handle(&self) -> &StoreObjectHandle<S> {
        &self.handle
    }

    pub fn is_deleted(&self) -> bool {
        self.is_deleted.load(Ordering::Relaxed)
    }

    pub fn set_deleted(&self) {
        self.is_deleted.store(true, Ordering::Relaxed);
    }

    /// Mode of directory (legacy, casefold, normal)
    pub fn dir_type(&self) -> DirType {
        *self.dir_type.lock()
    }

    /// Enables/disables casefolding. This can only be done on an empty directory.
    pub async fn set_casefold(&self, val: bool) -> Result<(), Error> {
        let dir_type = self.dir_type().with_casefold(val);
        // Nb: We lock the directory to ensure it doesn't change during our check for children.
        let mut transaction = self
            .store()
            .new_transaction(
                lock_keys![LockKey::object(self.store().store_object_id(), self.object_id())],
                Options::default(),
            )
            .await?;
        ensure!(!self.has_children().await?, FxfsError::NotEmpty);
        let mut mutation =
            self.store().txn_get_object_mutation(&transaction, self.object_id()).await?;
        if let ObjectValue::Object {
            kind: ObjectKind::Directory { dir_type: dest_dir_type, .. },
            ..
        } = &mut mutation.item.value
        {
            *dest_dir_type = dir_type;
        } else {
            return Err(
                anyhow!(FxfsError::Inconsistent).context("casefold only applies to directories")
            );
        }
        transaction.add(self.store().store_object_id(), Mutation::ObjectStore(mutation));
        transaction.commit_with_callback(|_| *self.dir_type.lock() = dir_type).await?;
        Ok(())
    }

    pub async fn create(
        transaction: &mut Transaction<'_>,
        owner: &Arc<S>,
        wrapping_key_id: Option<WrappingKeyId>,
    ) -> Result<Directory<S>, Error> {
        let dir_type = match wrapping_key_id {
            Some(id) => DirType::Encrypted(id),
            None => DirType::Normal,
        };
        Self::create_with_options(transaction, owner, dir_type).await
    }

    pub async fn create_with_options(
        transaction: &mut Transaction<'_>,
        owner: &Arc<S>,
        dir_type: DirType,
    ) -> Result<Directory<S>, Error> {
        let store = owner.as_ref().as_ref();
        let object_id = store.get_next_object_id().await?;
        let now = Timestamp::now();

        // The transaction takes ownership of the ID.
        let object_id = object_id.release().get();
        transaction.add(
            store.store_object_id(),
            Mutation::insert_object(
                ObjectKey::object(object_id),
                ObjectValue::Object {
                    kind: ObjectKind::Directory { sub_dirs: 0, dir_type },
                    attributes: ObjectAttributes {
                        creation_time: now.clone(),
                        modification_time: now.clone(),
                        project_id: None,
                        posix_attributes: None,
                        allocated_size: 0,
                        access_time: now.clone(),
                        change_time: now,
                    },
                },
            ),
        );
        if let Some(wrapping_key_id) = dir_type.wrapping_key_id() {
            if let Some(crypt) = store.crypt() {
                let (key, unwrapped_key) = crypt
                    .create_key_with_id(object_id, wrapping_key_id, ObjectType::Directory)
                    .await?;
                let cipher = key_to_cipher(&key, &unwrapped_key)?;
                transaction.add(
                    store.store_object_id(),
                    Mutation::insert_object(
                        ObjectKey::keys(object_id),
                        ObjectValue::keys(vec![(FSCRYPT_KEY_ID, key)].into()),
                    ),
                );
                // Note that it's possible that this entry gets inserted into the key manager but
                // this transaction doesn't get committed. This shouldn't be a problem because
                // unused keys get purged on a standard timeout interval and this key shouldn't
                // conflict with any other keys.
                store.key_manager.insert(
                    object_id,
                    Arc::new(vec![(FSCRYPT_KEY_ID, CipherHolder::Cipher(cipher))].into()),
                    false,
                );
            } else {
                return Err(anyhow!("No crypt"));
            }
        }
        Ok(Directory::new(owner.clone(), object_id, dir_type))
    }

    /// Sets the file-based-encryption (FBE) wrapping key for this directory.
    ///
    /// This can only be done on empty directories and must NOT be done as part of a transaction
    /// that creates entries in the same directory. The reason for this is that local state
    /// (self.wrapping_key_id) is used to control the type of child record written out. If children
    /// are written to a directory as part of the same transaction that enables FBE, they will be
    /// written as the wrong child record type.
    pub async fn set_wrapping_key(
        &self,
        transaction: &mut Transaction<'_>,
        id: WrappingKeyId,
    ) -> Result<Arc<dyn Cipher>, Error> {
        let object_id = self.object_id();
        let store = self.store();
        if let Some(crypt) = store.crypt() {
            let (key, unwrapped_key) =
                crypt.create_key_with_id(object_id, id, ObjectType::Directory).await?;
            let mut mutation = store.txn_get_object_mutation(transaction, object_id).await?;
            if let ObjectValue::Object { kind: ObjectKind::Directory { dir_type, .. }, .. } =
                &mut mutation.item.value
            {
                if dir_type.is_encrypted() {
                    return Err(anyhow!("wrapping key id is already set"));
                }
                if self.has_children().await? {
                    return Err(FxfsError::NotEmpty.into());
                }
                *dir_type = dir_type.with_encryption(id);
            } else {
                match mutation.item.value {
                    ObjectValue::None => bail!(FxfsError::NotFound),
                    _ => bail!(FxfsError::NotDir),
                }
            }
            transaction.add(store.store_object_id(), Mutation::ObjectStore(mutation));

            let keys_key = ObjectKey::keys(object_id);
            let value = if let Some(mutation) =
                transaction.get_object_mutation(store.store_object_id(), keys_key.clone())
            {
                Some(mutation.item.value.clone())
            } else {
                store.tree.find_value(&keys_key).await?
            };

            let cipher = key_to_cipher(&key, &unwrapped_key)?;
            match value {
                None | Some(ObjectValue::None) => {
                    transaction.add(
                        store.store_object_id(),
                        Mutation::insert_object(
                            ObjectKey::keys(object_id),
                            ObjectValue::keys(vec![(FSCRYPT_KEY_ID, key)].into()),
                        ),
                    );
                }
                Some(ObjectValue::Keys(mut keys)) => {
                    keys.insert(FSCRYPT_KEY_ID, key.into());
                    transaction.add(
                        store.store_object_id(),
                        Mutation::replace_or_insert_object(
                            ObjectKey::keys(object_id),
                            ObjectValue::keys(keys),
                        ),
                    );
                }
                Some(value) => bail!("Unexpected value in lookup: {value:?}"),
            }
            Ok(cipher)
        } else {
            Err(anyhow!("No crypt"))
        }
    }

    #[trace]
    pub async fn open(owner: &Arc<S>, object_id: u64) -> Result<Directory<S>, Error> {
        let store = owner.as_ref().as_ref();
        let dir_type = store
            .tree
            .find_map(&ObjectKey::object(object_id), |item| match item.value {
                ObjectValue::Object { kind: ObjectKind::Directory { dir_type, .. }, .. } => {
                    Ok(*dir_type)
                }
                _ => bail!(FxfsError::NotDir),
            })
            .await?
            .ok_or(FxfsError::NotFound)??;
        Ok(Directory::new(owner.clone(), object_id, dir_type))
    }

    /// Opens a directory. The caller is responsible for ensuring that the object exists and is a
    /// directory.
    pub fn open_unchecked(owner: Arc<S>, object_id: u64, dir_type: DirType) -> Self {
        Self::new(owner, object_id, dir_type)
    }

    /// Acquires the transaction with the appropriate locks to replace |dst| with |src.0|/|src.1|.
    /// |src| can be None in the case of unlinking |dst| from |self|.
    /// Returns the transaction, as well as the ID and type of the child and the src. If the child
    /// doesn't exist, then a transaction is returned with a lock only on the parent and None for
    /// the target info so that the transaction can be executed with the confidence that the target
    /// doesn't exist. If the src doesn't exist (in the case of unlinking), None is return for the
    /// source info.
    ///
    /// We need to lock |self|, but also the child if it exists. When it is a directory the lock
    /// prevents entries being added at the same time. When it is a file needs to be able to
    /// decrement the reference count.
    /// If src exists, we also need to lock |src.0| and |src.1|. This is to update their timestamps.
    pub async fn acquire_context_for_replace(
        &self,
        src: Option<(&Directory<S>, &str)>,
        dst: &str,
        borrow_metadata_space: bool,
    ) -> Result<ReplaceContext<'_>, Error> {
        // Since we don't know the child object ID until we've looked up the child, we need to loop
        // until we have acquired a lock on a child whose ID is the same as it was in the last
        // iteration. This also applies for src object ID if |src| is passed in.
        //
        // Note that the returned transaction may lock more objects than is necessary (for example,
        // if the child "foo" was first a directory, then was renamed to "bar" and a file "foo" was
        // created, we might acquire a lock on both the parent and "bar").
        //
        // We can look into not having this loop by adding support to try to add locks in the
        // transaction. If it fails, we can drop all the locks and start a new transaction.
        let store = self.store();
        let fs = store.filesystem();
        let mut child_object_id = INVALID_OBJECT_ID;
        let mut src_object_id = src.map(|_| INVALID_OBJECT_ID);
        let mut lock_keys = LockKeys::with_capacity(4);
        lock_keys.push(LockKey::object(store.store_object_id(), self.object_id()));
        loop {
            lock_keys.truncate(1);
            if let Some(src) = src {
                lock_keys.push(LockKey::object(store.store_object_id(), src.0.object_id()));
                if let Some(src_object_id) = src_object_id {
                    if src_object_id != INVALID_OBJECT_ID {
                        lock_keys.push(LockKey::object(store.store_object_id(), src_object_id));
                    }
                }
            }
            if child_object_id != INVALID_OBJECT_ID {
                lock_keys.push(LockKey::object(store.store_object_id(), child_object_id));
            };
            let truncate_guard = if child_object_id != INVALID_OBJECT_ID {
                Some(fs.truncate_guard_owned(store.store_object_id(), child_object_id).await)
            } else {
                None
            };
            let transaction = store
                .new_transaction(
                    lock_keys.clone(),
                    Options { borrow_metadata_space, ..Default::default() },
                )
                .await?;

            let mut have_required_locks = true;
            let mut src_id_and_descriptor = None;
            let mut src_name_out = None;
            let mut dst_name_out = None;
            if let Some((src_dir, src_name)) = src {
                match src_dir.lookup_ext(src_name).await? {
                    Some(entry) => match entry.descriptor {
                        ObjectDescriptor::File
                        | ObjectDescriptor::Directory
                        | ObjectDescriptor::Symlink => {
                            if src_object_id != Some(entry.object_id) {
                                have_required_locks = false;
                                src_object_id = Some(entry.object_id);
                            }
                            src_id_and_descriptor = Some((entry.object_id, entry.descriptor));
                            src_name_out = Some(
                                src_dir
                                    .get_case_preserved_name(entry.key)
                                    .await?
                                    .unwrap_or_else(|| src_name.to_string()),
                            );
                        }
                        _ => bail!(FxfsError::Inconsistent),
                    },
                    None => {
                        // Can't find src.0/src.1
                        bail!(FxfsError::NotFound)
                    }
                }
            };
            let dst_entry = self.lookup_ext(dst).await?;
            let dst_id_and_descriptor = match dst_entry {
                Some(entry) => match entry.descriptor {
                    ObjectDescriptor::File
                    | ObjectDescriptor::Directory
                    | ObjectDescriptor::Symlink => {
                        if child_object_id != entry.object_id {
                            have_required_locks = false;
                            child_object_id = entry.object_id
                        }
                        dst_name_out = Some(
                            self.get_case_preserved_name(entry.key)
                                .await?
                                .unwrap_or_else(|| dst.to_string()),
                        );
                        Some((entry.object_id, entry.descriptor.clone()))
                    }
                    _ => bail!(FxfsError::Inconsistent),
                },
                None => {
                    if child_object_id != INVALID_OBJECT_ID {
                        have_required_locks = false;
                        child_object_id = INVALID_OBJECT_ID;
                    }
                    None
                }
            };
            if have_required_locks {
                return Ok(ReplaceContext {
                    transaction,
                    truncate_guard,
                    src_id_and_descriptor,
                    dst_id_and_descriptor,
                    src_name: src_name_out,
                    dst_name: dst_name_out,
                });
            }
        }
    }

    async fn has_children(&self) -> Result<bool, Error> {
        if self.is_deleted() {
            return Ok(false);
        }
        let layer_set = self.store().tree().layer_set();
        let mut merger = layer_set.merger();
        Ok(self.iter(&mut merger).await?.get().is_some())
    }

    /// Returns the object ID and descriptor for the given child, or None if not found. If found,
    /// also returns a boolean indicating whether or not the parent directory was locked during the
    /// lookup.
    #[trace]
    pub async fn lookup(&self, name: &str) -> Result<Option<(u64, ObjectDescriptor, bool)>, Error> {
        Ok(self
            .lookup_ext(name)
            .await?
            .map(|entry| (entry.object_id, entry.descriptor, entry.locked)))
    }

    /// Like lookup, but also returns the key that was found.
    #[trace]
    pub async fn lookup_ext(&self, name: &str) -> Result<Option<LookupEntry>, Error> {
        let _measure =
            crate::metrics::DurationMeasureScope::new(&crate::metrics::directory_metrics().lookup);
        if self.is_deleted() {
            return Ok(None);
        }
        let cipher;
        let proxy_name;
        // In some cases, we need to iterate over directory entries to find a match.  The code below
        // finds a starting key and an optional predicate that is used to find a matching entry.
        // If there is no predicate, we can look for an exact match.
        let (key, predicate, locked): (_, Option<BoxPredicate<'_>>, _) = if self
            .dir_type()
            .is_encrypted()
        {
            cipher = self.get_fscrypt_key().await?;
            match &cipher {
                CipherHolder::Cipher(cipher) => {
                    if self.dir_type().is_casefold() {
                        // We must iterate over all directory entries that have a matching hash code
                        // until we find a match.
                        let target_hash_code = cipher.hash_code_casefold(name);
                        let key = ObjectKey::encrypted_child(
                            self.object_id(),
                            vec![],
                            Some(target_hash_code),
                        );
                        (
                            key,
                            Some(Box::new(encrypted_casefold_predicate(
                                cipher.as_ref(),
                                self.object_id(),
                                target_hash_code,
                                name,
                            ))),
                            false,
                        )
                    } else {
                        let encrypted_name =
                            encrypt_filename(cipher.as_ref(), self.object_id(), name)?;
                        let hash_code = cipher.hash_code(encrypted_name.as_bytes(), name);
                        (
                            ObjectKey::encrypted_child(self.object_id(), encrypted_name, hash_code),
                            None,
                            false,
                        )
                    }
                }
                CipherHolder::Unavailable => {
                    proxy_name = match ProxyFilename::try_from(name) {
                        Ok(name) => name,
                        Err(_) => return Ok(None),
                    };
                    let (key, predicate) =
                        self.get_key_and_predicate_for_unavailable_cipher(&proxy_name);
                    (key, predicate, true)
                }
            }
        } else {
            match self.dir_type() {
                DirType::Casefold => {
                    let target_key = ObjectKey::child(self.object_id(), name, DirType::Casefold);
                    let target_hash_code = match &target_key.data {
                        ObjectKeyData::CasefoldChild { hash_code, .. } => *hash_code,
                        _ => unreachable!(),
                    };
                    (
                        ObjectKey {
                            object_id: self.object_id(),
                            data: ObjectKeyData::CasefoldChild {
                                hash_code: target_hash_code,
                                name: "".to_string(),
                            },
                        },
                        Some(Box::new(casefold_predicate(
                            self.object_id(),
                            target_hash_code,
                            name,
                        ))),
                        false,
                    )
                }
                DirType::LegacyCasefold | DirType::Normal => {
                    (ObjectKey::child(self.object_id(), name, self.dir_type()), None, false)
                }
                DirType::Encrypted(_) | DirType::EncryptedCasefold(_) => {
                    unreachable!("is_encrypted() was already checked")
                }
            }
        };

        // If the directory is locked, we don't want to use `LMSTree::find` because it caches
        // results, and if the directory later becomes unlocked, we don't want the cache to yield
        // entries from when it was locked.
        if locked || predicate.is_some() {
            let layer_set = self.store().tree().layer_set();
            let mut merger = layer_set.merger();
            let mut iter = merger.query(Query::FullRange(&key)).await?;
            if let Some(predicate) = predicate {
                if !self.advance_until(&mut iter, predicate).await? {
                    return Ok(None);
                }
            } else if iter
                .get()
                .is_none_or(|item| item.key != &key || matches!(item.value, ObjectValue::None))
            {
                return Ok(None);
            }
            let item = iter.get().unwrap();
            match item.value {
                ObjectValue::Child(ChildValue { object_id, object_descriptor }) => {
                    Ok(Some(LookupEntry {
                        object_id: *object_id,
                        descriptor: object_descriptor.clone(),
                        key: item.key.clone(),
                        locked,
                    }))
                }
                _ => Err(anyhow!(FxfsError::Inconsistent)
                    .context(format!("Unexpected item in lookup: {item:?}"))),
            }
        } else {
            self.store()
                .tree()
                .find_map(&key, |item| match item.value {
                    ObjectValue::Child(ChildValue { object_id, object_descriptor }) => {
                        Ok(LookupEntry {
                            object_id: *object_id,
                            descriptor: *object_descriptor,
                            key: item.key.clone(),
                            locked: false,
                        })
                    }
                    _ => Err(anyhow!(FxfsError::Inconsistent)
                        .context(format!("Unexpected item in lookup: {item:?}",))),
                })
                .await?
                .transpose()
        }
    }

    pub async fn create_child_dir(
        &self,
        transaction: &mut Transaction<'_>,
        name: &str,
    ) -> Result<Directory<S>, Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);

        let handle =
            Directory::create_with_options(transaction, self.owner(), self.dir_type()).await?;
        if self.dir_type().is_encrypted() {
            let fscrypt_key =
                self.get_fscrypt_key().await?.into_cipher().ok_or(FxfsError::NoKey)?;
            let encrypted_name =
                encrypt_filename(&*fscrypt_key, self.object_id(), name).expect("encrypt_filename");
            let hash_code = if self.dir_type().is_casefold() {
                Some(fscrypt_key.hash_code_casefold(name))
            } else {
                fscrypt_key.hash_code(encrypted_name.as_bytes(), name)
            };
            transaction.add(
                self.store().store_object_id(),
                Mutation::replace_or_insert_object(
                    ObjectKey::encrypted_child(self.object_id(), encrypted_name, hash_code),
                    ObjectValue::child(handle.object_id(), ObjectDescriptor::Directory),
                ),
            );
        } else {
            transaction.add(
                self.store().store_object_id(),
                Mutation::replace_or_insert_object(
                    ObjectKey::child(self.object_id(), &name, self.dir_type()),
                    ObjectValue::child(handle.object_id(), ObjectDescriptor::Directory),
                ),
            );
        }
        let now = Timestamp::now();
        self.update_dir_attributes_internal(
            transaction,
            self.object_id(),
            MutableAttributesInternal {
                sub_dirs: 1,
                modification_time: Some(now.as_nanos()),
                change_time: Some(now),
                ..Default::default()
            },
        )
        .await?;
        self.copy_project_id_to_object_in_txn(transaction, handle.object_id())?;
        Ok(handle)
    }

    pub async fn add_child_file<'a>(
        &self,
        transaction: &mut Transaction<'a>,
        name: &str,
        handle: &DataObjectHandle<S>,
    ) -> Result<(), Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);
        if self.dir_type().is_encrypted() {
            let fscrypt_key =
                self.get_fscrypt_key().await?.into_cipher().ok_or(FxfsError::NoKey)?;
            let encrypted_name =
                encrypt_filename(&*fscrypt_key, self.object_id(), name).expect("encrypt_filename");
            let hash_code = if self.dir_type().is_casefold() {
                Some(fscrypt_key.hash_code_casefold(name))
            } else {
                fscrypt_key.hash_code(encrypted_name.as_bytes(), name)
            };
            transaction.add(
                self.store().store_object_id(),
                Mutation::replace_or_insert_object(
                    ObjectKey::encrypted_child(self.object_id(), encrypted_name, hash_code),
                    ObjectValue::child(handle.object_id(), ObjectDescriptor::File),
                ),
            );
        } else {
            transaction.add(
                self.store().store_object_id(),
                Mutation::replace_or_insert_object(
                    ObjectKey::child(self.object_id(), &name, self.dir_type()),
                    ObjectValue::child(handle.object_id(), ObjectDescriptor::File),
                ),
            );
        }
        let now = Timestamp::now();
        self.update_dir_attributes_internal(
            transaction,
            self.object_id(),
            MutableAttributesInternal {
                modification_time: Some(now.as_nanos()),
                change_time: Some(now),
                ..Default::default()
            },
        )
        .await
    }

    // This applies the project id of this directory (if nonzero) to an object. The method assumes
    // both this and child objects are already present in the mutations of the provided
    // transactions and that the child is of of zero size. This is meant for use inside
    // `create_child_file()` and `create_child_dir()` only, where such assumptions are safe.
    fn copy_project_id_to_object_in_txn<'a>(
        &self,
        transaction: &mut Transaction<'a>,
        object_id: u64,
    ) -> Result<(), Error> {
        let store_id = self.store().store_object_id();
        // This mutation must already be in here as we've just modified the mtime.
        let ObjectValue::Object { attributes: ObjectAttributes { project_id, .. }, .. } =
            transaction
                .get_object_mutation(store_id, ObjectKey::object(self.object_id()))
                .unwrap()
                .item
                .value
        else {
            return Err(anyhow!(FxfsError::Inconsistent));
        };
        if let Some(project_id) = project_id {
            // This mutation must be present as well since we've just created the object. So this
            // replaces it.
            let mut mutation = transaction
                .get_object_mutation(store_id, ObjectKey::object(object_id))
                .unwrap()
                .clone();
            if let ObjectValue::Object {
                attributes: ObjectAttributes { project_id: child_project_id, .. },
                ..
            } = &mut mutation.item.value
            {
                *child_project_id = Some(project_id);
            } else {
                return Err(anyhow!(FxfsError::Inconsistent));
            }
            transaction.add(store_id, Mutation::ObjectStore(mutation));
            transaction.add(
                store_id,
                Mutation::merge_object(
                    ObjectKey::project_usage(self.store().root_directory_object_id(), project_id),
                    ObjectValue::BytesAndNodes { bytes: 0, nodes: 1 },
                ),
            );
        }
        Ok(())
    }

    pub async fn create_child_file<'a>(
        &self,
        transaction: &mut Transaction<'a>,
        name: &str,
    ) -> Result<DataObjectHandle<S>, Error> {
        self.create_child_file_with_options(transaction, name, HandleOptions::default()).await
    }

    pub async fn create_child_file_with_options<'a>(
        &self,
        transaction: &mut Transaction<'a>,
        name: &str,
        options: HandleOptions,
    ) -> Result<DataObjectHandle<S>, Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);
        let wrapping_key_id = self.wrapping_key_id();
        let handle =
            ObjectStore::create_object(self.owner(), transaction, options, wrapping_key_id).await?;
        self.add_child_file(transaction, name, &handle).await?;
        self.copy_project_id_to_object_in_txn(transaction, handle.object_id())?;
        Ok(handle)
    }

    pub async fn create_child_unnamed_temporary_file<'a>(
        &self,
        transaction: &mut Transaction<'a>,
    ) -> Result<DataObjectHandle<S>, Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);
        let wrapping_key_id = self.wrapping_key_id();
        let handle = ObjectStore::create_object(
            self.owner(),
            transaction,
            HandleOptions::default(),
            wrapping_key_id,
        )
        .await?;

        // Copy project ID from self to the created file object.
        let ObjectValue::Object { attributes: ObjectAttributes { project_id, .. }, .. } = self
            .store()
            .txn_get_object_mutation(&transaction, self.object_id())
            .await
            .unwrap()
            .item
            .value
        else {
            bail!(
                anyhow!(FxfsError::Inconsistent)
                    .context("Directory.create_child_file_with_options: expected mutation object")
            );
        };

        // Update the object mutation with parent's project ID.
        let mut child_mutation = transaction
            .get_object_mutation(
                self.store().store_object_id(),
                ObjectKey::object(handle.object_id()),
            )
            .unwrap()
            .clone();
        if let ObjectValue::Object {
            attributes: ObjectAttributes { project_id: child_project_id, .. },
            ..
        } = &mut child_mutation.item.value
        {
            *child_project_id = project_id;
        } else {
            bail!(
                anyhow!(FxfsError::Inconsistent)
                    .context("Directory.create_child_file_with_options: expected file object")
            );
        }
        transaction.add(self.store().store_object_id(), Mutation::ObjectStore(child_mutation));

        // Add object to graveyard - the object should be removed on remount.
        self.store().add_to_graveyard(transaction, handle.object_id());

        Ok(handle)
    }

    pub async fn create_symlink(
        &self,
        transaction: &mut Transaction<'_>,
        link: &[u8],
        name: &str,
    ) -> Result<u64, Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);
        // Limit the length of link that might be too big to put in the tree.
        // https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/limits.h.html.
        // See _POSIX_SYMLINK_MAX.
        ensure!(link.len() <= 256, FxfsError::BadPath);
        let reserved_symlink_id = self.store().get_next_object_id().await?;
        let symlink_id = reserved_symlink_id.get();
        let mut link = link.to_vec();

        match self.dir_type() {
            DirType::Encrypted(wrapping_key_id) | DirType::EncryptedCasefold(wrapping_key_id) => {
                if let Some(crypt) = self.store().crypt() {
                    let (key, unwrapped_key) = crypt
                        .create_key_with_id(symlink_id, wrapping_key_id, ObjectType::Symlink)
                        .await?;

                    // Note that it's possible that this entry gets inserted into the key manager but
                    // this transaction doesn't get committed. This shouldn't be a problem because
                    // unused keys get purged on a standard timeout interval and this key shouldn't
                    // conflict with any other keys.
                    let cipher = key_to_cipher(&key, &unwrapped_key)?;
                    self.store().key_manager.insert(
                        symlink_id,
                        Arc::new(
                            vec![(FSCRYPT_KEY_ID, CipherHolder::Cipher(cipher.clone()))].into(),
                        ),
                        false,
                    );

                    let dir_key =
                        self.get_fscrypt_key().await?.into_cipher().ok_or(FxfsError::NoKey)?;
                    let encrypted_name = encrypt_filename(&*dir_key, self.object_id(), name)?;
                    let hash_code = if self.dir_type().is_casefold() {
                        Some(dir_key.hash_code_casefold(name))
                    } else {
                        dir_key.hash_code(encrypted_name.as_bytes(), name)
                    };
                    cipher.encrypt_symlink(symlink_id, &mut link)?;

                    transaction.add(
                        self.store().store_object_id(),
                        Mutation::insert_object(
                            ObjectKey::object(reserved_symlink_id.release().get()),
                            ObjectValue::encrypted_symlink(
                                link,
                                Timestamp::now(),
                                Timestamp::now(),
                                None,
                            ),
                        ),
                    );
                    transaction.add(
                        self.store().store_object_id(),
                        Mutation::insert_object(
                            ObjectKey::keys(symlink_id),
                            ObjectValue::keys(vec![(FSCRYPT_KEY_ID, key)].into()),
                        ),
                    );
                    transaction.add(
                        self.store().store_object_id(),
                        Mutation::replace_or_insert_object(
                            ObjectKey::encrypted_child(self.object_id(), encrypted_name, hash_code),
                            ObjectValue::child(symlink_id, ObjectDescriptor::Symlink),
                        ),
                    );
                } else {
                    return Err(anyhow!("No crypt"));
                }
            }
            _ => {
                transaction.add(
                    self.store().store_object_id(),
                    Mutation::insert_object(
                        ObjectKey::object(reserved_symlink_id.release().get()),
                        ObjectValue::symlink(link, Timestamp::now(), Timestamp::now(), None),
                    ),
                );
                transaction.add(
                    self.store().store_object_id(),
                    Mutation::replace_or_insert_object(
                        ObjectKey::child(self.object_id(), &name, self.dir_type()),
                        ObjectValue::child(symlink_id, ObjectDescriptor::Symlink),
                    ),
                );
            }
        }

        let now = Timestamp::now();
        self.update_dir_attributes_internal(
            transaction,
            self.object_id(),
            MutableAttributesInternal {
                modification_time: Some(now.as_nanos()),
                change_time: Some(now),
                ..Default::default()
            },
        )
        .await?;
        Ok(symlink_id)
    }

    pub async fn add_child_volume(
        &self,
        transaction: &mut Transaction<'_>,
        volume_name: &str,
        store_object_id: u64,
    ) -> Result<(), Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);
        transaction.add(
            self.store().store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::child(self.object_id(), volume_name, self.dir_type()),
                ObjectValue::child(store_object_id, ObjectDescriptor::Volume),
            ),
        );
        let now = Timestamp::now();
        self.update_dir_attributes_internal(
            transaction,
            self.object_id(),
            MutableAttributesInternal {
                modification_time: Some(now.as_nanos()),
                change_time: Some(now),
                ..Default::default()
            },
        )
        .await
    }

    pub fn delete_child_volume<'a>(
        &self,
        transaction: &mut Transaction<'a>,
        volume_name: &str,
        store_object_id: u64,
    ) -> Result<(), Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);
        transaction.add(
            self.store().store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::child(self.object_id(), volume_name, self.dir_type()),
                ObjectValue::None,
            ),
        );
        // We note in the journal that we've deleted the volume. ObjectManager applies this
        // mutation by forgetting the store. We do it this way to ensure that the store is removed
        // during replay where there may be mutations to the store prior to its deletion. Without
        // this, we will try (and fail) to open the store after replay.
        transaction.add(store_object_id, Mutation::DeleteVolume);
        Ok(())
    }

    /// Inserts a child into the directory.
    ///
    /// Requires transaction locks on |self|.
    pub async fn insert_child<'a>(
        &self,
        transaction: &mut Transaction<'a>,
        name: &str,
        object_id: u64,
        descriptor: ObjectDescriptor,
    ) -> Result<(), Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);
        let sub_dirs_delta = if descriptor == ObjectDescriptor::Directory { 1 } else { 0 };
        if self.dir_type().is_encrypted() {
            let fscrypt_key =
                self.get_fscrypt_key().await?.into_cipher().ok_or(FxfsError::NoKey)?;
            let encrypted_name = encrypt_filename(&*fscrypt_key, self.object_id(), name)?;
            let hash_code = if self.dir_type().is_casefold() {
                Some(fscrypt_key.hash_code_casefold(name))
            } else {
                fscrypt_key.hash_code(encrypted_name.as_bytes(), name)
            };
            transaction.add(
                self.store().store_object_id(),
                Mutation::replace_or_insert_object(
                    ObjectKey::encrypted_child(self.object_id(), encrypted_name, hash_code),
                    ObjectValue::child(object_id, descriptor),
                ),
            );
        } else {
            transaction.add(
                self.store().store_object_id(),
                Mutation::replace_or_insert_object(
                    ObjectKey::child(self.object_id(), &name, self.dir_type()),
                    ObjectValue::child(object_id, descriptor),
                ),
            );
        }
        let now = Timestamp::now();
        self.update_dir_attributes_internal(
            transaction,
            self.object_id(),
            MutableAttributesInternal {
                sub_dirs: sub_dirs_delta,
                modification_time: Some(now.as_nanos()),
                change_time: Some(now),
                ..Default::default()
            },
        )
        .await
    }

    /// Updates attributes for the directory.
    /// Nb: The `casefold` attribute is ignored here. It should be set/cleared via `set_casefold()`.
    pub async fn update_attributes<'a>(
        &self,
        mut transaction: Transaction<'a>,
        node_attributes: Option<&fio::MutableNodeAttributes>,
        sub_dirs_delta: i64,
        change_time: Option<Timestamp>,
    ) -> Result<(), Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);

        if sub_dirs_delta != 0 {
            let mut mutation =
                self.store().txn_get_object_mutation(&transaction, self.object_id()).await?;
            if let ObjectValue::Object { kind: ObjectKind::Directory { sub_dirs, .. }, .. } =
                &mut mutation.item.value
            {
                *sub_dirs = sub_dirs.saturating_add_signed(sub_dirs_delta);
            } else {
                bail!(
                    anyhow!(FxfsError::Inconsistent)
                        .context("Directory.update_attributes: expected directory object")
                );
            };

            transaction.add(self.store().store_object_id(), Mutation::ObjectStore(mutation));
        }

        let wrapping_key =
            if let Some(fio::MutableNodeAttributes { wrapping_key_id: Some(id), .. }) =
                node_attributes
            {
                Some((*id, self.set_wrapping_key(&mut transaction, *id).await?))
            } else {
                None
            };

        // Delegate to the StoreObjectHandle update_attributes for the rest of the updates.
        if node_attributes.is_some() || change_time.is_some() {
            self.handle.update_attributes(&mut transaction, node_attributes, change_time).await?;
        }
        transaction
            .commit_with_callback(|_| {
                if let Some((wrapping_key_id, cipher)) = wrapping_key {
                    {
                        let mut dir_type = self.dir_type.lock();
                        *dir_type = match *dir_type {
                            DirType::Normal => DirType::Encrypted(wrapping_key_id),
                            DirType::Casefold => DirType::EncryptedCasefold(wrapping_key_id),
                            _ => *dir_type,
                        };
                    }
                    self.store().key_manager.merge(self.object_id(), |existing| match existing {
                        Some(existing) => {
                            let mut cipher_set = (**existing).clone();
                            cipher_set.add_key(FSCRYPT_KEY_ID, CipherHolder::Cipher(cipher));
                            Arc::new(cipher_set)
                        }
                        None => {
                            Arc::new(vec![(FSCRYPT_KEY_ID, CipherHolder::Cipher(cipher))].into())
                        }
                    });
                }
            })
            .await?;
        Ok(())
    }

    /// Updates attributes set in `mutable_node_attributes`. MutableAttributesInternal can be
    /// extended but should never include wrapping_key_id. Useful for object store Directory
    /// methods that only have access to a reference to a transaction.
    pub async fn update_dir_attributes_internal<'a>(
        &self,
        transaction: &mut Transaction<'a>,
        object_id: u64,
        mutable_node_attributes: MutableAttributesInternal,
    ) -> Result<(), Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);

        let mut mutation = self.store().txn_get_object_mutation(transaction, object_id).await?;
        if let ObjectValue::Object {
            kind: ObjectKind::Directory { sub_dirs, .. },
            attributes,
            ..
        } = &mut mutation.item.value
        {
            if let Some(time) = mutable_node_attributes.modification_time {
                attributes.modification_time = Timestamp::from_nanos(time);
            }
            if let Some(time) = mutable_node_attributes.change_time {
                attributes.change_time = time;
            }
            if mutable_node_attributes.sub_dirs != 0 {
                *sub_dirs = sub_dirs.saturating_add_signed(mutable_node_attributes.sub_dirs);
            }
            if let Some(time) = mutable_node_attributes.creation_time {
                attributes.creation_time = Timestamp::from_nanos(time);
            }
        } else {
            bail!(
                anyhow!(FxfsError::Inconsistent)
                    .context("Directory.update_attributes: expected directory object")
            );
        };
        transaction.add(self.store().store_object_id(), Mutation::ObjectStore(mutation));
        Ok(())
    }

    pub async fn get_properties(&self) -> Result<ObjectProperties, Error> {
        if self.is_deleted() {
            return Ok(ObjectProperties {
                refs: 0,
                allocated_size: 0,
                data_attribute_size: 0,
                creation_time: Timestamp::zero(),
                modification_time: Timestamp::zero(),
                access_time: Timestamp::zero(),
                change_time: Timestamp::zero(),
                sub_dirs: 0,
                posix_attributes: None,
                dir_type: DirType::Normal,
            });
        }

        let value = self
            .store()
            .tree()
            .find_value(&ObjectKey::object(self.object_id()))
            .await?
            .ok_or(FxfsError::NotFound)?;
        match value {
            ObjectValue::Object {
                kind: ObjectKind::Directory { sub_dirs, dir_type },
                attributes:
                    ObjectAttributes {
                        creation_time,
                        modification_time,
                        posix_attributes,
                        access_time,
                        change_time,
                        ..
                    },
            } => Ok(ObjectProperties {
                refs: 1,
                allocated_size: 0,
                data_attribute_size: 0,
                creation_time,
                modification_time,
                access_time,
                change_time,
                sub_dirs,
                posix_attributes,
                dir_type,
            }),
            _ => {
                bail!(
                    anyhow!(FxfsError::Inconsistent)
                        .context("get_properties: Expected object value")
                )
            }
        }
    }

    pub async fn list_extended_attributes(&self) -> Result<Vec<Vec<u8>>, Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);
        self.handle.list_extended_attributes().await
    }

    pub async fn get_extended_attribute(&self, name: Vec<u8>) -> Result<Vec<u8>, Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);
        self.handle.get_extended_attribute(name).await
    }

    pub async fn set_extended_attribute(
        &self,
        name: Vec<u8>,
        value: Vec<u8>,
        mode: SetExtendedAttributeMode,
    ) -> Result<(), Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);
        self.handle.set_extended_attribute(name, value, mode).await
    }

    pub async fn remove_extended_attribute(&self, name: Vec<u8>) -> Result<(), Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);
        self.handle.remove_extended_attribute(name).await
    }

    /// Returns an iterator that will return directory entries skipping deleted ones.  Example
    /// usage:
    ///
    ///   let layer_set = dir.store().tree().layer_set();
    ///   let mut merger = layer_set.merger();
    ///   let mut iter = dir.iter(&mut merger).await?;
    ///
    pub async fn iter<'a, 'b>(
        &self,
        merger: &'a mut Merger<'b, ObjectKey, ObjectValue>,
    ) -> Result<DirectoryIterator<'a, 'b>, Error> {
        // It might be tempting to always use `ObjectKeyData::Child` here knowing that it should
        // come earlier than any other directory entries, but directories can have extended
        // attributes, and `ObjectKeyData::ExtendedAttribute` sorts after `ObjectKeyData::Child` but
        // before `ObjectKeyData::EncryptedChild`.
        self.iter_from_key(
            merger,
            &if self.dir_type().is_encrypted() {
                // This will return ObjectKeyData::EncryptedCasefoldChild which sorts before
                // ObjectKeyData::EncryptedChild, so this should work even if not an encrypted
                // casefold directory.
                ObjectKey::encrypted_child(self.object_id(), Vec::new(), Some(0))
            } else {
                ObjectKey::child(self.object_id(), "", self.dir_type())
            },
        )
        .await
    }

    /// Like `iter`, but seeks from a specific key.
    pub async fn iter_from_key<'a, 'b>(
        &self,
        merger: &'a mut Merger<'b, ObjectKey, ObjectValue>,
        key: &ObjectKey,
    ) -> Result<DirectoryIterator<'a, 'b>, Error> {
        ensure!(!self.is_deleted(), FxfsError::Deleted);

        DirectoryIterator::new(
            self.object_id(),
            merger.query(Query::FullRange(key)).await?,
            if self.dir_type().is_encrypted() {
                self.get_fscrypt_key().await?.into_cipher()
            } else {
                None
            },
        )
        .await
    }

    /// Like "iter", but seeks from a specific filename (inclusive).  This should *not* be
    /// used for encrypted entries, because it won't decrypt entries (and will panic on
    /// a debug build).
    ///
    /// Example usage:
    ///
    ///   let layer_set = dir.store().tree().layer_set();
    ///   let mut merger = layer_set.merger();
    ///   let mut iter = dir.iter_from(&mut merger, "foo").await?;
    ///
    pub async fn iter_from<'a, 'b>(
        &self,
        merger: &'a mut Merger<'b, ObjectKey, ObjectValue>,
        from: &str,
    ) -> Result<DirectoryIterator<'a, 'b>, Error> {
        debug_assert!(!self.dir_type().is_encrypted());

        self.iter_from_key(merger, &ObjectKey::child(self.object_id(), from, self.dir_type())).await
    }

    /// Like "iter_from", but takes bytes which is expected to be a serialized ObjectKey.  This will
    /// decrypt encrypted entries if the key is available.  This should *not* be used for
    /// unencrypted directories.
    pub async fn iter_from_bytes<'a, 'b>(
        &self,
        merger: &'a mut Merger<'b, ObjectKey, ObjectValue>,
        from: &[u8],
    ) -> Result<DirectoryIterator<'a, 'b>, Error> {
        debug_assert!(self.dir_type().is_encrypted());

        self.iter_from_key(merger, &bincode::deserialize(&from).unwrap()).await
    }

    /// Skips over directory entries for this directory until `predicate` returns a match.  Returns
    /// `false` if there is no match.
    async fn advance_until(
        &self,
        iter: &mut MergerIterator<'_, '_, ObjectKey, ObjectValue>,
        predicate: impl Fn(&ObjectKey) -> ControlFlow<bool>,
    ) -> Result<bool, Error> {
        while let Some(item) = iter.get()
            && matches!(
                item,
                ItemRef { key: ObjectKey { object_id, .. }, .. }
                    if *object_id == self.object_id()
            )
        {
            match item {
                // Skip deleted items.
                ItemRef { value: ObjectValue::None, .. } => {}
                ItemRef { key, .. } => match predicate(key) {
                    ControlFlow::Continue(()) => {}
                    ControlFlow::Break(result) => return Ok(result),
                },
            }
            iter.advance().await?
        }
        Ok(false)
    }

    /// Returns the starting key and an optional predicate (where an iteration is required) to be
    /// used when the cipher is unavailable.
    fn get_key_and_predicate_for_unavailable_cipher<'a>(
        &self,
        proxy_name: &'a ProxyFilename,
    ) -> (ObjectKey, Option<BoxPredicate<'a>>) {
        if self.dir_type().is_casefold() {
            (
                ObjectKey::encrypted_child(
                    self.object_id(),
                    proxy_name.raw_filename().to_vec(),
                    Some(proxy_name.hash_code as u32),
                ),
                proxy_name
                    .is_truncated()
                    .then(|| Box::new(long_proxy_prefix_casefold_predicate(&proxy_name)) as Box<_>),
            )
        } else {
            (
                ObjectKey::encrypted_child(
                    self.object_id(),
                    proxy_name.raw_filename().to_vec(),
                    None,
                ),
                proxy_name
                    .is_truncated()
                    .then(|| Box::new(long_proxy_prefix_predicate(&proxy_name)) as Box<_>),
            )
        }
    }
}

/// Used to find an encrypted casefold entry when the cipher is available.
fn encrypted_casefold_predicate<'a>(
    cipher: &'a dyn Cipher,
    object_id: u64,
    target_hash_code: u32,
    name: &'a str,
) -> impl Fn(&ObjectKey) -> ControlFlow<bool> + 'a {
    move |key| match key {
        ObjectKey {
            data:
                ObjectKeyData::EncryptedCasefoldChild(EncryptedCasefoldChild {
                    hash_code,
                    name: encrypted_name,
                }),
            ..
        } if *hash_code == target_hash_code => {
            let decrypted_name = decrypt_filename(cipher, object_id, encrypted_name);
            match decrypted_name {
                Ok(decrypted_name) => {
                    if fxfs_unicode::casefold_cmp(name, &decrypted_name)
                        == std::cmp::Ordering::Equal
                    {
                        ControlFlow::Break(true)
                    } else {
                        ControlFlow::Continue(())
                    }
                }
                Err(_) => ControlFlow::Continue(()),
            }
        }
        _ => ControlFlow::Break(false),
    }
}

fn casefold_predicate(
    object_id: u64,
    target_hash_code: u32,
    name: &str,
) -> impl Fn(&ObjectKey) -> ControlFlow<bool> + '_ {
    move |key| match key {
        ObjectKey {
            object_id: oid,
            data: ObjectKeyData::CasefoldChild { hash_code, name: actual_name },
        } if *oid == object_id && *hash_code == target_hash_code => {
            if fxfs_unicode::casefold_cmp(name, actual_name) == std::cmp::Ordering::Equal {
                ControlFlow::Break(true)
            } else {
                ControlFlow::Continue(())
            }
        }
        _ => ControlFlow::Break(false),
    }
}

/// Used when a long proxy prefix is used with case folding.
fn long_proxy_prefix_casefold_predicate(
    proxy_name: &ProxyFilename,
) -> impl Fn(&ObjectKey) -> ControlFlow<bool> + '_ {
    move |key| match key {
        ObjectKey {
            data: ObjectKeyData::EncryptedCasefoldChild(EncryptedCasefoldChild { hash_code, name }),
            ..
        } if *hash_code as u64 == proxy_name.hash_code
            && name.starts_with(&proxy_name.filename) =>
        {
            if ProxyFilename::compute_sha256(&name) == proxy_name.sha256 {
                ControlFlow::Break(true)
            } else {
                ControlFlow::Continue(())
            }
        }
        _ => ControlFlow::Break(false),
    }
}

/// Used when a long proxy prefix is used without case folding.
fn long_proxy_prefix_predicate(
    proxy_name: &ProxyFilename,
) -> impl Fn(&ObjectKey) -> ControlFlow<bool> + '_ {
    move |key| match key {
        ObjectKey { data: ObjectKeyData::EncryptedChild(EncryptedChild(name)), .. }
            if name.starts_with(&proxy_name.filename) =>
        {
            if ProxyFilename::compute_hash_code(name) == proxy_name.hash_code
                && ProxyFilename::compute_sha256(name) == proxy_name.sha256
            {
                ControlFlow::Break(true)
            } else {
                ControlFlow::Continue(())
            }
        }
        _ => ControlFlow::Break(false),
    }
}

impl<S: HandleOwner> fmt::Debug for Directory<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Directory")
            .field("store_id", &self.store().store_object_id())
            .field("object_id", &self.object_id())
            .finish()
    }
}

pub struct DirectoryIterator<'a, 'b> {
    object_id: u64,
    iter: MergerIterator<'a, 'b, ObjectKey, ObjectValue>,
    cipher: Option<Arc<dyn Cipher>>,
    // Holds decrypted or proxy filenames so we can return a reference from get().
    filename: Option<String>,
}

impl<'a, 'b> DirectoryIterator<'a, 'b> {
    pub async fn new(
        object_id: u64,
        iter: MergerIterator<'a, 'b, ObjectKey, ObjectValue>,
        cipher: Option<Arc<dyn Cipher>>,
    ) -> Result<Self, Error> {
        let mut this = DirectoryIterator { object_id, iter, cipher, filename: None };
        this.init_item().await?;
        Ok(this)
    }

    pub fn get(&self) -> Option<(&str, u64, &ObjectDescriptor)> {
        match self.iter.get() {
            Some(ItemRef {
                key: ObjectKey { object_id: oid, data: ObjectKeyData::Child { name } },
                value: ObjectValue::Child(ChildValue { object_id, object_descriptor }),
                ..
            }) if *oid == self.object_id => Some((&name, *object_id, object_descriptor)),
            Some(ItemRef {
                key:
                    ObjectKey {
                        object_id: oid,
                        data: ObjectKeyData::CasefoldChild { hash_code: _, name },
                    },
                value: ObjectValue::Child(ChildValue { object_id, object_descriptor }),
                ..
            }) if *oid == self.object_id => Some((&name, *object_id, object_descriptor)),
            Some(ItemRef {
                key: ObjectKey { object_id: oid, data: ObjectKeyData::LegacyCasefoldChild(name) },
                value: ObjectValue::Child(ChildValue { object_id, object_descriptor }),
                ..
            }) if *oid == self.object_id => Some((name.as_str(), *object_id, object_descriptor)),
            Some(ItemRef {
                key: ObjectKey { object_id: oid, data: ObjectKeyData::EncryptedChild(_) },
                value: ObjectValue::Child(ChildValue { object_id, object_descriptor }),
                ..
            }) if *oid == self.object_id => {
                Some((self.filename.as_ref().unwrap(), *object_id, object_descriptor))
            }
            Some(ItemRef {
                key: ObjectKey { object_id: oid, data: ObjectKeyData::EncryptedCasefoldChild(_) },
                value: ObjectValue::Child(ChildValue { object_id, object_descriptor }),
                ..
            }) if *oid == self.object_id => {
                Some((self.filename.as_ref().unwrap(), *object_id, object_descriptor))
            }
            _ => None,
        }
    }

    pub async fn advance(&mut self) -> Result<(), Error> {
        self.iter.advance().await?;
        self.init_item().await
    }

    /// Returns a traversal position.
    pub fn traversal_position<R>(
        &self,
        name_visitor: impl FnOnce(&str) -> R,
        bytes_visitor: impl FnOnce(Box<[u8]>) -> R,
    ) -> Option<R> {
        match self.iter.get() {
            Some(ItemRef {
                key: ObjectKey { object_id: oid, data: ObjectKeyData::Child { name } },
                ..
            }) if *oid == self.object_id => Some(name_visitor(name)),
            Some(ItemRef {
                key:
                    ObjectKey {
                        object_id: oid,
                        data: ObjectKeyData::CasefoldChild { hash_code: _, name },
                    },
                ..
            }) if *oid == self.object_id => Some(name_visitor(&name)),
            Some(ItemRef {
                key: ObjectKey { object_id: oid, data: ObjectKeyData::LegacyCasefoldChild(name) },
                ..
            }) if *oid == self.object_id => Some(name_visitor(name.as_str())),

            Some(ItemRef {
                key:
                    key @ ObjectKey {
                        object_id: oid,
                        data:
                            ObjectKeyData::EncryptedChild(_) | ObjectKeyData::EncryptedCasefoldChild(_),
                    },
                ..
            }) if *oid == self.object_id => {
                Some(bytes_visitor(bincode::serialize(key).unwrap().into()))
            }
            _ => None,
        }
    }

    /// Called to initialize the item after the iterator has moved.
    async fn init_item(&mut self) -> Result<(), Error> {
        loop {
            match self.iter.get() {
                Some(ItemRef {
                    key: ObjectKey { object_id, .. },
                    value: ObjectValue::None,
                    ..
                }) if *object_id == self.object_id => {}
                Some(ItemRef {
                    key:
                        ObjectKey {
                            object_id,
                            data:
                                ObjectKeyData::EncryptedCasefoldChild(EncryptedCasefoldChild {
                                    hash_code,
                                    name,
                                }),
                        },
                    value: ObjectValue::Child(_),
                    ..
                }) if *object_id == self.object_id => {
                    // We decrypt filenames on advance. This allows us to return errors on bad data
                    // and avoids repeated work if the user calls get() more than once.
                    self.update_encrypted_filename(Some(*hash_code), name.clone())?;
                    return Ok(());
                }
                Some(ItemRef {
                    key:
                        ObjectKey {
                            object_id,
                            data: ObjectKeyData::EncryptedChild(EncryptedChild(name)),
                        },
                    value: ObjectValue::Child(_),
                    ..
                }) if *object_id == self.object_id => {
                    // We decrypt filenames on advance. This allows us to return errors on bad data
                    // and avoids repeated work if the user calls get() more than once.
                    self.update_encrypted_filename(None, name.clone())?;
                    return Ok(());
                }
                _ => return Ok(()),
            }
            self.iter.advance().await?;
        }
    }

    // For encrypted children, we calculate the filename once and cache it.  This function is called
    // to update that cached name.
    fn update_encrypted_filename(
        &mut self,
        hash_code: Option<u32>,
        mut name: Vec<u8>,
    ) -> Result<(), Error> {
        if let Some(cipher) = &self.cipher {
            cipher.decrypt_filename(self.object_id, &mut name)?;
            self.filename = Some(String::from_utf8(name).map_err(|_| {
                anyhow!(FxfsError::Internal).context("Bad UTF-8 encrypted filename")
            })?);
        } else if let Some(hash_code) = hash_code {
            self.filename = Some(ProxyFilename::new_with_hash_code(hash_code as u64, &name).into());
        } else {
            self.filename = Some(ProxyFilename::new(&name).into());
        }
        Ok(())
    }
}

/// Return type for |replace_child| describing the object which was replaced. The u64 fields are all
/// object_ids.
#[derive(Debug)]
pub enum ReplacedChild {
    None,

    // "Object" can be a file or symbolic link, but not a directory.
    Object(u64),
    ObjectWithRemainingLinks(u64),
    Directory(u64),

    // The file or symlink was purged directly in the single transaction.
    Purged(u64),
}

/// Moves src.0/src.1 to dst.0/dst.1.
///
/// If |dst.0| already has a child |dst.1|, it is removed from dst.0.  For files, if this was their
/// last reference, the file is moved to the graveyard.  For directories, the removed directory will
/// be deleted permanently (and must be empty).
///
/// If |src| is None, this is effectively the same as unlink(dst.0/dst.1).
pub async fn replace_child<'a, S: HandleOwner>(
    transaction: &mut Transaction<'a>,
    src: Option<(&'a Directory<S>, &str)>,
    dst: (&'a Directory<S>, &str),
) -> Result<ReplacedChild, Error> {
    replace_child_with_purge(transaction, src, dst, false).await
}

/// Like `replace_child`, but if `can_purge` is true and the replaced child is a file or symlink
/// with no remaining references, it will attempt to purge the object directly in the current
/// transaction, skipping the graveyard.
pub async fn replace_child_with_purge<'a, S: HandleOwner>(
    transaction: &mut Transaction<'a>,
    src: Option<(&'a Directory<S>, &str)>,
    dst: (&'a Directory<S>, &str),
    can_purge: bool,
) -> Result<ReplacedChild, Error> {
    let mut sub_dirs_delta: i64 = 0;
    let now = Timestamp::now();

    let is_same_dir_casefold_rename = if let Some((src_dir, src_name)) = src {
        src_dir.object_id() == dst.0.object_id()
            && src_dir.dir_type().is_casefold()
            && fxfs_unicode::casefold_cmp(src_name, dst.1) == std::cmp::Ordering::Equal
    } else {
        false
    };

    let src = if let Some((src_dir, src_name)) = src {
        let store_id = dst.0.store().store_object_id();
        assert_eq!(store_id, src_dir.store().store_object_id());

        let src_entry = src_dir.lookup_ext(src_name).await?.ok_or(FxfsError::NotFound)?;
        let LookupEntry { object_id: id, descriptor, key: src_key, .. } = src_entry;

        match (src_dir.dir_type(), dst.0.dir_type()) {
            (
                DirType::Encrypted(src_id) | DirType::EncryptedCasefold(src_id),
                DirType::Encrypted(dst_id) | DirType::EncryptedCasefold(dst_id),
            ) => {
                ensure!(src_id == dst_id, FxfsError::InconsistentEncryptionPolicy);

                // Renames only work on unlocked encrypted directories. Fail rename if src is
                // locked.
                let _ = src_dir.get_fscrypt_key().await?.into_cipher().ok_or(FxfsError::NoKey)?;
            }
            (
                DirType::Normal | DirType::Casefold | DirType::LegacyCasefold,
                DirType::Normal | DirType::Casefold | DirType::LegacyCasefold,
            ) => {}
            _ => bail!(FxfsError::InconsistentEncryptionPolicy),
        }

        transaction.add(store_id, Mutation::replace_or_insert_object(src_key, ObjectValue::None));

        src_dir.store().update_attributes(transaction, id, None, Some(now)).await?;
        if src_dir.object_id() != dst.0.object_id() {
            sub_dirs_delta = if descriptor == ObjectDescriptor::Directory { 1 } else { 0 };
            src_dir
                .update_dir_attributes_internal(
                    transaction,
                    src_dir.object_id(),
                    MutableAttributesInternal {
                        sub_dirs: -sub_dirs_delta,
                        modification_time: Some(now.as_nanos()),
                        change_time: Some(now),
                        ..Default::default()
                    },
                )
                .await?;
        }
        Some((id, descriptor))
    } else {
        None
    };
    replace_child_with_object_impl(
        transaction,
        src,
        dst,
        sub_dirs_delta,
        is_same_dir_casefold_rename,
        now,
        can_purge,
    )
    .await
}

/// Replaces dst.0/dst.1 with the given object, or unlinks if `src` is None.
///
/// If |dst.0| already has a child |dst.1|, it is removed from dst.0.  For files, if this was their
/// last reference, the file is moved to the graveyard.  For directories, the removed directory will
/// be moved to the graveyard (and must be empty).  The caller is responsible for tombstoning files
/// (when it is no longer open) and directories (immediately after committing the transaction).
///
/// `sub_dirs_delta` can be used if `src` is a directory and happened to already be a child of
/// `dst`.
pub async fn replace_child_with_object<'a, S: HandleOwner>(
    transaction: &mut Transaction<'a>,
    src: Option<(u64, ObjectDescriptor)>,
    dst: (&'a Directory<S>, &str),
    sub_dirs_delta: i64,
    is_same_dir_casefold_rename: bool,
    timestamp: Timestamp,
) -> Result<ReplacedChild, Error> {
    replace_child_with_object_impl(
        transaction,
        src,
        dst,
        sub_dirs_delta,
        is_same_dir_casefold_rename,
        timestamp,
        false,
    )
    .await
}

async fn replace_child_with_object_impl<'a, S: HandleOwner>(
    transaction: &mut Transaction<'a>,
    src: Option<(u64, ObjectDescriptor)>,
    dst: (&'a Directory<S>, &str),
    mut sub_dirs_delta: i64,
    is_same_dir_casefold_rename: bool,
    timestamp: Timestamp,
    can_purge: bool,
) -> Result<ReplacedChild, Error> {
    let deleted_info =
        if is_same_dir_casefold_rename { None } else { dst.0.lookup_ext(dst.1).await? };

    let (deleted_id_and_descriptor, dst_key) = match deleted_info {
        Some(entry) => (Some((entry.object_id, entry.descriptor.clone())), Some(entry.key)),
        None => (None, None),
    };
    let store_id = dst.0.store().store_object_id();

    let result = match deleted_id_and_descriptor {
        Some((old_id, ObjectDescriptor::File | ObjectDescriptor::Symlink)) => {
            let was_last_ref =
                dst.0.store().adjust_refs_impl(transaction, old_id, -1, !can_purge).await?;
            if was_last_ref {
                if can_purge {
                    let trim_result = dst
                        .0
                        .store()
                        .trim_some(
                            transaction,
                            old_id,
                            AttributeId::SORTED_START,
                            TrimMode::Tombstone(TombstoneMode::Object),
                        )
                        .await?;
                    if matches!(trim_result, TrimResult::Done(None)) {
                        ReplacedChild::Purged(old_id)
                    } else {
                        // Could not purge completely in this transaction; fall back to graveyard.
                        dst.0.store().add_to_graveyard(transaction, old_id);
                        ReplacedChild::Object(old_id)
                    }
                } else {
                    dst.0
                        .store()
                        .update_attributes(transaction, old_id, None, Some(timestamp))
                        .await?;
                    ReplacedChild::Object(old_id)
                }
            } else {
                dst.0.store().update_attributes(transaction, old_id, None, Some(timestamp)).await?;
                ReplacedChild::ObjectWithRemainingLinks(old_id)
            }
        }
        Some((old_id, ObjectDescriptor::Directory)) => {
            let dir = Directory::open(&dst.0.owner(), old_id).await?;
            if dir.has_children().await? {
                bail!(FxfsError::NotEmpty);
            }

            // Directories might have extended attributes which might require multiple transactions
            // to delete, so we delete directories via the graveyard.
            dst.0.store().add_to_graveyard(transaction, old_id);
            sub_dirs_delta -= 1;
            ReplacedChild::Directory(old_id)
        }
        Some((_, ObjectDescriptor::Volume)) => {
            bail!(anyhow!(FxfsError::Inconsistent).context("Unexpected volume child"))
        }
        None => {
            if src.is_none() {
                // Neither src nor dst exist
                bail!(FxfsError::NotFound);
            }
            ReplacedChild::None
        }
    };
    let new_value = match src {
        Some((id, descriptor)) => ObjectValue::child(id, descriptor),
        None => ObjectValue::None,
    };
    let new_key = if matches!(new_value, ObjectValue::None) {
        None
    } else {
        if dst.0.dir_type().is_encrypted() {
            match dst.0.get_fscrypt_key().await? {
                CipherHolder::Cipher(cipher) => {
                    let encrypted_dst_name = encrypt_filename(&*cipher, dst.0.object_id(), dst.1)?;
                    let dst_hash_code = if dst.0.dir_type().is_casefold() {
                        Some(cipher.hash_code_casefold(dst.1))
                    } else {
                        cipher.hash_code(encrypted_dst_name.as_bytes(), dst.1)
                    };
                    Some(ObjectKey::encrypted_child(
                        dst.0.object_id(),
                        encrypted_dst_name,
                        dst_hash_code,
                    ))
                }
                CipherHolder::Unavailable => {
                    bail!(FxfsError::NoKey);
                }
            }
        } else {
            Some(ObjectKey::child(dst.0.object_id(), dst.1, dst.0.dir_type()))
        }
    };

    if let Some(dst_key) = dst_key
        && new_key.as_ref() != Some(&dst_key)
    {
        transaction.add(store_id, Mutation::replace_or_insert_object(dst_key, ObjectValue::None));
    }

    if let Some(new_key) = new_key {
        transaction.add(store_id, Mutation::replace_or_insert_object(new_key, new_value));
    }
    dst.0
        .update_dir_attributes_internal(
            transaction,
            dst.0.object_id(),
            MutableAttributesInternal {
                sub_dirs: sub_dirs_delta,
                modification_time: Some(timestamp.as_nanos()),
                change_time: Some(timestamp),
                ..Default::default()
            },
        )
        .await?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{ProxyFilename, encrypt_filename, replace_child_with_object};
    use crate::errors::FxfsError;
    use crate::filesystem::{FxFilesystem, SyncOptions};
    use crate::fsck::{fsck, fsck_volume};
    use crate::object_handle::{ObjectHandle, WriteObjectHandle};
    use crate::object_store::directory::{
        Directory, MutableAttributesInternal, ReplacedChild, replace_child,
        replace_child_with_purge,
    };
    use crate::object_store::object_record::{ObjectKey, ObjectValue, Timestamp};
    use crate::object_store::transaction::{Options, lock_keys};
    use crate::object_store::volume::root_volume;
    use crate::object_store::{
        AttributeId, HandleOptions, LockKey, NewChildStoreOptions, ObjectDescriptor, ObjectKind,
        ObjectStore, SetExtendedAttributeMode, StoreObjectHandle, StoreOptions,
    };
    use anyhow::Error;
    use assert_matches::assert_matches;
    use fidl_fuchsia_io as fio;
    use fxfs_crypt_common::CryptBase;
    use fxfs_crypto::{Cipher, Crypt, WrappingKeyId};
    use fxfs_insecure_crypto::new_insecure_crypt;
    use std::collections::HashSet;
    use std::future::poll_fn;
    use std::sync::Arc;
    use std::task::Poll;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;
    use test_case::test_case;

    #[fuchsia::test]
    fn test_casefold_equality_implies_hash_equality() {
        use fxfs_unicode::CasefoldString;

        let test_cases = vec![
            ("Hello", "hello"),
            ("straße", "STRASSE"),
            ("e\u{0301}", "\u{00c9}"), // e + acute accent vs E with acute accent
            ("hello\u{00ad}", "hello"), // soft hyphen (ignorable)
            ("foo\u{200b}bar", "FOOBAR"), // zero width space (ignorable)
        ];

        for (a, b) in test_cases {
            let cf_a = CasefoldString::new(a.to_string());
            let cf_b = CasefoldString::new(b.to_string());
            assert_eq!(cf_a, cf_b, "Strings {:?} and {:?} should be equal under casefolding", a, b);

            let bytes_a: Vec<u8> =
                cf_a.casefold_normalized_chars().collect::<String>().into_bytes();
            let bytes_b: Vec<u8> =
                cf_b.casefold_normalized_chars().collect::<String>().into_bytes();
            assert_eq!(
                bytes_a, bytes_b,
                "Normalized bytes for {:?} and {:?} should be identical",
                a, b
            );

            let hash_a = fscrypt::direntry::tea_hash_filename(bytes_a);
            let hash_b = fscrypt::direntry::tea_hash_filename(bytes_b);
            assert_eq!(hash_a, hash_b, "Hashes for {:?} and {:?} should be identical", a, b);
        }
    }

    const TEST_DEVICE_BLOCK_SIZE: u32 = 512;
    const WRAPPING_KEY_ID: WrappingKeyId = u128::to_le_bytes(2);

    /// The synthetic symlink we return when locked is not usable for anything but we still want
    /// it to match that returned by fscrypt so we will verify here that we get back the
    /// expected ProxyFilename-derived link content.
    #[fuchsia::test]
    async fn test_reopen_with_different_crypt_shows_proxy_name() -> Result<(), Error> {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let symlink_object_id;
        {
            let crypt = Arc::new(new_insecure_crypt());
            crypt
                .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
                .expect("add_wrapping_key failed");
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "test",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let root_dir = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let _ = root_dir.set_wrapping_key(&mut transaction, WRAPPING_KEY_ID).await?;
            transaction.commit().await.unwrap();

            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let root_dir = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            symlink_object_id = root_dir
                .create_symlink(&mut transaction, b"some_link_text", "a")
                .await
                .expect("create_symlink failed");
            transaction.commit().await.expect("commit failed");
        };
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);

        let fs = FxFilesystem::open(device).await.expect("open failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        // Open the volume without providing the keys.
        let store = root_volume
            .volume(
                "test",
                StoreOptions {
                    crypt: Some(Arc::new(new_insecure_crypt())),
                    ..StoreOptions::default()
                },
            )
            .await
            .expect("volume failed");

        let value = store
            .tree()
            .find_value(&ObjectKey::object(symlink_object_id))
            .await
            .expect("find failed")
            .expect("found record");
        let raw_link = match value {
            ObjectValue::Object { kind: ObjectKind::EncryptedSymlink { link, .. }, .. } => link,
            _ => panic!("Unexpected value {value:?}"),
        };
        let symlink_target = store.read_symlink(symlink_object_id).await?;
        // Locked symlinks always have hash_code of zero.
        let expected_symlink_target: String =
            ProxyFilename::new_with_hash_code(0, &raw_link).into();
        assert_eq!(symlink_target, expected_symlink_target.as_bytes());

        fs.close().await.expect("Close failed");
        Ok(())
    }

    async fn yield_to_executor() {
        let mut done = false;
        poll_fn(|cx| {
            if done {
                Poll::Ready(())
            } else {
                done = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_create_directory() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let object_id = {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let dir = Directory::create(&mut transaction, &fs.root_store(), None)
                .await
                .expect("create failed");

            let child_dir = dir
                .create_child_dir(&mut transaction, "foo")
                .await
                .expect("create_child_dir failed");
            let _child_dir_file = child_dir
                .create_child_file(&mut transaction, "bar")
                .await
                .expect("create_child_file failed");
            let _child_file = dir
                .create_child_file(&mut transaction, "baz")
                .await
                .expect("create_child_file failed");
            dir.add_child_volume(&mut transaction, "corge", 100)
                .await
                .expect("add_child_volume failed");
            transaction.commit().await.expect("commit failed");
            fs.sync(SyncOptions::default()).await.expect("sync failed");
            dir.object_id()
        };
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        {
            let dir = Directory::open(&fs.root_store(), object_id).await.expect("open failed");
            let (object_id, object_descriptor, _) =
                dir.lookup("foo").await.expect("lookup failed").expect("not found");
            assert_eq!(object_descriptor, ObjectDescriptor::Directory);
            let child_dir =
                Directory::open(&fs.root_store(), object_id).await.expect("open failed");
            let (object_id, object_descriptor, _) =
                child_dir.lookup("bar").await.expect("lookup failed").expect("not found");
            assert_eq!(object_descriptor, ObjectDescriptor::File);
            let _child_dir_file = ObjectStore::open_object(
                &fs.root_store(),
                object_id,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("open object failed");
            let (object_id, object_descriptor, _) =
                dir.lookup("baz").await.expect("lookup failed").expect("not found");
            assert_eq!(object_descriptor, ObjectDescriptor::File);
            let _child_file = ObjectStore::open_object(
                &fs.root_store(),
                object_id,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("open object failed");
            let (object_id, object_descriptor, _) =
                dir.lookup("corge").await.expect("lookup failed").expect("not found");
            assert_eq!(object_id, 100);
            if let ObjectDescriptor::Volume = object_descriptor {
            } else {
                panic!("wrong ObjectDescriptor");
            }

            assert_eq!(dir.lookup("qux").await.expect("lookup failed"), None);
        }
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_set_wrapping_key_does_not_exist() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 4096));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
        let store = root_volume
            .new_volume(
                "test",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .expect("new_volume failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let directory = root_directory
            .create_child_dir(&mut transaction, "foo")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        directory
            .set_wrapping_key(&mut transaction, WRAPPING_KEY_ID)
            .await
            .expect_err("wrapping key id 2 has not been added");
        transaction.commit().await.expect("commit failed");
        crypt.add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into()).expect("add_wrapping_key failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        directory
            .set_wrapping_key(&mut transaction, WRAPPING_KEY_ID)
            .await
            .expect("wrapping key id 2 has been added");
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_set_encryption_policy_on_unencrypted_nonempty_dir() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 4096));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
        let store = root_volume
            .new_volume(
                "test",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .expect("new_volume failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let directory = root_directory
            .create_child_dir(&mut transaction, "foo")
            .await
            .expect("create_child_dir failed");
        let _file = directory
            .create_child_file(&mut transaction, "bar")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        directory
            .set_wrapping_key(&mut transaction, WRAPPING_KEY_ID)
            .await
            .expect_err("directory is not empty");
        transaction.commit().await.expect("commit failed");
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_create_file_or_subdir_in_locked_directory() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 4096));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
        let store = root_volume
            .new_volume(
                "test",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .expect("new_volume failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let directory = root_directory
            .create_child_dir(&mut transaction, "foo")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");
        crypt.add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into()).expect("add_wrapping_key failed");
        let transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        directory
            .update_attributes(
                transaction,
                Some(&fio::MutableNodeAttributes {
                    wrapping_key_id: Some(WRAPPING_KEY_ID),
                    ..Default::default()
                }),
                0,
                None,
            )
            .await
            .expect("update attributes failed");
        crypt.forget_wrapping_key(&WRAPPING_KEY_ID).expect("forget wrapping key failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        directory
            .create_child_dir(&mut transaction, "bar")
            .await
            .expect_err("cannot create a dir inside of a locked encrypted directory");
        directory
            .create_child_file(&mut transaction, "baz")
            .await
            .map(|_| ())
            .expect_err("cannot create a file inside of a locked encrypted directory");
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_replace_child_with_object_in_locked_directory() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 4096));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt = Arc::new(new_insecure_crypt());

        let (parent_oid, src_oid, dst_oid) = {
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "test",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");

            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let directory = root_directory
                .create_child_dir(&mut transaction, "foo")
                .await
                .expect("create_child_dir failed");
            transaction.commit().await.expect("commit failed");
            crypt
                .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
                .expect("add_wrapping_key failed");
            let transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), directory.object_id())],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");
            directory
                .update_attributes(
                    transaction,
                    Some(&fio::MutableNodeAttributes {
                        wrapping_key_id: Some(WRAPPING_KEY_ID),
                        ..Default::default()
                    }),
                    0,
                    None,
                )
                .await
                .expect("update attributes failed");
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), directory.object_id())],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");
            let src_child = directory
                .create_child_dir(&mut transaction, "fee")
                .await
                .expect("create_child_dir failed");
            let dst_child = directory
                .create_child_dir(&mut transaction, "faa")
                .await
                .expect("create_child_dir failed");
            transaction.commit().await.expect("commit failed");
            crypt.forget_wrapping_key(&WRAPPING_KEY_ID).expect("forget_wrapping_key failed");
            (directory.object_id(), src_child.object_id(), dst_child.object_id())
        };
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_volume
            .volume(
                "test",
                StoreOptions {
                    crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                    ..StoreOptions::default()
                },
            )
            .await
            .expect("volume failed");

        {
            let parent_directory = Directory::open(&store, parent_oid).await.expect("open failed");
            let layer_set = store.tree().layer_set();
            let mut merger = layer_set.merger();
            let mut encrypted_src_name = None;
            let mut encrypted_dst_name = None;
            let mut iter = parent_directory.iter(&mut merger).await.expect("iter_from failed");
            while let Some((name, object_id, object_descriptor)) = iter.get() {
                assert!(matches!(object_descriptor, ObjectDescriptor::Directory));
                if object_id == dst_oid {
                    encrypted_dst_name = Some(name.to_string());
                } else if object_id == src_oid {
                    encrypted_src_name = Some(name.to_string());
                }
                iter.advance().await.expect("iter advance failed");
            }

            let src_child = parent_directory
                .lookup(&encrypted_src_name.expect("src child not found"))
                .await
                .expect("lookup failed")
                .expect("not found");
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        parent_directory.object_id(),
                    )],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");
            replace_child_with_object(
                &mut transaction,
                Some((src_child.0, src_child.1)),
                (&parent_directory, &encrypted_dst_name.expect("dst child not found")),
                0,
                false,
                Timestamp::now(),
            )
            .await
            .expect_err("renames should fail within a locked directory");
        }
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_set_encryption_policy_on_unencrypted_file() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 4096));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
        let store = root_volume
            .new_volume(
                "test",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .expect("new_volume failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let file_handle = root_directory
            .create_child_file(&mut transaction, "foo")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), file_handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        file_handle
            .update_attributes(
                &mut transaction,
                Some(&fio::MutableNodeAttributes {
                    wrapping_key_id: Some(WRAPPING_KEY_ID),
                    ..Default::default()
                }),
                None,
            )
            .await
            .expect_err("Cannot update the wrapping key id of a file");
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_delete_child() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let child;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");

        child =
            dir.create_child_file(&mut transaction, "foo").await.expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_matches!(
            replace_child(&mut transaction, None, (&dir, "foo"))
                .await
                .expect("replace_child failed"),
            ReplacedChild::Object(..)
        );
        transaction.commit().await.expect("commit failed");

        assert_eq!(dir.lookup("foo").await.expect("lookup failed"), None);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_delete_child_with_children_fails() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let child;
        let bar;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");

        child =
            dir.create_child_dir(&mut transaction, "foo").await.expect("create_child_dir failed");
        bar = child
            .create_child_file(&mut transaction, "bar")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_eq!(
            replace_child(&mut transaction, None, (&dir, "foo"))
                .await
                .expect_err("replace_child succeeded")
                .downcast::<FxfsError>()
                .expect("wrong error"),
            FxfsError::NotEmpty
        );
        transaction.commit().await.expect("commit failed");

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), child.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), bar.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_matches!(
            replace_child(&mut transaction, None, (&child, "bar"))
                .await
                .expect("replace_child failed"),
            ReplacedChild::Object(..)
        );
        transaction.commit().await.expect("commit failed");

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_matches!(
            replace_child(&mut transaction, None, (&dir, "foo"))
                .await
                .expect("replace_child failed"),
            ReplacedChild::Directory(..)
        );
        transaction.commit().await.expect("commit failed");

        assert_eq!(dir.lookup("foo").await.expect("lookup failed"), None);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_delete_and_reinsert_child() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let child;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");

        child =
            dir.create_child_file(&mut transaction, "foo").await.expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_matches!(
            replace_child(&mut transaction, None, (&dir, "foo"))
                .await
                .expect("replace_child failed"),
            ReplacedChild::Object(..)
        );
        transaction.commit().await.expect("commit failed");

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(fs.root_store().store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        dir.create_child_file(&mut transaction, "foo").await.expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        dir.lookup("foo").await.expect("lookup failed");
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_delete_child_persists() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let object_id = {
            let dir;
            let child;
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            dir = Directory::create(&mut transaction, &fs.root_store(), None)
                .await
                .expect("create failed");

            child = dir
                .create_child_file(&mut transaction, "foo")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            dir.lookup("foo").await.expect("lookup failed");

            transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![
                        LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                        LockKey::object(fs.root_store().store_object_id(), child.object_id()),
                    ],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            assert_matches!(
                replace_child(&mut transaction, None, (&dir, "foo"))
                    .await
                    .expect("replace_child failed"),
                ReplacedChild::Object(..)
            );
            transaction.commit().await.expect("commit failed");

            fs.sync(SyncOptions::default()).await.expect("sync failed");
            dir.object_id()
        };

        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        let dir = Directory::open(&fs.root_store(), object_id).await.expect("open failed");
        assert_eq!(dir.lookup("foo").await.expect("lookup failed"), None);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_replace_child() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let child_dir1;
        let child_dir2;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");

        child_dir1 =
            dir.create_child_dir(&mut transaction, "dir1").await.expect("create_child_dir failed");
        child_dir2 =
            dir.create_child_dir(&mut transaction, "dir2").await.expect("create_child_dir failed");
        let file = child_dir1
            .create_child_file(&mut transaction, "foo")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), child_dir1.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child_dir2.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), file.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_matches!(
            replace_child(&mut transaction, Some((&child_dir1, "foo")), (&child_dir2, "bar"))
                .await
                .expect("replace_child failed"),
            ReplacedChild::None
        );
        transaction.commit().await.expect("commit failed");

        assert_eq!(child_dir1.lookup("foo").await.expect("lookup failed"), None);
        child_dir2.lookup("bar").await.expect("lookup failed");
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_replace_child_overwrites_dst() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let child_dir1;
        let child_dir2;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");

        child_dir1 =
            dir.create_child_dir(&mut transaction, "dir1").await.expect("create_child_dir failed");
        child_dir2 =
            dir.create_child_dir(&mut transaction, "dir2").await.expect("create_child_dir failed");
        let foo = child_dir1
            .create_child_file(&mut transaction, "foo")
            .await
            .expect("create_child_file failed");
        let bar = child_dir2
            .create_child_file(&mut transaction, "bar")
            .await
            .expect("create_child_file failed");
        let foo_oid = foo.object_id();
        let bar_oid = bar.object_id();
        transaction.commit().await.expect("commit failed");

        {
            let mut buf = foo.allocate_buffer(TEST_DEVICE_BLOCK_SIZE as usize).await;
            buf.fill(0xaa);
            foo.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
            buf.fill(0xbb);
            bar.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        }
        std::mem::drop(bar);
        std::mem::drop(foo);

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), child_dir1.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child_dir2.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), foo_oid),
                    LockKey::object(fs.root_store().store_object_id(), bar_oid),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_matches!(
            replace_child(&mut transaction, Some((&child_dir1, "foo")), (&child_dir2, "bar"))
                .await
                .expect("replace_child failed"),
            ReplacedChild::Object(..)
        );
        transaction.commit().await.expect("commit failed");

        assert_eq!(child_dir1.lookup("foo").await.expect("lookup failed"), None);

        // Check the contents to ensure that the file was replaced.
        let (oid, object_descriptor, _) =
            child_dir2.lookup("bar").await.expect("lookup failed").expect("not found");
        assert_eq!(object_descriptor, ObjectDescriptor::File);
        let bar =
            ObjectStore::open_object(&child_dir2.owner(), oid, HandleOptions::default(), None)
                .await
                .expect("Open failed");
        let mut buf = bar.allocate_buffer(TEST_DEVICE_BLOCK_SIZE as usize).await;
        bar.read(0, buf.as_mut()).await.expect("read failed");
        assert_eq!(buf.to_vec(), vec![0xaa; TEST_DEVICE_BLOCK_SIZE as usize]);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_replace_child_with_purge() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");
        let file =
            dir.create_child_file(&mut transaction, "foo").await.expect("create_child_file failed");
        let file_oid = file.object_id();
        transaction.commit().await.expect("commit failed");

        {
            let mut buf = file.allocate_buffer(TEST_DEVICE_BLOCK_SIZE as usize).await;
            buf.fill(0xaa);
            file.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        }
        std::mem::drop(file);

        let initial_alloc = fs.allocator().get_allocated_bytes();
        assert!(initial_alloc >= fs.block_size());

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), file_oid),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_matches!(
            replace_child_with_purge(&mut transaction, None, (&dir, "foo"), true)
                .await
                .expect("replace_child_with_purge failed"),
            ReplacedChild::Purged(id) if id == file_oid
        );
        transaction.commit().await.expect("commit failed");

        assert_eq!(dir.lookup("foo").await.expect("lookup failed"), None);
        assert_eq!(fs.root_store().graveyard_count(), 0);
        assert_eq!(fs.allocator().get_allocated_bytes(), initial_alloc - fs.block_size());
        assert!(
            fs.root_store()
                .tree()
                .find(&ObjectKey::object(file_oid))
                .await
                .expect("find failed")
                .is_none()
        );

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_replace_child_fails_if_would_overwrite_nonempty_dir() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let child_dir1;
        let child_dir2;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");

        child_dir1 =
            dir.create_child_dir(&mut transaction, "dir1").await.expect("create_child_dir failed");
        child_dir2 =
            dir.create_child_dir(&mut transaction, "dir2").await.expect("create_child_dir failed");
        let foo = child_dir1
            .create_child_file(&mut transaction, "foo")
            .await
            .expect("create_child_file failed");
        let nested_child = child_dir2
            .create_child_dir(&mut transaction, "bar")
            .await
            .expect("create_child_file failed");
        nested_child
            .create_child_file(&mut transaction, "baz")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), child_dir1.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child_dir2.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), foo.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), nested_child.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_eq!(
            replace_child(&mut transaction, Some((&child_dir1, "foo")), (&child_dir2, "bar"))
                .await
                .expect_err("replace_child succeeded")
                .downcast::<FxfsError>()
                .expect("wrong error"),
            FxfsError::NotEmpty
        );
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_replace_child_within_dir() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");
        let foo =
            dir.create_child_file(&mut transaction, "foo").await.expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), foo.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_matches!(
            replace_child(&mut transaction, Some((&dir, "foo")), (&dir, "bar"))
                .await
                .expect("replace_child failed"),
            ReplacedChild::None
        );
        transaction.commit().await.expect("commit failed");

        assert_eq!(dir.lookup("foo").await.expect("lookup failed"), None);
        dir.lookup("bar").await.expect("lookup new name failed");
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_replace_child_normal_into_encrypted_fails() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
        crypt.add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into()).expect("add_wrapping_key failed");
        let store = root_volume(fs.clone())
            .await
            .expect("root_volume")
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..Default::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume");

        let normal_dir;
        let encrypted_dir;
        let foo;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        normal_dir =
            Directory::create(&mut transaction, &store, None).await.expect("create failed");
        encrypted_dir = Directory::create(&mut transaction, &store, Some(WRAPPING_KEY_ID))
            .await
            .expect("create failed");
        foo = normal_dir
            .create_child_file(&mut transaction, "foo")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(store.store_object_id(), normal_dir.object_id()),
                    LockKey::object(store.store_object_id(), encrypted_dir.object_id()),
                    LockKey::object(store.store_object_id(), foo.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_eq!(
            replace_child(&mut transaction, Some((&normal_dir, "foo")), (&encrypted_dir, "foo"))
                .await
                .expect_err("replace_child succeeded")
                .downcast::<FxfsError>()
                .expect("wrong error"),
            FxfsError::InconsistentEncryptionPolicy
        );
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_iterate() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");
        let _cat =
            dir.create_child_file(&mut transaction, "cat").await.expect("create_child_file failed");
        let _ball = dir
            .create_child_file(&mut transaction, "ball")
            .await
            .expect("create_child_file failed");
        let apple = dir
            .create_child_file(&mut transaction, "apple")
            .await
            .expect("create_child_file failed");
        let _dog =
            dir.create_child_file(&mut transaction, "dog").await.expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), apple.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        replace_child(&mut transaction, None, (&dir, "apple")).await.expect("replace_child failed");
        transaction.commit().await.expect("commit failed");
        let layer_set = dir.store().tree().layer_set();
        let mut merger = layer_set.merger();
        let mut iter = dir.iter(&mut merger).await.expect("iter failed");
        let mut entries = Vec::new();
        while let Some((name, _, _)) = iter.get() {
            entries.push(name.to_string());
            iter.advance().await.expect("advance failed");
        }
        assert_eq!(&entries, &["ball", "cat", "dog"]);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_sub_dir_count() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let child_dir;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");
        child_dir =
            dir.create_child_dir(&mut transaction, "foo").await.expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");
        assert_eq!(dir.get_properties().await.expect("get_properties failed").sub_dirs, 1);

        // Moving within the same directory should not change the sub_dir count.
        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child_dir.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        replace_child(&mut transaction, Some((&dir, "foo")), (&dir, "bar"))
            .await
            .expect("replace_child failed");
        transaction.commit().await.expect("commit failed");

        assert_eq!(dir.get_properties().await.expect("get_properties failed").sub_dirs, 1);
        assert_eq!(child_dir.get_properties().await.expect("get_properties failed").sub_dirs, 0);

        // Moving between two different directories should update source and destination.
        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    fs.root_store().store_object_id(),
                    child_dir.object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let second_child = child_dir
            .create_child_dir(&mut transaction, "baz")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");

        assert_eq!(child_dir.get_properties().await.expect("get_properties failed").sub_dirs, 1);

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), child_dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), second_child.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        replace_child(&mut transaction, Some((&child_dir, "baz")), (&dir, "foo"))
            .await
            .expect("replace_child failed");
        transaction.commit().await.expect("commit failed");

        assert_eq!(dir.get_properties().await.expect("get_properties failed").sub_dirs, 2);
        assert_eq!(child_dir.get_properties().await.expect("get_properties failed").sub_dirs, 0);

        // Moving over a directory.
        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), second_child.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child_dir.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        replace_child(&mut transaction, Some((&dir, "bar")), (&dir, "foo"))
            .await
            .expect("replace_child failed");
        transaction.commit().await.expect("commit failed");

        assert_eq!(dir.get_properties().await.expect("get_properties failed").sub_dirs, 1);

        // Unlinking a directory.
        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child_dir.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        replace_child(&mut transaction, None, (&dir, "foo")).await.expect("replace_child failed");
        transaction.commit().await.expect("commit failed");

        assert_eq!(dir.get_properties().await.expect("get_properties failed").sub_dirs, 0);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_deleted_dir() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");
        let child =
            dir.create_child_dir(&mut transaction, "foo").await.expect("create_child_dir failed");
        dir.create_child_dir(&mut transaction, "bar").await.expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");

        // Flush the tree so that we end up with records in different layers.
        dir.store().flush().await.expect("flush failed");

        // Unlink the child directory.
        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        replace_child(&mut transaction, None, (&dir, "foo")).await.expect("replace_child failed");
        transaction.commit().await.expect("commit failed");

        // Finding the child should fail now.
        assert_eq!(dir.lookup("foo").await.expect("lookup failed"), None);

        // But finding "bar" should succeed.
        assert!(dir.lookup("bar").await.expect("lookup failed").is_some());

        // If we mark dir as deleted, any further operations should fail.
        dir.set_deleted();

        assert_eq!(dir.lookup("foo").await.expect("lookup failed"), None);
        assert_eq!(dir.lookup("bar").await.expect("lookup failed"), None);
        assert!(!dir.has_children().await.expect("has_children failed"));

        transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");

        let assert_access_denied = |result| {
            if let Err(e) = result {
                assert!(FxfsError::Deleted.matches(&e));
            } else {
                panic!();
            }
        };
        assert_access_denied(dir.create_child_dir(&mut transaction, "baz").await.map(|_| {}));
        assert_access_denied(dir.create_child_file(&mut transaction, "baz").await.map(|_| {}));
        assert_access_denied(dir.add_child_volume(&mut transaction, "baz", 1).await);
        assert_access_denied(
            dir.insert_child(&mut transaction, "baz", 1, ObjectDescriptor::File).await,
        );
        assert_access_denied(
            dir.update_dir_attributes_internal(
                &mut transaction,
                dir.object_id(),
                MutableAttributesInternal {
                    creation_time: Some(Timestamp::zero().as_nanos()),
                    ..Default::default()
                },
            )
            .await,
        );
        let layer_set = dir.store().tree().layer_set();
        let mut merger = layer_set.merger();
        assert_access_denied(dir.iter(&mut merger).await.map(|_| {}));
    }

    #[fuchsia::test]
    async fn test_create_symlink() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let (dir_id, symlink_id) = {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let dir = Directory::create(&mut transaction, &fs.root_store(), None)
                .await
                .expect("create failed");

            let symlink_id = dir
                .create_symlink(&mut transaction, b"link", "foo")
                .await
                .expect("create_symlink failed");
            transaction.commit().await.expect("commit failed");

            fs.sync(SyncOptions::default()).await.expect("sync failed");
            (dir.object_id(), symlink_id)
        };
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        {
            let dir = Directory::open(&fs.root_store(), dir_id).await.expect("open failed");
            assert_eq!(
                dir.lookup("foo").await.expect("lookup failed").expect("not found"),
                (symlink_id, ObjectDescriptor::Symlink, false)
            );
        }
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_read_symlink() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let store = fs.root_store();
        let dir = Directory::create(&mut transaction, &store, None).await.expect("create failed");

        let symlink_id = dir
            .create_symlink(&mut transaction, b"link", "foo")
            .await
            .expect("create_symlink failed");
        transaction.commit().await.expect("commit failed");

        let link = store.read_symlink(symlink_id).await.expect("read_symlink failed");
        assert_eq!(&link, b"link");
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_unlink_symlink() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let store = fs.root_store();
        dir = Directory::create(&mut transaction, &store, None).await.expect("create failed");

        let symlink_id = dir
            .create_symlink(&mut transaction, b"link", "foo")
            .await
            .expect("create_symlink failed");
        transaction.commit().await.expect("commit failed");
        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(store.store_object_id(), dir.object_id()),
                    LockKey::object(store.store_object_id(), symlink_id),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_matches!(
            replace_child(&mut transaction, None, (&dir, "foo"))
                .await
                .expect("replace_child failed"),
            ReplacedChild::Object(_)
        );
        transaction.commit().await.expect("commit failed");

        assert_eq!(dir.lookup("foo").await.expect("lookup failed"), None);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_get_properties() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");

        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");
        transaction.commit().await.expect("commit failed");

        // Check attributes of `dir`
        let mut properties = dir.get_properties().await.expect("get_properties failed");
        let dir_creation_time = properties.creation_time;
        assert_eq!(dir_creation_time, properties.modification_time);
        assert_eq!(properties.sub_dirs, 0);
        assert!(properties.posix_attributes.is_none());

        // Create child directory
        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(fs.root_store().store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let child_dir =
            dir.create_child_dir(&mut transaction, "foo").await.expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");

        // Check attributes of `dir` after adding child directory
        properties = dir.get_properties().await.expect("get_properties failed");
        // The modification time property should have updated
        assert_eq!(dir_creation_time, properties.creation_time);
        assert!(dir_creation_time < properties.modification_time);
        assert_eq!(properties.sub_dirs, 1);
        assert!(properties.posix_attributes.is_none());

        // Check attributes of `child_dir`
        properties = child_dir.get_properties().await.expect("get_properties failed");
        assert_eq!(properties.creation_time, properties.modification_time);
        assert_eq!(properties.sub_dirs, 0);
        assert!(properties.posix_attributes.is_none());

        // Create child file with MutableAttributes
        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    fs.root_store().store_object_id(),
                    child_dir.object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let child_dir_file = child_dir
            .create_child_file(&mut transaction, "bar")
            .await
            .expect("create_child_file failed");
        child_dir_file
            .update_attributes(
                &mut transaction,
                Some(&fio::MutableNodeAttributes { gid: Some(1), ..Default::default() }),
                None,
            )
            .await
            .expect("Updating attributes");
        transaction.commit().await.expect("commit failed");

        // The modification time property of `child_dir` should have updated
        properties = child_dir.get_properties().await.expect("get_properties failed");
        assert!(properties.creation_time < properties.modification_time);
        assert!(properties.posix_attributes.is_none());

        // Check attributes of `child_dir_file`
        properties = child_dir_file.get_properties().await.expect("get_properties failed");
        assert_eq!(properties.creation_time, properties.modification_time);
        assert_eq!(properties.sub_dirs, 0);
        assert!(properties.posix_attributes.is_some());
        assert_eq!(properties.posix_attributes.unwrap().gid, 1);
        // The other POSIX attributes should be set to default values
        assert_eq!(properties.posix_attributes.unwrap().uid, 0);
        assert_eq!(properties.posix_attributes.unwrap().mode, 0);
        assert_eq!(properties.posix_attributes.unwrap().rdev, 0);
    }

    #[fuchsia::test]
    async fn test_update_create_attributes() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");

        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");
        transaction.commit().await.expect("commit failed");
        let mut properties = dir.get_properties().await.expect("get_properties failed");
        assert_eq!(properties.sub_dirs, 0);
        assert!(properties.posix_attributes.is_none());
        let creation_time = properties.creation_time;
        let modification_time = properties.modification_time;
        assert_eq!(creation_time, modification_time);

        // First update: test that
        // 1. updating attributes with a POSIX attribute will assign some PosixAttributes to the
        //    Object associated with `dir`,
        // 2. creation/modification time are only updated if specified in the update,
        // 3. any changes will not overwrite other attributes.
        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(fs.root_store().store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let now = Timestamp::now();
        dir.update_attributes(
            transaction,
            Some(&fio::MutableNodeAttributes {
                modification_time: Some(now.as_nanos()),
                uid: Some(1),
                gid: Some(2),
                ..Default::default()
            }),
            0,
            None,
        )
        .await
        .expect("update_attributes failed");
        properties = dir.get_properties().await.expect("get_properties failed");
        // Check that the properties reflect the updates
        assert_eq!(properties.modification_time, now);
        assert!(properties.posix_attributes.is_some());
        assert_eq!(properties.posix_attributes.unwrap().uid, 1);
        assert_eq!(properties.posix_attributes.unwrap().gid, 2);
        // The other POSIX attributes should be set to default values
        assert_eq!(properties.posix_attributes.unwrap().mode, 0);
        assert_eq!(properties.posix_attributes.unwrap().rdev, 0);
        // The remaining properties should not have changed
        assert_eq!(properties.sub_dirs, 0);
        assert_eq!(properties.creation_time, creation_time);

        // Second update: test that we can update attributes and that any changes will not overwrite
        // other attributes
        let transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(fs.root_store().store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        dir.update_attributes(
            transaction,
            Some(&fio::MutableNodeAttributes {
                creation_time: Some(now.as_nanos()),
                uid: Some(3),
                rdev: Some(10),
                ..Default::default()
            }),
            0,
            None,
        )
        .await
        .expect("update_attributes failed");
        properties = dir.get_properties().await.expect("get_properties failed");
        assert_eq!(properties.creation_time, now);
        assert!(properties.posix_attributes.is_some());
        assert_eq!(properties.posix_attributes.unwrap().uid, 3);
        assert_eq!(properties.posix_attributes.unwrap().rdev, 10);
        // The other properties should not have changed
        assert_eq!(properties.sub_dirs, 0);
        assert_eq!(properties.modification_time, now);
        assert_eq!(properties.posix_attributes.unwrap().gid, 2);
        assert_eq!(properties.posix_attributes.unwrap().mode, 0);
    }

    #[fuchsia::test]
    async fn write_to_directory_attribute_creates_keys() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt = Arc::new(new_insecure_crypt());

        {
            let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "vol",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone()),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");
            let mut transaction = filesystem
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let directory = root_directory
                .create_child_dir(&mut transaction, "foo")
                .await
                .expect("create_child_dir failed");
            transaction.commit().await.expect("commit failed");

            let mut transaction = filesystem
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), directory.object_id())],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");
            let _ = directory
                .handle
                .write_attr(&mut transaction, AttributeId::TEST_ID, b"bar")
                .await
                .expect("write_attr failed");
            transaction.commit().await.expect("commit failed");
        }

        filesystem.close().await.expect("Close failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.expect("open failed");

        {
            let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let volume = root_volume
                .volume("vol", StoreOptions { crypt: Some(crypt), ..StoreOptions::default() })
                .await
                .expect("volume failed");
            let root_directory = Directory::open(&volume, volume.root_directory_object_id())
                .await
                .expect("open failed");
            let directory = Directory::open(
                &volume,
                root_directory.lookup("foo").await.expect("lookup failed").expect("not found").0,
            )
            .await
            .expect("open failed");
            let mut buf = directory.handle.store().device.allocate_buffer(10).await;
            assert_eq!(
                directory
                    .handle
                    .read(AttributeId::TEST_ID, 0, buf.as_mut())
                    .await
                    .expect("read failed"),
                3
            );
            assert_eq!(buf.subslice(0..3).to_vec(), b"bar");
        }

        filesystem.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn directory_with_extended_attributes() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt = Arc::new(new_insecure_crypt());

        let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");
        let directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let test_small_name = b"security.selinux".to_vec();
        let test_small_value = b"foo".to_vec();
        let test_large_name = b"large.attribute".to_vec();
        let test_large_value = vec![1u8; 500];

        directory
            .set_extended_attribute(
                test_small_name.clone(),
                test_small_value.clone(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
        assert_eq!(
            directory.get_extended_attribute(test_small_name.clone()).await.unwrap(),
            test_small_value
        );

        directory
            .set_extended_attribute(
                test_large_name.clone(),
                test_large_value.clone(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
        assert_eq!(
            directory.get_extended_attribute(test_large_name.clone()).await.unwrap(),
            test_large_value
        );

        fsck(filesystem.clone()).await.unwrap();
        fsck_volume(filesystem.as_ref(), store.store_object_id(), Some(crypt.clone()))
            .await
            .unwrap();

        directory.remove_extended_attribute(test_small_name.clone()).await.unwrap();
        directory.remove_extended_attribute(test_large_name.clone()).await.unwrap();

        filesystem.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn remove_directory_with_extended_attributes() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt = Arc::new(new_insecure_crypt());

        let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");
        let mut transaction = filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let directory = root_directory
            .create_child_dir(&mut transaction, "foo")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");

        fsck(filesystem.clone()).await.unwrap();
        fsck_volume(filesystem.as_ref(), store.store_object_id(), Some(crypt.clone()))
            .await
            .unwrap();

        let test_small_name = b"security.selinux".to_vec();
        let test_small_value = b"foo".to_vec();
        let test_large_name = b"large.attribute".to_vec();
        let test_large_value = vec![1u8; 500];

        directory
            .set_extended_attribute(
                test_small_name.clone(),
                test_small_value.clone(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
        directory
            .set_extended_attribute(
                test_large_name.clone(),
                test_large_value.clone(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();

        fsck(filesystem.clone()).await.unwrap();
        fsck_volume(filesystem.as_ref(), store.store_object_id(), Some(crypt.clone()))
            .await
            .unwrap();

        let mut transaction = filesystem
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(store.store_object_id(), root_directory.object_id()),
                    LockKey::object(store.store_object_id(), directory.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        replace_child(&mut transaction, None, (&root_directory, "foo"))
            .await
            .expect("replace_child failed");
        transaction.commit().await.unwrap();

        fsck(filesystem.clone()).await.unwrap();
        fsck_volume(filesystem.as_ref(), store.store_object_id(), Some(crypt.clone()))
            .await
            .unwrap();

        filesystem.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn remove_symlink_with_extended_attributes() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt = Arc::new(new_insecure_crypt());

        let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");
        let mut transaction = filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let symlink_id = root_directory
            .create_symlink(&mut transaction, b"somewhere/else", "foo")
            .await
            .expect("create_symlink failed");
        transaction.commit().await.expect("commit failed");

        let symlink = StoreObjectHandle::new(
            store.clone(),
            symlink_id,
            false,
            HandleOptions::default(),
            false,
        );

        fsck(filesystem.clone()).await.unwrap();
        fsck_volume(filesystem.as_ref(), store.store_object_id(), Some(crypt.clone()))
            .await
            .unwrap();

        let test_small_name = b"security.selinux".to_vec();
        let test_small_value = b"foo".to_vec();
        let test_large_name = b"large.attribute".to_vec();
        let test_large_value = vec![1u8; 500];

        symlink
            .set_extended_attribute(
                test_small_name.clone(),
                test_small_value.clone(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
        symlink
            .set_extended_attribute(
                test_large_name.clone(),
                test_large_value.clone(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();

        fsck(filesystem.clone()).await.unwrap();
        fsck_volume(filesystem.as_ref(), store.store_object_id(), Some(crypt.clone()))
            .await
            .unwrap();

        let mut transaction = filesystem
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(store.store_object_id(), root_directory.object_id()),
                    LockKey::object(store.store_object_id(), symlink.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        replace_child(&mut transaction, None, (&root_directory, "foo"))
            .await
            .expect("replace_child failed");
        transaction.commit().await.unwrap();

        fsck(filesystem.clone()).await.unwrap();
        fsck_volume(filesystem.as_ref(), store.store_object_id(), Some(crypt.clone()))
            .await
            .unwrap();

        filesystem.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn test_update_timestamps() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");

        // Expect that atime, ctime, mtime (and creation time) to be the same when we create a
        // directory
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");
        transaction.commit().await.expect("commit failed");
        let mut properties = dir.get_properties().await.expect("get_properties failed");
        let starting_time = properties.creation_time;
        assert_eq!(properties.creation_time, starting_time);
        assert_eq!(properties.modification_time, starting_time);
        assert_eq!(properties.change_time, starting_time);
        assert_eq!(properties.access_time, starting_time);

        // Test that we can update the timestamps
        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(fs.root_store().store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let update1_time = Timestamp::now();
        dir.update_attributes(
            transaction,
            Some(&fio::MutableNodeAttributes {
                modification_time: Some(update1_time.as_nanos()),
                ..Default::default()
            }),
            0,
            Some(update1_time),
        )
        .await
        .expect("update_attributes failed");
        properties = dir.get_properties().await.expect("get_properties failed");
        assert_eq!(properties.modification_time, update1_time);
        assert_eq!(properties.access_time, starting_time);
        assert_eq!(properties.creation_time, starting_time);
        assert_eq!(properties.change_time, update1_time);
    }

    #[fuchsia::test]
    async fn test_move_dir_timestamps() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let child1;
        let child2;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");
        child1 = dir
            .create_child_dir(&mut transaction, "child1")
            .await
            .expect("create_child_dir failed");
        child2 = dir
            .create_child_dir(&mut transaction, "child2")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");
        let dir_properties = dir.get_properties().await.expect("get_properties failed");
        let child2_properties = child2.get_properties().await.expect("get_properties failed");

        // Move dir/child2 to dir/child1/child2
        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child1.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child2.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_matches!(
            replace_child(&mut transaction, Some((&dir, "child2")), (&child1, "child2"))
                .await
                .expect("replace_child failed"),
            ReplacedChild::None
        );
        transaction.commit().await.expect("commit failed");
        // Both mtime and ctime for dir should be updated
        let new_dir_properties = dir.get_properties().await.expect("get_properties failed");
        let time_of_replacement = new_dir_properties.change_time;
        assert!(new_dir_properties.change_time > dir_properties.change_time);
        assert_eq!(new_dir_properties.modification_time, time_of_replacement);
        // Both mtime and ctime for child1 should be updated
        let new_child1_properties = child1.get_properties().await.expect("get_properties failed");
        assert_eq!(new_child1_properties.modification_time, time_of_replacement);
        assert_eq!(new_child1_properties.change_time, time_of_replacement);
        // Only ctime for child2 should be updated
        let moved_child2_properties = child2.get_properties().await.expect("get_properties failed");
        assert_eq!(moved_child2_properties.change_time, time_of_replacement);
        assert_eq!(moved_child2_properties.creation_time, child2_properties.creation_time);
        assert_eq!(moved_child2_properties.access_time, child2_properties.access_time);
        assert_eq!(moved_child2_properties.modification_time, child2_properties.modification_time);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_unlink_timestamps() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let foo;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");
        foo =
            dir.create_child_file(&mut transaction, "foo").await.expect("create_child_dir failed");

        transaction.commit().await.expect("commit failed");
        let dir_properties = dir.get_properties().await.expect("get_properties failed");
        let foo_properties = foo.get_properties().await.expect("get_properties failed");

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), foo.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_matches!(
            replace_child(&mut transaction, None, (&dir, "foo"))
                .await
                .expect("replace_child failed"),
            ReplacedChild::Object(_)
        );
        transaction.commit().await.expect("commit failed");
        // Both mtime and ctime for dir should be updated
        let new_dir_properties = dir.get_properties().await.expect("get_properties failed");
        let time_of_replacement = new_dir_properties.change_time;
        assert!(new_dir_properties.change_time > dir_properties.change_time);
        assert_eq!(new_dir_properties.modification_time, time_of_replacement);
        // Only ctime for foo should be updated
        let moved_foo_properties = foo.get_properties().await.expect("get_properties failed");
        assert_eq!(moved_foo_properties.change_time, time_of_replacement);
        assert_eq!(moved_foo_properties.creation_time, foo_properties.creation_time);
        assert_eq!(moved_foo_properties.access_time, foo_properties.access_time);
        assert_eq!(moved_foo_properties.modification_time, foo_properties.modification_time);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_replace_dir_timestamps() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let dir;
        let child_dir1;
        let child_dir2;
        let foo;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        dir = Directory::create(&mut transaction, &fs.root_store(), None)
            .await
            .expect("create failed");
        child_dir1 =
            dir.create_child_dir(&mut transaction, "dir1").await.expect("create_child_dir failed");
        child_dir2 =
            dir.create_child_dir(&mut transaction, "dir2").await.expect("create_child_dir failed");
        foo = child_dir1
            .create_child_dir(&mut transaction, "foo")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");
        let dir_props = dir.get_properties().await.expect("get_properties failed");
        let foo_props = foo.get_properties().await.expect("get_properties failed");

        transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(fs.root_store().store_object_id(), dir.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child_dir1.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), child_dir2.object_id()),
                    LockKey::object(fs.root_store().store_object_id(), foo.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_matches!(
            replace_child(&mut transaction, Some((&child_dir1, "foo")), (&dir, "dir2"))
                .await
                .expect("replace_child failed"),
            ReplacedChild::Directory(_)
        );
        transaction.commit().await.expect("commit failed");
        // Both mtime and ctime for dir should be updated
        let new_dir_props = dir.get_properties().await.expect("get_properties failed");
        let time_of_replacement = new_dir_props.change_time;
        assert!(new_dir_props.change_time > dir_props.change_time);
        assert_eq!(new_dir_props.modification_time, time_of_replacement);
        // Both mtime and ctime for dir1 should be updated
        let new_dir1_props = child_dir1.get_properties().await.expect("get_properties failed");
        let time_of_replacement = new_dir1_props.change_time;
        assert_eq!(new_dir1_props.change_time, time_of_replacement);
        assert_eq!(new_dir1_props.modification_time, time_of_replacement);
        // Only ctime for foo should be updated
        let moved_foo_props = foo.get_properties().await.expect("get_properties failed");
        assert_eq!(moved_foo_props.change_time, time_of_replacement);
        assert_eq!(moved_foo_props.creation_time, foo_props.creation_time);
        assert_eq!(moved_foo_props.access_time, foo_props.access_time);
        assert_eq!(moved_foo_props.modification_time, foo_props.modification_time);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_create_casefold_directory() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let object_id = {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let dir = Directory::create(&mut transaction, &fs.root_store(), None)
                .await
                .expect("create failed");

            let child_dir = dir
                .create_child_dir(&mut transaction, "foo")
                .await
                .expect("create_child_dir failed");
            let _child_dir_file = child_dir
                .create_child_file(&mut transaction, "bAr")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            dir.object_id()
        };
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;

        // We now have foo/bAr which should be case sensitive (casefold not enabled).

        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        {
            let dir = Directory::open(&fs.root_store(), object_id).await.expect("open failed");
            let (object_id, object_descriptor, _) =
                dir.lookup("foo").await.expect("lookup failed").expect("not found");
            assert_eq!(object_descriptor, ObjectDescriptor::Directory);
            let child_dir =
                Directory::open(&fs.root_store(), object_id).await.expect("open failed");
            assert!(!child_dir.dir_type().is_casefold());
            assert!(child_dir.lookup("BAR").await.expect("lookup failed").is_none());
            let (object_id, descriptor, _) =
                child_dir.lookup("bAr").await.expect("lookup failed").unwrap();
            assert_eq!(descriptor, ObjectDescriptor::File);

            // We can't set casefold now because the directory isn't empty.
            child_dir.set_casefold(true).await.expect_err("not empty");

            // Delete the file and subdir and try again.
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![
                        LockKey::object(fs.root_store().store_object_id(), child_dir.object_id()),
                        LockKey::object(fs.root_store().store_object_id(), object_id),
                    ],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            assert_matches!(
                replace_child(&mut transaction, None, (&child_dir, "bAr"))
                    .await
                    .expect("replace_child failed"),
                ReplacedChild::Object(..)
            );
            transaction.commit().await.expect("commit failed");

            // This time enabling casefold should succeed.
            child_dir.set_casefold(true).await.expect("set casefold");

            assert!(child_dir.dir_type().is_casefold());

            // Create the file again now that casefold is enabled.
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        fs.root_store().store_object_id(),
                        child_dir.object_id()
                    ),],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let _child_dir_file = child_dir
                .create_child_file(&mut transaction, "bAr")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");

            // Check that we can lookup via a case insensitive name.
            assert!(child_dir.lookup("BAR").await.expect("lookup failed").is_some());
            assert!(child_dir.lookup("bAr").await.expect("lookup failed").is_some());

            // Enabling casefold should fail again as the dir is not empty.
            child_dir.set_casefold(true).await.expect_err("set casefold");
            assert!(child_dir.dir_type().is_casefold());

            // Confirm that casefold will affect created subdirectories.
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        fs.root_store().store_object_id(),
                        child_dir.object_id()
                    ),],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let sub_dir = child_dir
                .create_child_dir(&mut transaction, "sub")
                .await
                .expect("create_sub_dir failed");
            transaction.commit().await.expect("commit failed");
            assert!(sub_dir.dir_type().is_casefold());
        };
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_create_casefold_encrypted_directory() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let proxy_filename: ProxyFilename;
        let object_id;
        {
            let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
            let root_volume = root_volume(fs.clone()).await.unwrap();
            let store = root_volume
                .new_volume(
                    "vol",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone()),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            // Create a (very weak) key for our encrypted directory.
            crypt
                .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
                .expect("add wrapping key failed");

            object_id = {
                let mut transaction = fs
                    .root_store()
                    .new_transaction(lock_keys![], Options::default())
                    .await
                    .expect("new_transaction failed");
                let dir = Directory::create(&mut transaction, &store, Some(WRAPPING_KEY_ID))
                    .await
                    .expect("create failed");

                transaction.commit().await.expect("commit");
                dir.object_id()
            };
            let dir = Directory::open(&store, object_id).await.expect("open failed");

            dir.set_casefold(true).await.expect("set casefold");
            assert!(dir.dir_type().is_casefold());

            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), dir.object_id()),],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let _file = dir
                .create_child_file(&mut transaction, "bAr")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");

            // Check that we can look up the original name.
            assert!(dir.lookup("bAr").await.expect("original lookup failed").is_some());

            // Derive the proxy filename now, for use later when operating on the locked volume
            // as we won't have the key then.
            let key = dir.get_fscrypt_key().await.expect("key").into_cipher().unwrap();
            let encrypted_name =
                encrypt_filename(&*key, dir.object_id(), "bAr").expect("encrypt_filename");
            let hash_code = key.hash_code_casefold("bAr");
            proxy_filename = ProxyFilename::new_with_hash_code(hash_code as u64, &encrypted_name);

            // Check that we can lookup via a case insensitive name.
            assert!(dir.lookup("BAR").await.expect("casefold lookup failed").is_some());

            // Check hash values generated are stable across case.
            assert_eq!(key.hash_code_casefold("bar"), key.hash_code_casefold("BaR"));

            // We can't easily check iteration from here as we only get encrypted entries so
            // we just count instead.
            let mut count = 0;
            let layer_set = dir.store().tree().layer_set();
            let mut merger = layer_set.merger();
            let mut iter = dir.iter(&mut merger).await.expect("iter");
            while let Some(_entry) = iter.get() {
                count += 1;
                iter.advance().await.expect("advance");
            }
            assert_eq!(1, count, "unexpected number of entries.");

            fs.close().await.expect("Close failed");
        }

        let device = fs.take_device().await;

        // Now try and read the encrypted directory without keys.

        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        {
            let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .volume(
                    "vol",
                    StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                )
                .await
                .expect("volume failed");
            let dir = Directory::open(&store, object_id).await.expect("open failed");
            assert!(dir.dir_type().is_casefold());

            // Check that we can NOT look up the original name.
            assert!(dir.lookup("bAr").await.expect("lookup failed").is_none());
            // We should instead see the proxy filename.
            let filename: String = proxy_filename.into();
            assert!(dir.lookup(&filename).await.expect("lookup failed").is_some());

            let layer_set = dir.store().tree().layer_set();
            let mut merger = layer_set.merger();
            let mut iter = dir.iter(&mut merger).await.expect("iter");
            let item = iter.get().expect("expect item");
            let filename: String = proxy_filename.into();
            assert_eq!(item.0, &filename);
            iter.advance().await.expect("advance");
            assert_eq!(None, iter.get());

            fsck(fs.clone()).await.unwrap();
            fsck_volume(fs.as_ref(), store.store_object_id(), Some(crypt.clone())).await.unwrap();

            fs.close().await.expect("Close failed");
        }
    }

    /// Search for a pair of filenames that encode to the same casefold hash and same
    /// filename prefix, but different sha256.
    /// We are specifically looking for a case where encrypted child of a > encrypted child of b
    /// but proxy_filename of a < proxy filename of b or vice versa.
    /// This is to fully test the iterator logic for locked directories.
    ///
    /// Note this is a SLOW process (~12 seconds on my workstation with release build).
    /// For that reason, the solution is hard coded and this function is marked as ignored.
    ///
    /// Returns a pair of filenames on success, None on failure.
    #[allow(dead_code)]
    fn find_out_of_order_sha256_long_prefix_pair(
        object_id: u64,
        key: &Arc<dyn Cipher>,
    ) -> Option<[String; 2]> {
        let mut collision_map: std::collections::HashMap<u32, (usize, ProxyFilename, Vec<u8>)> =
            std::collections::HashMap::new();
        for i in 0..(1usize << 32) {
            let filename = format!("{:0>176}_{i}", 0);
            let encrypted_name =
                encrypt_filename(&**key, object_id, &filename).expect("encrypt_filename");
            let hash_code = key.hash_code_casefold(&filename);
            let a = ProxyFilename::new_with_hash_code(hash_code as u64, &encrypted_name);
            let hash_code = a.hash_code as u32;
            if let Some((j, b, b_encrypted_name)) = collision_map.get(&hash_code) {
                assert_eq!(a.filename, b.filename);
                if encrypted_name.cmp(b_encrypted_name) != a.sha256.cmp(&b.sha256) {
                    return Some([format!("{:0>176}_{i}", 0), format!("{:0>176}_{j}", 0)]);
                }
            } else {
                collision_map.insert(hash_code, (i, a, encrypted_name));
            }
        }
        None
    }

    #[fuchsia::test]
    async fn test_proxy_filename() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let mut filenames = Vec::new();
        let object_id;
        {
            let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
            let root_volume = root_volume(fs.clone()).await.unwrap();
            let store = root_volume
                .new_volume(
                    "vol",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone()),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            // Create a (very weak) key for our encrypted directory.
            crypt
                .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
                .expect("add wrapping key failed");

            object_id = {
                let mut transaction = fs
                    .root_store()
                    .new_transaction(lock_keys![], Options::default())
                    .await
                    .expect("new_transaction failed");
                let dir = Directory::create(&mut transaction, &store, Some(WRAPPING_KEY_ID))
                    .await
                    .expect("create failed");

                transaction.commit().await.expect("commit");
                dir.object_id()
            };

            let dir = Directory::open(&store, object_id).await.expect("open failed");

            dir.set_casefold(true).await.expect("set casefold");
            assert!(dir.dir_type().is_casefold());

            let key = dir.get_fscrypt_key().await.expect("key").into_cipher().unwrap();

            // Nb: We use a rather expensive brute force search to find two filenames that:
            //   1. Have the same hash_code.
            //   2. Have the same prefix.
            //   3. Have encrypted names and sha256 that sort differently.
            // This is to exercise iter_from and lookup() handling scanning of locked directories.
            // This search returns stable results so in the interest of cheap tests, this code
            // is commented out but should be equivalent to the constants below.
            // let collision_pair =
            //     find_out_of_order_sha256_long_prefix_pair(dir.object_id(), &key).unwrap();
            let collision_pair =
                [format!("{:0>176}_{}", 0, 93515), format!("{:0>176}_{}", 0, 15621)];

            // Create set of files with a common prefix, long enough to exceed prefix length of 48.
            // The first 48 encrypted name bytes will be the same, but the `sha256` will differ.
            for filename in (0..64)
                .into_iter()
                .map(|i| format!("{:0>176}_{i}", 0))
                .chain(collision_pair.into_iter())
            {
                let hash_code = key.hash_code_casefold(&filename);
                let encrypted_name =
                    encrypt_filename(&*key, dir.object_id(), &filename).expect("encrypt_filename");
                let proxy_filename =
                    ProxyFilename::new_with_hash_code(hash_code as u64, &encrypted_name);
                let mut transaction = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object(store.store_object_id(), dir.object_id()),],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                let file = dir
                    .create_child_file(&mut transaction, &filename)
                    .await
                    .expect("create_child_file failed");
                filenames.push((proxy_filename, file.object_id()));
                transaction.commit().await.expect("commit failed");
            }

            fs.close().await.expect("Close failed");
        }

        let device = fs.take_device().await;

        // Now try and read the encrypted directory without keys.
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        {
            let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .volume(
                    "vol",
                    StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                )
                .await
                .expect("volume failed");
            let dir = Directory::open(&store, object_id).await.expect("open failed");
            assert!(dir.dir_type().is_casefold());

            // Ensure uniqueness of the proxy filenames.
            assert_eq!(
                filenames.iter().map(|(name, _)| (*name).into()).collect::<HashSet<String>>().len(),
                filenames.len()
            );

            let filename = filenames[0].0.filename.clone();
            for (proxy_filename, object_id) in &filenames {
                // We used such a long prefix that we expect all files to share it.
                assert_eq!(filename, proxy_filename.filename);

                let proxy_filename_str: String = (*proxy_filename).into();
                let item = dir
                    .lookup(&proxy_filename_str)
                    .await
                    .expect("lookup failed")
                    .expect("lookup is not None");
                assert_eq!(item.0, *object_id, "Mismatch for filename '{proxy_filename:?}'");
            }

            fs.close().await.expect("Close failed");
        }
    }

    #[fuchsia::test]
    async fn test_replace_directory_and_tombstone_on_remount() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 4096));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt = Arc::new(new_insecure_crypt());
        {
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "test",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");

            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");

            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let _directory = root_directory
                .create_child_dir(&mut transaction, "foo")
                .await
                .expect("create_child_dir failed");
            let directory = root_directory
                .create_child_dir(&mut transaction, "bar")
                .await
                .expect("create_child_dir failed");
            let oid = directory.object_id();

            transaction.commit().await.expect("commit failed");

            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");

            replace_child_with_object(
                &mut transaction,
                Some((oid, ObjectDescriptor::Directory)),
                (&root_directory, "foo"),
                0,
                false,
                Timestamp::now(),
            )
            .await
            .expect("replace_child_with_object failed");

            // If replace_child_with_object erroneously were to queue a tombstone, this will allow
            // it to run before we've committed, which will cause the test to fail below when we
            // remount and try and tombstone the object again.
            yield_to_executor().await;

            transaction.commit().await.expect("commit failed");

            fs.close().await.expect("close failed");
        }

        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let _store = root_volume
            .volume(
                "test",
                StoreOptions {
                    crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                    ..StoreOptions::default()
                },
            )
            .await
            .expect("new_volume failed");

        // Allow the graveyard to run.
        yield_to_executor().await;

        fs.close().await.expect("close failed");
    }

    #[test_case(false; "non_casefold")]
    #[test_case(true; "casefold")]
    #[fuchsia::test]
    async fn test_lookup_long_filename_in_locked_directory(casefold: bool) {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let object_id;
        let mut filenames = Vec::new();
        {
            let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
            let root_volume = root_volume(fs.clone()).await.unwrap();
            let store = root_volume
                .new_volume(
                    "vol",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone()),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            crypt
                .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
                .expect("add_wrapping_key failed");

            object_id = {
                let mut transaction = fs
                    .root_store()
                    .new_transaction(lock_keys![], Options::default())
                    .await
                    .expect("new_transaction failed");
                let dir = Directory::create(&mut transaction, &store, Some(WRAPPING_KEY_ID))
                    .await
                    .expect("create failed");

                transaction.commit().await.expect("commit");
                dir.object_id()
            };
            let dir = Directory::open(&store, object_id).await.expect("open failed");
            if casefold {
                dir.set_casefold(true).await.expect("set casefold");
            }

            let key = dir.get_fscrypt_key().await.expect("key").into_cipher().unwrap();

            for len in [144, 145, 255] {
                let filename = "a".repeat(len);
                let encrypted_name =
                    encrypt_filename(&*key, dir.object_id(), &filename).expect("encrypt_filename");
                let proxy_filename = if casefold {
                    let hash_code = key.hash_code_casefold(&filename);
                    ProxyFilename::new_with_hash_code(hash_code as u64, &encrypted_name)
                } else {
                    ProxyFilename::new(&encrypted_name)
                };
                let mut transaction = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object(store.store_object_id(), dir.object_id()),],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                let file = dir
                    .create_child_file(&mut transaction, &filename)
                    .await
                    .expect("create_child_file failed");
                filenames.push((proxy_filename, file.object_id()));
                transaction.commit().await.expect("commit failed");
            }

            fs.close().await.expect("Close failed");
        }

        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        {
            let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .volume(
                    "vol",
                    StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                )
                .await
                .expect("volume failed");
            let dir = Directory::open(&store, object_id).await.expect("open failed");

            // Verify that iteration works.
            let layer_set = dir.store().tree().layer_set();
            let mut merger = layer_set.merger();
            let mut iter = dir.iter(&mut merger).await.expect("iter failed");
            let mut entries = Vec::new();
            while let Some((name, _, _)) = iter.get() {
                entries.push(name.to_string());
                iter.advance().await.expect("advance failed");
            }
            assert_eq!(entries.len(), filenames.len());

            for (proxy_filename, object_id) in &filenames {
                let proxy_filename_str: String = (*proxy_filename).into();
                assert!(entries.contains(&proxy_filename_str));
                let item = dir
                    .lookup(&proxy_filename_str)
                    .await
                    .expect("lookup failed")
                    .expect("lookup is not None");
                assert_eq!(item.0, *object_id, "Mismatch for filename '{proxy_filename:?}'");
            }

            fs.close().await.expect("Close failed");
        }
    }

    #[fuchsia::test]
    async fn test_lookup_cached_entry_after_unlock() {
        const FILENAME: &str = "foo";
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let object_id;
        let proxy_filename;
        {
            let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
            let root_volume = root_volume(fs.clone()).await.unwrap();
            let store = root_volume
                .new_volume(
                    "vol",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone()),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            crypt
                .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
                .expect("add_wrapping_key failed");

            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let dir = Directory::create(&mut transaction, &store, Some(WRAPPING_KEY_ID))
                .await
                .expect("create failed");
            transaction.commit().await.expect("commit");
            object_id = dir.object_id();

            let key = dir.get_fscrypt_key().await.expect("key").into_cipher().unwrap();
            let encrypted_name =
                encrypt_filename(&*key, object_id, FILENAME).expect("encrypt_filename");
            proxy_filename = ProxyFilename::new(&encrypted_name);

            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), object_id),],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            dir.create_child_file(&mut transaction, FILENAME)
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");

            fs.close().await.expect("Close failed");
        }

        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        {
            let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .volume(
                    "vol",
                    StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                )
                .await
                .expect("volume failed");
            let dir = Directory::open(&store, object_id).await.expect("open failed");

            let proxy_filename_str: String = proxy_filename.into();
            // This should succeed because the directory is locked.
            dir.lookup(&proxy_filename_str)
                .await
                .expect("lookup failed")
                .expect("lookup is not None");

            crypt
                .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
                .expect("add_wrapping_key failed");

            // This should fail because the directory is now unlocked and we shouldn't be able to
            // find the file using its encrypted name.
            assert!(dir.lookup(&proxy_filename_str).await.expect("lookup failed").is_none());

            fs.close().await.expect("Close failed");
        }
    }

    #[test_case(false, false; "no_encryption_no_casefold")]
    #[test_case(false, true; "no_encryption_casefold")]
    #[test_case(true, false; "encryption_no_casefold")]
    #[test_case(true, true; "encryption_casefold")]
    #[fuchsia::test]
    async fn test_traversal_position(encrypted: bool, casefold: bool) {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        {
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let crypt = Arc::new(new_insecure_crypt());
            crypt
                .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
                .expect("add_wrapping_key failed");
            let store = root_volume
                .new_volume(
                    "test",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");
            let mut root_dir = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            if encrypted {
                let mut transaction = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object(
                            store.store_object_id(),
                            store.root_directory_object_id()
                        )],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                root_dir.set_wrapping_key(&mut transaction, WRAPPING_KEY_ID).await.unwrap();
                transaction.commit().await.unwrap();

                // `set_wrapping_key` doesn't update the in-memory state, so reopen the directory.
                root_dir = Directory::open(&store, store.root_directory_object_id())
                    .await
                    .expect("open failed");
            }
            if casefold {
                root_dir.set_casefold(true).await.unwrap();
            }

            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let _ = root_dir.create_child_file(&mut transaction, "foo").await.unwrap();
            transaction.commit().await.unwrap();

            let layer_set = store.tree().layer_set();
            let mut merger = layer_set.merger();
            let iter = root_dir.iter(&mut merger).await.expect("iter failed");
            let pos =
                iter.traversal_position(|name| name.to_string(), |bytes| format!("{:?}", bytes));
            assert!(
                pos.is_some(),
                "traversal_position returned None for encrypted={}, casefold={}",
                encrypted,
                casefold
            );
        }
        fs.close().await.expect("close failed");
    }

    /// Verifies that renaming a file to a casefold-equivalent casing variant within
    /// the same directory works.
    #[fuchsia::test]
    async fn test_casefold_same_dir_rename() -> Result<(), Error> {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");

        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume.new_volume("vol", NewChildStoreOptions::default()).await.unwrap();

        let dir = {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let dir =
                Directory::create(&mut transaction, &store, None).await.expect("create failed");
            transaction.commit().await.expect("commit");
            dir
        };

        // 1. Enable casefolding on the directory
        dir.set_casefold(true).await.expect("set casefold");

        // 2. Create a child file "FOO" (casing: uppercase)
        let file_id = {
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), dir.object_id())],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let file =
                dir.create_child_file(&mut transaction, "FOO").await.expect("create file failed");
            transaction.commit().await.expect("commit failed");
            file.object_id()
        };

        // 3. Confirm target can be looked up under both "FOO" and "foo" (due to case-insensitivity)
        let (lookup_id_foo, _, _) = dir.lookup("foo").await.unwrap().unwrap();
        assert_eq!(lookup_id_foo, file_id);
        let (lookup_id_foo_upper, _, _) = dir.lookup("FOO").await.unwrap().unwrap();
        assert_eq!(lookup_id_foo_upper, file_id);

        // 4. Execute the same-directory casefolded rename "FOO" -> "foo" (lowercase)
        {
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![
                        LockKey::object(store.store_object_id(), dir.object_id()),
                        LockKey::object(store.store_object_id(), file_id),
                    ],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");

            replace_child(&mut transaction, Some((&dir, "FOO")), (&dir, "foo"))
                .await
                .expect("same-dir casefold rename failed");

            transaction.commit().await.expect("commit failed");
        }

        // 5. Assert the file was NOT purged (lookup still succeeds!)
        let (final_id_foo, _, _) = dir.lookup("foo").await.unwrap().unwrap();
        assert_eq!(final_id_foo, file_id);

        // Verify survival of the underlying node handle (ObjectStore::open_object)
        let file_handle =
            ObjectStore::open_object(&dir.owner(), file_id, HandleOptions::default(), None)
                .await
                .expect("Underlying file object was prematurely tombstoned / graveyarded!");

        // Assert reference count is exactly 1.
        let properties = file_handle.get_properties().await.unwrap();
        assert_eq!(properties.refs, 1);

        // Assert the graveyard is completely empty (proves NO graveyard tombstone leak occurred!)
        assert_eq!(dir.store().graveyard_count(), 0);

        // 6. Verify the internal casing entry was updated to "foo"
        let mut count = 0;
        let mut found_casing = String::new();
        let layer_set = dir.store().tree().layer_set();
        let mut merger = layer_set.merger();
        let mut iter = dir.iter(&mut merger).await.expect("iter");
        while let Some(entry) = iter.get() {
            count += 1;
            found_casing = entry.0.to_string();
            iter.advance().await.expect("advance");
        }
        assert_eq!(count, 1);
        assert_eq!(found_casing, "foo"); // mapping has successfully changed from "FOO" to "foo".

        fs.close().await.expect("Close failed");
        Ok(())
    }

    #[fuchsia::test]
    async fn test_casefold_rename_mismatched_casing() -> Result<(), Error> {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");

        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume.new_volume("vol", NewChildStoreOptions::default()).await.unwrap();

        let dir = {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let dir =
                Directory::create(&mut transaction, &store, None).await.expect("create failed");
            transaction.commit().await.expect("commit");
            dir
        };

        dir.set_casefold(true).await.expect("set casefold");

        // Create "foo" (lowercase)
        let file_id = {
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), dir.object_id())],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let file =
                dir.create_child_file(&mut transaction, "foo").await.expect("create file failed");
            transaction.commit().await.expect("commit failed");
            file.object_id()
        };

        // Rename "FOO" (uppercase) to "bar"
        {
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![
                        LockKey::object(store.store_object_id(), dir.object_id()),
                        LockKey::object(store.store_object_id(), file_id),
                    ],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");

            replace_child(&mut transaction, Some((&dir, "FOO")), (&dir, "bar"))
                .await
                .expect("rename failed");

            transaction.commit().await.expect("commit failed");
        }

        // Check if "foo" (or "FOO") is gone.
        let lookup_foo = dir.lookup("foo").await.unwrap();
        assert!(
            lookup_foo.is_none(),
            "Old name 'foo' still exists! Lookup returned: {:?}",
            lookup_foo
        );

        // Check if "bar" exists.
        let (lookup_id_bar, _, _) = dir.lookup("bar").await.unwrap().expect("bar not found");
        assert_eq!(lookup_id_bar, file_id);

        fs.close().await.expect("Close failed");
        Ok(())
    }

    #[fuchsia::test]
    async fn test_hard_link_encrypted_symlink_and_fsck_passes() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
        crypt.add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into()).expect("add_wrapping_key failed");

        let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");

        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        root_directory.set_wrapping_key(&mut transaction, WRAPPING_KEY_ID).await.unwrap();
        transaction.commit().await.unwrap();

        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        let symlink_id = root_directory
            .create_symlink(&mut transaction, b"target_path", "symlink")
            .await
            .expect("create_symlink failed");
        transaction.commit().await.expect("commit failed");

        fsck(filesystem.clone()).await.expect("fsck failed");
        fsck_volume(filesystem.as_ref(), store.store_object_id(), Some(crypt.clone()))
            .await
            .expect("fsck_volume failed");

        let mut transaction = filesystem
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(store.store_object_id(), root_directory.object_id()),
                    LockKey::object(store.store_object_id(), symlink_id)
                ],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        root_directory
            .insert_child(&mut transaction, "symlink_link", symlink_id, ObjectDescriptor::Symlink)
            .await
            .expect("insert_child failed");
        store.adjust_refs(&mut transaction, symlink_id, 1).await.expect("adjust_refs failed");
        transaction.commit().await.expect("commit failed");

        fsck(filesystem.clone()).await.expect("fsck failed");
        fsck_volume(filesystem.as_ref(), store.store_object_id(), Some(crypt.clone()))
            .await
            .expect("fsck_volume failed");

        let mut transaction = filesystem
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(store.store_object_id(), root_directory.object_id()),
                    LockKey::object(store.store_object_id(), symlink_id)
                ],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        replace_child(&mut transaction, None, (&root_directory, "symlink"))
            .await
            .expect("replace_child failed");
        transaction.commit().await.expect("commit failed");

        fsck(filesystem.clone()).await.expect("fsck failed");
        fsck_volume(filesystem.as_ref(), store.store_object_id(), Some(crypt.clone()))
            .await
            .expect("fsck_volume failed");

        filesystem.close().await.expect("close failed");
    }
}
