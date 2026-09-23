// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::new_policy::traits::HasPolicyId;
use crate::policy::{Policy, TypeId};
use crate::{KernelClass, ObjectClass};

use anyhow::{anyhow, bail};
use std::collections::HashMap;
use std::num::NonZeroU32;
use strum::VariantArray as _;

/// Encapsulates a set of access-check exceptions parsed from a supplied configuration.
pub(super) struct ExceptionsConfig {
    todo_deny_entries: HashMap<ExceptionsEntry, NonZeroU32>,
    permissive_entries: HashMap<TypeId, NonZeroU32>,
}

impl ExceptionsConfig {
    /// Parses the supplied `exceptions` lines and returns an `ExceptionsConfig` with an entry for
    /// each parsed exception definition. If a definition's source or target type/domain are not
    /// defined by the supplied `policy` then the entry is ignored, so that removal/renaming of
    /// policy elements will not break the exceptions configuration.
    pub(super) fn new(policy: &Policy, exceptions: &[&str]) -> Result<Self, anyhow::Error> {
        let mut result = Self {
            todo_deny_entries: HashMap::with_capacity(exceptions.len()),
            permissive_entries: HashMap::new(),
        };
        for line in exceptions {
            result.parse_config_line(policy, line)?;
        }
        result.todo_deny_entries.shrink_to_fit();
        Ok(result)
    }

    /// Returns the non-zero integer bug Id for the exception associated with the specified source,
    /// target and class, if any.
    pub(super) fn lookup(
        &self,
        source: TypeId,
        target: TypeId,
        class: ObjectClass,
    ) -> Option<NonZeroU32> {
        self.todo_deny_entries
            .get(&ExceptionsEntry { source, target, class })
            .or_else(|| self.permissive_entries.get(&source))
            .copied()
    }

    fn parse_config_line(&mut self, policy: &Policy, line: &str) -> Result<(), anyhow::Error> {
        let mut parts = line.trim().split_whitespace();
        if let Some(statement) = parts.next() {
            match statement {
                "todo_deny" => {
                    // "todo_deny" lines have the form:
                    //   todo_deny b/<id> <source> <target> <class>

                    // Parse the bug Id, which must be present
                    let bug_id = bug_ref_to_id(
                        parts.next().ok_or_else(|| anyhow!("Expected bug identifier"))?,
                    )?;

                    // Parse the source & target types. If either of these is not defined by the
                    // `policy` then the statement is ignored.
                    let stype = policy
                        .types()
                        .get_by_name(
                            parts.next().ok_or_else(|| anyhow!("Expected source type"))?.as_bytes(),
                        )
                        .map(|t| t.id());
                    let ttype = policy
                        .types()
                        .get_by_name(
                            parts.next().ok_or_else(|| anyhow!("Expected target type"))?.as_bytes(),
                        )
                        .map(|t| t.id());

                    let class_name = parts.next().ok_or_else(|| anyhow!("Target class missing"))?;

                    // Parse the object class name to the corresponding policy-specific Id.
                    // This allows non-kernel classes, and userspace queries against kernel classes,
                    // to have exceptions applied to them.
                    let policy_class =
                        policy.classes().get_by_name(class_name.as_bytes()).map(|x| x.id());

                    // Parse the kernel object class. This must correspond to a known kernel object
                    // class, regardless of whether the policy actually defines the class.
                    let kernel_class = object_class_by_name(class_name);

                    // If the class isn't defined by policy, or used by the kernel, then there is
                    // no way to apply the exception.
                    if policy_class.is_none() && kernel_class.is_none() {
                        println!("Ignoring statement: {} (unknown class)", line);
                        return Ok(());
                    }

                    // If the source or target domains are unrecognized then there is no way to
                    // apply the exception.
                    let (Some(source), Some(target)) = (stype, ttype) else {
                        println!("Ignoring statement: {} (unknown source or target)", line);
                        return Ok(());
                    };

                    if let Some(policy_class) = policy_class {
                        self.todo_deny_entries.insert(
                            ExceptionsEntry { source, target, class: policy_class.into() },
                            bug_id,
                        );
                    }
                    if let Some(kernel_class) = kernel_class {
                        self.todo_deny_entries.insert(
                            ExceptionsEntry { source, target, class: kernel_class.into() },
                            bug_id,
                        );
                    }
                }
                "todo_permissive" => {
                    // "todo_permissive" lines have the form:
                    //   todo_permissive b/<id> <source>

                    // Parse the bug Id, which must be present
                    let bug_id = bug_ref_to_id(
                        parts.next().ok_or_else(|| anyhow!("Expected bug identifier"))?,
                    )?;

                    // Parse the source type. The statement is ignored if the type is not defined by policy.
                    let stype = policy
                        .types()
                        .get_by_name(
                            parts.next().ok_or_else(|| anyhow!("Expected source type"))?.as_bytes(),
                        )
                        .map(|t| t.id());

                    if let Some(source) = stype {
                        self.permissive_entries.insert(source, bug_id);
                    } else {
                        println!("Ignoring statement: {}", line);
                    }
                }
                _ => bail!("Unknown statement {}", statement),
            }
        }
        Ok(())
    }
}

