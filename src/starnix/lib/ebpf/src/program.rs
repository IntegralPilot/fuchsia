// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::executor::execute;
use crate::verifier::VerifiedEbpfProgram;
use crate::{
    BPF_CALL, BPF_DW, BPF_JMP, BPF_LDDW, BPF_PSEUDO_MAP_IDX, BPF_PSEUDO_MAP_IDX_VALUE,
    BPF_SIZE_MASK, CbpfConfig, DataWidth, EbpfError, EbpfInstruction, EbpfPtr, MapSchema, MemoryId,
    StructAccess, Type,
};
use derivative::Derivative;
use std::collections::HashMap;
use std::fmt::Formatter;
use std::mem::size_of;
use zerocopy::{FromBytes, Immutable, IntoBytes};

/// Trait that should be implemented for arguments passed to eBPF programs.
pub trait ProgramArgument: Into<BpfValue> {
    /// Returns eBPF type that corresponds to `Self`. Used when program argument types
    /// are checked statically.
    fn get_type() -> &'static Type;

    /// Returns eBPF type for a specific value of `Self`. For most types this is the
    /// same type that's returned by `get_type()`, but that's not always the case.
    /// In particular for scalar values this will return `Type::ScalarValue` with
    /// the actual value of the scalar and with `unknown_mask = 0`.
    fn get_value_type(&self) -> Type {
        Self::get_type().clone()
    }

    /// Returns the list of field mappings that should be applied with this argument.
    /// If not empty then `get_type()` must be a `PtrToStruct`.
    fn field_mappings() -> &'static [FieldMapping] {
        static NO_MAPPINGS: [FieldMapping; 0] = [];
        &NO_MAPPINGS
    }

    /// Returns the `StructMapping` that should be applied with this argument. Implementations
    /// should override `field_mappings()` and keep default implementation of `struct_mapping()`.
    fn struct_mapping() -> Option<StructMapping> {
        let fields = Self::field_mappings();
        if fields.is_empty() {
            return None;
        }
        Some(StructMapping {
            memory_id: match Self::get_type() {
                Type::PtrToStruct { id, .. } => id.clone(),
                _ => panic!("type must be PtrToStruct"),
            },
            fields: fields.iter().cloned().collect(),
        })
    }
}

/// Trait that should be implemented for types that can be converted from `BpfValue`.
/// Used to get a `Packet` when loading a value from the packet.
pub trait FromBpfValue<C>: Sized {
    /// # Safety
    /// Should be called only by the eBPF interpreter when executing verified eBPF code.
    unsafe fn from_bpf_value(context: &mut C, v: BpfValue) -> Self;
}

impl ProgramArgument for () {
    fn get_type() -> &'static Type {
        &Type::UNINITIALIZED
    }
}

impl<C> FromBpfValue<C> for () {
    unsafe fn from_bpf_value(_context: &mut C, _v: BpfValue) -> Self {
        unreachable!();
    }
}

impl ProgramArgument for usize {
    fn get_type() -> &'static Type {
        &Type::UNKNOWN_SCALAR
    }

    fn get_value_type(&self) -> Type {
        Type::from(*self as u64)
    }
}

/// Implements `ProgramArgument` for `EbpfPtr` when it's implemented for
/// `&'a mut T`.
impl<'a, T> ProgramArgument for EbpfPtr<'a, T>
where
    &'a mut T: ProgramArgument,
{
    fn get_type() -> &'static Type {
        <&'a mut T as ProgramArgument>::get_type()
    }

    fn field_mappings() -> &'static [FieldMapping] {
        <&'a mut T as ProgramArgument>::field_mappings()
    }

    fn struct_mapping() -> Option<StructMapping> {
        <&'a mut T as ProgramArgument>::struct_mapping()
    }
}

impl<'a, T, C> FromBpfValue<C> for &'a mut T
where
    &'a mut T: ProgramArgument,
{
    unsafe fn from_bpf_value(_context: &mut C, v: BpfValue) -> Self {
        #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
        unsafe {
            &mut *v.as_ptr::<T>()
        }
    }
}

impl<'a, T, C> FromBpfValue<C> for &'a T
where
    &'a T: ProgramArgument,
{
    unsafe fn from_bpf_value(_context: &mut C, v: BpfValue) -> Self {
        #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
        unsafe {
            &*v.as_ptr::<T>()
        }
    }
}

/// A strong reference to an eBPF map held for the lifetime of an eBPF linked
/// with the map. Can be converted to `BpfValue`, which is used by the program
/// to identify the map when it calls map helpers.
pub trait MapReference {
    fn schema(&self) -> &MapSchema;
    fn as_bpf_value(&self) -> BpfValue;

    /// Returns the address of the first element of this map.
    ///
    /// This will only ever be called on a map of type `BPF_MAP_TYPE_ARRAY` with at least one
    /// element.
    fn get_data_ptr(&self) -> Option<BpfValue>;
}

/// `MapReference` for `EbpfProgramContext` where maps are not used.
pub enum NoMap {}

impl MapReference for NoMap {
    fn schema(&self) -> &MapSchema {
        unreachable!()
    }
    fn as_bpf_value(&self) -> BpfValue {
        unreachable!()
    }
    fn get_data_ptr(&self) -> Option<BpfValue> {
        unreachable!()
    }
}

