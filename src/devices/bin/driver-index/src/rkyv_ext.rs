// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;

use fidl_fuchsia_driver_framework as fdf;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fidl::marker::SourceBreaking)]
#[rkyv(archived = ArchivedSourceBreaking)]
pub struct SourceBreakingDef;

impl From<SourceBreakingDef> for fidl::marker::SourceBreaking {
    fn from(_: SourceBreakingDef) -> Self {
        Self
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::BindRule)]
#[rkyv(archived = ArchivedBindRule)]
pub struct BindRuleDef {
    #[rkyv(with = NodePropertyKeyDef)]
    pub key: fdf::NodePropertyKey,
    #[rkyv(with = ConditionDef)]
    pub condition: fdf::Condition,
    #[rkyv(with = rkyv::with::Map<NodePropertyValueDef>)]
    pub values: Vec<fdf::NodePropertyValue>,
}

impl From<BindRuleDef> for fdf::BindRule {
    fn from(value: BindRuleDef) -> Self {
        Self { key: value.key, condition: value.condition, values: value.values }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::BindRule2)]
#[rkyv(archived = ArchivedBindRule2)]
pub struct BindRule2Def {
    pub key: String,
    #[rkyv(with = ConditionDef)]
    pub condition: fdf::Condition,
    #[rkyv(with = rkyv::with::Map<NodePropertyValueDef>)]
    pub values: Vec<fdf::NodePropertyValue>,
}

impl From<BindRule2Def> for fdf::BindRule2 {
    fn from(value: BindRule2Def) -> Self {
        Self { key: value.key, condition: value.condition, values: value.values }
    }
}

#[derive(Debug)]
pub struct BlobIdSizeError;

impl std::fmt::Display for BlobIdSizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BlobId byte representation is not 32 bytes long")
    }
}

impl std::error::Error for BlobIdSizeError {}

pub struct BlobIdDef;

impl rkyv::with::ArchiveWith<fidl_fuchsia_pkg_ext::BlobId> for BlobIdDef {
    type Archived = [u8; 32];
    type Resolver = [(); 32];

    fn resolve_with(
        field: &fidl_fuchsia_pkg_ext::BlobId,
        resolver: Self::Resolver,
        out: rkyv::Place<Self::Archived>,
    ) {
        use rkyv::Archive as _;

        field.as_bytes().as_array::<32>().unwrap().resolve(resolver, out)
    }
}

impl<S> rkyv::with::SerializeWith<fidl_fuchsia_pkg_ext::BlobId, S> for BlobIdDef
where
    S: rkyv::rancor::Fallible + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &fidl_fuchsia_pkg_ext::BlobId,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        use rkyv::Serialize as _;
        use rkyv::rancor::ResultExt as _;

        field.as_bytes().as_array::<32>().ok_or(BlobIdSizeError).into_error()?.serialize(serializer)
    }
}

