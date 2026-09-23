// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::one_or_many::OneOrMany;
use crate::types::capability::ContextCapability;
use crate::types::common::{ContextCapabilityClause, option_one_or_many_as_ref_context};
use crate::types::r#use::ContextUse;
use crate::{AsClauseContext, ContextSpanned, Error, alias_or_name_context};
pub use cm_types::{
    Availability, BorrowedName, BoundedName, DeliveryType, DependencyType, HandleType, Name,
    NamespacePath, OnTerminate, ParseError, Path, RelativePath, StartupMode, StorageId, Url,
};

use std::fmt;
use std::sync::Arc;

/// A name/identity of a capability exposed/offered to another component.
///
/// Exposed or offered capabilities have an identifier whose format
/// depends on the capability type. For directories and services this is
/// a path, while for storage this is a storage name. Paths and storage
/// names, however, are in different conceptual namespaces, and can't
/// collide with each other.
///
/// This enum allows such names to be specified disambiguating what
/// namespace they are in.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum CapabilityId<'a> {
    Service(&'a BorrowedName),
    Protocol(&'a BorrowedName),
    Directory(&'a BorrowedName),
    // A service in a `use` declaration has a target path in the component's namespace.
    UsedService(Path),
    // A protocol in a `use` declaration has a target path in the component's namespace.
    UsedProtocol(Path),
    // A protocol in a `use` declaration has a numbered handle in the component's namespace.
    UsedProtocolNumberedHandle(HandleType),
    // A directory in a `use` declaration has a target path in the component's namespace.
    UsedDirectory(Path),
    // A storage in a `use` declaration has a target path in the component's namespace.
    UsedStorage(Path),
    // An event stream in a `use` declaration has a target path in the component's namespace.
    UsedEventStream(Path),
    // A configuration in a `use` declaration has a target name that matches a config.
    UsedConfiguration(&'a BorrowedName),
    UsedRunner(&'a BorrowedName),
    // A dictionary in a `use` declaration that has a target path in the component's namespace.
    UsedDictionary(Path),
    Storage(&'a BorrowedName),
    Runner(&'a BorrowedName),
    Resolver(&'a BorrowedName),
    EventStream(&'a BorrowedName),
    Dictionary(&'a BorrowedName),
    Configuration(&'a BorrowedName),
}

/// Generates a `Vec<ContextSpanned<&BorrowedName>>` -> `Vec<(CapabilityId, Arc<std::path::Path>)>` conversion function.
macro_rules! capability_ids_from_context_names {
    ($name:ident, $variant:expr) => {
        fn $name(
            names: Vec<ContextSpanned<&'a BorrowedName>>,
        ) -> Vec<(Self, Arc<std::path::Path>)> {
            names
                .into_iter()
                .map(|spanned_name| ($variant(spanned_name.value), spanned_name.origin))
                .collect()
        }
    };
}

/// Generates a `Vec<ContextSpanned<Path>>` -> `Vec<(CapabilityId, Arc<std::path::Path>)>` conversion function.
macro_rules! capability_ids_from_context_paths {
    ($name:ident, $variant:expr) => {
        fn $name(paths: Vec<ContextSpanned<Path>>) -> Vec<(Self, Arc<std::path::Path>)> {
            paths
                .into_iter()
                .map(|spanned_path| ($variant(spanned_path.value), spanned_path.origin))
                .collect()
        }
    };
}

impl<'a> CapabilityId<'a> {
    /// Human readable description of this capability type.
    pub fn type_str(&self) -> &'static str {
        match self {
            CapabilityId::Service(_) => "service",
            CapabilityId::Protocol(_) => "protocol",
            CapabilityId::Directory(_) => "directory",
            CapabilityId::UsedService(_) => "service",
            CapabilityId::UsedProtocol(_) => "protocol",
            CapabilityId::UsedProtocolNumberedHandle(_) => "protocol",
            CapabilityId::UsedDirectory(_) => "directory",
            CapabilityId::UsedStorage(_) => "storage",
            CapabilityId::UsedEventStream(_) => "event_stream",
            CapabilityId::UsedRunner(_) => "runner",
            CapabilityId::UsedConfiguration(_) => "config",
            CapabilityId::UsedDictionary(_) => "dictionary",
            CapabilityId::Storage(_) => "storage",
            CapabilityId::Runner(_) => "runner",
            CapabilityId::Resolver(_) => "resolver",
            CapabilityId::EventStream(_) => "event_stream",
            CapabilityId::Dictionary(_) => "dictionary",
            CapabilityId::Configuration(_) => "config",
        }
    }

    /// Return the directory containing the capability, if this capability takes a target path.
    pub fn get_dir_path(&self) -> Option<NamespacePath> {
        match self {
            CapabilityId::UsedService(p)
            | CapabilityId::UsedProtocol(p)
            | CapabilityId::UsedEventStream(p) => Some(p.parent()),
            CapabilityId::UsedDirectory(p)
            | CapabilityId::UsedStorage(p)
            | CapabilityId::UsedDictionary(p) => Some(p.clone().into()),
            _ => None,
        }
    }

    /// Return the target path of the capability, if this capability has one.
    pub fn get_target_path(&self) -> Option<NamespacePath> {
        match self {
            CapabilityId::UsedService(p)
            | CapabilityId::UsedProtocol(p)
            | CapabilityId::UsedEventStream(p)
            | CapabilityId::UsedDirectory(p)
            | CapabilityId::UsedStorage(p)
            | CapabilityId::UsedDictionary(p) => Some(p.clone().into()),
            _ => None,
        }
    }

    pub fn from_context_capability(
        capability_input: &'a ContextSpanned<ContextCapability>,
    ) -> Result<Vec<(Self, Arc<std::path::Path>)>, Error> {
        let capability = &capability_input.value;
        let origin = &capability_input.origin;

        if let Some(n) = capability.service() {
            if n.value.is_many()
                && let Some(cs_path) = &capability.path
            {
                return Err(Error::validate_context(
                    "\"path\" can only be specified when one `service` is supplied.",
                    Some(cs_path.origin.clone()),
                ));
            }
            return Ok(Self::services_from_context(Self::get_one_or_many_names_context(
                n,
                None,
                capability.capability_type(None).unwrap(),
            )?));
        } else if let Some(n) = capability.protocol() {
            if n.value.is_many()
                && let Some(cs_path) = &capability.path
            {
                return Err(Error::validate_context(
                    "\"path\" can only be specified when one `protocol` is supplied.",
                    Some(cs_path.origin.clone()),
                ));
            }
            return Ok(Self::protocols_from_context(Self::get_one_or_many_names_context(
                n,
                None,
                capability.capability_type(None).unwrap(),
            )?));
        } else if let Some(n) = capability.directory() {
            return Ok(Self::directories_from_context(Self::get_one_or_many_names_context(
                n,
                None,
                capability.capability_type(None).unwrap(),
            )?));
        } else if let Some(cs_storage) = capability.storage() {
            if capability.storage_id.is_none() {
                return Err(Error::validate_context(
                    "Storage declaration is missing \"storage_id\", but is required.",
                    Some(cs_storage.origin),
                ));
            }
            return Ok(Self::storages_from_context(Self::get_one_or_many_names_context(
                cs_storage,
                None,
                capability.capability_type(None).unwrap(),
            )?));
        } else if let Some(n) = capability.runner() {
            return Ok(Self::runners_from_context(Self::get_one_or_many_names_context(
                n,
                None,
                capability.capability_type(None).unwrap(),
            )?));
        } else if let Some(n) = capability.resolver() {
            return Ok(Self::resolvers_from_context(Self::get_one_or_many_names_context(
                n,
                None,
                capability.capability_type(None).unwrap(),
            )?));
        } else if let Some(n) = capability.event_stream() {
            return Ok(Self::event_streams_from_context(Self::get_one_or_many_names_context(
                n,
                None,
                capability.capability_type(None).unwrap(),
            )?));
        } else if let Some(n) = capability.dictionary() {
            return Ok(Self::dictionaries_from_context(Self::get_one_or_many_names_context(
                n,
                None,
                capability.capability_type(None).unwrap(),
            )?));
        } else if let Some(n) = capability.config() {
            return Ok(Self::configurations_from_context(Self::get_one_or_many_names_context(
                n,
                None,
                capability.capability_type(None).unwrap(),
            )?));
        }

        // Unsupported capability type.
        let supported_keywords = capability
            .supported()
            .iter()
            .map(|k| format!("\"{}\"", k))
            .collect::<Vec<_>>()
            .join(", ");
        Err(Error::validate_context(
            format!(
                "`{}` declaration is missing a capability keyword, one of: {}",
                capability.decl_type(),
                supported_keywords,
            ),
            Some(origin.clone()),
        ))
    }

    pub fn from_context_offer_expose<T>(
        clause_input: &'a ContextSpanned<T>,
    ) -> Result<Vec<(Self, Arc<std::path::Path>)>, Error>
    where
        T: ContextCapabilityClause + AsClauseContext + fmt::Debug,
    {
        let clause = &clause_input.value;
        let origin = &clause_input.origin;

        let alias = clause.r#as();

        if let Some(n) = clause.service() {
            return Ok(Self::services_from_context(Self::get_one_or_many_names_context(
                n,
                alias,
                clause.capability_type(Some(origin.clone())).unwrap(),
            )?));
        } else if let Some(n) = clause.protocol() {
            return Ok(Self::protocols_from_context(Self::get_one_or_many_names_context(
                n,
                alias,
                clause.capability_type(Some(origin.clone())).unwrap(),
            )?));
        } else if let Some(n) = clause.directory() {
            return Ok(Self::directories_from_context(Self::get_one_or_many_names_context(
                n,
                alias,
                clause.capability_type(Some(origin.clone())).unwrap(),
            )?));
        } else if let Some(n) = clause.storage() {
            return Ok(Self::storages_from_context(Self::get_one_or_many_names_context(
                n,
                alias,
                clause.capability_type(Some(origin.clone())).unwrap(),
            )?));
        } else if let Some(n) = clause.runner() {
            return Ok(Self::runners_from_context(Self::get_one_or_many_names_context(
                n,
                alias,
                clause.capability_type(Some(origin.clone())).unwrap(),
            )?));
        } else if let Some(n) = clause.resolver() {
            return Ok(Self::resolvers_from_context(Self::get_one_or_many_names_context(
                n,
                alias,
                clause.capability_type(Some(origin.clone())).unwrap(),
            )?));
        } else if let Some(event_stream) = clause.event_stream() {
            return Ok(Self::event_streams_from_context(Self::get_one_or_many_names_context(
                event_stream,
                alias,
                clause.capability_type(Some(origin.clone())).unwrap(),
            )?));
        } else if let Some(n) = clause.dictionary() {
            return Ok(Self::dictionaries_from_context(Self::get_one_or_many_names_context(
                n,
                alias,
                clause.capability_type(Some(origin.clone())).unwrap(),
            )?));
        } else if let Some(n) = clause.config() {
            return Ok(Self::configurations_from_context(Self::get_one_or_many_names_context(
                n,
                alias,
                clause.capability_type(Some(origin.clone())).unwrap(),
            )?));
        }

        // Unsupported capability type.
        let supported_keywords =
            clause.supported().iter().map(|k| format!("\"{}\"", k)).collect::<Vec<_>>().join(", ");
        Err(Error::validate_context(
            format!(
                "`{}` declaration is missing a capability keyword, one of: {}",
                clause.decl_type(),
                supported_keywords,
            ),
            Some(origin.clone()),
        ))
    }

    /// Given a ContextUse clause, return the set of target identifiers.
    ///
    /// When only one capability identifier is specified, the target identifier name is derived
    /// using the "path" clause. If a "path" clause is not specified, the target identifier is the
    /// same name as the source.
    ///
    /// When multiple capability identifiers are specified, the target names are the same as the
    /// source names.
    pub fn from_context_use(
        use_input: &'a ContextSpanned<ContextUse>,
    ) -> Result<Vec<(Self, Arc<std::path::Path>)>, Error> {
        let use_ = &use_input.value;
        let origin = &use_input.origin;

        let alias = use_.path.as_ref();

        if let Some(n) = option_one_or_many_as_ref_context(&use_.service) {
            return Ok(Self::used_services_from_context(Self::get_one_or_many_svc_paths_context(
                n,
                alias,
                use_input.capability_type(Some(origin.clone())).unwrap(),
            )?));
        } else if let Some(n) = option_one_or_many_as_ref_context(&use_.protocol) {
            if let Some(numbered_handle) = &use_.numbered_handle {
                return Ok(n
                    .value
                    .iter()
                    .map(|_| {
                        (
                            CapabilityId::UsedProtocolNumberedHandle(numbered_handle.value),
                            n.origin.clone(),
                        )
                    })
                    .collect());
            }

            return Ok(Self::used_protocols_from_context(Self::get_one_or_many_svc_paths_context(
                n,
                alias,
                use_input.capability_type(Some(origin.clone())).unwrap(),
            )?));
        } else if let Some(_) = &use_.directory {
            if use_.path.is_none() {
                return Err(Error::validate_context(
                    "\"path\" should be present for `use directory`.",
                    Some(origin.clone()),
                ));
            }
            return Ok(vec![(
                CapabilityId::UsedDirectory(use_.path.as_ref().unwrap().value.clone()),
                origin.clone(),
            )]);
        } else if let Some(_) = &use_.storage {
            if use_.path.is_none() {
                return Err(Error::validate_context(
                    "\"path\" should be present for `use storage`.",
                    Some(origin.clone()),
                ));
            }
            return Ok(vec![(
                CapabilityId::UsedStorage(use_.path.as_ref().unwrap().value.clone()),
                origin.clone(),
            )]);
        } else if let Some(_) = &use_.event_stream {
            if let Some(path) = &use_.path {
                return Ok(vec![(
                    CapabilityId::UsedEventStream(path.value.clone()),
                    origin.clone(),
                )]);
            }
            return Ok(vec![(
                CapabilityId::UsedEventStream(Path::new("/svc/fuchsia.component.EventStream")?),
                origin.clone(),
            )]);
        } else if let Some(n) = &use_.runner {
            return Ok(vec![(CapabilityId::UsedRunner(&n.value), n.origin.clone())]);
        } else if let Some(_) = &use_.config {
            return match &use_.key {
                None => Err(Error::validate_context(
                    "\"key\" should be present for `use config`.",
                    Some(origin.clone()),
                )),
                Some(name) => {
                    Ok(vec![(CapabilityId::UsedConfiguration(&name.value), origin.clone())])
                }
            };
        } else if let Some(n) = option_one_or_many_as_ref_context(&use_.dictionary) {
            return Ok(Self::used_dictionaries_from_context(
                Self::get_one_or_many_svc_paths_context(
                    n,
                    alias,
                    use_input.capability_type(Some(origin.clone())).unwrap(),
                )?,
            ));
        }

        // Unsupported capability type.
        let supported_keywords = use_input
            .supported()
            .iter()
            .map(|k| format!("\"{}\"", k))
            .collect::<Vec<_>>()
            .join(", ");

        Err(Error::validate_context(
            format!(
                "`{}` declaration is missing a capability keyword, one of: {}",
                use_input.decl_type(),
                supported_keywords,
            ),
            Some(origin.clone()),
        ))
    }

    /// Returns the target names as a `Vec` from a declaration with `names` and `alias` as a `Vec`.
    fn get_one_or_many_names_context<'b>(
        name_wrapper: ContextSpanned<OneOrMany<&'b BorrowedName>>,
        alias: Option<ContextSpanned<&'b BorrowedName>>,
        capability_type: &str,
    ) -> Result<Vec<ContextSpanned<&'b BorrowedName>>, Error> {
        let names_origin = name_wrapper.origin;
        let names_vec: Vec<&'b BorrowedName> = name_wrapper.value.into_iter().collect();
        let num_names = names_vec.len();

        if num_names > 1 && alias.is_some() {
            return Err(Error::validate_contexts(
                format!("\"as\" can only be specified when one `{}` is supplied.", capability_type),
                vec![alias.map(|s| s.origin).unwrap_or(names_origin)],
            ));
        }

        if num_names == 1 {
            let final_name_span = alias_or_name_context(alias, names_vec[0], names_origin);
            return Ok(vec![final_name_span]);
        }

        let final_names = names_vec
            .into_iter()
            .map(|name| ContextSpanned { value: name, origin: names_origin.clone() })
            .collect();

        Ok(final_names)
    }

    fn get_one_or_many_svc_paths_context(
        names: ContextSpanned<OneOrMany<&BorrowedName>>,
        alias: Option<&ContextSpanned<Path>>,
        capability_type: &str,
    ) -> Result<Vec<ContextSpanned<Path>>, Error> {
        let names_origin = &names.origin;
        let names_vec: Vec<_> = names.value.into_iter().collect();

        match (names_vec.len(), alias) {
            (_, None) => {
                let generated_paths = names_vec
                    .into_iter()
                    .map(|n| {
                        let new_path: Path = format!("/svc/{}", n).parse().unwrap();
                        ContextSpanned { value: new_path, origin: names_origin.clone() }
                    })
                    .collect();
                Ok(generated_paths)
            }

            (1, Some(spanned_alias)) => Ok(vec![spanned_alias.clone()]),

            (_, Some(spanned_alias)) => Err(Error::validate_contexts(
                format!(
                    "\"path\" can only be specified when one `{}` is supplied.",
                    capability_type,
                ),
                vec![spanned_alias.origin.clone()],
            )),
        }
    }

    capability_ids_from_context_names!(services_from_context, CapabilityId::Service);
    capability_ids_from_context_names!(protocols_from_context, CapabilityId::Protocol);
    capability_ids_from_context_names!(directories_from_context, CapabilityId::Directory);
    capability_ids_from_context_names!(storages_from_context, CapabilityId::Storage);
    capability_ids_from_context_names!(runners_from_context, CapabilityId::Runner);
    capability_ids_from_context_names!(resolvers_from_context, CapabilityId::Resolver);
    capability_ids_from_context_names!(event_streams_from_context, CapabilityId::EventStream);
    capability_ids_from_context_names!(dictionaries_from_context, CapabilityId::Dictionary);
    capability_ids_from_context_names!(configurations_from_context, CapabilityId::Configuration);

    capability_ids_from_context_paths!(used_services_from_context, CapabilityId::UsedService);
    capability_ids_from_context_paths!(used_protocols_from_context, CapabilityId::UsedProtocol);
    capability_ids_from_context_paths!(
        used_dictionaries_from_context,
        CapabilityId::UsedDictionary
    );
}

impl fmt::Display for CapabilityId<'_> {
    /// Return the string ID of this clause.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapabilityId::Service(n)
            | CapabilityId::Storage(n)
            | CapabilityId::Runner(n)
            | CapabilityId::UsedRunner(n)
            | CapabilityId::Resolver(n)
            | CapabilityId::EventStream(n)
            | CapabilityId::Configuration(n)
            | CapabilityId::UsedConfiguration(n)
            | CapabilityId::Dictionary(n) => write!(f, "{}", n),
            CapabilityId::UsedService(p)
            | CapabilityId::UsedProtocol(p)
            | CapabilityId::UsedDirectory(p)
            | CapabilityId::UsedStorage(p)
            | CapabilityId::UsedEventStream(p)
            | CapabilityId::UsedDictionary(p) => write!(f, "{}", p),
            CapabilityId::UsedProtocolNumberedHandle(p) => write!(f, "{}", p),
            CapabilityId::Protocol(p) | CapabilityId::Directory(p) => write!(f, "{}", p),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::offer::ContextOffer;
    use assert_matches::assert_matches;
    use std::sync::Arc;

    #[test]
    fn test_offer_service() -> Result<(), Error> {
        let a: Name = "a".parse().unwrap();
        let b: Name = "b".parse().unwrap();

        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_eq!(
            CapabilityId::from_context_offer_expose(&ContextSpanned {
                value: ContextOffer {
                    service: Some(ContextSpanned {
                        value: OneOrMany::One(a.clone()),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextOffer::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::Service(&a), synthetic_origin.clone())]
        );

        assert_eq!(
            CapabilityId::from_context_offer_expose(&ContextSpanned {
                value: ContextOffer {
                    service: Some(ContextSpanned {
                        value: OneOrMany::Many(vec![a.clone(), b.clone()]),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextOffer::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![
                (CapabilityId::Service(&a), synthetic_origin.clone()),
                (CapabilityId::Service(&b), synthetic_origin.clone())
            ]
        );

        // "as" aliasing.
        assert_eq!(
            CapabilityId::from_context_offer_expose(&ContextSpanned {
                value: ContextOffer {
                    service: Some(ContextSpanned {
                        value: OneOrMany::One(a.clone()),
                        origin: synthetic_origin.clone(),
                    }),
                    r#as: Some(ContextSpanned {
                        value: b.clone(),
                        origin: synthetic_origin.clone()
                    }),
                    ..ContextOffer::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::Service(&b), synthetic_origin)]
        );

        Ok(())
    }

    #[test]
    fn test_use_service() -> Result<(), Error> {
        let a: Name = "a".parse().unwrap();
        let b: Name = "b".parse().unwrap();

        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    service: Some(ContextSpanned {
                        value: OneOrMany::One(a.clone()),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::UsedService("/svc/a".parse().unwrap()), synthetic_origin.clone())]
        );

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    service: Some(ContextSpanned {
                        value: OneOrMany::Many(vec![a.clone(), b.clone(),]),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![
                (CapabilityId::UsedService("/svc/a".parse().unwrap()), synthetic_origin.clone()),
                (CapabilityId::UsedService("/svc/b".parse().unwrap()), synthetic_origin.clone())
            ]
        );

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    service: Some(ContextSpanned {
                        value: OneOrMany::One(a.clone()),
                        origin: synthetic_origin.clone(),
                    }),
                    path: Some(ContextSpanned {
                        value: "/b".parse().unwrap(),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::UsedService("/b".parse().unwrap()), synthetic_origin.clone())]
        );

        Ok(())
    }

    #[test]
    fn test_use_event_stream() -> Result<(), Error> {
        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    event_stream: Some(ContextSpanned {
                        value: OneOrMany::One(Name::new("test".to_string()).unwrap()),
                        origin: synthetic_origin.clone(),
                    }),
                    path: Some(ContextSpanned {
                        value: cm_types::Path::new("/svc/myevent".to_string()).unwrap(),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(
                CapabilityId::UsedEventStream("/svc/myevent".parse().unwrap()),
                synthetic_origin.clone()
            )]
        );

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    event_stream: Some(ContextSpanned {
                        value: OneOrMany::One(Name::new("test".to_string()).unwrap()),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(
                CapabilityId::UsedEventStream(
                    "/svc/fuchsia.component.EventStream".parse().unwrap()
                ),
                synthetic_origin.clone()
            )]
        );

        Ok(())
    }

    #[test]
    fn test_offer_protocol() -> Result<(), Error> {
        let a: Name = "a".parse().unwrap();
        let b: Name = "b".parse().unwrap();

        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_eq!(
            CapabilityId::from_context_offer_expose(&ContextSpanned {
                value: ContextOffer {
                    protocol: Some(ContextSpanned {
                        value: OneOrMany::One(a.clone()),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextOffer::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::Protocol(&a), synthetic_origin.clone())]
        );

        assert_eq!(
            CapabilityId::from_context_offer_expose(&ContextSpanned {
                value: ContextOffer {
                    protocol: Some(ContextSpanned {
                        value: OneOrMany::Many(vec![a.clone(), b.clone()]),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextOffer::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![
                (CapabilityId::Protocol(&a), synthetic_origin.clone()),
                (CapabilityId::Protocol(&b), synthetic_origin)
            ]
        );

        Ok(())
    }

    #[test]
    fn test_use_protocol() -> Result<(), Error> {
        let a: Name = "a".parse().unwrap();
        let b: Name = "b".parse().unwrap();

        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    protocol: Some(ContextSpanned {
                        value: OneOrMany::One(a.clone()),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::UsedProtocol("/svc/a".parse().unwrap()), synthetic_origin.clone())]
        );

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    protocol: Some(ContextSpanned {
                        value: OneOrMany::Many(vec![a.clone(), b.clone(),]),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![
                (CapabilityId::UsedProtocol("/svc/a".parse().unwrap()), synthetic_origin.clone()),
                (CapabilityId::UsedProtocol("/svc/b".parse().unwrap()), synthetic_origin.clone())
            ]
        );

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    protocol: Some(ContextSpanned {
                        value: OneOrMany::One(a.clone()),
                        origin: synthetic_origin.clone(),
                    }),
                    path: Some(ContextSpanned {
                        value: "/b".parse().unwrap(),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::UsedProtocol("/b".parse().unwrap()), synthetic_origin.clone())]
        );

        Ok(())
    }

    #[test]
    fn test_offer_directory() -> Result<(), Error> {
        let a: Name = "a".parse().unwrap();
        let b: Name = "b".parse().unwrap();

        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_eq!(
            CapabilityId::from_context_offer_expose(&ContextSpanned {
                value: ContextOffer {
                    directory: Some(ContextSpanned {
                        value: OneOrMany::One(a.clone()),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextOffer::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::Directory(&a), synthetic_origin.clone())]
        );

        assert_eq!(
            CapabilityId::from_context_offer_expose(&ContextSpanned {
                value: ContextOffer {
                    directory: Some(ContextSpanned {
                        value: OneOrMany::Many(vec![a.clone(), b.clone()]),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextOffer::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![
                (CapabilityId::Directory(&a), synthetic_origin.clone()),
                (CapabilityId::Directory(&b), synthetic_origin.clone())
            ]
        );

        Ok(())
    }

    #[test]
    fn test_use_directory() -> Result<(), Error> {
        let a: Name = "a".parse().unwrap();

        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    directory: Some(ContextSpanned {
                        value: a.clone(),
                        origin: synthetic_origin.clone(),
                    }),
                    path: Some(ContextSpanned {
                        value: "/b".parse().unwrap(),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::UsedDirectory("/b".parse().unwrap()), synthetic_origin.clone())]
        );

        Ok(())
    }

    #[test]
    fn test_offer_storage() -> Result<(), Error> {
        let a: Name = "a".parse().unwrap();
        let b: Name = "b".parse().unwrap();

        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_eq!(
            CapabilityId::from_context_offer_expose(&ContextSpanned {
                value: ContextOffer {
                    storage: Some(ContextSpanned {
                        value: OneOrMany::One(a.clone()),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextOffer::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::Storage(&a), synthetic_origin.clone())]
        );

        assert_eq!(
            CapabilityId::from_context_offer_expose(&ContextSpanned {
                value: ContextOffer {
                    storage: Some(ContextSpanned {
                        value: OneOrMany::Many(vec![a.clone(), b.clone()]),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextOffer::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![
                (CapabilityId::Storage(&a), synthetic_origin.clone()),
                (CapabilityId::Storage(&b), synthetic_origin.clone())
            ]
        );

        Ok(())
    }

    #[test]
    fn test_use_storage() -> Result<(), Error> {
        let a: Name = "a".parse().unwrap();

        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    storage: Some(ContextSpanned {
                        value: a.clone(),
                        origin: synthetic_origin.clone(),
                    }),
                    path: Some(ContextSpanned {
                        value: "/b".parse().unwrap(),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::UsedStorage("/b".parse().unwrap()), synthetic_origin.clone())]
        );

        Ok(())
    }

    #[test]
    fn test_use_runner() -> Result<(), Error> {
        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    runner: Some(ContextSpanned {
                        value: "elf".parse().unwrap(),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(
                CapabilityId::UsedRunner(BorrowedName::new("elf").unwrap()),
                synthetic_origin.clone()
            )]
        );

        Ok(())
    }

    #[test]
    fn test_offer_dictionary() -> Result<(), Error> {
        let a: Name = "a".parse().unwrap();
        let b: Name = "b".parse().unwrap();

        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_eq!(
            CapabilityId::from_context_offer_expose(&ContextSpanned {
                value: ContextOffer {
                    dictionary: Some(ContextSpanned {
                        value: OneOrMany::One(a.clone()),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextOffer::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::Dictionary(&a), synthetic_origin.clone())]
        );

        assert_eq!(
            CapabilityId::from_context_offer_expose(&ContextSpanned {
                value: ContextOffer {
                    dictionary: Some(ContextSpanned {
                        value: OneOrMany::Many(vec![a.clone(), b.clone()]),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextOffer::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![
                (CapabilityId::Dictionary(&a), synthetic_origin.clone()),
                (CapabilityId::Dictionary(&b), synthetic_origin.clone())
            ]
        );

        Ok(())
    }

    #[test]
    fn test_use_dictionary() -> Result<(), Error> {
        let a: Name = "a".parse().unwrap();
        let b: Name = "b".parse().unwrap();

        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    dictionary: Some(ContextSpanned {
                        value: OneOrMany::One(a.clone()),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(
                CapabilityId::UsedDictionary("/svc/a".parse().unwrap()),
                synthetic_origin.clone()
            )]
        );

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    dictionary: Some(ContextSpanned {
                        value: OneOrMany::Many(vec![a.clone(), b.clone()]),
                        origin: synthetic_origin.clone(),
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![
                (CapabilityId::UsedDictionary("/svc/a".parse().unwrap()), synthetic_origin.clone()),
                (CapabilityId::UsedDictionary("/svc/b".parse().unwrap()), synthetic_origin.clone())
            ]
        );

        assert_eq!(
            CapabilityId::from_context_use(&ContextSpanned {
                value: ContextUse {
                    dictionary: Some(ContextSpanned {
                        value: OneOrMany::One(a.clone()),
                        origin: synthetic_origin.clone(),
                    }),
                    path: Some(ContextSpanned {
                        value: "/b".parse().unwrap(),
                        origin: synthetic_origin.clone()
                    }),
                    ..ContextUse::default()
                },
                origin: synthetic_origin.clone(),
            })?,
            vec![(CapabilityId::UsedDictionary("/b".parse().unwrap()), synthetic_origin.clone())]
        );

        Ok(())
    }

    #[test]
    fn test_errors() -> Result<(), Error> {
        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        assert_matches!(
            CapabilityId::from_context_offer_expose(&ContextSpanned {
                value: ContextOffer::default(),
                origin: synthetic_origin
            }),
            Err(_)
        );

        Ok(())
    }
}