pub trait StaticHelperSet: EbpfProgramContext {
    /// Returns the set of helpers available to the program.
    fn helpers() -> &'static HelperSet<Self>;
}

pub trait EbpfProgramContext: 'static + Sized {
    /// Context for an invocation of an eBPF program.
    type RunContext<'a>;

    /// Packet used by the program.
    type Packet<'a>: Packet + FromBpfValue<Self::RunContext<'a>>;

    /// Arguments passed to the program
    type Arg1<'a>: ProgramArgument;
    type Arg2<'a>: ProgramArgument;
    type Arg3<'a>: ProgramArgument;
    type Arg4<'a>: ProgramArgument;
    type Arg5<'a>: ProgramArgument;

    /// Type used to reference eBPF maps for the lifetime of a program.
    type Map: MapReference;

    /// Returns the set of struct mappings that should be applied when linking a program. The
    /// default implementation collects mappings from all arguments.
    fn struct_mappings() -> StructMappings {
        let mut mappings = StructMappings::new();
        if let Some(mapping) = Self::Arg1::struct_mapping() {
            mappings.push(mapping);
        }
        if let Some(mapping) = Self::Arg2::struct_mapping() {
            mappings.push(mapping);
        }
        if let Some(mapping) = Self::Arg3::struct_mapping() {
            mappings.push(mapping);
        }
        if let Some(mapping) = Self::Arg4::struct_mapping() {
            mappings.push(mapping);
        }
        if let Some(mapping) = Self::Arg5::struct_mapping() {
            mappings.push(mapping);
        }
        mappings
    }
}

/// Trait that should be implemented by packets passed to eBPF programs.
pub trait Packet {
    fn load(&self, offset: i32, width: DataWidth) -> Option<BpfValue>;
}

impl Packet for () {
    fn load(&self, _offset: i32, _width: DataWidth) -> Option<BpfValue> {
        None
    }
}

/// Simple `Packet` implementation for packets that can be accessed directly.
impl<P: IntoBytes + Immutable> Packet for &P {
    fn load(&self, offset: i32, width: DataWidth) -> Option<BpfValue> {
        let data = (*self).as_bytes();
        if offset < 0 || offset as usize >= data.len() {
            return None;
        }
        let slice = &data[(offset as usize)..];
        match width {
            DataWidth::U8 => u8::read_from_prefix(slice).ok().map(|(v, _)| v.into()),
            DataWidth::U16 => u16::read_from_prefix(slice).ok().map(|(v, _)| v.into()),
            DataWidth::U32 => u32::read_from_prefix(slice).ok().map(|(v, _)| v.into()),
            DataWidth::U64 => u64::read_from_prefix(slice).ok().map(|(v, _)| v.into()),
        }
    }
}

/// A context for a BPF program that's compatible with eBPF and cBPF.
pub trait BpfProgramContext: 'static + Sized {
    type RunContext<'a>;
    type Packet<'a>: ProgramArgument + Packet + FromBpfValue<Self::RunContext<'a>>;
    type Map: MapReference;
    const CBPF_CONFIG: &'static CbpfConfig;

    fn get_arg_types() -> Vec<Type> {
        vec![<Self::Packet<'_> as ProgramArgument>::get_type().clone()]
    }
}

impl<T: BpfProgramContext> EbpfProgramContext for T {
    type RunContext<'a> = <T as BpfProgramContext>::RunContext<'a>;
    type Packet<'a> = T::Packet<'a>;
    type Arg1<'a> = T::Packet<'a>;
    type Arg2<'a> = ();
    type Arg3<'a> = ();
    type Arg4<'a> = ();
    type Arg5<'a> = ();
    type Map = T::Map;
}

#[derive(Clone, Copy, Debug)]
pub struct BpfValue(u64);

static_assertions::const_assert_eq!(size_of::<BpfValue>(), size_of::<*const u8>());

impl Default for BpfValue {
    fn default() -> Self {
        Self::from(0)
    }
}

impl From<()> for BpfValue {
    fn from(_v: ()) -> Self {
        Self(0)
    }
}

impl From<i32> for BpfValue {
    fn from(v: i32) -> Self {
        Self((v as u32) as u64)
    }
}

impl From<u8> for BpfValue {
    fn from(v: u8) -> Self {
        Self::from(v as u64)
    }
}

impl From<u16> for BpfValue {
    fn from(v: u16) -> Self {
        Self::from(v as u64)
    }
}

impl From<u32> for BpfValue {
    fn from(v: u32) -> Self {
        Self::from(v as u64)
    }
}
impl From<u64> for BpfValue {
    fn from(v: u64) -> Self {
        Self(v)
    }
}
impl From<i64> for BpfValue {
    fn from(v: i64) -> Self {
        Self(v as u64)
    }
}

impl From<usize> for BpfValue {
    fn from(v: usize) -> Self {
        Self(v as u64)
    }
}

impl<T> From<*const T> for BpfValue {
    fn from(v: *const T) -> Self {
        Self(v as u64)
    }
}

impl<T> From<*mut T> for BpfValue {
    fn from(v: *mut T) -> Self {
        Self(v as u64)
    }
}

