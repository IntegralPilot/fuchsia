// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::Path;

use crate::error::Error;
use crate::{CanonicalizeContext, OneOrMany};
use cm_types::{Availability, BorrowedName, Name};
use serde::Serialize;

#[derive(Debug, Clone, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ContextSpanned<T> {
    pub value: T,

    #[serde(skip)]
    pub origin: Arc<std::path::Path>,
}

impl<T: PartialEq> PartialEq for ContextSpanned<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: Eq> Eq for ContextSpanned<T> {}

impl<T: Hash> Hash for ContextSpanned<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T> ContextSpanned<T> {
    pub fn map<U, F>(self, f: F) -> ContextSpanned<U>
    where
        F: FnOnce(T) -> U,
    {
        ContextSpanned { value: f(self.value), origin: self.origin }
    }

    pub fn new_synthetic(value: T, file: &std::path::Path) -> Self {
        Self { value, origin: Arc::from(file) }
    }

    pub fn maybe_synthetic(val: Option<T>, file: &std::path::Path) -> Option<Self> {
        val.map(|v| Self::new_synthetic(v, file))
    }
}

impl<T: CanonicalizeContext> CanonicalizeContext for ContextSpanned<T> {
    fn canonicalize_context(&mut self) {
        self.value.canonicalize_context();
    }
}

impl<T: ContextPathClause> ContextPathClause for ContextSpanned<T> {
    fn path(&self) -> Option<&ContextSpanned<Path>> {
        self.value.path()
    }
}

/// Helper to wrap programmatic values in a ContextSpanned wrapper.
pub fn synthetic_span<T>(value: T) -> ContextSpanned<T> {
    ContextSpanned { value, origin: Arc::from(std::path::Path::new("programmatic_manifest.cml")) }
}

/// Hydrate is used to translate a type to a
/// Result<ContextSpanned> type. The ContextSpanned type is used by validation.
///
/// It is possible to error when merging if a field is defined as multiple,
/// incompatible data structures.
pub trait Hydrate {
    type Output;

    fn hydrate(self, file: &Arc<std::path::Path>) -> Result<Self::Output, Error>;
}

pub fn hydrate_list<P, C>(
    raw_list: Option<Vec<P>>,
    file: &Arc<std::path::Path>,
) -> Result<Option<Vec<ContextSpanned<C>>>, Error>
where
    P: Hydrate<Output = C>,
{
    raw_list
        .map(|vec| {
            vec.into_iter()
                .map(|item| {
                    let context_item_result = item.hydrate(file);
                    context_item_result
                        .map(|c_value| ContextSpanned { value: c_value, origin: file.clone() })
                })
                .collect::<Result<Vec<ContextSpanned<C>>, Error>>()
        })
        .transpose()
}

pub fn hydrate_required<P, C>(
    parsed_value: P,
    file: &Arc<std::path::Path>,
) -> Result<ContextSpanned<C>, Error>
where
    P: Hydrate<Output = C>,
{
    let context_value = parsed_value.hydrate(file)?;

    Ok(ContextSpanned { value: context_value, origin: file.clone() })
}

pub fn hydrate_simple<T>(value: T, file: &Arc<std::path::Path>) -> ContextSpanned<T> {
    ContextSpanned { value, origin: file.clone() }
}

pub fn hydrate_opt<P, C>(
    opt: Option<P>,
    file: &Arc<std::path::Path>,
) -> Result<Option<ContextSpanned<C>>, Error>
where
    P: Hydrate<Output = C>,
{
    opt.map(|s| hydrate_required(s, file)).transpose()
}

pub fn hydrate_opt_simple<T>(
    opt_spanned: Option<T>,
    file: &Arc<std::path::Path>,
) -> Option<ContextSpanned<T>> {
    opt_spanned.map(|s| hydrate_simple(s, file))
}

