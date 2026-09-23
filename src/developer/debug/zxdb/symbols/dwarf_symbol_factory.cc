// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/dwarf_symbol_factory.h"

#include <algorithm>

#include <llvm/BinaryFormat/Dwarf.h>

#include "llvm/DebugInfo/DWARF/DWARFContext.h"
#include "llvm/DebugInfo/DWARF/DWARFDataExtractor.h"
#include "llvm/DebugInfo/DWARF/DWARFDebugInfoEntry.h"
#include "llvm/DebugInfo/DWARF/DWARFDebugLoc.h"
#include "llvm/DebugInfo/DWARF/DWARFSection.h"
#include "llvm/DebugInfo/DWARF/DWARFUnit.h"
#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/zxdb/symbols/array_type.h"
#include "src/developer/debug/zxdb/symbols/base_type.h"
#include "src/developer/debug/zxdb/symbols/call_site.h"
#include "src/developer/debug/zxdb/symbols/call_site_parameter.h"
#include "src/developer/debug/zxdb/symbols/code_block.h"
#include "src/developer/debug/zxdb/symbols/collection.h"
#include "src/developer/debug/zxdb/symbols/compile_unit.h"
#include "src/developer/debug/zxdb/symbols/data_member.h"
#include "src/developer/debug/zxdb/symbols/dwarf_abstract_child_iterator.h"
#include "src/developer/debug/zxdb/symbols/dwarf_binary_impl.h"
#include "src/developer/debug/zxdb/symbols/dwarf_die_decoder.h"
#include "src/developer/debug/zxdb/symbols/dwarf_location.h"
#include "src/developer/debug/zxdb/symbols/dwarf_unit_impl.h"
#include "src/developer/debug/zxdb/symbols/enumeration.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/function_type.h"
#include "src/developer/debug/zxdb/symbols/inherited_from.h"
#include "src/developer/debug/zxdb/symbols/member_ptr.h"
#include "src/developer/debug/zxdb/symbols/modified_type.h"
#include "src/developer/debug/zxdb/symbols/namespace.h"
#include "src/developer/debug/zxdb/symbols/symbol.h"
#include "src/developer/debug/zxdb/symbols/template_parameter.h"
#include "src/developer/debug/zxdb/symbols/variable.h"
#include "src/developer/debug/zxdb/symbols/variant.h"
#include "src/developer/debug/zxdb/symbols/variant_part.h"

namespace zxdb {

namespace {

// Appends the given (low, high) range to the vector if everything is valid and nonempty.
void AppendAddressRange(uint64_t low, uint64_t high, AddressRanges::RangeVector* ranges) {
  // A zero DW_AT_low_pc means the code is removed during the linking, either due to garbage
  // collection (of unused functions) or identical code folding. Functions inlined and not used
  // outside their compilation units will also get removed.
  if (low > 0 && low < high)
    ranges->emplace_back(low, high);
}

// Extracts a FileLine if possible from the given input. If the optional values aren't present, or
// are empty, returns an empty FileLine.
FileLine MakeFileLine(llvm::DWARFUnit* unit, const std::optional<std::string>& file,
                      const std::optional<uint64_t>& line, std::string compilation_dir) {
  if (compilation_dir.empty() && unit->getCompilationDir())
    compilation_dir = unit->getCompilationDir();
  if (file && !file->empty() && line && *line > 0)
    return FileLine(*file, compilation_dir, static_cast<int>(*line));
  return FileLine();
}

// Extracts the subrange size from an array subrange DIE. Returns the value on success, nullopt on
// failure.
std::optional<size_t> ReadArraySubrange(llvm::DWARFContext* context,
                                        const llvm::DWARFDie& subrange_die) {
  // Extract the DW_AT_count attribute which Clang generates, and DW_AT_upper_bound which GCC
  // generated.
  DwarfDieDecoder range_decoder(context);

  std::optional<uint64_t> count;
  range_decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_count, &count);

  std::optional<uint64_t> upper_bound;
  range_decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_upper_bound, &upper_bound);

  if (!range_decoder.Decode(subrange_die) || (!count && !upper_bound))
    return std::nullopt;

  if (count)
    return static_cast<size_t>(*count);
  return static_cast<size_t>(*upper_bound);
}

// Returns true if the form uses any of the addr "x" encodings (relocatable references to the
// address table).
bool IsAddrXForm(llvm::dwarf::Form form) {
  return form == llvm::dwarf::DW_FORM_addrx || form == llvm::dwarf::DW_FORM_addrx1 ||
         form == llvm::dwarf::DW_FORM_addrx2 || form == llvm::dwarf::DW_FORM_addrx3 ||
         form == llvm::dwarf::DW_FORM_addrx4 || form == llvm::dwarf::DW_FORM_GNU_addr_index;
}

DwarfDieRef GetDieRef(const llvm::DWARFDie& die) {
  if (!die.isValid())
    return DwarfDieRef();
  llvm::DWARFUnit* unit = die.getDwarfUnit();
  if (unit && unit->isTypeUnit() && unit->getVersion() < 5)
    return DwarfDieRef::Type(die.getOffset());
  return DwarfDieRef::Main(die.getOffset());
}

}  // namespace

DwarfSymbolFactory::DwarfSymbolFactory(fxl::WeakPtr<DwarfBinaryImpl> binary,
                                       fxl::WeakPtr<Delegate> delegate, FileType file_type)
    : binary_(std::move(binary)), delegate_(std::move(delegate)), file_type_(file_type) {}
DwarfSymbolFactory::~DwarfSymbolFactory() = default;

fxl::RefPtr<Symbol> DwarfSymbolFactory::CreateSymbol(DwarfDieRef die_ref) const {
  if (!binary_ || !delegate_ || !die_ref.is_valid())
    return fxl::MakeRefCounted<Symbol>();

  // LLVMContext::getDIEForOffset() only works for normal (non-DWO) units so we have to look up
  // manually. This is basically the implementation of getDIEForOffset() with the variable unit
  // type.
  const llvm::DWARFUnitVector& unit_vector = file_type_ == kDWO
                                                 ? GetLLVMContext()->getDWOUnitsVector()
                                                 : GetLLVMContext()->getNormalUnitsVector();
  llvm::DWARFUnit* unit = GetUnitForOffset(unit_vector, die_ref);
  if (!unit)
    return fxl::MakeRefCounted<Symbol>();

  llvm::DWARFDie die = unit->getDIEForOffset(die_ref.offset());

  if (!die.isValid())
    return fxl::MakeRefCounted<Symbol>();

  return DecodeSymbol(die);
}