impl<T> From<&'_ T> for BpfValue {
    fn from(v: &'_ T) -> Self {
        Self((v as *const T) as u64)
    }
}

impl<T> From<&'_ mut T> for BpfValue {
    fn from(v: &'_ mut T) -> Self {
        Self((v as *const T) as u64)
    }
}

impl<'a, T> From<EbpfPtr<'a, T>> for BpfValue {
    fn from(value: EbpfPtr<'a, T>) -> Self {
        value.ptr().into()
    }
}

impl TryFrom<BpfValue> for u8 {
    type Error = core::num::TryFromIntError;
    fn try_from(v: BpfValue) -> Result<Self, Self::Error> {
        v.0.try_into()
    }
}

impl TryFrom<BpfValue> for u16 {
    type Error = core::num::TryFromIntError;
    fn try_from(v: BpfValue) -> Result<Self, Self::Error> {
        v.0.try_into()
    }
}

impl TryFrom<BpfValue> for u32 {
    type Error = core::num::TryFromIntError;
    fn try_from(v: BpfValue) -> Result<Self, Self::Error> {
        v.0.try_into()
    }
}

impl From<BpfValue> for u64 {
    fn from(v: BpfValue) -> u64 {
        v.0
    }
}

impl From<BpfValue> for usize {
    fn from(v: BpfValue) -> usize {
        v.0 as usize
    }
}

impl BpfValue {
    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }

    pub fn as_u8(&self) -> u8 {
        self.0 as u8
    }

    pub fn as_u16(&self) -> u16 {
        self.0 as u16
    }

    pub fn as_u32(&self) -> u32 {
        self.0 as u32
    }

    pub fn as_i32(&self) -> i32 {
        self.0 as i32
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }

    pub fn as_ptr<T>(&self) -> *mut T {
        self.0 as *mut T
    }
}

impl From<BpfValue> for () {
    fn from(_v: BpfValue) -> Self {
        ()
    }
}

#[derive(Derivative)]
#[derivative(Clone(bound = ""))]
pub struct EbpfHelperImpl<C: EbpfProgramContext>(
    pub  for<'a> fn(
        &mut C::RunContext<'a>,
        BpfValue,
        BpfValue,
        BpfValue,
        BpfValue,
        BpfValue,
    ) -> BpfValue,
);

/// Stores set of helpers for an eBPF program. The helpers are stored packed
/// in a vector for fast access in the interpreter.
#[derive(Derivative)]
#[derivative(Clone(bound = ""), Default(bound = ""))]
pub struct HelperSet<C: EbpfProgramContext> {
    helpers: Vec<EbpfHelperImpl<C>>,

    // Maps helper IDs used in eBPF to location of the helper in `helpers`.
    indices: HashMap<u32, u32>,
}

impl<C: EbpfProgramContext> FromIterator<(u32, EbpfHelperImpl<C>)> for HelperSet<C> {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = (u32, EbpfHelperImpl<C>)>,
    {
        let mut helpers = Vec::new();
        let mut indices = HashMap::new();
        for (helper_id, helper) in iter {
            let helper_index = helpers.len();
            helpers.push(helper);
            indices.insert(helper_id, helper_index as u32);
        }
        Self { helpers, indices }
    }
}

impl<C: EbpfProgramContext> HelperSet<C> {
    pub fn new(iter: impl IntoIterator<Item = (u32, EbpfHelperImpl<C>)>) -> Self {
        Self::from_iter(iter)
    }

    /// Looks up a helper by ID, returns helper `index`.
    pub fn get_index_by_id(&self, id: u32) -> Option<u32> {
        self.indices.get(&id).cloned()
    }

    /// Returns the helper with the specified index.
    pub fn get_by_index(&self, index: u32) -> Option<&EbpfHelperImpl<C>> {
        self.helpers.get(index as usize)
    }
}

/// A mapping for a field in a struct where the original ebpf program knows a different offset and
/// data size than the one it receives from the kernel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldMapping {
    /// The offset of the field as known by the original ebpf program.
    pub source_offset: usize,
    /// The actual offset of the field in the data provided by the kernel.
    pub target_offset: usize,
}

pub type FieldMappings = smallvec::SmallVec<[FieldMapping; 2]>;

#[derive(Clone, Debug)]
pub struct StructMapping {
    /// Memory ID used in the struct definition.
    pub memory_id: MemoryId,

    /// The list of mappings in the buffer. The verifier must rewrite the actual ebpf to ensure
    /// the right offset and operand are use to access the mapped fields. Mappings are allowed
    /// only for pointer fields.
    pub fields: FieldMappings,
}

pub type StructMappings = smallvec::SmallVec<[StructMapping; 1]>;

pub trait ArgumentTypeChecker<C: EbpfProgramContext>: Sized {
    fn link(program: &VerifiedEbpfProgram) -> Result<Self, EbpfError>;
    fn run_time_check<'a>(
        &self,
        arg1: &C::Arg1<'a>,
        arg2: &C::Arg2<'a>,
        arg3: &C::Arg3<'a>,
        arg4: &C::Arg4<'a>,
        arg5: &C::Arg5<'a>,
    ) -> Result<(), EbpfError>;
}

pub struct StaticTypeChecker();

