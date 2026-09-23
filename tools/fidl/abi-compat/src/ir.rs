// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module is concerned with reading the FIDL JSON IR. It supports a subset
//! of the IR that is relevant to version compatibility comparisons.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::io::BufReader;
use std::path::Path;
use std::rc::Rc;

#[derive(Deserialize, Debug, Clone)]
pub struct AttributeArgument {
    pub name: String,
    pub value: Constant,
}

impl AttributeArgument {
    #[cfg(test)]
    pub fn new(name: impl AsRef<str>, value: Constant) -> Self {
        Self { name: name.as_ref().to_string(), value }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct Attribute {
    pub name: String,
    pub arguments: Vec<AttributeArgument>,
}

impl Attribute {
    #[cfg(test)]
    pub fn new(name: impl AsRef<str>, arguments: Vec<AttributeArgument>) -> Self {
        Self { name: name.as_ref().to_string(), arguments }
    }
    pub fn get_argument(&self, name: impl AsRef<str>) -> Option<Constant> {
        let name = name.as_ref();
        if let Some(arg) = self.arguments.iter().filter(|a| a.name == name).next() {
            Some(arg.value.clone())
        } else {
            None
        }
    }
}

pub fn get_attribute(
    attributes: &Option<Vec<Attribute>>,
    name: impl AsRef<str>,
) -> Option<Attribute> {
    let name = name.as_ref();
    if let Some(attrs) = attributes {
        attrs.iter().filter(|a| a.name == name).next().cloned()
    } else {
        None
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Constant {
    Literal { value: String },
    Identifier { value: String },
}

impl Constant {
    pub fn value(&self) -> &str {
        match self {
            Constant::Literal { value } => value,
            Constant::Identifier { value } => value,
        }
    }
    pub fn integer_value(&self) -> Result<i128> {
        Ok(self
            .value()
            .parse()
            .with_context(|| format!("Parsing {:?} as integer constant.", self.value()))?)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EndpointRole {
    Client,
    Server,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind_v2", rename_all = "snake_case")]
pub enum Type {
    Array { element_count: u64, element_type: Box<Type> },
    StringArray { element_count: u64 },
    Vector { element_type: Box<Type>, maybe_element_count: Option<u64>, nullable: bool },
    String { maybe_element_count: Option<u64>, nullable: bool },
    Handle { nullable: bool, subtype: String, rights: u32 },
    Endpoint { role: EndpointRole, protocol: String, protocol_transport: String, nullable: bool },
    Identifier { identifier: String, nullable: bool },
    Internal { subtype: String },
    Primitive { subtype: String },
}

#[derive(Deserialize, Debug, Clone)]
pub struct BitsMember {
    pub value: Constant,
    pub name: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct BitsDeclaration {
    pub name: String,
    pub members: Vec<BitsMember>,
    pub strict: bool,
    pub r#type: Type,
}

#[derive(Deserialize, Debug, Clone)]
pub struct EnumMember {
    pub value: Constant,
    pub name: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct EnumDeclaration {
    pub name: String,
    pub members: Vec<EnumMember>,
    pub strict: bool,
    pub r#type: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ProtocolMethod {
    pub name: String,
    pub ordinal: u64,
    pub strict: bool,
    pub has_request: bool,
    pub has_response: bool,
    pub maybe_request_payload: Option<Type>,
    pub maybe_response_payload: Option<Type>,
}

impl ProtocolMethod {}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum Openness {
    Open,
    Ajar,
    Closed,
}
impl Display for Openness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Openness::Open => write!(f, "open"),
            Openness::Ajar => write!(f, "ajar"),
            Openness::Closed => write!(f, "closed"),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct ProtocolDeclaration {
    pub name: String,
    pub methods: Vec<ProtocolMethod>,
    pub openness: Openness,
    pub maybe_attributes: Option<Vec<Attribute>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TableMember {
    pub ordinal: u64,
    pub name: String,
    pub r#type: Type,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TableDeclaration {
    pub name: String,
    pub members: Vec<TableMember>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct StructMember {
    pub name: String,
    pub r#type: Type,
}

#[derive(Deserialize, Debug, Clone)]
pub struct StructDeclaration {
    pub name: String,
    pub members: Vec<StructMember>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct UnionMember {
    pub ordinal: u64,
    pub name: String,
    pub r#type: Type,
}

#[derive(Deserialize, Debug, Clone)]
pub struct UnionDeclaration {
    pub name: String,
    pub members: Vec<UnionMember>,
    pub strict: bool,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum DeclarationKind {
    Alias,
    Bits,
    Const,
    Enum,
    ExperimentalResource,
    NewType,
    Overlay,
    Protocol,
    Service,
    Struct,
    Table,
    Union,
}

#[derive(Debug)]
pub enum Declaration {
    Bits(BitsDeclaration),
    Enum(EnumDeclaration),
    Protocol(ProtocolDeclaration),
    Struct(StructDeclaration),
    Table(TableDeclaration),
    Union(UnionDeclaration),
}

#[derive(Deserialize, Default)]
struct InternalIR {
    #[cfg(test)]
    pub maybe_attributes: Option<Vec<Attribute>>,
    pub available: HashMap<String, Vec<String>>,
    bits_declarations: Vec<BitsDeclaration>,
    enum_declarations: Vec<EnumDeclaration>,
    pub protocol_declarations: Vec<ProtocolDeclaration>,
    struct_declarations: Vec<StructDeclaration>,
    table_declarations: Vec<TableDeclaration>,
    union_declarations: Vec<UnionDeclaration>,
    declarations: HashMap<String, DeclarationKind>,
}

pub struct IR {
    pub declarations: HashMap<String, Declaration>,
    pub available: HashMap<String, Vec<String>>,
}

impl IR {
    fn from_internal(internal: InternalIR) -> Rc<Self> {
        let mut declarations = HashMap::with_capacity(internal.declarations.len());
        declarations.extend(
            internal.bits_declarations.into_iter().map(|d| (d.name.clone(), Declaration::Bits(d))),
        );
        declarations.extend(
            internal.enum_declarations.into_iter().map(|d| (d.name.clone(), Declaration::Enum(d))),
        );
        declarations.extend(
            internal
                .protocol_declarations
                .into_iter()
                .map(|d| (d.name.clone(), Declaration::Protocol(d))),
        );
        declarations.extend(
            internal
                .struct_declarations
                .into_iter()
                .map(|d| (d.name.clone(), Declaration::Struct(d))),
        );
        declarations.extend(
            internal
                .table_declarations
                .into_iter()
                .map(|d| (d.name.clone(), Declaration::Table(d))),
        );
        declarations.extend(
            internal
                .union_declarations
                .into_iter()
                .map(|d| (d.name.clone(), Declaration::Union(d))),
        );

        Rc::new(Self { declarations, available: internal.available })
    }
    pub fn load(path: impl AsRef<Path>) -> Result<Rc<Self>> {
        let file = fs::File::open(path)?;
        let reader = BufReader::new(file);
        Ok(Self::from_internal(serde_json::from_reader(reader)?))
    }

    #[cfg(test)]
    pub fn from_source(
        fuchsia_available: &str,
        fidl_source: &str,
        library_name: &str,
    ) -> Result<Rc<Self>> {
        testing::ir_from_source(fuchsia_available, fidl_source, library_name)
    }

    pub fn get(&self, name: &str) -> Result<&Declaration> {
        self.declarations.get(name).ok_or_else(|| anyhow!("Declaration not found: {}", name))
    }

    #[cfg(test)]
    pub fn empty_for_tests() -> Rc<Self> {
        return Rc::new(Self { declarations: Default::default(), available: Default::default() });
    }
}

#[test]
fn test_load_simple_library() {
    use maplit::hashmap;
    let ir = IR::from_source(
        "1",
        "
@available(added=1)
library fuchsia.test.library;

type Foo = table {
    1: bar string;
    3: baz int32;
};

open protocol Example {
    flexible SetFoo(struct { foo Foo; });
    flexible GetFoo() -> (struct { foo Foo; }) error int32;
};
",
        "fuchsia.test.library",
    )
    .expect("loading simple test library");

    assert_eq!(hashmap! {"fuchsia".to_owned() => vec!["1".to_owned()]}, ir.available);

    assert_eq!(
        1,
        ir.declarations.iter().filter(|(_, d)| matches!(d, Declaration::Table(_))).count()
    );
}

#[cfg(test)]
mod testing {
    use super::*;
    use std::ffi::OsString;
    use std::path::Path;
    use std::process::Command;
    use std::rc::Rc;

    // A minimal zx library for tests.
    const ZX_SOURCE: &'static str = "
    library zx;

    type ObjType = enum : uint32 {
        NONE = 0;
        PROCESS = 1;
        THREAD = 2;
        VMO = 3;
        CHANNEL = 4;
        EVENT = 5;
        PORT = 6;
    };

    type Rights = bits : uint32 {
        DUPLICATE = 0x00000001;
        TRANSFER = 0x00000002;
        READ = 0x00000004;
        WRITE = 0x00000008;
        EXECUTE = 0x00000010;
    };

    resource_definition Handle : uint32 {
        properties {
            subtype ObjType;
            rights Rights;
        };
    };";

    fn path_to_fidlc() -> OsString {
        let argv0 = std::env::args().next().expect("Can't get argv[0]");
        let fidlc = Path::new(&argv0).parent().unwrap().join("fidlc");
        if !fidlc.is_file() {
            panic!("{:?} is not a file", fidlc);
        }
        fidlc.into()
    }
    pub fn ir_from_source(
        fuchsia_available: &str,
        fidl_source: &str,
        library_name: &str,
    ) -> Result<Rc<IR>> {
        let fidlc_path = path_to_fidlc();

        let dir = tempfile::tempdir().expect("Creating a temporary directory");
        let temp_path =
            |filename: &str| -> String { dir.path().join(filename).to_string_lossy().to_string() };
        let source_path = temp_path("test.fidl");
        let ir_path = temp_path("test.fidl.json");
        std::fs::write(&source_path, fidl_source).expect("Writing FIDL source for test");
        let mut args = vec![
            "--available".to_string(),
            format!("fuchsia:{}", fuchsia_available),
            "--json".to_string(),
            ir_path.clone(),
            "--name".to_string(),
            library_name.to_string(),
            "--versioned".to_string(),
            "fuchsia".to_string(),
        ];

        if fidl_source.contains("using zx;") {
            let zx_path = temp_path("zx.fidl");
            std::fs::write(&zx_path, ZX_SOURCE).expect("Writing zx source for test");
            args.append(&mut vec!["--files".to_string(), zx_path])
        }

        args.append(&mut vec!["--files".to_string(), source_path]);

        let status = Command::new(fidlc_path).args(args).status().expect("Failed to run fidlc");
        assert!(status.success());

        let mut ir: InternalIR = serde_json::from_reader(BufReader::new(fs::File::open(ir_path)?))
            .context("Parsing IR JSON")?;

        let library_added = super::get_attribute(&ir.maybe_attributes, "available")
            .expect("top-level library should have @available")
            .get_argument("added")
            .expect("top-level library @available should have added=");
        let added_argument = AttributeArgument::new("added", library_added);

        // Make protocol declarations inherit the added version from the library @available,
        // if they don't have one themselves.
        // This matches the behavior of the platform-ir Python script.
        for p in ir.protocol_declarations.iter_mut() {
            if p.maybe_attributes.is_none() {
                // If there are no attributes, just add @available(added=library_added)
                p.maybe_attributes =
                    Some(vec![Attribute::new("available", vec![added_argument.clone()])]);
            } else if get_attribute(&p.maybe_attributes, "available").is_none() {
                // if there are attributes but no @available, add one
                p.maybe_attributes
                    .as_mut()
                    .unwrap()
                    .push(Attribute::new("available", vec![added_argument.clone()]));
            } else {
                // find the @available and if it doesn't have added= add one
                for attribute in p.maybe_attributes.as_mut().unwrap().iter_mut() {
                    if attribute.name != "available" {
                        continue;
                    }
                    if attribute.arguments.iter().filter(|a| a.name == "added").next().is_none() {
                        // No added=
                        attribute.arguments.push(added_argument.clone())
                    }
                }
            }
        }

        Ok(IR::from_internal(ir))
    }
}
