// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ir::Declaration;

use crate::compare::problems::CompatibilityProblems;
use crate::compare::{AbiSurface, Protocol, Type};
use crate::convert::{Context, ConvertType};
use crate::ir::IR;
use crate::Version;
use std::collections::HashMap;
use std::rc::Rc;

// Test library name
pub const LIBRARY: &str = "fuchsia.compat.test";

struct TestLibrary {
    source: String,
}

impl TestLibrary {
    fn new_added_at(added: &str, source_fragment: &str) -> Self {
        Self {
            source: format!(
                "
        @available(added={added})
        library {LIBRARY};

        {source_fragment}
        "
            ),
        }
    }
    fn new(source_fragment: &str) -> Self {
        Self::new_added_at("1", source_fragment)
    }

    fn member_name(&self, name: &str) -> String {
        format!("{LIBRARY}/{name}")
    }

    fn compile<'a>(&self, version: &Version) -> CompiledTestLibrary<'a> {
        CompiledTestLibrary::new(version, self)
    }
}

struct CompiledTestLibrary<'a> {
    version: Version,
    source: String,
    ir: Rc<IR>,
    context: Context<'a>,
}

impl CompiledTestLibrary<'_> {
    fn new(version: &Version, library: &TestLibrary) -> Self {
        let ir =
            IR::from_source(version.api_level(), &library.source, LIBRARY).unwrap_or_else(|_| {
                panic!("Compiling {0:?}:\n{1}", version.api_level(), library.source)
            });
        let context = Context::new(ir.clone(), &version);
        Self { version: version.clone(), source: library.source.clone(), ir, context }
    }

    fn api_level(&self) -> &str {
        self.version.api_level()
    }

    fn get_decl(&self, name: &str) -> &Declaration {
        self.ir.get(name).unwrap_or_else(|_| {
            panic!(
                "Couldn't find declaration {:?} at {:?} in:\n{}",
                name,
                self.api_level(),
                self.source,
            )
        })
    }

    fn get_type(&self, name: &str) -> Type {
        let decl = self.get_decl(name);
        let context = self.context.nest_member(name, decl.identifier());
        use crate::convert::ConvertType;
        decl.convert(&context).unwrap_or_else(|_| {
            panic!(
                "Couldn't convert {name} to a type at {:?} in:\n{}",
                self.api_level(),
                self.source,
            )
        })
    }

    fn get_protocol(&self, name: &str) -> Protocol {
        let decl = self.get_decl(name);
        let context = self.context.nest_member(name, decl.identifier());

        if let Declaration::Protocol(protocol) = decl {
            crate::convert::convert_protocol(&protocol, &context).unwrap_or_else(|_| {
                panic!(
                    "Couldn't convert {name} to a protocol at {:?} in:\n{}",
                    self.api_level(),
                    self.source,
                )
            })
        } else {
            panic!("{name} is not a protocol at {:?} in:\n{}", self.api_level(), self.source)
        }
    }

    fn get_abi_surface(&self) -> AbiSurface {
        crate::convert::convert_abi_surface(self.ir.clone()).unwrap_or_else(|_| {
            panic!("Converting platform at {:?}:\n{}", self.api_level(), self.source)
        })
    }
}

#[allow(unused)]
pub(super) fn compare_fidl_library(
    versions: [&'static str; 2],
    source_fragment: impl AsRef<str>,
) -> CompatibilityProblems {
    let lib = TestLibrary::new(source_fragment.as_ref());

    let compiled = versions.map(|v| lib.compile(&Version::new(v)));
    let surfaces = compiled.map(|c| c.get_abi_surface());
    super::compatible(surfaces.each_ref(), &Default::default()).unwrap_or_else(|_| {
        panic!("Comparing {:?} and {:?}:\n{}", versions[0], versions[1], lib.source)
    })
}

#[derive(Clone)]
pub struct TypeVersions {
    pub send: Version,
    pub recv: Version,
}

pub(super) fn compare_fidl_type_between(
    name: &str,
    versions: &[TypeVersions],
    source_fragment: &str,
) -> CompatibilityProblems {
    let lib = TestLibrary::new(source_fragment);
    let name = lib.member_name(name);
    let mut problems = CompatibilityProblems::default();

    let mut type_versions: HashMap<Version, Type> = HashMap::new();

    for v in versions {
        if !type_versions.contains_key(&v.send) {
            type_versions.insert(v.send.clone(), lib.compile(&v.send).get_type(&name));
        }
        if !type_versions.contains_key(&v.recv) {
            type_versions.insert(v.recv.clone(), lib.compile(&v.recv).get_type(&name));
        }

        problems.append(super::compare_types(
            type_versions.get(&v.send).unwrap(),
            type_versions.get(&v.recv).unwrap(),
            &Default::default(),
        ));
    }

    problems
}

pub(super) fn compare_fidl_type(name: &str, source_fragment: &str) -> CompatibilityProblems {
    let v1: Version = Version::new("1");
    let v2: Version = Version::new("2");

    compare_fidl_type_between(
        name,
        &[TypeVersions { send: v1.clone(), recv: v2.clone() }, TypeVersions { send: v2, recv: v1 }],
        source_fragment,
    )
}

pub(super) fn compare_fidl_protocol(name: &str, source_fragment: &str) -> CompatibilityProblems {
    let lib = TestLibrary::new(source_fragment);
    let name = lib.member_name(name);

    let external = lib.compile(&Version::new("1")).get_protocol(&name);
    let platform = lib.compile(&Version::new("1,2,NEXT,HEAD")).get_protocol(&name);

    Protocol::compatible([&external, &platform], &Default::default())
}