pub fn option_one_or_many_as_ref_context<T, S: ?Sized>(
    o: &Option<ContextSpanned<OneOrMany<T>>>,
) -> Option<ContextSpanned<OneOrMany<&S>>>
where
    T: AsRef<S>,
{
    o.as_ref().map(|spanned| ContextSpanned {
        origin: spanned.origin.clone(),
        value: match &spanned.value {
            OneOrMany::One(item) => OneOrMany::One(item.as_ref()),
            OneOrMany::Many(items) => {
                OneOrMany::Many(items.iter().map(|item| item.as_ref()).collect())
            }
        },
    })
}

pub trait ContextPathClause {
    fn path(&self) -> Option<&ContextSpanned<Path>>;
}

pub trait ContextCapabilityClause: Clone + PartialEq + std::fmt::Debug {
    fn service(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>>;
    fn protocol(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>>;
    fn directory(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>>;
    fn storage(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>>;
    fn runner(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>>;
    fn resolver(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>>;
    fn dictionary(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>>;
    fn config(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>>;
    fn event_stream(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>>;
    fn origin(&self) -> &Arc<std::path::Path>;
    fn availability(&self) -> Option<ContextSpanned<Availability>>;

    fn set_availability(&mut self, a: Option<ContextSpanned<Availability>>);
    fn set_service(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>);
    fn set_protocol(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>);
    fn set_directory(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>);
    fn set_storage(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>);
    fn set_runner(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>);
    fn set_resolver(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>);
    fn set_event_stream(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>);
    fn set_dictionary(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>);
    fn set_config(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>);

    // /// Returns the name of the capability for display purposes.
    // /// If `service()` returns `Some`, the capability name must be "service", etc.
    // ///
    // /// Returns an error if the capability name is not set, or if there is more than one.
    fn capability_type(&self, origin: Option<Arc<std::path::Path>>) -> Result<&'static str, Error> {
        let mut types = Vec::new();
        if self.service().is_some() {
            types.push("service");
        }
        if self.protocol().is_some() {
            types.push("protocol");
        }
        if self.directory().is_some() {
            types.push("directory");
        }
        if self.storage().is_some() {
            types.push("storage");
        }
        if self.event_stream().is_some() {
            types.push("event_stream");
        }
        if self.runner().is_some() {
            types.push("runner");
        }
        if self.config().is_some() {
            types.push("config");
        }
        if self.resolver().is_some() {
            types.push("resolver");
        }
        if self.dictionary().is_some() {
            types.push("dictionary");
        }
        match types.len() {
            0 => {
                let supported_keywords = self
                    .supported()
                    .iter()
                    .map(|k| format!("\"{}\"", k))
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(Error::validate_context(
                    format!(
                        "`{}` declaration is missing a capability keyword, one of: {}",
                        self.decl_type(),
                        supported_keywords,
                    ),
                    origin,
                ))
            }
            1 => Ok(types[0]),
            _ => Err(Error::validate_context(
                format!(
                    "{} declaration has multiple capability types defined: {:?}",
                    self.decl_type(),
                    types
                ),
                origin,
            )),
        }
    }

    /// Returns the names of the capabilities in this clause, wrapped in ContextSpanned.
    /// This allows the caller to know the file source of every individual name.
    fn names(&self) -> Vec<ContextSpanned<Name>> {
        let extract = |field: Option<&ContextSpanned<OneOrMany<&BorrowedName>>>| -> Vec<ContextSpanned<Name>> {
        match field {
                    Some(wrapper) => match &wrapper.value {
                        // n is &&BorrowedName. We deref once to get &BorrowedName,
                        // then .to_owned() converts it to Name.
                        OneOrMany::One(n) => vec![ContextSpanned {
                            value: (*n).to_owned(),
                            origin: wrapper.origin.clone(),
                        }],
                        OneOrMany::Many(names) => names
                            .iter()
                            .map(|n| ContextSpanned {
                                value: (*n).to_owned(),
                                origin: wrapper.origin.clone(),
                            })
                            .collect(),
                    },
                    None => vec![],
                }
            };
        // Collect names from all possible fields
        let mut res = Vec::new();
        res.extend(extract(self.service().as_ref()));
        res.extend(extract(self.protocol().as_ref()));
        res.extend(extract(self.directory().as_ref()));
        res.extend(extract(self.storage().as_ref()));
        res.extend(extract(self.runner().as_ref()));
        res.extend(extract(self.config().as_ref()));
        res.extend(extract(self.resolver().as_ref()));
        res.extend(extract(self.event_stream().as_ref()));
        res.extend(extract(self.dictionary().as_ref()));
        res
    }

    /// Sets the names for this capability, preserving origin information.
    fn set_names(&mut self, names: Vec<ContextSpanned<Name>>) {
        let cap_type = self.capability_type(None).expect("Cannot set names on empty capability");

        let mut update_field = |wrapped_val: Option<ContextSpanned<OneOrMany<Name>>>| match cap_type
        {
            "protocol" => self.set_protocol(wrapped_val),
            "service" => self.set_service(wrapped_val),
            "directory" => self.set_directory(wrapped_val),
            "storage" => self.set_storage(wrapped_val),
            "runner" => self.set_runner(wrapped_val),
            "resolver" => self.set_resolver(wrapped_val),
            "event_stream" => self.set_event_stream(wrapped_val),
            "dictionary" => self.set_dictionary(wrapped_val),
            "config" => self.set_config(wrapped_val),
            _ => panic!("Unknown capability type {}", cap_type),
        };

        if names.is_empty() {
            update_field(None);
            return;
        }

        let first_origin = names[0].origin.clone();

        let raw_names: Vec<Name> = names.into_iter().map(|n| n.value).collect();
        let one_or_many = if raw_names.len() == 1 {
            OneOrMany::One(raw_names.into_iter().next().unwrap())
        } else {
            OneOrMany::Many(raw_names)
        };

        let wrapped = Some(ContextSpanned { value: one_or_many, origin: first_origin });

        update_field(wrapped);
    }

    /// Returns true if this capability type allows the ::Many variant of OneOrMany.
    fn are_many_names_allowed(&self) -> bool;

    fn decl_type(&self) -> &'static str;
    fn supported(&self) -> &[&'static str];
}

impl<T: ContextCapabilityClause> ContextCapabilityClause for ContextSpanned<T> {
    fn service(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.value.service()
    }
    fn protocol(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.value.protocol()
    }
    fn directory(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.value.directory()
    }
    fn storage(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.value.storage()
    }
    fn runner(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.value.runner()
    }
    fn resolver(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.value.resolver()
    }
    fn dictionary(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.value.dictionary()
    }
    fn config(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.value.config()
    }
    fn event_stream(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.value.event_stream()
    }

    fn origin(&self) -> &Arc<std::path::Path> {
        &self.origin
    }

    fn set_service(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.value.set_service(o)
    }
    fn set_protocol(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.value.set_protocol(o)
    }
    fn set_directory(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.value.set_directory(o)
    }
    fn set_storage(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.value.set_storage(o)
    }
    fn set_runner(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.value.set_runner(o)
    }
    fn set_resolver(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.value.set_resolver(o)
    }
    fn set_event_stream(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.value.set_event_stream(o)
    }
    fn set_dictionary(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.value.set_dictionary(o)
    }
    fn set_config(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.value.set_config(o)
    }

    fn are_many_names_allowed(&self) -> bool {
        self.value.are_many_names_allowed()
    }
    fn decl_type(&self) -> &'static str {
        self.value.decl_type()
    }
    fn supported(&self) -> &[&'static str] {
        self.value.supported()
    }

    fn availability(&self) -> Option<ContextSpanned<Availability>> {
        self.value.availability()
    }
    fn set_availability(&mut self, a: Option<ContextSpanned<Availability>>) {
        self.value.set_availability(a)
    }
}

#[macro_export]
macro_rules! merge_spanned_vec {
    ($self:expr, $other:expr, $field:ident) => {
        if let Some(other_vec) = $other.$field.take() {
            if let Some(self_vec) = $self.$field.as_mut() {
                for item in other_vec {
                    if !self_vec.contains(&item) {
                        self_vec.push(item);
                    }
                }
            } else {
                $self.$field = Some(other_vec);
            }
        }
    };
}