impl<D> rkyv::with::DeserializeWith<[u8; 32], fidl_fuchsia_pkg_ext::BlobId, D> for BlobIdDef
where
    D: rkyv::rancor::Fallible + ?Sized,
{
    fn deserialize_with(
        field: &[u8; 32],
        _: &mut D,
    ) -> Result<fidl_fuchsia_pkg_ext::BlobId, D::Error> {
        Ok(fidl_fuchsia_pkg_ext::BlobId::from(*field))
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::CompositeDriverInfo)]
#[rkyv(archived = ArchivedCompositeDriverInfo)]
pub struct CompositeDriverInfoDef {
    pub composite_name: Option<String>,
    #[rkyv(with = rkyv::with::Map<DriverInfoDef>)]
    pub driver_info: Option<fdf::DriverInfo>,
    #[rkyv(with = SourceBreakingDef)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

impl From<CompositeDriverInfoDef> for fdf::CompositeDriverInfo {
    fn from(value: CompositeDriverInfoDef) -> Self {
        Self {
            composite_name: value.composite_name,
            driver_info: value.driver_info,
            __source_breaking: value.__source_breaking,
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::CompositeDriverMatch)]
#[rkyv(archived = ArchivedCompositeDriverMatch)]
pub struct CompositeDriverMatchDef {
    #[rkyv(with = rkyv::with::Map<CompositeDriverInfoDef>)]
    pub composite_driver: Option<fdf::CompositeDriverInfo>,
    pub parent_names: Option<Vec<String>>,
    pub primary_parent_index: Option<u32>,
    #[rkyv(with = SourceBreakingDef)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

impl From<CompositeDriverMatchDef> for fdf::CompositeDriverMatch {
    fn from(value: CompositeDriverMatchDef) -> Self {
        Self {
            composite_driver: value.composite_driver,
            parent_names: value.parent_names,
            primary_parent_index: value.primary_parent_index,
            __source_breaking: value.__source_breaking,
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::CompositeInfo)]
#[rkyv(archived = ArchivedCompositeInfo)]
pub struct CompositeInfoDef {
    #[rkyv(with = rkyv::with::Map<CompositeNodeSpecDef>)]
    pub spec: Option<fdf::CompositeNodeSpec>,
    #[rkyv(with = rkyv::with::Map<CompositeDriverMatchDef>)]
    pub matched_driver: Option<fdf::CompositeDriverMatch>,
    #[rkyv(with = SourceBreakingDef)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

impl From<CompositeInfoDef> for fdf::CompositeInfo {
    fn from(value: CompositeInfoDef) -> Self {
        Self {
            spec: value.spec,
            matched_driver: value.matched_driver,
            __source_breaking: value.__source_breaking,
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::CompositeNodeSpec)]
#[rkyv(archived = ArchivedCompositeNodeSpec)]
pub struct CompositeNodeSpecDef {
    pub name: Option<String>,
    #[rkyv(with = rkyv::with::Map<rkyv::with::Map<ParentSpecDef>>)]
    pub parents: Option<Vec<fdf::ParentSpec>>,
    #[rkyv(with = rkyv::with::Map<rkyv::with::Map<ParentSpec2Def>>)]
    pub parents2: Option<Vec<fdf::ParentSpec2>>,
    pub driver_host: Option<String>,
    #[rkyv(with = SourceBreakingDef)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

impl From<CompositeNodeSpecDef> for fdf::CompositeNodeSpec {
    fn from(value: CompositeNodeSpecDef) -> Self {
        Self {
            name: value.name,
            parents: value.parents,
            parents2: value.parents2,
            driver_host: value.driver_host,
            __source_breaking: value.__source_breaking,
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::Condition)]
#[rkyv(archived = ArchivedCondition)]
#[rkyv(derive(Hash, PartialEq, Eq, PartialOrd, Ord))]
pub enum ConditionDef {
    Unknown,
    Accept,
    Reject,
}

impl From<ConditionDef> for fdf::Condition {
    fn from(value: ConditionDef) -> Self {
        match value {
            ConditionDef::Unknown => fdf::Condition::Unknown,
            ConditionDef::Accept => fdf::Condition::Accept,
            ConditionDef::Reject => fdf::Condition::Reject,
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = bind::interpreter::decode_bind_rules::DecodedBindRules)]
#[rkyv(archived = ArchivedDecodedBindRules)]
pub struct DecodedBindRulesDef {
    pub symbol_table: HashMap<u32, String>,
    pub instructions: Vec<u8>,
    #[rkyv(with = rkyv::with::Map<DecodedInstructionDef>)]
    pub decoded_instructions: Vec<bind::interpreter::instruction_decoder::DecodedInstruction>,
    #[rkyv(with = rkyv::with::Map<DecodedDebugInfoDef>)]
    pub debug_info: Option<bind::interpreter::decode_bind_rules::DecodedDebugInfo>,
}

impl From<DecodedBindRulesDef> for bind::interpreter::decode_bind_rules::DecodedBindRules {
    fn from(value: DecodedBindRulesDef) -> Self {
        Self {
            symbol_table: value.symbol_table,
            instructions: value.instructions,
            decoded_instructions: value.decoded_instructions,
            debug_info: value.debug_info,
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = bind::interpreter::decode_bind_rules::DecodedCompositeBindRules)]
#[rkyv(archived = ArchivedDecodedCompositeBindRules)]
pub struct DecodedCompositeBindRulesDef {
    pub symbol_table: HashMap<u32, String>,
    pub device_name_id: u32,
    #[rkyv(with = ParentDef)]
    pub primary_parent: bind::interpreter::decode_bind_rules::Parent,
    #[rkyv(with = rkyv::with::Map<ParentDef>)]
    pub additional_parents: Vec<bind::interpreter::decode_bind_rules::Parent>,
    #[rkyv(with = rkyv::with::Map<ParentDef>)]
    pub optional_parents: Vec<bind::interpreter::decode_bind_rules::Parent>,
    #[rkyv(with = rkyv::with::Map<DecodedDebugInfoDef>)]
    pub debug_info: Option<bind::interpreter::decode_bind_rules::DecodedDebugInfo>,
}

impl From<DecodedCompositeBindRulesDef>
    for bind::interpreter::decode_bind_rules::DecodedCompositeBindRules
{
    fn from(value: DecodedCompositeBindRulesDef) -> Self {
        Self {
            symbol_table: value.symbol_table,
            device_name_id: value.device_name_id,
            primary_parent: value.primary_parent,
            additional_parents: value.additional_parents,
            optional_parents: value.optional_parents,
            debug_info: value.debug_info,
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = bind::interpreter::instruction_decoder::DecodedCondition)]
#[rkyv(archived = ArchivedDecodedCondition)]
pub struct DecodedConditionDef {
    pub is_equal: bool,
    #[rkyv(with = SymbolDef)]
    pub lhs: bind::compiler::Symbol,
    #[rkyv(with = SymbolDef)]
    pub rhs: bind::compiler::Symbol,
}

impl From<DecodedConditionDef> for bind::interpreter::instruction_decoder::DecodedCondition {
    fn from(value: DecodedConditionDef) -> Self {
        Self { is_equal: value.is_equal, lhs: value.lhs, rhs: value.rhs }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = bind::interpreter::decode_bind_rules::DecodedDebugInfo)]
#[rkyv(archived = ArchivedDecodedDebugInfo)]
pub struct DecodedDebugInfoDef {
    pub symbol_table: HashMap<u32, String>,
}

impl From<DecodedDebugInfoDef> for bind::interpreter::decode_bind_rules::DecodedDebugInfo {
    fn from(value: DecodedDebugInfoDef) -> Self {
        Self { symbol_table: value.symbol_table }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = bind::interpreter::instruction_decoder::DecodedInstruction)]
#[rkyv(archived = ArchivedDecodedInstruction)]
pub enum DecodedInstructionDef {
    UnconditionalAbort,
    Condition(
        #[rkyv(with = DecodedConditionDef)]
        bind::interpreter::instruction_decoder::DecodedCondition,
    ),
    Jump(
        #[rkyv(with = rkyv::with::Map<DecodedConditionDef>)]
        Option<bind::interpreter::instruction_decoder::DecodedCondition>,
    ),
    Label,
}

impl From<DecodedInstructionDef> for bind::interpreter::instruction_decoder::DecodedInstruction {
    fn from(value: DecodedInstructionDef) -> Self {
        match value {
            DecodedInstructionDef::UnconditionalAbort => Self::UnconditionalAbort,
            DecodedInstructionDef::Condition(a) => Self::Condition(a),
            DecodedInstructionDef::Jump(a) => Self::Jump(a),
            DecodedInstructionDef::Label => Self::Label,
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = bind::interpreter::decode_bind_rules::DecodedRules)]
#[rkyv(archived = ArchivedDecodedRules)]
pub enum DecodedRulesDef {
    Normal(
        #[rkyv(with = DecodedBindRulesDef)] bind::interpreter::decode_bind_rules::DecodedBindRules,
    ),
    Composite(
        #[rkyv(with = DecodedCompositeBindRulesDef)]
        bind::interpreter::decode_bind_rules::DecodedCompositeBindRules,
    ),
}

impl From<DecodedRulesDef> for bind::interpreter::decode_bind_rules::DecodedRules {
    fn from(value: DecodedRulesDef) -> Self {
        match value {
            DecodedRulesDef::Normal(a) => Self::Normal(a),
            DecodedRulesDef::Composite(a) => Self::Composite(a),
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::DeviceCategory)]
#[rkyv(archived = ArchivedDeviceCategory)]
pub struct DeviceCategoryDef {
    pub category: Option<String>,
    pub subcategory: Option<String>,
    #[rkyv(with = SourceBreakingDef)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

impl From<DeviceCategoryDef> for fdf::DeviceCategory {
    fn from(value: DeviceCategoryDef) -> Self {
        Self {
            category: value.category,
            subcategory: value.subcategory,
            __source_breaking: value.__source_breaking,
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::DriverInfo)]
#[rkyv(archived = ArchivedDriverInfo)]
pub struct DriverInfoDef {
    pub url: Option<String>,
    pub name: Option<String>,
    pub colocate: Option<bool>,
    #[rkyv(with = rkyv::with::Map<DriverPackageTypeDef>)]
    pub package_type: Option<fdf::DriverPackageType>,
    pub is_fallback: Option<bool>,
    #[rkyv(with = rkyv::with::Map<rkyv::with::Map<DeviceCategoryDef>>)]
    pub device_categories: Option<Vec<fdf::DeviceCategory>>,
    pub bind_rules_bytecode: Option<Vec<u8>>,
    pub driver_framework_version: Option<u8>,
    pub is_disabled: Option<bool>,
    #[rkyv(with = SourceBreakingDef)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

impl From<DriverInfoDef> for fdf::DriverInfo {
    fn from(value: DriverInfoDef) -> Self {
        Self {
            url: value.url,
            name: value.name,
            colocate: value.colocate,
            package_type: value.package_type,
            is_fallback: value.is_fallback,
            device_categories: value.device_categories,
            bind_rules_bytecode: value.bind_rules_bytecode,
            driver_framework_version: value.driver_framework_version,
            is_disabled: value.is_disabled,
            __source_breaking: value.__source_breaking,
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::DriverPackageType)]
#[rkyv(archived = ArchivedDriverPackageType)]
pub enum DriverPackageTypeDef {
    Boot,
    Base,
    Cached,
    Universe,
    __SourceBreaking { unknown_ordinal: u8 },
}

impl From<DriverPackageTypeDef> for fdf::DriverPackageType {
    fn from(value: DriverPackageTypeDef) -> Self {
        match value {
            DriverPackageTypeDef::Boot => Self::Boot,
            DriverPackageTypeDef::Base => Self::Base,
            DriverPackageTypeDef::Cached => Self::Cached,
            DriverPackageTypeDef::Universe => Self::Universe,
            DriverPackageTypeDef::__SourceBreaking { unknown_ordinal } => {
                Self::__SourceBreaking { unknown_ordinal }
            }
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::ParentSpec)]
#[rkyv(archived = ArchivedParentSpec)]
pub struct ParentSpecDef {
    #[rkyv(with = rkyv::with::Map<BindRuleDef>)]
    pub bind_rules: Vec<fdf::BindRule>,
    #[rkyv(with = rkyv::with::Map<NodePropertyDef>)]
    pub properties: Vec<fdf::NodeProperty>,
}

impl From<ParentSpecDef> for fdf::ParentSpec {
    fn from(value: ParentSpecDef) -> Self {
        Self { bind_rules: value.bind_rules, properties: value.properties }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::NodeProperty)]
#[rkyv(archived = ArchivedNodeProperty)]
pub struct NodePropertyDef {
    #[rkyv(with = NodePropertyKeyDef)]
    pub key: fdf::NodePropertyKey,
    #[rkyv(with = NodePropertyValueDef)]
    pub value: fdf::NodePropertyValue,
}

impl From<NodePropertyDef> for fdf::NodeProperty {
    fn from(value: NodePropertyDef) -> Self {
        Self { key: value.key, value: value.value }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::NodeProperty2)]
#[rkyv(archived = ArchivedNodeProperty2)]
pub struct NodeProperty2Def {
    pub key: String,
    #[rkyv(with = NodePropertyValueDef)]
    pub value: fdf::NodePropertyValue,
}

impl From<NodeProperty2Def> for fdf::NodeProperty2 {
    fn from(value: NodeProperty2Def) -> Self {
        Self { key: value.key, value: value.value }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::NodePropertyKey)]
#[rkyv(archived = ArchivedNodePropertyKey)]
pub enum NodePropertyKeyDef {
    IntValue(u32),
    StringValue(String),
}

impl From<NodePropertyKeyDef> for fdf::NodePropertyKey {
    fn from(value: NodePropertyKeyDef) -> Self {
        match value {
            NodePropertyKeyDef::IntValue(a) => Self::IntValue(a),
            NodePropertyKeyDef::StringValue(a) => Self::StringValue(a),
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::NodePropertyValue)]
#[rkyv(archived = ArchivedNodePropertyValue)]
pub enum NodePropertyValueDef {
    IntValue(u32),
    StringValue(String),
    BoolValue(bool),
    EnumValue(String),
    __SourceBreaking { unknown_ordinal: u64 },
}

impl From<NodePropertyValueDef> for fdf::NodePropertyValue {
    fn from(value: NodePropertyValueDef) -> Self {
        match value {
            NodePropertyValueDef::IntValue(a) => Self::IntValue(a),
            NodePropertyValueDef::StringValue(a) => Self::StringValue(a),
            NodePropertyValueDef::BoolValue(a) => Self::BoolValue(a),
            NodePropertyValueDef::EnumValue(a) => Self::EnumValue(a),
            NodePropertyValueDef::__SourceBreaking { unknown_ordinal } => {
                Self::__SourceBreaking { unknown_ordinal }
            }
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = bind::interpreter::decode_bind_rules::Parent)]
#[rkyv(archived = ArchivedParent)]
pub struct ParentDef {
    pub name_id: u32,
    pub instructions: Vec<u8>,
    #[rkyv(with = rkyv::with::Map<DecodedInstructionDef>)]
    pub decoded_instructions: Vec<bind::interpreter::instruction_decoder::DecodedInstruction>,
}

impl From<ParentDef> for bind::interpreter::decode_bind_rules::Parent {
    fn from(value: ParentDef) -> Self {
        Self {
            name_id: value.name_id,
            instructions: value.instructions,
            decoded_instructions: value.decoded_instructions,
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = fdf::ParentSpec2)]
#[rkyv(archived = ArchivedParentSpec2)]
pub struct ParentSpec2Def {
    #[rkyv(with = rkyv::with::Map<BindRule2Def>)]
    pub bind_rules: Vec<fdf::BindRule2>,
    #[rkyv(with = rkyv::with::Map<NodeProperty2Def>)]
    pub properties: Vec<fdf::NodeProperty2>,
}

impl From<ParentSpec2Def> for fdf::ParentSpec2 {
    fn from(value: ParentSpec2Def) -> Self {
        Self { bind_rules: value.bind_rules, properties: value.properties }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = bind::interpreter::match_bind::PropertyKey)]
#[rkyv(archived = ArchivedPropertyKey)]
#[rkyv(derive(Hash, PartialEq, Eq, PartialOrd, Ord))]
pub enum PropertyKeyDef {
    NumberKey(u64),
    StringKey(String),
}

impl From<PropertyKeyDef> for bind::interpreter::match_bind::PropertyKey {
    fn from(value: PropertyKeyDef) -> Self {
        match value {
            PropertyKeyDef::NumberKey(a) => Self::NumberKey(a),
            PropertyKeyDef::StringKey(a) => Self::StringKey(a),
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = bind::compiler::Symbol)]
#[rkyv(archived = ArchivedSymbol)]
#[rkyv(derive(Hash, PartialEq, Eq, PartialOrd, Ord))]
pub enum SymbolDef {
    DeprecatedKey(u32),
    Key(String, #[rkyv(with = ValueTypeDef)] bind::parser::bind_library::ValueType),
    NumberValue(u64),
    StringValue(String),
    BoolValue(bool),
    EnumValue(String),
}

impl From<SymbolDef> for bind::compiler::Symbol {
    fn from(value: SymbolDef) -> Self {
        match value {
            SymbolDef::DeprecatedKey(a) => Self::DeprecatedKey(a),
            SymbolDef::Key(a, b) => Self::Key(a, b),
            SymbolDef::NumberValue(a) => Self::NumberValue(a),
            SymbolDef::StringValue(a) => Self::StringValue(a),
            SymbolDef::BoolValue(a) => Self::BoolValue(a),
            SymbolDef::EnumValue(a) => Self::EnumValue(a),
        }
    }
}

pub struct UrlDef;

impl rkyv::with::ArchiveWith<cm_types::Url> for UrlDef {
    type Archived = rkyv::string::ArchivedString;
    type Resolver = rkyv::string::StringResolver;

    fn resolve_with(
        field: &cm_types::Url,
        resolver: Self::Resolver,
        out: rkyv::Place<Self::Archived>,
    ) {
        rkyv::string::ArchivedString::resolve_from_str(field.as_str(), resolver, out);
    }
}

impl<S> rkyv::with::SerializeWith<cm_types::Url, S> for UrlDef
where
    S: rkyv::rancor::Fallible + ?Sized,
    S::Error: rkyv::rancor::Source,
    str: rkyv::SerializeUnsized<S>,
{
    fn serialize_with(
        field: &cm_types::Url,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        rkyv::string::ArchivedString::serialize_from_str(field.as_str(), serializer)
    }
}

impl<D> rkyv::with::DeserializeWith<rkyv::string::ArchivedString, cm_types::Url, D> for UrlDef
where
    D: rkyv::rancor::Fallible + ?Sized,
    D::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &rkyv::string::ArchivedString,
        _: &mut D,
    ) -> Result<cm_types::Url, D::Error> {
        use rkyv::rancor::ResultExt as _;

        cm_types::Url::new(field.as_str()).into_error()
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = bind::parser::bind_library::ValueType)]
#[rkyv(archived = ArchivedValueType)]
#[rkyv(derive(Hash, PartialEq, Eq, PartialOrd, Ord))]
pub enum ValueTypeDef {
    Number,
    Str,
    Bool,
    Enum,
}

impl From<ValueTypeDef> for bind::parser::bind_library::ValueType {
    fn from(value: ValueTypeDef) -> Self {
        match value {
            ValueTypeDef::Number => Self::Number,
            ValueTypeDef::Str => Self::Str,
            ValueTypeDef::Bool => Self::Bool,
            ValueTypeDef::Enum => Self::Enum,
        }
    }
}