impl<C: EbpfProgramContext> ArgumentTypeChecker<C> for StaticTypeChecker {
    fn link(program: &VerifiedEbpfProgram) -> Result<Self, EbpfError> {
        let arg_types = [
            C::Arg1::get_type(),
            C::Arg2::get_type(),
            C::Arg3::get_type(),
            C::Arg4::get_type(),
            C::Arg5::get_type(),
        ];
        for i in 0..5 {
            let verified_type = program.args.get(i).unwrap_or(&Type::UNINITIALIZED);
            if !arg_types[i].is_subtype(verified_type) {
                return Err(EbpfError::ProgramLinkError(format!(
                    "Type of argument {} doesn't match. Verified type: {:?}. Context type: {:?}",
                    i + 1,
                    verified_type,
                    arg_types[i],
                )));
            }
        }

        Ok(Self())
    }

    fn run_time_check<'a>(
        &self,
        _arg1: &C::Arg1<'a>,
        _arg2: &C::Arg2<'a>,
        _arg3: &C::Arg3<'a>,
        _arg4: &C::Arg4<'a>,
        _arg5: &C::Arg5<'a>,
    ) -> Result<(), EbpfError> {
        // No-op since argument types were checked in `link()`.
        Ok(())
    }
}

pub struct DynamicTypeChecker {
    types: Vec<Type>,
}

impl<C: EbpfProgramContext> ArgumentTypeChecker<C> for DynamicTypeChecker {
    fn link(program: &VerifiedEbpfProgram) -> Result<Self, EbpfError> {
        Ok(Self { types: program.args.clone() })
    }

    fn run_time_check<'a>(
        &self,
        arg1: &C::Arg1<'a>,
        arg2: &C::Arg2<'a>,
        arg3: &C::Arg3<'a>,
        arg4: &C::Arg4<'a>,
        arg5: &C::Arg5<'a>,
    ) -> Result<(), EbpfError> {
        let arg_types = [
            arg1.get_value_type(),
            arg2.get_value_type(),
            arg3.get_value_type(),
            arg4.get_value_type(),
            arg5.get_value_type(),
        ];
        for i in 0..5 {
            let verified_type = self.types.get(i).unwrap_or(&Type::UNINITIALIZED);
            if !&arg_types[i].is_subtype(verified_type) {
                return Err(EbpfError::ProgramLinkError(format!(
                    "Type of argument {} doesn't match. Verified type: {:?}. Value type: {:?}",
                    i + 1,
                    verified_type,
                    arg_types[i],
                )));
            }
        }

        Ok(())
    }
}

/// An abstraction over an eBPF program and its registered helper functions.
pub struct EbpfProgram<C: EbpfProgramContext, T: ArgumentTypeChecker<C> = StaticTypeChecker> {
    pub(crate) code: Vec<EbpfInstruction>,

    /// List of references to the maps used by the program. This field is not used directly,
    /// but it's kept here to ensure that the maps outlive the program.
    #[allow(dead_code)]
    pub(crate) maps: Vec<C::Map>,

    type_checker: T,
}

impl<C: EbpfProgramContext, T: ArgumentTypeChecker<C>> std::fmt::Debug for EbpfProgram<C, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("EbpfProgram").field("code", &self.code).finish()
    }
}

impl<C: EbpfProgramContext, T: ArgumentTypeChecker<C>> EbpfProgram<C, T> {
    pub fn code(&self) -> &[EbpfInstruction] {
        &self.code[..]
    }
}

impl<C, T: ArgumentTypeChecker<C>> EbpfProgram<C, T>
where
    C: StaticHelperSet,
    C: for<'a> EbpfProgramContext<Arg2<'a> = (), Arg3<'a> = (), Arg4<'a> = (), Arg5<'a> = ()>,
{
    pub fn run_with_1_argument<'a>(
        &self,
        run_context: &mut C::RunContext<'a>,
        arg1: C::Arg1<'a>,
    ) -> u64 {
        self.type_checker
            .run_time_check(&arg1, &(), &(), &(), &())
            .expect("Failed argument type check");
        execute(&self.code[..], C::helpers(), run_context, &[arg1.into()])
    }
}

impl<C, T: ArgumentTypeChecker<C>> EbpfProgram<C, T>
where
    C: StaticHelperSet,
    C: for<'a> EbpfProgramContext<Arg3<'a> = (), Arg4<'a> = (), Arg5<'a> = ()>,
{
    pub fn run_with_2_arguments<'a>(
        &self,
        run_context: &mut C::RunContext<'a>,
        arg1: C::Arg1<'a>,
        arg2: C::Arg2<'a>,
    ) -> u64 {
        self.type_checker
            .run_time_check(&arg1, &arg2, &(), &(), &())
            .expect("Failed argument type check");
        execute(&self.code[..], C::helpers(), run_context, &[arg1.into(), arg2.into()])
    }
}

impl<C: BpfProgramContext, T: ArgumentTypeChecker<C>> EbpfProgram<C, T>
where
    C: BpfProgramContext + StaticHelperSet,
    C: for<'a> EbpfProgramContext<Arg2<'a> = (), Arg3<'a> = (), Arg4<'a> = (), Arg5<'a> = ()>,
{
    /// Executes the current program on the specified `packet`.
    /// The program receives a pointer to the `packet` and the size of the packet as the first
    /// two arguments.
    pub fn run<'a>(
        &self,
        run_context: &mut <C as EbpfProgramContext>::RunContext<'a>,
        packet: <C as EbpfProgramContext>::Arg1<'a>,
    ) -> u64 {
        self.run_with_1_argument(run_context, packet)
    }
}

