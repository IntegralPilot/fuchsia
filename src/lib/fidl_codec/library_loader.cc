// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/fidl_codec/library_loader.h"

#include <zircon/assert.h>
#include <zircon/rights.h>

#include <fstream>
#include <ios>
#include <sstream>

#include <rapidjson/error/en.h>

#include "src/lib/fidl_codec/builtin_semantic.h"
#include "src/lib/fidl_codec/logger.h"
#include "src/lib/fidl_codec/semantic_parser.h"
#include "src/lib/fidl_codec/wire_types.h"

// See library_loader.h for details.

namespace fidl_codec {

EnumOrBits::EnumOrBits(const rapidjson::Value* json_definition)
    : json_definition_(json_definition) {}

EnumOrBits::EnumOrBits(std::string name, uint64_t size, std::unique_ptr<Type> type,
                       std::vector<EnumOrBitsMember> members)
    : json_definition_(nullptr),
      name_(std::move(name)),
      size_(size),
      type_(std::move(type)),
      members_(std::move(members)) {}

EnumOrBits::~EnumOrBits() = default;

Enum::Enum(std::string name, uint64_t size, std::unique_ptr<Type> type,
           std::vector<EnumOrBitsMember> members)
    : EnumOrBits(std::move(name), size, std::move(type), std::move(members)) {}

const Enum& Enum::FrameworkErrorEnum() {
  static Enum result("FrameworkError", 4, std::make_unique<Int32Type>(),
                     {EnumOrBitsMember("UNKNOWN_METHOD", 2, true)});
  return result;
}

void EnumOrBits::DecodeTypes(bool is_scalar, const std::string& supertype_name,
                             Library* enclosing_library) {
  if (json_definition_ == nullptr) {
    return;
  }
  const rapidjson::Value* json_definition = json_definition_;
  // Reset json_definition_ member to allow recursive declarations.
  json_definition_ = nullptr;
  name_ = enclosing_library->ExtractString(json_definition, supertype_name, "<unknown>", "name");
  if (is_scalar) {
    type_ = enclosing_library->ExtractScalarType(json_definition, supertype_name, name_, "type");
  } else {
    type_ = enclosing_library->ExtractType(json_definition, supertype_name, name_, "type");
  }

  if (!json_definition->HasMember("members")) {
    enclosing_library->FieldNotFound(supertype_name, name_, "members");
  } else {
    if (json_definition->HasMember("members")) {
      for (auto& member : (*json_definition)["members"].GetArray()) {
        if (member.HasMember("value") && member["value"].HasMember("literal")) {
          if (!member.HasMember("name")) {
            continue;
          }

          const char* data = member["value"]["literal"]["value"].GetString();
          bool negative = false;
          if (*data == '-') {
            ++data;
            negative = true;
          }
          std::stringstream stream;
          stream.str(std::string(data));
          uint64_t absolute_value;
          stream >> absolute_value;
          members_.emplace_back(member["name"].GetString(), absolute_value, negative);
        }
      }
    }
  }

  size_ = type_->InlineSize(WireVersion::kWireV2);
}

std::string Enum::GetName(uint64_t absolute_value, bool negative) const {
  for (auto& member : members()) {
    if ((member.absolute_value() == absolute_value) && (member.negative() == negative)) {
      return member.name();
    }
  }
  return "<unknown>";
}

std::string Bits::GetName(uint64_t absolute_value, bool negative) const {
  std::ostringstream os;
  auto emit = [&os, first = true]() mutable -> std::ostringstream& {
    if (!first) {
      os << '|';
    }
    first = false;
    return os;
  };
  if (!negative) {
    for (const auto& member : members()) {
      if (member.negative()) {
        continue;
      }
      if (absolute_value & member.absolute_value()) {
        emit() << member.name();
      }
      absolute_value &= ~member.absolute_value();
    }
    if (absolute_value) {
      emit() << std::hex << std::showbase << absolute_value;
    }
  }

  std::string str = os.str();
  if (!str.empty()) {
    return str;
  }
  return "<none>";
}

UnionMember::UnionMember(const Union& union_definition, Library* enclosing_library,
                         const rapidjson::Value* json_definition)
    : union_definition_(union_definition),
      name_(enclosing_library->ExtractString(json_definition, "union member", "<unknown>", "name")),
      ordinal_(enclosing_library->ExtractUint64(json_definition, "union member", name_, "ordinal")),
      type_(enclosing_library->ExtractType(json_definition, "union member", name_, "type")) {}

Union::Union(Library* enclosing_library, const rapidjson::Value* json_definition)
    : enclosing_library_(enclosing_library), json_definition_(json_definition) {}

void Union::DecodeTypes() {
  if (json_definition_ == nullptr) {
    return;
  }
  const rapidjson::Value* json_definition = json_definition_;
  // Reset json_definition_ member to allow recursive declarations.
  json_definition_ = nullptr;
  name_ = enclosing_library_->ExtractString(json_definition, "union", "<unknown>", "name");

  if (!json_definition->HasMember("members")) {
    enclosing_library_->FieldNotFound("union", name_, "members");
  } else {
    auto member_arr = (*json_definition)["members"].GetArray();
    members_.reserve(member_arr.Size());
    for (auto& member : member_arr) {
      members_.push_back(std::make_unique<UnionMember>(*this, enclosing_library_, &member));
    }
  }
}

const UnionMember* Union::MemberFromOrdinal(Ordinal64 ordinal) const {
  for (const auto& member : members_) {
    if (member->ordinal() == ordinal) {
      return member.get();
    }
  }
  return nullptr;
}

UnionMember* Union::SearchMember(std::string_view name) const {
  for (const auto& member : members_) {
    if (member->name() == name) {
      return member.get();
    }
  }
  return nullptr;
}

std::string Union::ToString(bool expand) const {
  UnionType type(*this, false);
  return type.ToString(expand);
}

StructMember::StructMember(Library* enclosing_library, const rapidjson::Value* json_definition)
    : name_(
          enclosing_library->ExtractString(json_definition, "struct member", "<unknown>", "name")),
      offset_(enclosing_library->ExtractFieldOffset(json_definition, "struct member", name_,
                                                    "field_shape_v2")),
      type_(enclosing_library->ExtractType(json_definition, "struct member", name_, "type")) {}

StructMember::StructMember(std::string_view name, std::unique_ptr<Type> type)
    : name_(name), type_(std::move(type)) {}

StructMember::StructMember(std::string_view name, std::unique_ptr<Type> type, uint8_t id)
    : name_(name), type_(std::move(type)), id_(id) {}

void StructMember::reset_type() { type_ = nullptr; }

Struct::Struct(Library* enclosing_library, const rapidjson::Value* json_definition)
    : enclosing_library_(enclosing_library), json_definition_(json_definition) {}

Struct::Struct(std::string_view name)
    : enclosing_library_(nullptr), json_definition_(nullptr), name_(name) {}

void Struct::AddMember(std::string_view name, std::unique_ptr<Type> type, uint32_t id) {
  members_.emplace_back(std::make_unique<StructMember>(name, std::move(type), id));
}

void Struct::DecodeTypes() {
  if (json_definition_ == nullptr) {
    return;
  }
  const rapidjson::Value* json_definition = json_definition_;
  // Reset json_definition_ member to allow recursive declarations.
  json_definition_ = nullptr;
  name_ = enclosing_library_->ExtractString(json_definition, "struct", "<unknown>", "name");

  if (!json_definition->HasMember("type_shape_v2")) {
    enclosing_library_->FieldNotFound("struct", name_, "type_shape_v2");
  } else {
    const rapidjson::Value& type_shape = (*json_definition)["type_shape_v2"];
    size_ = static_cast<uint32_t>(
        enclosing_library_->ExtractUint64(&type_shape, "struct", name_, "inline_size"));
  }

  if (!json_definition->HasMember("members")) {
    enclosing_library_->FieldNotFound("struct", name_, "members");
  } else {
    auto member_arr = (*json_definition)["members"].GetArray();
    members_.reserve(member_arr.Size());
    for (auto& member : member_arr) {
      members_.push_back(std::make_unique<StructMember>(enclosing_library_, &member));
    }
  }
}

StructMember* Struct::SearchMember(std::string_view name, uint32_t id) const {
  for (const auto& member : members_) {
    if (member->name() == name && member->id() == id) {
      return member.get();
    }
  }
  return nullptr;
}

uint32_t Struct::Size(WireVersion version) const { return size_; }

std::string Struct::ToString(bool expand) const {
  StructType type(*this, false);
  return type.ToString(expand);
}

TableMember::TableMember(Library* enclosing_library, const rapidjson::Value* json_definition)
    : name_(enclosing_library->ExtractString(json_definition, "table member", "<unknown>", "name")),
      ordinal_(enclosing_library->ExtractUint32(json_definition, "table member", name_, "ordinal")),
      type_(enclosing_library->ExtractType(json_definition, "table member", name_, "type")) {}

Table::Table(Library* enclosing_library, const rapidjson::Value* json_definition)
    : enclosing_library_(enclosing_library), json_definition_(json_definition) {}

void Table::DecodeTypes() {
  if (json_definition_ == nullptr) {
    return;
  }
  const rapidjson::Value* json_definition = json_definition_;
  // Reset json_definition_ member to allow recursive declarations.
  json_definition_ = nullptr;
  name_ = enclosing_library_->ExtractString(json_definition, "table", "<unknown>", "name");

  if (!json_definition->HasMember("members")) {
    enclosing_library_->FieldNotFound("table", name_, "members");
  } else {
    auto member_arr = (*json_definition)["members"].GetArray();
    for (auto& member : member_arr) {
      auto table_member = std::make_unique<TableMember>(enclosing_library_, &member);
      Ordinal32 ordinal = table_member->ordinal();
      if (ordinal >= members_.size()) {
        members_.resize(ordinal + 1);
      }
      members_[ordinal] = std::move(table_member);
    }
  }
}

const TableMember* Table::MemberFromOrdinal(Ordinal64 ordinal) const {
  if (ordinal >= members_.size()) {
    return nullptr;
  }
  return members_[ordinal].get();
}

const TableMember* Table::SearchMember(std::string_view name) const {
  for (const auto& member : members_) {
    if ((member != nullptr) && (member->name() == name)) {
      return member.get();
    }
  }
  return nullptr;
}

std::string Table::ToString(bool expand) const {
  TableType type(*this);
  return type.ToString(expand);
}

ProtocolMethod::ProtocolMethod(Library* enclosing_library, const Protocol& protocol,
                               const rapidjson::Value* json_definition)
    : json_definition_(json_definition),
      enclosing_protocol_(&protocol),
      name_(protocol.enclosing_library()->ExtractString(json_definition, "method", "<unknown>",
                                                        "name")),
      ordinal_(
          protocol.enclosing_library()->ExtractUint64(json_definition, "method", name_, "ordinal")),
      is_composed_(protocol.enclosing_library()->ExtractBool(json_definition, "method", name_,
                                                             "is_composed")),
      has_request_(protocol.enclosing_library()->ExtractBool(json_definition, "method", name_,
                                                             "has_request")),
      has_response_(protocol.enclosing_library()->ExtractBool(json_definition, "method", name_,
                                                              "has_response")) {}

static std::unique_ptr<Type> GetPayloadType(LibraryLoader* loader,
                                            const rapidjson::Value* json_definition,
                                            const char* key) {
  if (json_definition->HasMember(key)) {
    return Type::GetType(loader, (*json_definition)[key]);
  }
  return std::make_unique<EmptyPayloadType>();
}

void ProtocolMethod::ProtocolMethod::DecodeTypes() {
  if (json_definition_ == nullptr) {
    return;
  }
  const rapidjson::Value* json_definition = json_definition_;
  // Reset json_definition_ member to allow recursive declarations.
  json_definition_ = nullptr;

  LibraryLoader* loader = enclosing_protocol_->enclosing_library()->enclosing_loader();
  if (has_request_) {
    request_ = GetPayloadType(loader, json_definition, "maybe_request_payload");
  }
  if (has_response_) {
    response_ = GetPayloadType(loader, json_definition, "maybe_response_payload");
  }
}

ProtocolMethod::~ProtocolMethod() = default;

std::string ProtocolMethod::fully_qualified_name() const {
  std::string fqn(enclosing_protocol_->name());
  fqn.append(".");
  fqn.append(name());
  return fqn;
}

void Protocol::AddMethodsToIndex(LibraryLoader* library_loader) {
  for (auto& protocol_method : protocol_methods_) {
    library_loader->AddMethod(protocol_method.get());
  }
}

bool Protocol::GetMethodByFullName(const std::string& name,
                                   const ProtocolMethod** method_ptr) const {
  for (const auto& method : methods()) {
    if (method->fully_qualified_name() == name) {
      method->DecodeTypes();
      *method_ptr = method.get();
      return true;
    }
  }
  return false;
}

ProtocolMethod* Protocol::GetMethodByName(std::string_view name) const {
  for (const auto& method : methods()) {
    if (method->name() == name) {
      method->DecodeTypes();
      return method.get();
    }
  }
  return nullptr;
}

Library::Library(std::string source, LibraryLoader* enclosing_loader,
                 rapidjson::Document& json_definition)
    : enclosing_loader_(enclosing_loader),
      json_definition_(std::move(json_definition)),
      source_(std::move(source)) {
  if (!json_definition_.HasMember("struct_declarations")) {
    FieldNotFound("library", name_, "struct_declarations");
  } else if (!json_definition_.HasMember("external_struct_declarations")) {
    FieldNotFound("library", name_, "external_struct_declarations");
  } else if (!json_definition_.HasMember("table_declarations")) {
    FieldNotFound("library", name_, "table_declarations");
  } else if (!json_definition_.HasMember("union_declarations")) {
    FieldNotFound("library", name_, "union_declarations");
  }

  auto protocols_array = json_definition_["protocol_declarations"].GetArray();
  protocols_.reserve(protocols_array.Size());

  for (auto& decl : protocols_array) {
    protocols_.emplace_back(new Protocol(this, decl));
    protocols_.back()->AddMethodsToIndex(enclosing_loader);
  }
}

Library::~Library() { enclosing_loader()->Delete(this); }

void Library::DecodeTypes() {
  if (decoded_) {
    return;
  }
  decoded_ = true;
  name_ = ExtractString(&json_definition_, "library", "<unknown>", "name");

  if (!json_definition_.HasMember("enum_declarations")) {
    FieldNotFound("library", name_, "enum_declarations");
  } else {
    for (auto& enu : json_definition_["enum_declarations"].GetArray()) {
      enums_.emplace(std::piecewise_construct, std::forward_as_tuple(enu["name"].GetString()),
                     std::forward_as_tuple(new Enum(&enu)));
    }
  }

  if (!json_definition_.HasMember("bits_declarations")) {
    FieldNotFound("library", name_, "bits_declarations");
  } else {
    for (auto& bits : json_definition_["bits_declarations"].GetArray()) {
      bits_.emplace(std::piecewise_construct, std::forward_as_tuple(bits["name"].GetString()),
                    std::forward_as_tuple(new Bits(&bits)));
    }
  }

  if (!json_definition_.HasMember("struct_declarations")) {
    FieldNotFound("library", name_, "struct_declarations");
  } else {
    for (auto& str : json_definition_["struct_declarations"].GetArray()) {
      structs_.emplace(std::piecewise_construct, std::forward_as_tuple(str["name"].GetString()),
                       std::forward_as_tuple(new Struct(this, &str)));
    }
  }

  if (!json_definition_.HasMember("external_struct_declarations")) {
    FieldNotFound("library", name_, "external_struct_declarations");
  } else {
    for (auto& str : json_definition_["external_struct_declarations"].GetArray()) {
      structs_.emplace(std::piecewise_construct, std::forward_as_tuple(str["name"].GetString()),
                       std::forward_as_tuple(new Struct(this, &str)));
    }
  }

  if (!json_definition_.HasMember("table_declarations")) {
    FieldNotFound("library", name_, "table_declarations");
  } else {
    for (auto& tab : json_definition_["table_declarations"].GetArray()) {
      tables_.emplace(std::piecewise_construct, std::forward_as_tuple(tab["name"].GetString()),
                      std::forward_as_tuple(new Table(this, &tab)));
    }
  }

  if (!json_definition_.HasMember("union_declarations")) {
    FieldNotFound("library", name_, "union_declarations");
  } else {
    for (auto& uni : json_definition_["union_declarations"].GetArray()) {
      unions_.emplace(std::piecewise_construct, std::forward_as_tuple(uni["name"].GetString()),
                      std::forward_as_tuple(new Union(this, &uni)));
    }
  }
}

bool Library::DecodeAll() {
  DecodeTypes();
  for (const auto& tmp : structs_) {
    tmp.second->DecodeTypes();
  }
  for (const auto& tmp : enums_) {
    tmp.second->DecodeTypes(this);
  }
  for (const auto& tmp : bits_) {
    tmp.second->DecodeTypes(this);
  }
  for (const auto& tmp : tables_) {
    tmp.second->DecodeTypes();
  }
  for (const auto& tmp : unions_) {
    tmp.second->DecodeTypes();
  }
  for (const auto& protocol : protocols_) {
    for (const auto& method : protocol->methods()) {
      method->DecodeTypes();
    }
  }
  return !has_errors_;
}

std::unique_ptr<Type> Library::TypeFromIdentifier(bool is_nullable, const std::string& identifier) {
  auto str = structs_.find(identifier);
  if (str != structs_.end()) {
    str->second->DecodeTypes();
    std::unique_ptr<Type> type(new StructType(std::ref(*str->second), is_nullable));
    return type;
  }
  auto enu = enums_.find(identifier);
  if (enu != enums_.end()) {
    enu->second->DecodeTypes(this);
    return std::make_unique<EnumType>(std::ref(*enu->second));
  }
  auto bits = bits_.find(identifier);
  if (bits != bits_.end()) {
    bits->second->DecodeTypes(this);
    return std::make_unique<BitsType>(std::ref(*bits->second));
  }
  auto tab = tables_.find(identifier);
  if (tab != tables_.end()) {
    tab->second->DecodeTypes();
    return std::make_unique<TableType>(std::ref(*tab->second));
  }
  auto uni = unions_.find(identifier);
  if (uni != unions_.end()) {
    uni->second->DecodeTypes();
    return std::make_unique<UnionType>(std::ref(*uni->second), is_nullable);
  }
  return std::make_unique<InvalidType>();
}

bool Library::GetProtocolByName(std::string_view name, Protocol** ptr) const {
  for (const auto& protocol : protocols()) {
    if (protocol->name() == name) {
      *ptr = protocol.get();
      return true;
    }
  }
  return false;
}

bool Library::ExtractBool(const rapidjson::Value* json_definition, std::string_view container_type,
                          std::string_view container_name, const char* field_name) {
  if (!json_definition->HasMember(field_name)) {
    FieldNotFound(container_type, container_name, field_name);
    return false;
  }
  return (*json_definition)[field_name].GetBool();
}

std::string Library::ExtractString(const rapidjson::Value* json_definition,
                                   std::string_view container_type, std::string_view container_name,
                                   const char* field_name) {
  if (!json_definition->HasMember(field_name)) {
    FieldNotFound(container_type, container_name, field_name);
    return "<unknown>";
  }
  return (*json_definition)["name"].GetString();
}

uint64_t Library::ExtractUint64(const rapidjson::Value* json_definition,
                                std::string_view container_type, std::string_view container_name,
                                const char* field_name) {
  if (!json_definition->HasMember(field_name)) {
    FieldNotFound(container_type, container_name, field_name);
    return 0;
  }
  return std::strtoll((*json_definition)[field_name].GetString(), nullptr, kDecimalBase);
}

uint32_t Library::ExtractUint32(const rapidjson::Value* json_definition,
                                std::string_view container_type, std::string_view container_name,
                                const char* field_name) {
  if (!json_definition->HasMember(field_name)) {
    FieldNotFound(container_type, container_name, field_name);
    return 0;
  }
  return static_cast<uint32_t>(
      std::strtoul((*json_definition)[field_name].GetString(), nullptr, kDecimalBase));
}

std::unique_ptr<Type> Library::ExtractScalarType(const rapidjson::Value* json_definition,
                                                 std::string_view container_type,
                                                 std::string_view container_name,
                                                 const char* field_name) {
  if (!json_definition->HasMember(field_name)) {
    FieldNotFound(container_type, container_name, field_name);
    return std::make_unique<InvalidType>();
  }
  return Type::ScalarTypeFromName((*json_definition)[field_name].GetString());
}

std::unique_ptr<Type> Library::ExtractType(const rapidjson::Value* json_definition,
                                           std::string_view container_type,
                                           std::string_view container_name,
                                           const char* field_name) {
  if (!json_definition->HasMember(field_name)) {
    FieldNotFound(container_type, container_name, field_name);
    return std::make_unique<InvalidType>();
  }
  return Type::GetType(enclosing_loader(), (*json_definition)[field_name]);
}

uint64_t Library::ExtractFieldOffset(const rapidjson::Value* json_definition,
                                     std::string_view container_type,
                                     std::string_view container_name, const char* field_name) {
  if (!json_definition->HasMember(field_name)) {
    FieldNotFound(container_type, container_name, field_name);
    return 0;
  }
  return ExtractUint64(&(*json_definition)[field_name], container_type, container_name, "offset");
}

void Library::FieldNotFound(std::string_view container_type, std::string_view container_name,
                            const char* field_name) {
  has_errors_ = true;
  FX_LOGS_OR_CAPTURE(ERROR) << "File " << name() << " field '" << field_name << "' missing for "
                            << container_type << ' ' << container_name;
}

LibraryLoader::LibraryLoader(const std::vector<std::string>& library_paths, LibraryReadError* err) {
  AddAll(library_paths, err);
}

bool LibraryLoader::AddAll(const std::vector<std::string>& library_paths, LibraryReadError* err) {
  bool ok = true;
  // Go backwards through the streams; we refuse to load the same library twice, and the last
  // one wins.
  for (auto path = library_paths.rbegin(); path != library_paths.rend(); ++path) {
    AddPath(*path, err);
    if (err->value != LibraryReadError::kOk) {
      ok = false;
    }
  }
  return ok;
}

bool LibraryLoader::DecodeAll() {
  bool ok = true;
  for (const auto& representation : representations_) {
    Library* library = representation.second.get();
    if (!library->DecodeAll()) {
      ok = false;
    }
  }
  return ok;
}

void LibraryLoader::AddPath(const std::string& path, LibraryReadError* err) {
  std::ifstream infile(path.c_str());
  std::string content(std::istreambuf_iterator<char>(infile), {});
  if (infile.fail()) {
    err->value = LibraryReadError ::kIoError;
    err->errno_value = errno;
    return;
  }
  AddContent(content, err, path);
  if (err->value != LibraryReadError::kOk) {
    FX_LOGS_OR_CAPTURE(ERROR) << path << ": JSON parse error: "
                              << rapidjson::GetParseError_En(err->parse_result.Code())
                              << " at offset " << err->parse_result.Offset();
    return;
  }
}

void LibraryLoader::AddContent(const std::string& content, LibraryReadError* err,
                               std::string source) {
  rapidjson::Document document;
  err->parse_result = document.Parse<rapidjson::kParseNumbersAsStringsFlag>(content.c_str());
  // TODO: This would be a good place to validate that the resulting JSON
  // matches the schema in tools/fidl/fidlc/schema.json.  If there are
  // errors, we will currently get mysterious crashes.
  if (document.HasParseError()) {
    err->value = LibraryReadError::kParseError;
    return;
  }
  std::string library_name = document["name"].GetString();
  if (representations_.find(library_name) == representations_.end()) {
    representations_.emplace(std::piecewise_construct, std::forward_as_tuple(library_name),
                             std::forward_as_tuple(new Library(std::move(source), this, document)));
  }
  err->value = LibraryReadError::kOk;
}

void LibraryLoader::AddMethod(ProtocolMethod* method) {
  if (ordinal_map_[method->ordinal()] == nullptr) {
    ordinal_map_[method->ordinal()] = std::make_unique<std::vector<ProtocolMethod*>>();
  }
  // Ensure composed methods come after non-composed methods.  The fidl_codec
  // libraries pick the first one they find.
  if (method->is_composed()) {
    ordinal_map_[method->ordinal()]->push_back(method);
  } else {
    ordinal_map_[method->ordinal()]->insert(ordinal_map_[method->ordinal()]->begin(), method);
  }
}

void LibraryLoader::ParseBuiltinSemantics() {
  semantic::ParserErrors parser_errors;
  {
    semantic::SemanticParser parser(this, semantic::kBuiltinSemantics, &parser_errors);
    parser.ParseSemantic();
  }
}

}  // namespace fidl_codec