llvm::DWARFContext* DwarfSymbolFactory::GetLLVMContext() const {
  if (binary_) {
    return binary_->GetLLVMContext();
  }
  return nullptr;
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeSymbol(const llvm::DWARFDie& die) const {
  DwarfTag tag = static_cast<DwarfTag>(die.getTag());
  if (DwarfTagIsTypeModifier(tag))
    return DecodeModifiedType(die);

  fxl::RefPtr<Symbol> symbol;
  switch (tag) {
    case DwarfTag::kArrayType:
      symbol = DecodeArrayType(die);
      break;
    case DwarfTag::kBaseType:
      symbol = DecodeBaseType(die);
      break;
    case DwarfTag::kCallSite:
      symbol = DecodeCallSite(die);
      break;
    case DwarfTag::kCallSiteParameter:
      symbol = DecodeCallSiteParameter(die);
      break;
    case DwarfTag::kCompileUnit:
    case DwarfTag::kSkeletonUnit:
    case DwarfTag::kTypeUnit:
      symbol = DecodeCompileUnit(die, tag);
      break;
    case DwarfTag::kEnumerationType:
      symbol = DecodeEnum(die);
      break;
    case DwarfTag::kFormalParameter:
    case DwarfTag::kVariable:
      symbol = DecodeVariable(die);
      break;
    case DwarfTag::kSubroutineType:
      symbol = DecodeFunctionType(die);
      break;
    case DwarfTag::kImportedDeclaration:
      symbol = DecodeImportedDeclaration(die);
      break;
    case DwarfTag::kInheritance:
      symbol = DecodeInheritedFrom(die);
      break;
    case DwarfTag::kLexicalBlock:
      symbol = DecodeLexicalBlock(die);
      break;
    case DwarfTag::kMember:
      symbol = DecodeDataMember(die);
      break;
    case DwarfTag::kNamespace:
      symbol = DecodeNamespace(die);
      break;
    case DwarfTag::kPtrToMemberType:
      symbol = DecodeMemberPtr(die);
      break;
    case DwarfTag::kInlinedSubroutine:
    case DwarfTag::kSubprogram:
      symbol = DecodeFunction(die, tag);
      break;
    case DwarfTag::kStructureType:
    case DwarfTag::kClassType:
    case DwarfTag::kUnionType:
      symbol = DecodeCollection(die);
      break;
    case DwarfTag::kTemplateTypeParameter:
    case DwarfTag::kTemplateValueParameter:
      symbol = DecodeTemplateParameter(die, tag);
      break;
    case DwarfTag::kVariantPart:
      symbol = DecodeVariantPart(die);
      break;
    case DwarfTag::kVariant:
      symbol = DecodeVariant(die);
      break;
    case DwarfTag::kUnspecifiedType:
      symbol = DecodeUnspecifiedType(die);
      break;
    default:
      // All unhandled Tag types get a Symbol that has the correct tag, but no other data.
      symbol = fxl::MakeRefCounted<Symbol>(static_cast<DwarfTag>(die.getTag()));
  }

  symbol->set_lazy_this(MakeUncachedLazy(die));

  // Set the parent block if it hasn't been set already by the type-specific factory. In particular,
  // we want the function/variable specification's parent block if there was a specification since
  // it will contain the namespace and class stuff.
  if (!symbol->parent()) {
    llvm::DWARFDie parent = die.getParent();
    if (parent)
      symbol->set_parent(MakeUncachedLazy(parent));
  }

  return symbol;
}

LazySymbol DwarfSymbolFactory::MakeLazy(const llvm::DWARFDie& die) const {
  return MakeLazy(GetDieRef(die));
}

UncachedLazySymbol DwarfSymbolFactory::MakeUncachedLazy(const llvm::DWARFDie& die) const {
  return MakeUncachedLazy(GetDieRef(die));
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeFunction(const llvm::DWARFDie& die, DwarfTag tag,
                                                       bool is_specification) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  llvm::DWARFDie parent;
  decoder.AddAbstractParent(&parent);

  llvm::DWARFDie specification;
  decoder.AddReference(llvm::dwarf::DW_AT_specification, &specification);

  std::optional<const char*> name;
  decoder.AddCString(llvm::dwarf::DW_AT_name, &name);

  std::optional<const char*> linkage_name;
  decoder.AddCString(llvm::dwarf::DW_AT_linkage_name, &linkage_name);

  llvm::DWARFDie return_type;
  decoder.AddReference(llvm::dwarf::DW_AT_type, &return_type);

  // Declaration location.
  std::optional<std::string> decl_file;
  std::optional<uint64_t> decl_line;
  decoder.AddFile(llvm::dwarf::DW_AT_decl_file, &decl_file);
  decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_decl_line, &decl_line);

  // Call location (inline functions only).
  std::optional<std::string> call_file;
  std::optional<uint64_t> call_line;
  if (tag == DwarfTag::kInlinedSubroutine) {
    decoder.AddFile(llvm::dwarf::DW_AT_call_file, &call_file);
    decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_call_line, &call_line);
  }

  VariableLocation frame_base;
  decoder.AddCustom(llvm::dwarf::DW_AT_frame_base,
                    [&frame_base, source = MakeUncachedLazy(die)](
                        llvm::DWARFUnit* unit, const llvm::DWARFFormValue& value) {
                      frame_base = DecodeVariableLocation(unit, value, source);
                    });

  llvm::DWARFDie object_ptr;
  decoder.AddReference(llvm::dwarf::DW_AT_object_pointer, &object_ptr);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  fxl::RefPtr<Function> function;

  // If this DIE has a link to a function specification (and we haven't already followed such a
  // link), first read that in to get things like the mangled name, parent context, and declaration
  // locations. Then we'll overlay our values on that object.
  if (!is_specification && specification) {
    auto spec = DecodeFunction(specification, tag, true);
    Function* spec_function = spec->As<Function>();
    // If the specification is invalid, just ignore it and read out the values that we can find in
    // this DIE. An empty one will be created below.
    if (spec_function)
      function = fxl::RefPtr<Function>(spec_function);
  }
  if (!function)
    function = fxl::MakeRefCounted<Function>(tag);

  if (name)
    function->set_assigned_name(*name);
  if (linkage_name)
    function->set_linkage_name(*linkage_name);
  function->set_code_ranges(GetCodeRanges(die));
  if (decl_file) {
    function->set_decl_line(MakeFileLine(die.getDwarfUnit(), decl_file, decl_line,
                                         delegate_->GetBuildDirForSymbolFactory()));
  }
  function->set_call_line(MakeFileLine(die.getDwarfUnit(), call_file, call_line,
                                       delegate_->GetBuildDirForSymbolFactory()));
  if (return_type)
    function->set_return_type(MakeLazy(return_type));
  function->set_frame_base(std::move(frame_base));
  if (object_ptr)
    function->set_object_pointer(MakeLazy(object_ptr));

  // Handle sub-DIEs: parameters, child blocks, and variables.
  //
  // Note: this partially duplicates a similar loop in DecodeLexicalBlock().
  std::vector<LazySymbol> parameters;
  std::vector<LazySymbol> inner_blocks;
  std::vector<LazySymbol> variables;
  std::vector<LazySymbol> template_params;
  std::vector<LazySymbol> call_sites;
  for (const llvm::DWARFDie& child : DwarfAbstractChildIterator(die)) {
    switch (child.getTag()) {
      case llvm::dwarf::DW_TAG_formal_parameter:
        parameters.push_back(MakeLazy(child));
        break;
      case llvm::dwarf::DW_TAG_variable:
        variables.push_back(MakeLazy(child));
        break;
      case llvm::dwarf::DW_TAG_inlined_subroutine:
      case llvm::dwarf::DW_TAG_lexical_block:
        inner_blocks.push_back(MakeLazy(child));
        break;
      case llvm::dwarf::DW_TAG_template_type_parameter:
      case llvm::dwarf::DW_TAG_template_value_parameter:
        template_params.push_back(MakeLazy(child));
        break;
      case llvm::dwarf::DW_TAG_call_site:
        call_sites.push_back(MakeLazy(child));
        break;
      default:
        break;  // Skip everything else.
    }
  }
  function->set_parameters(std::move(parameters));
  function->set_inner_blocks(std::move(inner_blocks));
  function->set_variables(std::move(variables));
  function->set_template_params(std::move(template_params));
  function->set_call_sites(std::move(call_sites));

  if (parent && !function->parent()) {
    // Set the parent symbol when it hasn't already been set. We always want the specification's
    // parent instead of the implementation block's parent (if they're different) because the
    // namespace and enclosing class information comes from the declaration.
    //
    // If this is already set, it means we recursively followed the specification which already set
    // it.
    function->set_parent(MakeUncachedLazy(parent));
  }

  if (tag == DwarfTag::kInlinedSubroutine) {
    // In contrast to the logic for parent() above, the direct containing block of the inlined
    // subroutine will save the CodeBlock inlined functions are embedded in.
    llvm::DWARFDie direct_parent = die.getParent();
    if (direct_parent)
      function->set_containing_block(MakeUncachedLazy(direct_parent));
  }

  return function;
}