/// Rewrites the code to ensure mapped fields are correctly handled. Returns
/// runnable `EbpfProgram<C>`.
pub fn link_program_internal<C, T>(
    program: &VerifiedEbpfProgram,
    maps: Vec<C::Map>,
) -> Result<EbpfProgram<C, T>, EbpfError>
where
    C: EbpfProgramContext + StaticHelperSet,
    T: ArgumentTypeChecker<C>,
{
    let type_checker = T::link(program)?;

    let mut code = program.code.clone();
    let struct_mappings = C::struct_mappings();

    // Update offsets in the instructions that access structs.
    for StructAccess { pc, memory_id, field_offset, is_32_bit_ptr_load } in
        program.struct_access_instructions.iter()
    {
        let field_mapping =
            struct_mappings.iter().find(|m| m.memory_id == *memory_id).and_then(|struct_map| {
                struct_map.fields.iter().find(|m| m.source_offset == *field_offset)
            });

        if let Some(field_mapping) = field_mapping {
            let instruction = &mut code[*pc];

            // Note that `instruction.off` may be different from `field.source_offset`. It's adjuststed
            // by the difference between `target_offset` and `source_offset` to ensure the instructions
            // will access the right field.
            let offset_diff = i16::try_from(
                i64::try_from(field_mapping.target_offset).unwrap()
                    - i64::try_from(field_mapping.source_offset).unwrap(),
            )
            .unwrap();

            let new_offset = instruction.offset().checked_add(offset_diff).ok_or_else(|| {
                EbpfError::ProgramLinkError(format!("Struct field offset overflow at PC {}", *pc))
            })?;
            instruction.set_offset(new_offset);

            // 32-bit pointer loads must be updated to 64-bit loads.
            if *is_32_bit_ptr_load {
                instruction.set_code((instruction.code() & !BPF_SIZE_MASK) | BPF_DW);
            }
        } else {
            if *is_32_bit_ptr_load {
                return Err(EbpfError::ProgramLinkError(format!(
                    "32-bit field isn't mapped at pc  {}",
                    *pc,
                )));
            }
        }
    }

    let helpers = C::helpers();

    for pc in 0..code.len() {
        let instruction = &mut code[pc];

        // Replace helper IDs with helper indices in call instructions.
        if instruction.code() == (BPF_JMP | BPF_CALL) {
            assert!(instruction.src_reg() == 0);
            let helper_id = instruction.imm() as u32;
            let Some(index) = helpers.get_index_by_id(helper_id) else {
                return Err(EbpfError::ProgramLinkError(format!(
                    "Missing implementation for helper with id={}",
                    helper_id,
                )));
            };
            instruction.set_imm(index as i32);
        }

        // Link maps.
        if instruction.code() == BPF_LDDW {
            // If the instruction references BPF_PSEUDO_MAP_FD, then we need to look up the map fd
            // and create a reference from this program to that object.
            match instruction.src_reg() {
                0 => (),
                BPF_PSEUDO_MAP_IDX | BPF_PSEUDO_MAP_IDX_VALUE => {
                    let map_index = usize::try_from(instruction.imm())
                        .expect("negative map index in a verified program");
                    let map = maps.get(map_index).ok_or_else(|| {
                        EbpfError::ProgramLinkError(format!("Invalid map_index: {}", map_index))
                    })?;
                    assert!(*map.schema() == program.maps[map_index]);

                    let (instruction, next_instruction) = {
                        // The code was verified, so this is not expected to overflow.
                        let (a1, a2) = code.split_at_mut(pc + 1);
                        (&mut a1[pc], &mut a2[0])
                    };

                    let value = if instruction.src_reg() == BPF_PSEUDO_MAP_IDX {
                        map.as_bpf_value().as_u64()
                    } else {
                        // BPF_PSEUDO_MAP_IDX_VALUE
                        let map_value = map
                            .get_data_ptr()
                            .ok_or_else(|| {
                                EbpfError::ProgramLinkError(format!(
                                    "Unable to get value at 0 for map at index {map_index}"
                                ))
                            })?
                            .as_u64();
                        map_value.checked_add_signed(next_instruction.imm().into()).ok_or_else(
                            || {
                                EbpfError::ProgramLinkError(format!(
                                    "Unable to use offset for map access"
                                ))
                            },
                        )?
                    };

                    let (high, low) = ((value >> 32) as i32, value as i32);
                    instruction.set_src_reg(0);
                    instruction.set_imm(low);
                    next_instruction.set_imm(high);
                }
                value => {
                    return Err(EbpfError::ProgramLinkError(format!(
                        "Unsupported value for src_reg in lddw: {}",
                        value,
                    )));
                }
            }
        }
    }

    Ok(EbpfProgram { code, maps, type_checker })
}

/// Rewrites the code to ensure mapped fields are correctly handled. Returns
/// runnable `EbpfProgram<C>`.
pub fn link_program<C>(
    program: &VerifiedEbpfProgram,
    maps: Vec<C::Map>,
) -> Result<EbpfProgram<C>, EbpfError>
where
    C: EbpfProgramContext + StaticHelperSet,
{
    link_program_internal::<C, StaticTypeChecker>(program, maps)
}