/// Key used to index the access check exceptions table.
#[derive(Eq, Hash, PartialEq)]
struct ExceptionsEntry {
    source: TypeId,
    target: TypeId,
    class: ObjectClass,
}

/// Returns the numeric bug Id parsed from a bug URL reference.
fn bug_ref_to_id(bug_ref: &str) -> Result<NonZeroU32, anyhow::Error> {
    let bug_id_part = bug_ref
        .strip_prefix("b/")
        .or_else(|| bug_ref.strip_prefix("https://fxbug.dev/"))
        .ok_or_else(|| {
            anyhow!("Expected bug Identifier of the form b/<id> or https://fxbug.dev/<id>")
        })?;
    bug_id_part.parse::<NonZeroU32>().map_err(|_| anyhow!("Malformed bug Id: {}", bug_id_part))
}

/// Returns the `KernelClass` corresponding to the supplied `name`, if any.
/// `None` is returned if no such kernel object class exists in the Starnix implementation.
fn object_class_by_name(name: &str) -> Option<KernelClass> {
    KernelClass::VARIANTS.iter().find(|class| class.name() == name).map(Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::parse_policy_by_value;
    use std::sync::Arc;

    const TEST_POLICY: &[u8] =
        include_bytes!("../testdata/composite_policies/compiled/exceptions_config_policy");

    const EXCEPTION_SOURCE_TYPE: &[u8] = b"test_exception_source_t";
    const EXCEPTION_TARGET_TYPE: &[u8] = b"test_exception_target_t";
    const _EXCEPTION_OTHER_TYPE: &[u8] = b"test_exception_other_t";
    const UNMATCHED_TYPE: &[u8] = b"test_exception_unmatched_t";

    const NON_KERNEL_CLASS: &str = "test_exception_non_kernel_class";

    const TEST_CONFIG: &[&str] = &[
        // These statements should resolve into both kernel-Id and policy-Id indexed entries.
        "todo_deny b/001 test_exception_source_t test_exception_target_t file",
        "todo_deny b/002 test_exception_other_t test_exception_target_t chr_file",
        // This statement should resolve into a kernel-Id indexed entry, because neither the "base"
        // policy fragment, nor the exceptions test fragment, define the `anon_inode` class.
        "todo_deny b/003 test_exception_source_t test_exception_other_t anon_inode",
        // This statement should resolve into a policy-Id indexed entry, because the class is not
        // one known to the kernel.
        "todo_deny b/004 test_exception_source_t test_exception_target_t test_exception_non_kernel_class",
        // These statements should not be resolved.
        "todo_deny b/101 test_undefined_source_t test_exception_target_t file",
        "todo_deny b/102 test_exception_source_t test_undefined_target_t file",
        "todo_deny b/103 test_exception_source_t test_exception_target_t test_exception_non_existent_class",
    ];

    struct TestData {
        policy: Arc<Policy>,
        defined_source: TypeId,
        defined_target: TypeId,
        unmatched_type: TypeId,
    }

    impl TestData {
        fn expect_policy_class(&self, name: &str) -> ObjectClass {
            self.policy
                .classes()
                .get_by_name(name.as_bytes())
                .map(|x| x.id())
                .expect("Unable to resolve policy class Id")
                .into()
        }
    }
    fn test_data() -> TestData {
        let parsed = parse_policy_by_value(TEST_POLICY.to_vec()).unwrap();
        let policy = Arc::new(parsed.validate().unwrap());
        let defined_source = policy.types().get_by_name(EXCEPTION_SOURCE_TYPE).unwrap().id();
        let defined_target = policy.types().get_by_name(EXCEPTION_TARGET_TYPE).unwrap().id();
        let unmatched_type = policy.types().get_by_name(UNMATCHED_TYPE).unwrap().id();

        assert!(policy.types().get_by_name(b"test_undefined_source_t").is_none());
        assert!(policy.types().get_by_name(b"test_undefined_target_t").is_none());

        TestData { policy, defined_source, defined_target, unmatched_type }
    }

    #[test]
    fn empty_config_is_valid() {
        let _ = ExceptionsConfig::new(&test_data().policy, &[])
            .expect("Empty exceptions config is valid");
    }

    #[test]
    fn extra_separating_whitespace_is_valid() {
        let _ = ExceptionsConfig::new(
            &test_data().policy,
            &["
            todo_deny b/001\ttest_exception_source_t     test_exception_target_t   file
    "],
        )
        .expect("Config with extra separating whitespace is valid");
    }

    #[test]
    fn only_defined_types_resolve_to_lookup_entries() {
        let test_data = test_data();

        let config = ExceptionsConfig::new(&test_data.policy, TEST_CONFIG)
            .expect("Config with unresolved types is valid");

        assert_eq!(config.todo_deny_entries.len(), 6);
    }

    #[test]
    fn lookup_matching() {
        let test_data = test_data();

        let config = ExceptionsConfig::new(&test_data.policy, TEST_CONFIG)
            .expect("Config with unresolved types is valid");

        // Matching source, target & kernel class will resolve to the corresponding bug Id.
        assert_eq!(
            config.lookup(
                test_data.defined_source,
                test_data.defined_target,
                KernelClass::File.into()
            ),
            Some(NonZeroU32::new(1).unwrap())
        );

        // Matching source, target and kernel class identified via policy-defined Id will resolve to
        // the same bug Id as if looked up via the kernel enum.
        assert_eq!(
            config.lookup(
                test_data.defined_source,
                test_data.defined_target,
                test_data.expect_policy_class("file")
            ),
            Some(NonZeroU32::new(1).unwrap())
        );

        // Matching source, target and non-kernel class will resolve.
        assert_eq!(
            config.lookup(
                test_data.defined_source,
                test_data.defined_target,
                test_data.expect_policy_class(NON_KERNEL_CLASS),
            ),
            Some(NonZeroU32::new(4).unwrap())
        );

        // Mismatched class, source or target returns no Id.
        assert_eq!(
            config.lookup(
                test_data.defined_source,
                test_data.defined_target,
                KernelClass::Dir.into()
            ),
            None
        );
        assert_eq!(
            config.lookup(
                test_data.unmatched_type,
                test_data.defined_target,
                KernelClass::File.into()
            ),
            None
        );
        assert_eq!(
            config.lookup(
                test_data.defined_source,
                test_data.unmatched_type,
                KernelClass::File.into()
            ),
            None
        );
    }
}