// We expect array types to have two things:
// - An attribute linking to the underlying type of the array.
// - One or more DW_TAG_subrange_type children that hold the size of the array in a DW_AT_count
//   attribute.
//
// The subrange child is weird because the subrange links to its own type.  LLVM generates a
// synthetic type __ARRAY_SIZE_TYPE__ that the DW_TAG_subrange_count DIE references from DW_AT_type
// attribute. We ignore this and only use the count.
//
// One might expect 2-dimensional arrays to be expressed as an array of one dimension where the
// contained type is an array of another. But both Clang and GCC generate one array entry with two
// subrange children. The order of these represents the declaration order in the code.
fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeArrayType(const llvm::DWARFDie& die) const {
  // Extract the type attribute from the root DIE (should be a DW_TAG_array_type).
  DwarfDieDecoder array_decoder(GetLLVMContext());
  llvm::DWARFDie type;
  array_decoder.AddReference(llvm::dwarf::DW_AT_type, &type);
  if (!array_decoder.Decode(die) || !type)
    return fxl::MakeRefCounted<Symbol>();

  // Need the concrete symbol for the contained type for the array constructor.
  auto contained = DecodeSymbol(type);
  if (!contained)
    return fxl::MakeRefCounted<Symbol>();
  Type* contained_type = contained->As<Type>();
  if (!contained_type)
    return fxl::MakeRefCounted<Symbol>();

  // Find all subranges stored in the declaration order. More than one means a multi-dimensional
  // array.
  std::vector<std::optional<size_t>> subrange_sizes;
  for (const llvm::DWARFDie& child : die) {
    if (child.getTag() == llvm::dwarf::DW_TAG_subrange_type)
      subrange_sizes.push_back(ReadArraySubrange(GetLLVMContext(), child));
  }

  // Require a subrange with a count in it. If we find cases where this isn't the case, we could add
  // support for array types with unknown lengths, but currently ArrayType requires a size.
  if (subrange_sizes.empty())
    return fxl::MakeRefCounted<Symbol>();

  // Work backwards in the array dimensions generating nested array definitions. The innermost
  // definition refers to the contained type.
  fxl::RefPtr<Type> cur(contained_type);
  for (int i = static_cast<int>(subrange_sizes.size()) - 1; i >= 0; i--) {
    auto new_array = fxl::MakeRefCounted<ArrayType>(std::move(cur), subrange_sizes[i]);
    cur = std::move(new_array);
  }
  return cur;
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeBaseType(const llvm::DWARFDie& die) const {
  // This object and its setup could be cached for better performance.
  DwarfDieDecoder decoder(GetLLVMContext());

  // Types must always use the parent of the abstract origin (if it exists) so they can be nested in
  // the correct namespace.
  llvm::DWARFDie parent;
  decoder.AddAbstractParent(&parent);

  std::optional<const char*> name;
  decoder.AddCString(llvm::dwarf::DW_AT_name, &name);

  std::optional<uint64_t> encoding;
  decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_encoding, &encoding);

  std::optional<uint64_t> byte_size;
  decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_byte_size, &byte_size);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  auto base_type = fxl::MakeRefCounted<BaseType>();
  if (name)
    base_type->set_assigned_name(*name);
  if (encoding)
    base_type->set_base_type(static_cast<int>(*encoding));
  if (byte_size)
    base_type->set_byte_size(static_cast<uint32_t>(*byte_size));

  if (parent)
    base_type->set_parent(MakeUncachedLazy(parent));

  return base_type;
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeCallSite(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  std::optional<uint64_t> return_pc;
  decoder.AddAddress(llvm::dwarf::DW_AT_call_return_pc, &return_pc);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  // Decode parameters.
  std::vector<LazySymbol> parameters;
  for (const llvm::DWARFDie& child : die) {
    if (child.getTag() == llvm::dwarf::DW_TAG_call_site_parameter)
      parameters.push_back(MakeLazy(child));
  }

  return fxl::MakeRefCounted<CallSite>(return_pc, std::move(parameters));
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeCallSiteParameter(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  // We currently assume that the location and value for call site parameters are always blocks
  // having inline location expressions. Technically, these are location expressions that could
  // be an offset to a location list, but our current supported compilers don't express it this way,
  // and there would seem to no point in doing so for this case.

  std::optional<std::vector<uint8_t>> location;
  decoder.AddBlock(llvm::dwarf::DW_AT_location, &location);

  std::optional<std::vector<uint8_t>> value;
  decoder.AddBlock(llvm::dwarf::DW_AT_call_value, &value);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  // Decode the register number. We currently expect the location to be exactly one DWARF opcode
  // indicating the register (see call_site_parameter.h for more).
  std::optional<uint32_t> register_num;
  if (location && location->size() == 1) {
    uint8_t location_op = location->at(0);
    if (location_op >= llvm::dwarf::DW_OP_reg0 && location_op <= llvm::dwarf::DW_OP_reg31)
      register_num = location_op - llvm::dwarf::DW_OP_reg0;
  }

  // Convert a nonexistent value to one with an empty block. These are equivalent for our needs.
  if (!value)
    value.emplace();

  return fxl::MakeRefCounted<CallSiteParameter>(
      register_num, DwarfExpr(std::move(*value), MakeUncachedLazy(die)));
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeCollection(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  // Types must always use the parent of the abstract origin (if it exists) so they can be nested in
  // the correct namespace.
  llvm::DWARFDie parent;
  decoder.AddAbstractParent(&parent);

  std::optional<uint64_t> signature;
  decoder.AddCustom(llvm::dwarf::DW_AT_signature,
                    [&signature](llvm::DWARFUnit*, const llvm::DWARFFormValue& val) {
                      signature = val.getAsReferenceUVal();
                    });

  std::optional<const char*> name;
  decoder.AddCString(llvm::dwarf::DW_AT_name, &name);

  std::optional<uint64_t> byte_size;
  decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_byte_size, &byte_size);

  std::optional<bool> is_declaration;
  decoder.AddBool(llvm::dwarf::DW_AT_declaration, &is_declaration);

  std::optional<uint64_t> calling_convention;
  decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_calling_convention, &calling_convention);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  if (signature) {
    if (binary_) {
      if (DwarfDieRef sig_ref = binary_->GetDieRefForSignature(*signature)) {
        return CreateSymbol(sig_ref);
      }
    }
    return fxl::MakeRefCounted<Symbol>();
  }

  auto result = fxl::MakeRefCounted<Collection>(static_cast<DwarfTag>(die.getTag()));
  if (name)
    result->set_assigned_name(*name);
  if (byte_size)
    result->set_byte_size(static_cast<uint32_t>(*byte_size));

  // Handle sub-DIEs: data members and inheritance.
  std::vector<LazySymbol> data;
  std::vector<LazySymbol> inheritance;
  std::vector<LazySymbol> template_params;
  LazySymbol variant_part;
  for (const llvm::DWARFDie& child : die) {
    switch (child.getTag()) {
      case llvm::dwarf::DW_TAG_inheritance:
        inheritance.push_back(MakeLazy(child));
        break;
      case llvm::dwarf::DW_TAG_member:
        data.push_back(MakeLazy(child));
        break;
      case llvm::dwarf::DW_TAG_variant_part:
        // Currently we only support one variant_part per struct. This could be expanded to a vector
        // if a compiler generates such a structure.
        variant_part = MakeLazy(child);
        break;
      case llvm::dwarf::DW_TAG_template_type_parameter:
      case llvm::dwarf::DW_TAG_template_value_parameter:
        template_params.push_back(MakeLazy(child));
        break;
      default:
        break;  // Skip everything else.
    }
  }
  result->set_data_members(std::move(data));
  result->set_inherited_from(std::move(inheritance));
  result->set_template_params(std::move(template_params));
  result->set_variant_part(variant_part);
  if (is_declaration)
    result->set_is_declaration(*is_declaration);

  if (calling_convention) {
    if (*calling_convention == Collection::kNormalCall ||
        *calling_convention == Collection::kPassByReference ||
        *calling_convention == Collection::kPassByValue) {
      result->set_calling_convention(
          static_cast<Collection::CallingConvention>(*calling_convention));
    }
  }

  if (parent)
    result->set_parent(MakeUncachedLazy(parent));

  return result;
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeCompileUnit(const llvm::DWARFDie& die,
                                                          DwarfTag tag) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  std::optional<const char*> name;
  decoder.AddCString(llvm::dwarf::DW_AT_name, &name);

  std::optional<uint64_t> language;
  decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_language, &language);

  std::optional<uint64_t> addr_base;
  decoder.AddSectionOffset(llvm::dwarf::DW_AT_addr_base, &addr_base);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  std::string name_str;
  if (name)
    name_str = *name;

  DwarfLang lang_enum = DwarfLang::kNone;
  if (language && *language >= 0 && *language < static_cast<int>(DwarfLang::kLast))
    lang_enum = static_cast<DwarfLang>(*language);

  // The weak "binary_" pointer should have already been checked for null by CreateSymbol() so we
  // can dereference unconditionally.
  fxl::RefPtr<DwarfUnit> dwarf_unit =
      fxl::MakeRefCounted<DwarfUnitImpl>(binary_.get(), die.getDwarfUnit());

  // We know the delegate_ is valid, that was checked on entry to CreateSymbol().
  return fxl::MakeRefCounted<CompileUnit>(tag, delegate_->GetModuleSymbols(), std::move(dwarf_unit),
                                          lang_enum, std::move(name_str), addr_base);
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeDataMember(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  std::optional<const char*> name;
  decoder.AddCString(llvm::dwarf::DW_AT_name, &name);

  llvm::DWARFDie type;
  decoder.AddReference(llvm::dwarf::DW_AT_type, &type);

  std::optional<bool> artificial;
  decoder.AddBool(llvm::dwarf::DW_AT_artificial, &artificial);

  std::optional<bool> external;
  decoder.AddBool(llvm::dwarf::DW_AT_external, &external);

  std::optional<uint64_t> member_offset;
  decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_data_member_location, &member_offset);

  std::optional<uint64_t> byte_size;
  decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_byte_size, &byte_size);

  std::optional<uint64_t> bit_size;
  decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_bit_size, &bit_size);
  std::optional<int64_t> bit_offset;
  decoder.AddSignedConstant(llvm::dwarf::DW_AT_bit_offset, &bit_offset);
  std::optional<uint64_t> data_bit_offset;
  decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_data_bit_offset, &data_bit_offset);

  ConstValue const_value;
  decoder.AddConstValue(llvm::dwarf::DW_AT_const_value, &const_value);

  // These are set for Rust generators.
  std::optional<std::string> decl_file;
  std::optional<uint64_t> decl_line;
  decoder.AddFile(llvm::dwarf::DW_AT_decl_file, &decl_file);
  decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_decl_line, &decl_line);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  auto result = fxl::MakeRefCounted<DataMember>();
  if (name)
    result->set_assigned_name(*name);
  if (type)
    result->set_type(MakeLazy(type));
  if (artificial)
    result->set_artificial(*artificial);
  if (external)
    result->set_is_external(*external);
  if (member_offset)
    result->set_member_location(static_cast<uint32_t>(*member_offset));
  if (byte_size)
    result->set_byte_size(static_cast<uint32_t>(*byte_size));
  if (bit_offset)
    result->set_bit_offset(static_cast<int32_t>(*bit_offset));
  if (bit_size)
    result->set_bit_size(static_cast<uint32_t>(*bit_size));
  if (data_bit_offset)
    result->set_data_bit_offset(static_cast<uint32_t>(*data_bit_offset));
  result->set_const_value(std::move(const_value));
  if (decl_file) {
    result->set_decl_line(MakeFileLine(die.getDwarfUnit(), decl_file, decl_line,
                                       delegate_->GetBuildDirForSymbolFactory()));
  }
  return result;
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeEnum(const llvm::DWARFDie& die) const {
  DwarfDieDecoder main_decoder(GetLLVMContext());

  // Types must always use the parent of the abstract origin (if it exists) so they can be nested in
  // the correct namespace.
  llvm::DWARFDie parent;
  main_decoder.AddAbstractParent(&parent);

  std::optional<uint64_t> signature;
  main_decoder.AddCustom(llvm::dwarf::DW_AT_signature,
                         [&signature](llvm::DWARFUnit*, const llvm::DWARFFormValue& val) {
                           signature = val.getAsReferenceUVal();
                         });

  // Name is optional (enums can be anonymous).
  std::optional<const char*> type_name;
  main_decoder.AddCString(llvm::dwarf::DW_AT_name, &type_name);

  std::optional<uint64_t> byte_size;
  main_decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_byte_size, &byte_size);

  std::optional<bool> is_declaration;
  main_decoder.AddBool(llvm::dwarf::DW_AT_declaration, &is_declaration);

  // The type is optional for an enumeration.
  llvm::DWARFDie type;
  main_decoder.AddReference(llvm::dwarf::DW_AT_type, &type);

  // For decoding the individual enum values.
  DwarfDieDecoder enumerator_decoder(GetLLVMContext());

  std::optional<const char*> enumerator_name;
  enumerator_decoder.AddCString(llvm::dwarf::DW_AT_name, &enumerator_name);

  // Enum values can be signed or unsigned. This is determined by looking at the form of the storage
  // for the underlying types. Since there are many values, we set the "signed" flag if any of them
  // were signed, since a small positive integer could be represented either way but a signed value
  // must be encoded differently.
  //
  // This could be enhanced by using ConstValues directly. See the enumeration header file for more.
  std::optional<uint64_t> enumerator_value;
  bool is_signed = false;
  enumerator_decoder.AddCustom(
      llvm::dwarf::DW_AT_const_value,
      [&enumerator_value, &is_signed](llvm::DWARFUnit*, const llvm::DWARFFormValue& value) {
        if (value.getForm() == llvm::dwarf::DW_FORM_udata) {
          enumerator_value = value.getAsUnsignedConstant();
        } else if (value.getForm() == llvm::dwarf::DW_FORM_sdata) {
          // Cast signed values to unsigned.
          if (auto signed_value = value.getAsSignedConstant()) {
            is_signed = true;
            enumerator_value = static_cast<uint64_t>(*signed_value);
          }
          // Else case is corrupted symbols or an unsupported format, just
          // ignore this one.
        }
      });

  if (!main_decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  if (signature) {
    if (binary_) {
      if (DwarfDieRef sig_ref = binary_->GetDieRefForSignature(*signature)) {
        return CreateSymbol(sig_ref);
      }
    }
    return fxl::MakeRefCounted<Symbol>();
  }

  FX_CHECK(byte_size.has_value());

  Enumeration::Map map;
  for (const llvm::DWARFDie& child : die) {
    if (child.getTag() != llvm::dwarf::DW_TAG_enumerator)
      continue;

    enumerator_name.reset();
    enumerator_value.reset();
    if (!enumerator_decoder.Decode(child))
      continue;
    if (enumerator_name && enumerator_value)
      map[*enumerator_value] = std::string(*enumerator_name);
  }

  LazySymbol lazy_type;
  if (type)
    lazy_type = MakeLazy(type);
  const char* type_name_str = type_name ? *type_name : "";
  auto result = fxl::MakeRefCounted<Enumeration>(type_name_str, std::move(lazy_type), *byte_size,
                                                 is_signed, std::move(map));
  if (parent)
    result->set_parent(MakeUncachedLazy(parent));
  if (is_declaration)
    result->set_is_declaration(*is_declaration);
  return result;
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeFunctionType(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  // Types must always use the parent of the abstract origin (if it exists) so they can be nested in
  // the correct namespace.
  llvm::DWARFDie parent;
  decoder.AddAbstractParent(&parent);

  llvm::DWARFDie return_type;
  decoder.AddReference(llvm::dwarf::DW_AT_type, &return_type);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  // Handle sub-DIEs (this only has parameters).
  std::vector<LazySymbol> parameters;
  for (const llvm::DWARFDie& child : die) {
    switch (child.getTag()) {
      case llvm::dwarf::DW_TAG_formal_parameter:
        parameters.push_back(MakeLazy(child));
        break;
      default:
        break;  // Skip everything else.
    }
  }

  LazySymbol lazy_return_type;
  if (return_type)
    lazy_return_type = MakeLazy(return_type);

  auto function = fxl::MakeRefCounted<FunctionType>(lazy_return_type, std::move(parameters));
  if (parent)
    function->set_parent(MakeUncachedLazy(parent));
  return function;
}

// Imported declarations are "using" statements that don't provide a new name like
// "using std::vector;".
//
// Type renames like "using Foo = std::vector;" is encoded as a typedef.
fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeImportedDeclaration(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  llvm::DWARFDie imported;
  decoder.AddReference(llvm::dwarf::DW_AT_import, &imported);

  if (!decoder.Decode(die) || !imported)
    return fxl::MakeRefCounted<Symbol>();

  return fxl::MakeRefCounted<ModifiedType>(DwarfTag::kImportedDeclaration, MakeLazy(imported));
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeInheritedFrom(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  llvm::DWARFDie type;
  decoder.AddReference(llvm::dwarf::DW_AT_type, &type);

  // The DW_AT_data_member_location can either be a constant or an expression.
  std::optional<uint64_t> member_offset;
  std::vector<uint8_t> offset_expression;
  decoder.AddCustom(llvm::dwarf::DW_AT_data_member_location,
                    [&member_offset, &offset_expression](llvm::DWARFUnit* unit,
                                                         const llvm::DWARFFormValue& form) {
                      if (form.isFormClass(llvm::DWARFFormValue::FC_Exprloc)) {
                        // Location expression.
                        llvm::ArrayRef<uint8_t> block = *form.getAsBlock();
                        offset_expression.assign(block.data(), block.data() + block.size());
                      } else if (form.isFormClass(llvm::DWARFFormValue::FC_Constant)) {
                        // Constant value.
                        member_offset = form.getAsUnsignedConstant();
                      }
                      // Otherwise leave both empty.
                    });

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  LazySymbol lazy_type;
  if (type)
    lazy_type = MakeLazy(type);

  if (member_offset)
    return fxl::MakeRefCounted<InheritedFrom>(std::move(lazy_type), *member_offset);
  if (!offset_expression.empty()) {
    return fxl::MakeRefCounted<InheritedFrom>(std::move(lazy_type),
                                              DwarfExpr(std::move(offset_expression)));
  }
  return fxl::MakeRefCounted<Symbol>();  // Missing location.
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeLexicalBlock(const llvm::DWARFDie& die) const {
  auto block = fxl::MakeRefCounted<CodeBlock>(DwarfTag::kLexicalBlock);
  block->set_code_ranges(GetCodeRanges(die));

  // Handle sub-DIEs: child blocks and variables.
  //
  // Note: this partially duplicates a similar loop in DecodeFunction().
  std::vector<LazySymbol> inner_blocks;
  std::vector<LazySymbol> variables;
  std::vector<LazySymbol> call_sites;
  for (const llvm::DWARFDie& child : die) {
    switch (child.getTag()) {
      case llvm::dwarf::DW_TAG_variable:
        variables.push_back(MakeLazy(child));
        break;
      case llvm::dwarf::DW_TAG_inlined_subroutine:
      case llvm::dwarf::DW_TAG_lexical_block:
        inner_blocks.push_back(MakeLazy(child));
        break;
      case llvm::dwarf::DW_TAG_call_site:
        call_sites.push_back(MakeLazy(child));
        break;
      default:
        break;  // Skip everything else.
    }
  }
  block->set_inner_blocks(std::move(inner_blocks));
  block->set_variables(std::move(variables));
  block->set_call_sites(std::move(call_sites));

  return block;
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeMemberPtr(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  llvm::DWARFDie container_type;
  decoder.AddReference(llvm::dwarf::DW_AT_containing_type, &container_type);
  llvm::DWARFDie type;
  decoder.AddReference(llvm::dwarf::DW_AT_type, &type);

  if (!decoder.Decode(die) || !container_type || !type)
    return fxl::MakeRefCounted<Symbol>();

  auto member_ptr = fxl::MakeRefCounted<MemberPtr>(MakeLazy(container_type), MakeLazy(type));
  return member_ptr;
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeModifiedType(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  // Types must always use the parent of the abstract origin (if it exists) so they can be nested in
  // the correct namespace.
  llvm::DWARFDie parent;
  decoder.AddAbstractParent(&parent);

  std::optional<const char*> name;
  decoder.AddCString(llvm::dwarf::DW_AT_name, &name);

  llvm::DWARFDie modified;
  decoder.AddReference(llvm::dwarf::DW_AT_type, &modified);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  // Modified type may be null for "void*".
  LazySymbol lazy_modified;
  if (modified)
    lazy_modified = MakeLazy(modified);

  auto result = fxl::MakeRefCounted<ModifiedType>(static_cast<DwarfTag>(die.getTag()),
                                                  std::move(lazy_modified));
  if (name)
    result->set_assigned_name(*name);

  result->set_lazy_this(MakeUncachedLazy(die));
  if (parent)
    result->set_parent(MakeUncachedLazy(parent));

  return result;
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeNamespace(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  // Types must always use the parent of the abstract origin (if it exists) so they can be nested in
  // the correct namespace.
  llvm::DWARFDie parent;
  decoder.AddAbstractParent(&parent);

  std::optional<const char*> name;
  decoder.AddCString(llvm::dwarf::DW_AT_name, &name);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  auto result = fxl::MakeRefCounted<Namespace>();
  if (name)
    result->set_assigned_name(*name);

  if (parent)
    result->set_parent(MakeUncachedLazy(parent));
  return result;
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeTemplateParameter(const llvm::DWARFDie& die,
                                                                DwarfTag tag) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  std::optional<const char*> name;
  decoder.AddCString(llvm::dwarf::DW_AT_name, &name);

  llvm::DWARFDie type;
  decoder.AddReference(llvm::dwarf::DW_AT_type, &type);

  // Account for DW_TAG_template_value_parameter. This will change the behavior to print the value
  // of the template parameter for things like template <int i> struct Foo {};.
  ConstValue const_value;
  decoder.AddConstValue(llvm::dwarf::DW_AT_const_value, &const_value);

  if (!decoder.Decode(die) || !type)
    return fxl::MakeRefCounted<Symbol>();

  std::string name_str = name ? *name : "";
  fxl::RefPtr<TemplateParameter> result = fxl::MakeRefCounted<TemplateParameter>(
      name_str, MakeLazy(type), tag == DwarfTag::kTemplateValueParameter);

  result->set_const_value(std::move(const_value));

  return result;
}

// Clang and GCC use "unspecified" types to encode "decltype(nullptr)". When used as a variable this
// appears as a pointer with 0 value, despite not having any declared size in the symbols.
// Therefore, we make up a byte size equal to the pointer size (8 bytes on our 64-bit systems).
fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeUnspecifiedType(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  // Types must always use the parent of the abstract origin (if it exists) so
  // they can be nested in the correct namespace.
  llvm::DWARFDie parent;
  decoder.AddAbstractParent(&parent);

  std::optional<const char*> name;
  decoder.AddCString(llvm::dwarf::DW_AT_name, &name);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  auto result = fxl::MakeRefCounted<Type>(DwarfTag::kUnspecifiedType);
  if (name)
    result->set_assigned_name(*name);
  result->set_byte_size(8);  // Assume pointer.
  if (parent)
    result->set_parent(MakeUncachedLazy(parent));

  return result;
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeVariable(const llvm::DWARFDie& die,
                                                       bool is_specification) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  llvm::DWARFDie specification;
  decoder.AddReference(llvm::dwarf::DW_AT_specification, &specification);

  std::optional<const char*> name;
  decoder.AddCString(llvm::dwarf::DW_AT_name, &name);

  VariableLocation location;
  decoder.AddCustom(llvm::dwarf::DW_AT_location,
                    [&location, source = MakeUncachedLazy(die)](llvm::DWARFUnit* unit,
                                                                const llvm::DWARFFormValue& value) {
                      location = DecodeVariableLocation(unit, value, source);
                    });

  llvm::DWARFDie type;
  decoder.AddReference(llvm::dwarf::DW_AT_type, &type);

  std::optional<bool> external;
  decoder.AddBool(llvm::dwarf::DW_AT_external, &external);

  std::optional<bool> artificial;
  decoder.AddBool(llvm::dwarf::DW_AT_artificial, &artificial);

  ConstValue const_value;
  decoder.AddConstValue(llvm::dwarf::DW_AT_const_value, &const_value);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  fxl::RefPtr<Variable> variable;

  // If this DIE has a link to a specification (and we haven't already followed such a link), first
  // read that in to get things like the mangled name, parent context, and declaration locations.
  // Then we'll overlay our values on that object.
  if (!is_specification && specification) {
    auto spec = DecodeVariable(specification, true);
    Variable* spec_variable = spec->As<Variable>();
    // If the specification is invalid, just ignore it and read out the values
    // that we can find in this DIE. An empty one will be created below.
    if (spec_variable)
      variable = fxl::RefPtr<Variable>(spec_variable);
  }
  if (!variable) {
    variable = fxl::MakeRefCounted<Variable>(static_cast<DwarfTag>(die.getTag()));
  }

  if (name)
    variable->set_assigned_name(*name);
  if (type)
    variable->set_type(MakeLazy(type));
  if (external)
    variable->set_is_external(*external);
  if (artificial)
    variable->set_artificial(*artificial);
  variable->set_location(std::move(location));
  variable->set_const_value(std::move(const_value));

  if (!variable->parent()) {
    // Set the parent symbol when it hasn't already been set. As with functions, we always want the
    // specification's parent. See DecodeFunction().
    llvm::DWARFDie parent = die.getParent();
    if (parent)
      variable->set_parent(MakeUncachedLazy(parent));
  }
  return variable;
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeVariant(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  // Assume unsigned discriminant values since this is always true for our current uses. See
  // Variant::discr_value() comment for more.
  std::optional<uint64_t> discr_value;
  decoder.AddUnsignedConstant(llvm::dwarf::DW_AT_discr_value, &discr_value);

  if (!decoder.Decode(die))
    return fxl::MakeRefCounted<Symbol>();

  // Collect the data members.
  std::vector<LazySymbol> members;
  for (const llvm::DWARFDie& child : die) {
    if (child.getTag() == llvm::dwarf::DW_TAG_member)
      members.push_back(MakeLazy(child));
  }

  // Convert from LLVM-style to C++-style optional.
  std::optional<uint64_t> discr;
  if (discr_value)
    discr = *discr_value;

  return fxl::MakeRefCounted<Variant>(discr, std::move(members));
}

fxl::RefPtr<Symbol> DwarfSymbolFactory::DecodeVariantPart(const llvm::DWARFDie& die) const {
  DwarfDieDecoder decoder(GetLLVMContext());

  // The discriminant is the DataMember in the variant whose value indicates which variant currently
  // applies.
  llvm::DWARFDie discriminant;
  decoder.AddReference(llvm::dwarf::DW_AT_discr, &discriminant);

  if (!decoder.Decode(die) || !discriminant)
    return fxl::MakeRefCounted<Symbol>();

  // Look for variants in this variant_part. It will also have a data member for the discriminant
  // but we will have already found that above via reference.
  std::vector<LazySymbol> variants;
  for (const llvm::DWARFDie& child : die) {
    if (child.getTag() == llvm::dwarf::DW_TAG_variant)
      variants.push_back(MakeLazy(child));
  }

  return fxl::MakeRefCounted<VariantPart>(MakeLazy(discriminant), std::move(variants));
}

AddressRanges DwarfSymbolFactory::GetCodeRanges(const llvm::DWARFDie& die) const {
  // It would be trivially more efficient to get the DW_AT_ranges, etc. attributes out when we're
  // iterating through the DIE. But the address ranges have several different forms and also vary
  // between DWARF versions 4 and 5 and this code isn't called too often.
  AddressRanges::RangeVector code_ranges;

  // First check for a single range on the DIE.
  if (std::optional<AddressRange> range = GetLowHighEncodedCodeRange(die)) {
    AppendAddressRange(range->begin(), range->end(), &code_ranges);
    // With only one range we know it's canonical.
    return AddressRanges(AddressRanges::kCanonical, std::move(code_ranges));
  }

  std::optional<llvm::DWARFFormValue> form_value = die.find(llvm::dwarf::DW_AT_ranges);
  if (!form_value) {
    // This DIE has no address ranges.
    return AddressRanges();
  }

  // Got a range list, decode.
  llvm::DWARFAddressRangesVector ranges;
  switch (form_value->getForm()) {
    case llvm::dwarf::DW_FORM_rnglistx: {
      llvm::Expected<llvm::DWARFAddressRangesVector> opt_ranges =
          GetCodeRangesFromRangelistXIndex(die, *form_value->getAsSectionOffset());
      if (!opt_ranges)
        return AddressRanges();
      ranges = std::move(opt_ranges.get());
      break;
    }

    case llvm::dwarf::DW_FORM_sec_offset: {
      // These encodings do not support Debug Fission (requiring lookup in the main symbol file if
      // the current one is a DWO) so we can have LLVM look it up in the current unit directly.
      llvm::Expected<llvm::DWARFAddressRangesVector> opt_ranges =
          die.getDwarfUnit()->findRnglistFromOffset(*form_value->getAsSectionOffset());
      if (!opt_ranges)
        return AddressRanges();
      ranges = std::move(opt_ranges.get());
      break;
    }

    default:
      break;
  }

  if (ranges.empty())
    return AddressRanges();

  code_ranges.reserve(ranges.size());
  for (const llvm::DWARFAddressRange& range : ranges) {
    AppendAddressRange(range.LowPC, range.HighPC, &code_ranges);
  }

  // Can't trust DWARF to have stored them in any particular order so specify "non canonical".
  return AddressRanges(AddressRanges::kNonCanonical, std::move(code_ranges));
}

// Similar to llvm::DWARFDie::getLowAndHighPC but works in our implementat ion debug fission.
std::optional<AddressRange> DwarfSymbolFactory::GetLowHighEncodedCodeRange(
    const llvm::DWARFDie& die) const {
  std::optional<llvm::DWARFFormValue> low_form = die.find(llvm::dwarf::DW_AT_low_pc);
  std::optional<llvm::DWARFFormValue> high_form = die.find(llvm::dwarf::DW_AT_high_pc);
  // Note that the "high pc" has an offset form so don't check that it has an FC_Address form yet.
  if (!low_form || !high_form || !low_form->isFormClass(llvm::DWARFFormValue::FC_Address)) {
    return std::nullopt;
  }

  // Low range.
  uint64_t low_addr = 0;
  if (std::optional<uint64_t> opt_low_addr = GetAddrFromFormValue(die, *low_form)) {
    low_addr = *opt_low_addr;
  } else {
    // The low address must be a valid address type.
    return std::nullopt;
  }

  // High range, this can be encoded as an address or an offset according to the form class.
  uint64_t high_addr = 0;
  if (std::optional<uint64_t> opt_high_addr = GetAddrFromFormValue(die, *high_form)) {
    // Encoded as an address.
    high_addr = *opt_high_addr;
  } else if (std::optional<uint64_t> opt_offset = high_form->getAsUnsignedConstant()) {
    // Encoded as an offset from the base of the range.
    high_addr = low_addr + *opt_offset;
  } else {
    return std::nullopt;
  }

  return AddressRange(low_addr, high_addr);
}

// The DW_FORM_rnglistx in DWARF 5 is:
//
//   "An index into the .debug_rnglists section (DW_FORM_rnglistx). The unsigned ULEB operand
//   identifies an offset location relative to the base of that section (the location of the first
//   offset in the section, not the first byte of the section). The contents of that location is
//   then added to the base to determine the location of the target range list of entries."
//
// In the case of a DWO, the address table for the DWO's range lists is stored in the main binary.
// The skeleton unit in the main binary corresponding to this DWO has a DW_AT_addr_base attribute
// that locates its address table where we look up the index in the main binary.
//
// For non-DWO, we do the same thing except the DW_AT_addr_base and the table data come from the
// current binary.
//
// This function returns LLVM types so it can have the same return value as the LLVM lookups for the
// other address ranges forms.
llvm::Expected<llvm::DWARFAddressRangesVector> DwarfSymbolFactory::GetCodeRangesFromRangelistXIndex(
    const llvm::DWARFDie& die, uint64_t index) const {
  if (CompileUnit* skeleton_unit = delegate_->GetSkeletonCompileUnit()) {
    // This is a DWO object so we use the main binary's DWARF unit corresponding to this file to
    // look up the data.
    return skeleton_unit->dwarf_unit()->GetLLVMUnit()->findRnglistFromIndex(index);
  }

  // This is not a DWO object and we use the current DIE's unit for all information.
  return die.getDwarfUnit()->findRnglistFromIndex(index);
}

std::optional<uint64_t> DwarfSymbolFactory::GetAddrFromFormValue(
    const llvm::DWARFDie& die, const llvm::DWARFFormValue& form_value) const {
  llvm::DWARFUnit* unit = die.getDwarfUnit();
  llvm::dwarf::Form form = form_value.getForm();
  if (!llvm::dwarf::doesFormBelongToClass(form, llvm::DWARFFormValue::FC_Address,
                                          unit->getVersion())) {
    // Not an address type.
    return std::nullopt;
  }

  uint64_t addr = 0;
  if (IsAddrXForm(form_value.getForm())) {
    // Lookup in the address table.
    uint64_t index = form_value.getRawUValue();
    if (std::optional<uint64_t> opt_addr = GetAddrFromIndex(die, index)) {
      addr = *opt_addr;
    } else {
      return std::nullopt;
    }
  } else {
    // Simple lookup of an address value.
    addr = form_value.getRawUValue();
  }

  if (addr == std::numeric_limits<uint64_t>::max()) {
    // Indicates that this range was optimized out.
    return std::nullopt;
  }

  return addr;
}

std::optional<uint64_t> DwarfSymbolFactory::GetAddrFromIndex(const llvm::DWARFDie& die,
                                                             uint64_t index) const {
  std::optional<llvm::object::SectionedAddress> addr;
  if (CompileUnit* skeleton_unit = delegate_->GetSkeletonCompileUnit()) {
    // This is a DWO object so we use the main binary's DWARF unit corresponding to this file to
    // look up the data.
    addr = skeleton_unit->dwarf_unit()->GetLLVMUnit()->getAddrOffsetSectionItem(index);
  } else {
    // Look up in the unit for this DIE.
    addr = die.getDwarfUnit()->getAddrOffsetSectionItem(index);
  }

  if (!addr) {
    return std::nullopt;
  }
  return addr->Address;
}

}  // namespace zxdb