/// Same as above, but allows to check argument types in runtime instead of in link time.
pub fn link_program_dynamic<C>(
    program: &VerifiedEbpfProgram,
    maps: Vec<C::Map>,
) -> Result<EbpfProgram<C, DynamicTypeChecker>, EbpfError>
where
    C: EbpfProgramContext + StaticHelperSet,
{
    link_program_internal::<C, DynamicTypeChecker>(program, maps)
}

#[macro_export]
macro_rules! static_helper_set {
    ($context:ty, $value:expr) => {
        impl $crate::StaticHelperSet for $context {
            fn helpers() -> &'static $crate::HelperSet<$context> {
                static HELPERS: std::sync::LazyLock<$crate::HelperSet<$context>> =
                    std::sync::LazyLock::new(|| $value);
                &HELPERS
            }
        }
    };
}

#[macro_export]
macro_rules! empty_static_helper_set {
    ($context:ty) => {
        $crate::static_helper_set!($context, $crate::HelperSet::default());
    };
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::api::*;
    use crate::conformance::test::parse_asm;
    use crate::{
        CallingContext, FieldDescriptor, FieldMapping, FieldType, NullVerifierLogger,
        ProgramArgument, StructDescriptor, Type, verify_program,
    };
    use std::mem::offset_of;
    use std::sync::{Arc, LazyLock};
    use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

    #[repr(C)]
    #[derive(Debug, Copy, Clone, IntoBytes, Immutable, KnownLayout, FromBytes)]
    struct TestArgument {
        // A field that should not be writable by the program.
        pub read_only_field: u32,
        pub _padding1: u32,
        /// Pointer to an array.
        pub data: u64,
        /// End of the array.
        pub data_end: u64,
        // A field that can be updated by the program.
        pub mutable_field: u32,
        pub _padding2: u32,
    }

    static TEST_ARG_TYPE: LazyLock<Type> = LazyLock::new(|| {
        let data_memory_id = MemoryId::new();
        let descriptor = Arc::new(StructDescriptor {
            fields: vec![
                FieldDescriptor {
                    offset: offset_of!(TestArgument, read_only_field),
                    field_type: FieldType::Scalar { size: 4 },
                },
                FieldDescriptor {
                    offset: offset_of!(TestArgument, data),
                    field_type: FieldType::PtrToArray {
                        is_32_bit: false,
                        id: data_memory_id.clone(),
                    },
                },
                FieldDescriptor {
                    offset: offset_of!(TestArgument, data_end),
                    field_type: FieldType::PtrToEndArray {
                        is_32_bit: false,
                        id: data_memory_id.clone(),
                    },
                },
                FieldDescriptor {
                    offset: offset_of!(TestArgument, mutable_field),
                    field_type: FieldType::MutableScalar { size: 4 },
                },
            ],
        });

        Type::PtrToStruct { id: MemoryId::new(), offset: 0.into(), descriptor }
    });

    impl Default for TestArgument {
        fn default() -> Self {
            Self {
                read_only_field: 1,
                _padding1: 0,
                data: 0,
                data_end: 0,
                mutable_field: 2,
                _padding2: 0,
            }
        }
    }

    impl TestArgument {
        fn from_data(data: &[u64]) -> Self {
            let ptr_range = data.as_ptr_range();
            Self {
                data: ptr_range.start as u64,
                data_end: ptr_range.end as u64,
                ..Default::default()
            }
        }
    }

    impl ProgramArgument for &'_ mut TestArgument {
        fn get_type() -> &'static Type {
            &*TEST_ARG_TYPE
        }
    }

    // A version of TestArgument with 32-bit remapped pointers. It's used to define struct layout
    // for eBPF programs, but not used in the Rust code directly.
    #[repr(C)]
    struct TestArgument32 {
        pub read_only_field: u32,
        pub data: u32,
        pub data_end: u32,
        pub mutable_field: u32,
    }

    #[repr(C)]
    struct TestArgument32BitMapped(TestArgument);

    static TEST_ARG_32_BIT_MEMORY_ID: LazyLock<MemoryId> = LazyLock::new(|| MemoryId::new());
    static TEST_ARG_32_BIT_TYPE: LazyLock<Type> = LazyLock::new(|| {
        let data_memory_id = MemoryId::new();
        let descriptor = Arc::new(StructDescriptor {
            fields: vec![
                FieldDescriptor {
                    offset: offset_of!(TestArgument32, read_only_field),
                    field_type: FieldType::Scalar { size: 4 },
                },
                FieldDescriptor {
                    offset: offset_of!(TestArgument32, data),
                    field_type: FieldType::PtrToArray {
                        is_32_bit: true,
                        id: data_memory_id.clone(),
                    },
                },
                FieldDescriptor {
                    offset: offset_of!(TestArgument32, data_end),
                    field_type: FieldType::PtrToEndArray {
                        is_32_bit: true,
                        id: data_memory_id.clone(),
                    },
                },
                FieldDescriptor {
                    offset: offset_of!(TestArgument32, mutable_field),
                    field_type: FieldType::MutableScalar { size: 4 },
                },
            ],
        });

        Type::PtrToStruct { id: TEST_ARG_32_BIT_MEMORY_ID.clone(), offset: 0.into(), descriptor }
    });

    impl ProgramArgument for &'_ TestArgument32BitMapped {
        fn get_type() -> &'static Type {
            &*TEST_ARG_32_BIT_TYPE
        }

        fn field_mappings() -> &'static [FieldMapping] {
            static FIELD_MAPPINGS: [FieldMapping; 3] = [
                FieldMapping {
                    source_offset: offset_of!(TestArgument32, data),
                    target_offset: offset_of!(TestArgument, data),
                },
                FieldMapping {
                    source_offset: offset_of!(TestArgument32, data_end),
                    target_offset: offset_of!(TestArgument, data_end),
                },
                FieldMapping {
                    source_offset: offset_of!(TestArgument32, mutable_field),
                    target_offset: offset_of!(TestArgument, mutable_field),
                },
            ];
            &FIELD_MAPPINGS
        }
    }

    struct TestEbpfProgramContext {}

    impl EbpfProgramContext for TestEbpfProgramContext {
        type RunContext<'a> = ();

        type Packet<'a> = ();
        type Arg1<'a> = &'a mut TestArgument;
        type Arg2<'a> = ();
        type Arg3<'a> = ();
        type Arg4<'a> = ();
        type Arg5<'a> = ();

        type Map = NoMap;
    }

    empty_static_helper_set!(TestEbpfProgramContext);

    fn initialize_test_program(
        code: Vec<EbpfInstruction>,
    ) -> Result<EbpfProgram<TestEbpfProgramContext>, EbpfError> {
        let verified_program = verify_program(
            code,
            CallingContext { args: vec![TEST_ARG_TYPE.clone()], ..Default::default() },
            &mut NullVerifierLogger,
        )?;
        link_program(&verified_program, vec![])
    }

    struct TestEbpfProgramContext32BitMapped {}

    impl EbpfProgramContext for TestEbpfProgramContext32BitMapped {
        type RunContext<'a> = ();

        type Packet<'a> = ();
        type Arg1<'a> = &'a TestArgument32BitMapped;
        type Arg2<'a> = ();
        type Arg3<'a> = ();
        type Arg4<'a> = ();
        type Arg5<'a> = ();

        type Map = NoMap;
    }

    empty_static_helper_set!(TestEbpfProgramContext32BitMapped);

    fn initialize_test_program_for_32bit_arg(
        code: Vec<EbpfInstruction>,
    ) -> Result<EbpfProgram<TestEbpfProgramContext32BitMapped>, EbpfError> {
        let verified_program = verify_program(
            code,
            CallingContext { args: vec![TEST_ARG_32_BIT_TYPE.clone()], ..Default::default() },
            &mut NullVerifierLogger,
        )?;
        link_program(&verified_program, vec![])
    }

    #[test]
    fn test_data_end() {
        let program = r#"
        mov %r0, 0
        ldxdw %r2, [%r1+16]
        ldxdw %r1, [%r1+8]
        # ensure data contains at least 8 bytes
        mov %r3, %r1
        add %r3, 0x8
        jgt %r3, %r2, +1
        # read 8 bytes from data
        ldxdw %r0, [%r1]
        exit
        "#;
        let program = initialize_test_program(parse_asm(program)).expect("load");

        let v = [42];
        let mut data = TestArgument::from_data(&v[..]);
        assert_eq!(program.run_with_1_argument(&mut (), &mut data), v[0]);
    }

    #[test]
    fn test_past_data_end() {
        let program = r#"
        mov %r0, 0
        ldxdw %r2, [%r1+16]
        ldxdw %r1, [%r1+6]
        # ensure data contains at least 4 bytes
        mov %r3, %r1
        add %r3, 0x4
        jgt %r3, %r2, +1
        # read 8 bytes from data
        ldxdw %r0, [%r1]
        exit
        "#;
        initialize_test_program(parse_asm(program)).expect_err("incorrect program");
    }

    #[test]
    fn test_mapping() {
        let program = r#"
          # Return `TestArgument32.mutable_field`
          ldxw %r0, [%r1+12]
          exit
        "#;
        let program = initialize_test_program_for_32bit_arg(parse_asm(program)).expect("load");

        let mut data = TestArgument32BitMapped(TestArgument::default());
        assert_eq!(program.run_with_1_argument(&mut (), &mut data), data.0.mutable_field as u64);
    }

    #[test]
    fn test_mapping_partial_load() {
        // Verify that we can access middle of a remapped scalar field.
        let program = r#"
          # Returns two upper bytes of `TestArgument32.mutable_filed`
          ldxh %r0, [%r1+14]
          exit
        "#;
        let program = initialize_test_program_for_32bit_arg(parse_asm(program)).expect("load");

        let mut data = TestArgument32BitMapped(TestArgument::default());
        data.0.mutable_field = 0x12345678;
        assert_eq!(program.run_with_1_argument(&mut (), &mut data), 0x1234 as u64);
    }

    #[test]
    fn test_mapping_ptr() {
        let program = r#"
        mov %r0, 0
        # Load data and data_end as 32 bits pointers in TestArgument32
        ldxw %r2, [%r1+8]
        ldxw %r1, [%r1+4]
        # ensure data contains at least 8 bytes
        mov %r3, %r1
        add %r3, 0x8
        jgt %r3, %r2, +1
        # read 8 bytes from data
        ldxdw %r0, [%r1]
        exit
        "#;
        let program = initialize_test_program_for_32bit_arg(parse_asm(program)).expect("load");

        let v = [42];
        let mut data = TestArgument32BitMapped(TestArgument::from_data(&v[..]));
        assert_eq!(program.run_with_1_argument(&mut (), &mut data), v[0]);
    }

    #[test]
    fn test_mapping_with_offset() {
        let program = r#"
        mov %r0, 0
        add %r1, 0x8
        # Load data and data_end as 32 bits pointers in TestArgument32
        ldxw %r2, [%r1]
        ldxw %r1, [%r1-4]
        # ensure data contains at least 8 bytes
        mov %r3, %r1
        add %r3, 0x8
        jgt %r3, %r2, +1
        # read 8 bytes from data
        ldxdw %r0, [%r1]
        exit
        "#;
        let program = initialize_test_program_for_32bit_arg(parse_asm(program)).expect("load");

        let v = [42];
        let mut data = TestArgument32BitMapped(TestArgument::from_data(&v[..]));
        assert_eq!(program.run_with_1_argument(&mut (), &mut data), v[0]);
    }

    #[test]
    fn test_ptr_diff() {
        let program = r#"
          mov %r0, %r1
          add %r0, 0x2
          # Substract 2 ptr to memory
          sub %r0, %r1

          mov %r2, %r10
          add %r2, 0x3
          # Substract 2 ptr to stack
          sub %r2, %r10
          add %r0, %r2

          ldxdw %r2, [%r1+16]
          ldxdw %r1, [%r1+8]
          # Substract ptr to array and ptr to array end
          sub %r2, %r1
          add %r0, %r2

          mov %r2, %r1
          add %r2, 0x4
          # Substract 2 ptr to array
          sub %r2, %r1
          add %r0, %r2

          exit
        "#;
        let code = parse_asm(program);

        let program = initialize_test_program(code).expect("load");

        let v = [42];
        let mut data = TestArgument::from_data(&v[..]);
        assert_eq!(program.run_with_1_argument(&mut (), &mut data), 17);
    }

    #[test]
    fn test_invalid_packet_load() {
        let program = r#"
        mov %r6, %r2
        mov %r0, 0
        ldpw
        exit
        "#;
        let args = vec![
            Type::PtrToMemory { id: MemoryId::new(), offset: 0.into(), buffer_size: 16 },
            Type::PtrToMemory { id: MemoryId::new(), offset: 0.into(), buffer_size: 16 },
        ];
        let verify_result = verify_program(
            parse_asm(program),
            CallingContext { args, ..Default::default() },
            &mut NullVerifierLogger,
        );

        assert_eq!(
            verify_result.expect_err("validation should fail"),
            EbpfError::ProgramVerifyError("at PC 2: R6 is not a packet".to_string())
        );
    }

    #[test]
    fn test_invalid_field_size() {
        // Load with a field size too large fails validation.
        let program = r#"
          ldxdw %r0, [%r1]
          exit
        "#;
        initialize_test_program(parse_asm(program)).expect_err("incorrect program");
    }

    #[test]
    fn test_unknown_field() {
        // Load outside of the know fields fails validation.
        let program = r#"
          ldxw %r0, [%r1 + 4]
          exit
        "#;
        initialize_test_program(parse_asm(program)).expect_err("incorrect program");
    }

    #[test]
    fn test_partial_ptr_field() {
        // Partial loads of ptr fields are not allowed.
        let program = r#"
          ldxw %r0, [%r1 + 8]
          exit
        "#;
        initialize_test_program(parse_asm(program)).expect_err("incorrect program");
    }

    #[test]
    fn test_readonly_field() {
        // Store to a read only field fails validation.
        let program = r#"
          stw [%r1], 0x42
          exit
        "#;
        initialize_test_program(parse_asm(program)).expect_err("incorrect program");
    }

    #[test]
    fn test_store_mutable_field() {
        // Store to a mutable field is allowed.
        let program = r#"
          stw [%r1 + 24], 0x42
          mov %r0, 1
          exit
        "#;
        let program = initialize_test_program(parse_asm(program)).expect("load");

        let mut data = TestArgument::default();
        assert_eq!(program.run_with_1_argument(&mut (), &mut data), 1);
        assert_eq!(data.mutable_field, 0x42);
    }

    #[test]
    fn test_fake_array_bounds_check() {
        // Verify that negative offsets in memory ptrs are handled properly and cannot be used to
        // bypass array bounds checks.
        let program = r#"
        mov %r0, 0
        ldxdw %r2, [%r1+16]
        ldxdw %r1, [%r1+8]
        # Subtract 8 from `data` and pretend checking array bounds.
        mov %r3, %r1
        sub %r3, 0x8
        jgt %r3, %r2, +1
        # Read 8 bytes from `data`. This should be rejected by the verifier.
        ldxdw %r0, [%r1]
        exit
        "#;
        initialize_test_program(parse_asm(program)).expect_err("incorrect program");
    }
}
