// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_DIAGNOSTICS_H_
#define TOOLS_FIDL_FIDLC_SRC_DIAGNOSTICS_H_

#include <string_view>
#include <vector>

#include "tools/fidl/fidlc/src/diagnostic_types.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/source_span.h"
#include "tools/fidl/fidlc/src/versioning_types.h"

namespace fidlc {

constexpr ErrorDef<1, std::string_view> ErrInvalidCharacter("invalid character '{0}'");
constexpr ErrorDef<2> ErrUnexpectedLineBreak("unexpected line-break in string literal");
constexpr ErrorDef<3, std::string_view> ErrInvalidEscapeSequence("invalid escape sequence '{0}'");
constexpr ErrorDef<4, char> ErrInvalidHexDigit("invalid hex digit '{0}'");
constexpr RetiredDef<5> ErrInvalidOctDigit;
constexpr ErrorDef<6, std::string_view> ErrExpectedDeclaration("invalid declaration type {0}");
constexpr ErrorDef<7> ErrUnexpectedToken("found unexpected token");
constexpr ErrorDef<8, Token::KindAndSubkind, Token::KindAndSubkind> ErrUnexpectedTokenOfKind(
    "unexpected token {0}, was expecting {1}");
constexpr ErrorDef<9, Token::KindAndSubkind, Token::KindAndSubkind> ErrUnexpectedIdentifier(
    "unexpected identifier {0}, was expecting {1}");
constexpr ErrorDef<10, std::string_view> ErrInvalidIdentifier("invalid identifier '{0}'");
constexpr ErrorDef<11, std::string_view> ErrInvalidLibraryNameComponent(
    "Invalid library name component {0}");
constexpr ErrorDef<12> ErrInvalidLayoutClass(
    "layouts must be of the class: bits, enum, struct, table, or union.");
constexpr ErrorDef<13> ErrInvalidWrappedType("wrapped type for bits/enum must be an identifier");
constexpr ErrorDef<14> ErrAttributeWithEmptyParens(
    "attributes without arguments must omit the trailing empty parentheses");
constexpr ErrorDef<15> ErrAttributeArgsMustAllBeNamed(
    "attributes that take multiple arguments must name all of them explicitly");
constexpr ErrorDef<16> ErrMissingOrdinalBeforeMember("missing ordinal before member");
constexpr ErrorDef<17> ErrOrdinalOutOfBound("ordinal out-of-bound");
constexpr ErrorDef<18> ErrOrdinalsMustStartAtOne("ordinals must start at 1");
constexpr ErrorDef<19> ErrMustHaveOneMember("must have at least one member");
constexpr ErrorDef<20> ErrInvalidProtocolMember("invalid protocol member");
constexpr RetiredDef<21> ErrExpectedProtocolMember;
constexpr ErrorDef<22> ErrCannotAttachAttributeToIdentifier(
    "cannot attach attributes to identifiers");
constexpr ErrorDef<23> ErrAttributeInsideTypeDeclaration(
    "attributes are not allowed here; put the attribute before the `type` keyword instead");
constexpr ErrorDef<24> ErrDocCommentOnParameters("cannot have doc comment on parameters");
constexpr ErrorDef<25> ErrLibraryImportsMustBeGroupedAtTopOfFile(
    "library imports must be grouped at top-of-file");
constexpr WarningDef<26> WarnCommentWithinDocCommentBlock(
    "cannot have comment within doc comment block");
constexpr WarningDef<27> WarnBlankLinesWithinDocCommentBlock(
    "cannot have blank lines within doc comment block");
constexpr WarningDef<28> WarnDocCommentMustBeFollowedByDeclaration(
    "doc comment must be followed by a declaration");
constexpr ErrorDef<29> ErrMustHaveOneProperty("must have at least one property");
constexpr ErrorDef<30, Token::KindAndSubkind, Token::KindAndSubkind> ErrCannotSpecifyModifier(
    "cannot specify modifier {0} for {1}");
constexpr ErrorDef<31, Token::KindAndSubkind> ErrCannotSpecifySubtype(
    "cannot specify subtype for {0}");
constexpr ErrorDef<32, const Modifier *> ErrDuplicateModifier("duplicate occurrence of {0}");
constexpr ErrorDef<33, const Modifier *, const Modifier *> ErrConflictingModifier(
    "{0} conflicts with {1}");
constexpr ErrorDef<34, Element::Kind, std::string_view, Element::Kind, SourceSpan> ErrNameCollision(
    "{0} '{1}' has the same name as the {2} declared at {3}");
constexpr ErrorDef<35, Element::Kind, std::string_view, Element::Kind, std::string_view, SourceSpan,
                   std::string_view>
    ErrNameCollisionCanonical(
        "{0} '{1}' conflicts with {2} '{3}' declared at {4}; both names are "
        "represented by the canonical form '{5}'");
constexpr ErrorDef<36, Element::Kind, std::string_view, Element::Kind, SourceSpan, VersionSet,
                   Platform>
    ErrNameOverlap(
        "{0} '{1}' has the same name as the {2} declared at {3}; both are "
        "available {4} of platform '{5}'");
constexpr ErrorDef<37, Element::Kind, std::string_view, Element::Kind, std::string_view, SourceSpan,
                   std::string_view, VersionSet, Platform>
    ErrNameOverlapCanonical(
        "{0} '{1}' conflicts with {2} '{3}' declared at {4}; both names are "
        "represented by the canonical form '{5}' and are available {6} of "
        "platform '{7}'");
constexpr ErrorDef<38, Name> ErrDeclNameConflictsWithLibraryImport(
    "Declaration name '{0}' conflicts with a library import. Consider using the "
    "'as' keyword to import the library under a different name.");
constexpr ErrorDef<39, Name, std::string_view> ErrDeclNameConflictsWithLibraryImportCanonical(
    "Declaration name '{0}' conflicts with a library import due to its "
    "canonical form '{1}'. Consider using the 'as' keyword to import the "
    "library under a different name.");
constexpr ErrorDef<40> ErrFilesDisagreeOnLibraryName(
    "Two files in the library disagree about the name of the library");
constexpr ErrorDef<41, std::string_view> ErrMultipleLibrariesWithSameName(
    "There are multiple libraries named '{0}'");
constexpr ErrorDef<42, std::string_view> ErrDuplicateLibraryImport(
    "Library {0} already imported. Did you require it twice?");
constexpr ErrorDef<43, std::string_view> ErrConflictingLibraryImport(
    "import of library '{0}' conflicts with another library import");
constexpr ErrorDef<44, std::string_view, std::string_view> ErrConflictingLibraryImportAlias(
    "import of library '{0}' under alias '{1}' conflicts with another library import");
constexpr ErrorDef<45> ErrAttributesNotAllowedOnLibraryImport(
    "attributes and doc comments are not allowed on `using` statements");
constexpr ErrorDef<46, std::string_view> ErrUnknownLibrary(
    "Could not find library named {0}. Did you include its sources with --files?");
constexpr RetiredDef<47> ErrProtocolComposedMultipleTimes;
constexpr ErrorDef<48> ErrOptionalTableMember("Table members cannot be optional");
constexpr ErrorDef<49> ErrOptionalUnionMember("Union members cannot be optional");
constexpr ErrorDef<50> ErrDeprecatedStructDefaults(
    "Struct defaults are deprecated and should not be used (see RFC-0160)");
constexpr ErrorDef<51, std::string_view, std::string_view> ErrUnknownDependentLibrary(
    "Unknown dependent library {0} or reference to member of "
    "library {1}. Did you require it with `using`?");
constexpr ErrorDef<52, std::string_view, const Library *> ErrNameNotFound(
    "cannot find '{0}' in {1}");
constexpr ErrorDef<53, const Decl *> ErrCannotReferToMember("cannot refer to member of {0}");
constexpr ErrorDef<54, const Decl *, std::string_view> ErrMemberNotFound("{0} has no member '{1}'");
constexpr ErrorDef<55, const Element *, VersionRange, Platform, const Element *>
    ErrInvalidReferenceToDeprecated(
        "invalid reference to {0}, which is deprecated {1} of platform '{2}' while {3} "
        "is not; either remove this reference or mark {3} as deprecated");
constexpr ErrorDef<56, const Element *, VersionRange, Platform, const Element *, VersionRange,
                   Platform>
    ErrInvalidReferenceToDeprecatedOtherPlatform(
        "invalid reference to {0}, which is deprecated {1} of platform '{2}' while {3} "
        "is not deprecated {4} of platform '{5}'; either remove this reference or mark {3} as "
        "deprecated");
constexpr ErrorDef<57, std::vector<const Decl *>> ErrIncludeCycle(
    "There is an includes-cycle in declarations: {0}");
constexpr ErrorDef<58, Name> ErrAnonymousNameReference("cannot refer to anonymous name {0}");
constexpr ErrorDef<59, const Type *> ErrInvalidConstantType("invalid constant type {0}");
constexpr ErrorDef<60> ErrCannotResolveConstantValue("unable to resolve constant value");
constexpr ErrorDef<61> ErrOrOperatorOnNonPrimitiveValue(
    "Or operator can only be applied to primitive-kinded values");
constexpr ErrorDef<62, Name, std::string_view> ErrNewTypesNotAllowed(
    "newtypes not allowed: type declaration {0} defines a new type of the "
    "existing {1} type, which is not yet supported");
constexpr ErrorDef<63, Name> ErrExpectedValueButGotType("{0} is a type, but a value was expected");
constexpr ErrorDef<64, Name, Name> ErrMismatchedNameTypeAssignment(
    "mismatched named type assignment: cannot define a constant or default "
    "value of type {0} using a value of type {1}");
constexpr ErrorDef<65, const Constant *, const Type *, const Type *> ErrTypeCannotBeConvertedToType(
    "{0} (type {1}) cannot be converted to type {2}");
constexpr ErrorDef<66, const Constant *, const Type *> ErrConstantOverflowsType(
    "{0} overflows type {1}");
constexpr ErrorDef<67> ErrBitsMemberMustBePowerOfTwo("bits members must be powers of two");
constexpr ErrorDef<68, std::string_view> ErrFlexibleEnumMemberWithMaxValue(
    "flexible enums must not have a member with a value of {0}, which is "
    "reserved for the unknown value. either: remove the member, change its "
    "value to something else, or explicitly specify the unknown value with "
    "the @unknown attribute. see "
    "<https://fuchsia.dev/fuchsia-src/reference/fidl/language/attributes#unknown> "
    "for more info.");
constexpr ErrorDef<69, const Type *> ErrBitsTypeMustBeUnsignedIntegralPrimitive(
    "bits may only be of unsigned integral primitive type, found {0}");
constexpr ErrorDef<70, const Type *> ErrEnumTypeMustBeIntegralPrimitive(
    "enums may only be of integral primitive type, found {0}");
constexpr ErrorDef<71> ErrUnknownAttributeOnStrictEnumMember(
    "the @unknown attribute can be only be used on flexible enum members.");
constexpr ErrorDef<72> ErrUnknownAttributeOnMultipleEnumMembers(
    "the @unknown attribute can be only applied to one enum member.");
constexpr ErrorDef<73> ErrComposingNonProtocol("This declaration is not a protocol");
constexpr ErrorDef<74, Decl::Kind> ErrInvalidMethodPayloadLayoutClass(
    "cannot use {0} as a request/response; must use a struct, table, or union");
constexpr ErrorDef<75, const Type *> ErrInvalidMethodPayloadType(
    "invalid request/response type '{0}'; must use a struct, table, or union");
constexpr RetiredDef<76> ErrResponsesWithErrorsMustNotBeEmpty;
constexpr ErrorDef<77> ErrEmptyPayloadStructs(
    "(struct {}) is not allowed as a request or response, use () instead");
constexpr RetiredDef<78> ErrDuplicateElementName;
constexpr RetiredDef<79> ErrDuplicateElementNameCanonical;
constexpr ErrorDef<80> ErrGeneratedZeroValueOrdinal("Ordinal value 0 disallowed.");
constexpr ErrorDef<81, SourceSpan> ErrDuplicateMethodOrdinal(
    "Multiple methods with the same ordinal in a protocol; previous was at {0}.");
constexpr ErrorDef<82> ErrInvalidSelectorValue(
    "invalid selector value, must be a method name or a fully qualified method name");
constexpr ErrorDef<83> ErrFuchsiaIoExplicitOrdinals(
    "fuchsia.io must have explicit ordinals (https://fxbug.dev/42157659)");
constexpr ErrorDef<84> ErrPayloadStructHasDefaultMembers(
    "default values are not allowed on members of request/response structs");
constexpr RetiredDef<85> ErrDuplicateServiceMemberName;
constexpr RetiredDef<86> ErrStrictUnionMustHaveNonReservedMember;
constexpr RetiredDef<87> ErrDuplicateServiceMemberNameCanonical;
constexpr ErrorDef<88> ErrOptionalServiceMember("service members cannot be optional");
constexpr RetiredDef<89> ErrDuplicateStructMemberName;
constexpr RetiredDef<90> ErrDuplicateStructMemberNameCanonical;
constexpr ErrorDef<91, std::string_view, const Type *> ErrInvalidStructMemberType(
    "struct field {0} has an invalid default type {1}");
constexpr ErrorDef<92> ErrTableOrdinalTooLarge(
    "ordinal is too large; table ordinals cannot be greater than 64");
constexpr ErrorDef<93> ErrMaxOrdinalNotTable(
    "the 64th ordinal of a table may only contain a table type");
constexpr ErrorDef<94, SourceSpan> ErrDuplicateTableFieldOrdinal(
    "multiple table fields with the same ordinal; previous was at {0}");
constexpr RetiredDef<95> ErrDuplicateTableFieldName;
constexpr RetiredDef<96> ErrDuplicateTableFieldNameCanonical;
constexpr ErrorDef<97, SourceSpan> ErrDuplicateUnionMemberOrdinal(
    "multiple union fields with the same ordinal; previous was at {0}");
constexpr RetiredDef<98> ErrDuplicateUnionMemberName;
constexpr RetiredDef<99> ErrDuplicateUnionMemberNameCanonical;
constexpr RetiredDef<100> ErrNonDenseOrdinal;
constexpr ErrorDef<101> ErrCouldNotResolveSizeBound("unable to resolve size bound");
constexpr ErrorDef<102, Decl::Kind> ErrCouldNotResolveMember("unable to resolve {0} member");
constexpr ErrorDef<103, std::string_view> ErrCouldNotResolveMemberDefault(
    "unable to resolve {0} default value");
constexpr ErrorDef<104> ErrCouldNotResolveAttributeArg("unable to resolve attribute argument");
constexpr RetiredDef<105> ErrDuplicateMemberName;
constexpr RetiredDef<106> ErrDuplicateMemberNameCanonical;
constexpr ErrorDef<107, Decl::Kind, std::string_view, std::string_view, SourceSpan>
    ErrDuplicateMemberValue(
        "value of {0} member '{1}' conflicts with previously declared member '{2}' at {3}");
constexpr RetiredDef<108> ErrDuplicateResourcePropertyName;
constexpr RetiredDef<109> ErrDuplicateResourcePropertyNameCanonical;
constexpr ErrorDef<110, Decl::Kind, Name, std::string_view, SourceSpan> ErrTypeMustBeResource(
    "{0} '{1}' may contain handles (due to member '{2}' at {3}), so it must "
    "be declared with the `resource` modifier: `resource {0} {1}`");
constexpr ErrorDef<111, Name, uint32_t, uint32_t> ErrInlineSizeExceedsLimit(
    "'{0}' has an inline size of {1} bytes, which exceeds the maximum allowed "
    "inline size of {2} bytes");
constexpr ErrorDef<112> ErrOnlyClientEndsInServices("service members must be client_end:P");
constexpr ErrorDef<113, std::string_view, std::string_view, std::string_view, std::string_view>
    ErrMismatchedTransportInServices(
        "service member {0} is over the {1} transport, but member {2} is over "
        "the {3} transport. Multiple transports are not allowed.");
constexpr ErrorDef<114, Openness, Name, Openness, Name> ErrComposedProtocolTooOpen(
    "{0} protocol '{1}' cannot compose {2} protocol '{3}'; composed protocol "
    "may not be more open than composing protocol");
constexpr ErrorDef<115, Openness> ErrFlexibleTwoWayMethodRequiresOpenProtocol(
    "flexible two-way method may only be defined in an open protocol, not {0}");
constexpr ErrorDef<116, Protocol::Method::Kind> ErrFlexibleOneWayMethodInClosedProtocol(
    "flexible {0} may only be defined in an open or ajar protocol, not closed");
constexpr ErrorDef<117, std::string_view, std::string_view, const Decl *>
    ErrHandleUsedInIncompatibleTransport(
        "handle of type {0} may not be sent over transport {1} used by {2}");
constexpr ErrorDef<118, std::string_view, std::string_view, const Decl *>
    ErrTransportEndUsedInIncompatibleTransport(
        "client_end / server_end of transport type {0} may not be sent over "
        "transport {1} used by {2}");
constexpr RetiredDef<119> ErrEventErrorSyntax;
constexpr ErrorDef<120, const Attribute *> ErrInvalidAttributePlacement(
    "placement of attribute '{0}' disallowed here");
constexpr ErrorDef<121, const Attribute *> ErrDeprecatedAttribute("attribute '{0}' is deprecated");
constexpr ErrorDef<122, std::string_view, SourceSpan> ErrDuplicateAttribute(
    "duplicate attribute '{0}'; previous was at {1}");
constexpr ErrorDef<123, std::string_view, std::string_view, SourceSpan, std::string_view>
    ErrDuplicateAttributeCanonical(
        "attribute '{0}' conflicts with attribute '{1}' from {2}; both are "
        "represented by the canonical form '{3}'");
constexpr ErrorDef<124, const AttributeArg *, const Attribute *> ErrCanOnlyUseStringOrBool(
    "argument '{0}' on user-defined attribute '{1}' cannot be a numeric "
    "value; use a bool or string instead");
constexpr ErrorDef<125> ErrAttributeArgMustNotBeNamed(
    "attributes that take a single argument must not name that argument");
constexpr ErrorDef<126, std::string_view> ErrAttributeArgNotNamed(
    "attributes that take multiple arguments must name all of them explicitly, but '{0}' was not");
constexpr ErrorDef<127, const Attribute *, std::string_view> ErrMissingRequiredAttributeArg(
    "attribute '{0}' is missing the required '{1}' argument");
constexpr ErrorDef<128, const Attribute *> ErrMissingRequiredAnonymousAttributeArg(
    "attribute '{0}' is missing its required argument");
constexpr ErrorDef<129, const Attribute *, std::string_view> ErrUnknownAttributeArg(
    "attribute '{0}' does not support the '{1}' argument");
constexpr ErrorDef<130, const Attribute *, std::string_view, SourceSpan> ErrDuplicateAttributeArg(
    "attribute '{0}' provides the '{1}' argument multiple times; previous was at {2}");
constexpr ErrorDef<131, const Attribute *, std::string_view, std::string_view, SourceSpan,
                   std::string_view>
    ErrDuplicateAttributeArgCanonical(
        "attribute '{0}' argument '{1}' conflicts with argument '{2}' from {3}; both "
        "are represented by the canonical form '{4}'");
constexpr ErrorDef<132, const Attribute *> ErrAttributeDisallowsArgs(
    "attribute '{0}' does not support arguments");
constexpr ErrorDef<133, std::string_view, const Attribute *> ErrAttributeArgRequiresLiteral(
    "argument '{0}' of attribute '{1}' does not support referencing constants; "
    "please use a literal instead");
constexpr RetiredDef<134> ErrAttributeConstraintNotSatisfied;
constexpr ErrorDef<135, std::string_view> ErrInvalidDiscoverableName(
    "invalid @discoverable name '{0}'; must follow the format 'the.library.name.TheProtocolName'");
constexpr RetiredDef<136> ErrTableCannotBeSimple;
constexpr RetiredDef<137> ErrUnionCannotBeSimple;
constexpr RetiredDef<138> ErrElementMustBeSimple;
constexpr RetiredDef<139> ErrTooManyBytes;
constexpr RetiredDef<140> ErrTooManyHandles;
constexpr ErrorDef<141> ErrInvalidErrorType(
    "invalid error type: must be int32, uint32 or an enum thereof");
constexpr ErrorDef<142, std::string_view, std::set<std::string_view>> ErrInvalidTransportType(
    "invalid transport type: got {0} expected one of {1}");
constexpr RetiredDef<143> ErrBoundIsTooBig;
constexpr RetiredDef<144> ErrUnableToParseBound;
constexpr WarningDef<145, std::string_view, std::string_view> WarnAttributeTypo(
    "suspect attribute with name '{0}'; did you mean '{1}'?");
constexpr ErrorDef<146> ErrInvalidGeneratedName("generated name must be a valid identifier");
constexpr ErrorDef<147> ErrAvailableMissingArguments(
    "at least one argument is required: 'added', 'deprecated', or 'removed'");
constexpr ErrorDef<148> ErrNoteWithoutDeprecationOrRemoval(
    "the @available argument 'note' cannot be used without 'deprecated', 'removed', or "
    "'replaced'");
constexpr ErrorDef<149> ErrPlatformNotOnLibrary(
    "the @available argument 'platform' can only be used on the library's @available attribute");
constexpr ErrorDef<150> ErrLibraryAvailabilityMissingAdded(
    "missing 'added' argument on the library's @available attribute");
constexpr ErrorDef<151, std::string_view> ErrMissingLibraryAvailability(
    "to use the @available attribute here, you must also annotate the "
    "`library {0};` declaration in one of the library's files");
constexpr ErrorDef<152, std::string_view> ErrInvalidPlatform(
    "invalid platform '{0}'; must match the regex [a-z][a-z0-9]*");
constexpr ErrorDef<153, std::string_view> ErrInvalidVersion(
    "invalid version '{0}'; must be an integer from 1 to 2^31-1 inclusive, or "
    "one of the special constants `NEXT` or `HEAD`");
constexpr ErrorDef<154, std::string_view> ErrInvalidAvailabilityOrder(
    "invalid @available attribute; must have {0}");
constexpr ErrorDef<155, const AttributeArg *, std::string_view, const AttributeArg *,
                   std::string_view, SourceSpan, std::string_view, std::string_view,
                   std::string_view>
    ErrAvailabilityConflictsWithParent(
        "the argument {0}={1} conflicts with {2}={3} at {4}; a child element "
        "cannot be {5} {6} its parent element is {7}");
constexpr ErrorDef<156, Name> ErrCannotBeOptional("{0} cannot be optional");
constexpr ErrorDef<157, Name> ErrMustBeAProtocol("{0} must be a protocol");
constexpr ErrorDef<158, Name> ErrCannotBoundTwice("{0} cannot bound twice");
constexpr ErrorDef<159, Name> ErrStructCannotBeOptional(
    "structs can no longer be marked optional; please use the new syntax, "
    "`box<{0}>`");
constexpr ErrorDef<160, Name> ErrCannotIndicateOptionalTwice(
    "{0} is already optional, cannot indicate optionality twice");
constexpr ErrorDef<161, Name> ErrMustHaveNonZeroSize("{0} must have non-zero size");
constexpr ErrorDef<162, Name, size_t, size_t> ErrWrongNumberOfLayoutParameters(
    "{0} expected {1} layout parameter(s), but got {2}");
constexpr ErrorDef<163> ErrMultipleConstraintDefinitions(
    "cannot specify multiple constraint sets on a type");
constexpr ErrorDef<164, Name, size_t, size_t> ErrTooManyConstraints(
    "{0} expected at most {1} constraints, but got {2}");
constexpr ErrorDef<165> ErrExpectedType("expected type but got a literal or constant");
constexpr ErrorDef<166, Name> ErrUnexpectedConstraint("{0} failed to resolve constraint");
constexpr ErrorDef<167, Name> ErrCannotConstrainTwice("{0} cannot add additional constraint");
constexpr ErrorDef<168, Name> ErrProtocolConstraintRequired(
    "{0} requires a protocol as its first constraint");
// The same error as ErrCannotBeOptional, but with a more specific message since the
// optionality of boxes may be confusing
constexpr ErrorDef<169> ErrBoxCannotBeOptional(
    "cannot specify optionality for box, boxes are optional by default");
constexpr RetiredDef<170> ErrBoxedTypeCannotBeOptional;
constexpr ErrorDef<171, Name> ErrCannotBeBoxedShouldBeOptional(
    "type {0} cannot be boxed, try using optional instead");
constexpr ErrorDef<172, Name> ErrResourceMustBeUint32Derived("resource {0} must be uint32");
constexpr ErrorDef<173, Name> ErrResourceMissingSubtypeProperty(
    "resource {0} expected to have the subtype property, but it was missing");
constexpr RetiredDef<174> ErrResourceMissingRightsProperty;
constexpr ErrorDef<175, Name> ErrResourceSubtypePropertyMustReferToEnum(
    "the subtype property must be an enum, but wasn't in resource {0}");
constexpr RetiredDef<176> ErrHandleSubtypeMustReferToResourceSubtype;
constexpr ErrorDef<177, Name> ErrResourceRightsPropertyMustReferToBits(
    "the rights property must be a uint32 or a uint32-based bits, "
    "but wasn't defined as such in resource {0}");
constexpr ErrorDef<178, const Library *, const Library *> ErrUnusedImport(
    "{0} imports {1} but does not use it; either use it or remove the import");
constexpr ErrorDef<179, Name> ErrNewTypeCannotHaveConstraint(
    "{0} is a newtype, which cannot carry constraints");
constexpr ErrorDef<180, Name> ErrExperimentalZxCTypesDisallowed(
    "{0} is an experimental type that must be enabled by with `--experimental zx_c_types`");
constexpr ErrorDef<181> ErrReferenceInLibraryAttribute(
    "attributes on the 'library' declaration do not support referencing constants");
constexpr RetiredDef<182> ErrLegacyWithoutRemoval;
constexpr RetiredDef<183> ErrLegacyConflictsWithParent;
constexpr ErrorDef<184, std::string_view> ErrUnexpectedControlCharacter(
    "unexpected control character in string literal; use the Unicode escape `\\u{{0}}` instead");
constexpr ErrorDef<185> ErrUnicodeEscapeMissingBraces(
    "Unicode escape must use braces, like `\\u{a}` for U+000A");
constexpr ErrorDef<186> ErrUnicodeEscapeUnterminated(
    "Unicode escape is missing a closing brace '}'");
constexpr ErrorDef<187> ErrUnicodeEscapeEmpty("Unicode escape must have at least 1 hex digit");
constexpr ErrorDef<188> ErrUnicodeEscapeTooLong("Unicode escape must have at most 6 hex digits");
constexpr ErrorDef<189, std::string_view> ErrUnicodeEscapeTooLarge(
    "invalid Unicode code point '{0}'; maximum is 10FFFF");
constexpr RetiredDef<190> ErrSimpleProtocolMustBeClosed;
constexpr ErrorDef<191, std::string_view> ErrMethodMustDefineStrictness(
    "Method {0} must explicitly specify strict or flexible. (The default is changing "
    "from strict to flexible, and explicit modifiers are mandatory during the migration.)");
constexpr ErrorDef<192, std::string_view> ErrProtocolMustDefineOpenness(
    "Protocol {0} must explicitly specify open, ajar, or closed. (The default is changing "
    "from closed to open, and explicit modifiers are mandatory during the migration.)");
constexpr ErrorDef<193, Name> ErrCannotBeBoxedNorOptional("type {0} cannot be boxed");
constexpr RetiredDef<194> ErrEmptyPayloadStructsWhenResultUnion;
constexpr RetiredDef<195> ErrExperimentalOverflowingAttributeMissingExperimentalFlag;
constexpr RetiredDef<196> ErrExperimentalOverflowingIncorrectUsage;
constexpr ErrorDef<197> ErrOverlayMustBeStrict("overlays must be strict", {.documented = false});
constexpr ErrorDef<198> ErrOverlayMustBeValue("overlays must be value (not resource) types",
                                              {.documented = false});
constexpr ErrorDef<199> ErrOverlayMemberMustBeValue("overlays may not contain resource members",
                                                    {.documented = false});
constexpr RetiredDef<200> ErrOverlayMustNotContainReserved;
constexpr ErrorDef<201, const Library *, Platform> ErrPlatformVersionNotSelected(
    "{0} belongs to platform '{1}', but no version was selected for it; "
    "please choose a version N by passing `--available {1}:N`");
constexpr RetiredDef<202> ErrTransitionalNotAllowed;
constexpr ErrorDef<203> ErrRemovedAndReplaced(
    "the @available arguments 'removed' and 'replaced' are mutually exclusive");
constexpr ErrorDef<204> ErrLibraryReplaced(
    "the @available argument 'replaced' cannot be used on the library "
    "declaration; used 'removed' instead");
constexpr ErrorDef<205, const Element *, Version, SourceSpan> ErrInvalidRemoved(
    "{0} is marked removed={1}, but there is a replacement marked added={1} at {2}; "
    "either change removed={1} to replaced={1}, or delete the replacement");
constexpr ErrorDef<206, const Element *, Version> ErrInvalidReplaced(
    "{0} is marked replaced={1}, but there is no replacement marked added={1}; "
    "either change replaced={1} to removed={1}, or define a replacement");
constexpr ErrorDef<207, uint32_t, char, uint32_t> ErrTypeShapeIntegerOverflow(
    "cannot calculate type shape because of integer overflow in {0} {1} {2}");
constexpr ErrorDef<208, Platform> ErrReservedPlatform(
    "platform '{0}' is reserved; choose a different platform name using "
    "@available(platform=\"...\", added=...)");
constexpr ErrorDef<209> ErrReservedNotAllowed(
    "FIDL no longer supports reserved table or union fields; use @available instead");
constexpr ErrorDef<210, std::string_view> ErrInvalidDiscoverableLocation(
    "invalid @discoverable location '{0}'; must be comma separated 'platform' and/or 'external'");
constexpr ErrorDef<211, Element::Kind> ErrCannotBeRenamed(
    "the @available argument 'renamed' cannot be used on a {0}; it can only be "
    "used on members of a declaration");
constexpr ErrorDef<212> ErrRenamedWithoutReplacedOrRemoved(
    "the @available argument 'renamed' cannot be used without 'replaced' or 'removed'");
constexpr ErrorDef<213, std::string_view> ErrRenamedToSameName(
    "renaming to '{0}' has no effect because the element is already named '{0}'; "
    "either remove the 'renamed' argument or choose a different name");
constexpr ErrorDef<214, const Element *, Version, std::string_view, SourceSpan>
    ErrInvalidRemovedAndRenamed(
        "{0} is marked removed={1}, renamed=\"{2}\" but the name '{2}' is already used at {3}");
constexpr ErrorDef<215, const Element *, Version, std::string_view> ErrInvalidReplacedAndRenamed(
    "{0} is marked replaced={1}, renamed=\"{2}\" but there is no replacement "
    "'{2}' marked added={1}; please define it");
constexpr ErrorDef<216, const Element *, Version, AbiKind, AbiValue, SourceSpan, std::string_view>
    ErrInvalidRemovedAbi(
        "{0} is marked removed={1}, but its {2} ({3}) is reused at {4}; use "
        "replaced={1}, renamed=\"{5}\" instead of removed={1} if you intend to "
        "replace the ABI, otherwise choose a different {2}");
constexpr ErrorDef<217, const Element *, Version, AbiKind, AbiValue, AbiValue, SourceSpan>
    ErrInvalidReplacedAbi(
        "{0} is marked replaced={1}, but its {2} ({3}) does not match the "
        "replacement's {2} ({4}) at {5}; use removed={1} if you intend to "
        "remove the ABI, otherwise use the same {2}");
constexpr ErrorDef<218, const AttributeArg *> ErrInvalidModifierAvailableArgument(
    "invalid argument '{0}'; only 'added' and 'removed' are allowed on modifier availabilities");
constexpr ErrorDef<219> ErrCannotChangeMethodStrictness(
    "changing the strictness of a two-way method without error syntax is not "
    "allowed because it is ABI breaking");
constexpr ErrorDef<220, std::string_view, const Library *, VersionRange,
                   std::vector<std::pair<VersionRange, SourceSpan>>>
    ErrNameNotFoundInVersionRange("cannot find '{0}' in {1} {2}; did find it {3}");
constexpr ErrorDef<221> ErrResourceForbiddenHere(
    "'resource' appears in declaration annotated '@no_resource'");
constexpr ErrorDef<222> ErrExperimentalNoResource(
    "'@no_resource' is an experimental attribute that must be enabled "
    "with --experimental no_resource_attribute");
constexpr ErrorDef<223, std::string_view, std::string_view> ErrNoResourceForbidsCompose(
    "'{0}' has the '@no_resource` attribute, and thus "
    "cannot compose '{1}' unless it is also has the '@no_resource' attribute");

// To add a new error:
//
// 1. Define it above using N = last error's number + 1
// 2. Add it to the end of kAllDiagnosticDefs below
// 3. Run $FUCHSIA_DIR/tools/fidl/scripts/add_errcat_entry.py N
//
// To retire an error:
//
// 1. Change its definition above to be a RetiredDef
// 2. Run $FUCHSIA_DIR/tools/fidl/scripts/add_errcat_entry.py -r N

static constexpr const DiagnosticDef *kAllDiagnosticDefs[] = {
    /* fi-0001 */ &ErrInvalidCharacter,
    /* fi-0002 */ &ErrUnexpectedLineBreak,
    /* fi-0003 */ &ErrInvalidEscapeSequence,
    /* fi-0004 */ &ErrInvalidHexDigit,
    /* fi-0005 */ &ErrInvalidOctDigit,
    /* fi-0006 */ &ErrExpectedDeclaration,
    /* fi-0007 */ &ErrUnexpectedToken,
    /* fi-0008 */ &ErrUnexpectedTokenOfKind,
    /* fi-0009 */ &ErrUnexpectedIdentifier,
    /* fi-0010 */ &ErrInvalidIdentifier,
    /* fi-0011 */ &ErrInvalidLibraryNameComponent,
    /* fi-0012 */ &ErrInvalidLayoutClass,
    /* fi-0013 */ &ErrInvalidWrappedType,
    /* fi-0014 */ &ErrAttributeWithEmptyParens,
    /* fi-0015 */ &ErrAttributeArgsMustAllBeNamed,
    /* fi-0016 */ &ErrMissingOrdinalBeforeMember,
    /* fi-0017 */ &ErrOrdinalOutOfBound,
    /* fi-0018 */ &ErrOrdinalsMustStartAtOne,
    /* fi-0019 */ &ErrMustHaveOneMember,
    /* fi-0020 */ &ErrInvalidProtocolMember,
    /* fi-0021 */ &ErrExpectedProtocolMember,
    /* fi-0022 */ &ErrCannotAttachAttributeToIdentifier,
    /* fi-0023 */ &ErrAttributeInsideTypeDeclaration,
    /* fi-0024 */ &ErrDocCommentOnParameters,
    /* fi-0025 */ &ErrLibraryImportsMustBeGroupedAtTopOfFile,
    /* fi-0026 */ &WarnCommentWithinDocCommentBlock,
    /* fi-0027 */ &WarnBlankLinesWithinDocCommentBlock,
    /* fi-0028 */ &WarnDocCommentMustBeFollowedByDeclaration,
    /* fi-0029 */ &ErrMustHaveOneProperty,
    /* fi-0030 */ &ErrCannotSpecifyModifier,
    /* fi-0031 */ &ErrCannotSpecifySubtype,
    /* fi-0032 */ &ErrDuplicateModifier,
    /* fi-0033 */ &ErrConflictingModifier,
    /* fi-0034 */ &ErrNameCollision,
    /* fi-0035 */ &ErrNameCollisionCanonical,
    /* fi-0036 */ &ErrNameOverlap,
    /* fi-0037 */ &ErrNameOverlapCanonical,
    /* fi-0038 */ &ErrDeclNameConflictsWithLibraryImport,
    /* fi-0039 */ &ErrDeclNameConflictsWithLibraryImportCanonical,
    /* fi-0040 */ &ErrFilesDisagreeOnLibraryName,
    /* fi-0041 */ &ErrMultipleLibrariesWithSameName,
    /* fi-0042 */ &ErrDuplicateLibraryImport,
    /* fi-0043 */ &ErrConflictingLibraryImport,
    /* fi-0044 */ &ErrConflictingLibraryImportAlias,
    /* fi-0045 */ &ErrAttributesNotAllowedOnLibraryImport,
    /* fi-0046 */ &ErrUnknownLibrary,
    /* fi-0047 */ &ErrProtocolComposedMultipleTimes,
    /* fi-0048 */ &ErrOptionalTableMember,
    /* fi-0049 */ &ErrOptionalUnionMember,
    /* fi-0050 */ &ErrDeprecatedStructDefaults,
    /* fi-0051 */ &ErrUnknownDependentLibrary,
    /* fi-0052 */ &ErrNameNotFound,
    /* fi-0053 */ &ErrCannotReferToMember,
    /* fi-0054 */ &ErrMemberNotFound,
    /* fi-0055 */ &ErrInvalidReferenceToDeprecated,
    /* fi-0056 */ &ErrInvalidReferenceToDeprecatedOtherPlatform,
    /* fi-0057 */ &ErrIncludeCycle,
    /* fi-0058 */ &ErrAnonymousNameReference,
    /* fi-0059 */ &ErrInvalidConstantType,
    /* fi-0060 */ &ErrCannotResolveConstantValue,
    /* fi-0061 */ &ErrOrOperatorOnNonPrimitiveValue,
    /* fi-0062 */ &ErrNewTypesNotAllowed,
    /* fi-0063 */ &ErrExpectedValueButGotType,
    /* fi-0064 */ &ErrMismatchedNameTypeAssignment,
    /* fi-0065 */ &ErrTypeCannotBeConvertedToType,
    /* fi-0066 */ &ErrConstantOverflowsType,
    /* fi-0067 */ &ErrBitsMemberMustBePowerOfTwo,
    /* fi-0068 */ &ErrFlexibleEnumMemberWithMaxValue,
    /* fi-0069 */ &ErrBitsTypeMustBeUnsignedIntegralPrimitive,
    /* fi-0070 */ &ErrEnumTypeMustBeIntegralPrimitive,
    /* fi-0071 */ &ErrUnknownAttributeOnStrictEnumMember,
    /* fi-0072 */ &ErrUnknownAttributeOnMultipleEnumMembers,
    /* fi-0073 */ &ErrComposingNonProtocol,
    /* fi-0074 */ &ErrInvalidMethodPayloadLayoutClass,
    /* fi-0075 */ &ErrInvalidMethodPayloadType,
    /* fi-0076 */ &ErrResponsesWithErrorsMustNotBeEmpty,
    /* fi-0077 */ &ErrEmptyPayloadStructs,
    /* fi-0078 */ &ErrDuplicateElementName,
    /* fi-0079 */ &ErrDuplicateElementNameCanonical,
    /* fi-0080 */ &ErrGeneratedZeroValueOrdinal,
    /* fi-0081 */ &ErrDuplicateMethodOrdinal,
    /* fi-0082 */ &ErrInvalidSelectorValue,
    /* fi-0083 */ &ErrFuchsiaIoExplicitOrdinals,
    /* fi-0084 */ &ErrPayloadStructHasDefaultMembers,
    /* fi-0085 */ &ErrDuplicateServiceMemberName,
    /* fi-0086 */ &ErrStrictUnionMustHaveNonReservedMember,
    /* fi-0087 */ &ErrDuplicateServiceMemberNameCanonical,
    /* fi-0088 */ &ErrOptionalServiceMember,
    /* fi-0089 */ &ErrDuplicateStructMemberName,
    /* fi-0090 */ &ErrDuplicateStructMemberNameCanonical,
    /* fi-0091 */ &ErrInvalidStructMemberType,
    /* fi-0092 */ &ErrTableOrdinalTooLarge,
    /* fi-0093 */ &ErrMaxOrdinalNotTable,
    /* fi-0094 */ &ErrDuplicateTableFieldOrdinal,
    /* fi-0095 */ &ErrDuplicateTableFieldName,
    /* fi-0096 */ &ErrDuplicateTableFieldNameCanonical,
    /* fi-0097 */ &ErrDuplicateUnionMemberOrdinal,
    /* fi-0098 */ &ErrDuplicateUnionMemberName,
    /* fi-0099 */ &ErrDuplicateUnionMemberNameCanonical,
    /* fi-0100 */ &ErrNonDenseOrdinal,
    /* fi-0101 */ &ErrCouldNotResolveSizeBound,
    /* fi-0102 */ &ErrCouldNotResolveMember,
    /* fi-0103 */ &ErrCouldNotResolveMemberDefault,
    /* fi-0104 */ &ErrCouldNotResolveAttributeArg,
    /* fi-0105 */ &ErrDuplicateMemberName,
    /* fi-0106 */ &ErrDuplicateMemberNameCanonical,
    /* fi-0107 */ &ErrDuplicateMemberValue,
    /* fi-0108 */ &ErrDuplicateResourcePropertyName,
    /* fi-0109 */ &ErrDuplicateResourcePropertyNameCanonical,
    /* fi-0110 */ &ErrTypeMustBeResource,
    /* fi-0111 */ &ErrInlineSizeExceedsLimit,
    /* fi-0112 */ &ErrOnlyClientEndsInServices,
    /* fi-0113 */ &ErrMismatchedTransportInServices,
    /* fi-0114 */ &ErrComposedProtocolTooOpen,
    /* fi-0115 */ &ErrFlexibleTwoWayMethodRequiresOpenProtocol,
    /* fi-0116 */ &ErrFlexibleOneWayMethodInClosedProtocol,
    /* fi-0117 */ &ErrHandleUsedInIncompatibleTransport,
    /* fi-0118 */ &ErrTransportEndUsedInIncompatibleTransport,
    /* fi-0119 */ &ErrEventErrorSyntax,
    /* fi-0120 */ &ErrInvalidAttributePlacement,
    /* fi-0121 */ &ErrDeprecatedAttribute,
    /* fi-0122 */ &ErrDuplicateAttribute,
    /* fi-0123 */ &ErrDuplicateAttributeCanonical,
    /* fi-0124 */ &ErrCanOnlyUseStringOrBool,
    /* fi-0125 */ &ErrAttributeArgMustNotBeNamed,
    /* fi-0126 */ &ErrAttributeArgNotNamed,
    /* fi-0127 */ &ErrMissingRequiredAttributeArg,
    /* fi-0128 */ &ErrMissingRequiredAnonymousAttributeArg,
    /* fi-0129 */ &ErrUnknownAttributeArg,
    /* fi-0130 */ &ErrDuplicateAttributeArg,
    /* fi-0131 */ &ErrDuplicateAttributeArgCanonical,
    /* fi-0132 */ &ErrAttributeDisallowsArgs,
    /* fi-0133 */ &ErrAttributeArgRequiresLiteral,
    /* fi-0134 */ &ErrAttributeConstraintNotSatisfied,
    /* fi-0135 */ &ErrInvalidDiscoverableName,
    /* fi-0136 */ &ErrTableCannotBeSimple,
    /* fi-0137 */ &ErrUnionCannotBeSimple,
    /* fi-0138 */ &ErrElementMustBeSimple,
    /* fi-0139 */ &ErrTooManyBytes,
    /* fi-0140 */ &ErrTooManyHandles,
    /* fi-0141 */ &ErrInvalidErrorType,
    /* fi-0142 */ &ErrInvalidTransportType,
    /* fi-0143 */ &ErrBoundIsTooBig,
    /* fi-0144 */ &ErrUnableToParseBound,
    /* fi-0145 */ &WarnAttributeTypo,
    /* fi-0146 */ &ErrInvalidGeneratedName,
    /* fi-0147 */ &ErrAvailableMissingArguments,
    /* fi-0148 */ &ErrNoteWithoutDeprecationOrRemoval,
    /* fi-0149 */ &ErrPlatformNotOnLibrary,
    /* fi-0150 */ &ErrLibraryAvailabilityMissingAdded,
    /* fi-0151 */ &ErrMissingLibraryAvailability,
    /* fi-0152 */ &ErrInvalidPlatform,
    /* fi-0153 */ &ErrInvalidVersion,
    /* fi-0154 */ &ErrInvalidAvailabilityOrder,
    /* fi-0155 */ &ErrAvailabilityConflictsWithParent,
    /* fi-0156 */ &ErrCannotBeOptional,
    /* fi-0157 */ &ErrMustBeAProtocol,
    /* fi-0158 */ &ErrCannotBoundTwice,
    /* fi-0159 */ &ErrStructCannotBeOptional,
    /* fi-0160 */ &ErrCannotIndicateOptionalTwice,
    /* fi-0161 */ &ErrMustHaveNonZeroSize,
    /* fi-0162 */ &ErrWrongNumberOfLayoutParameters,
    /* fi-0163 */ &ErrMultipleConstraintDefinitions,
    /* fi-0164 */ &ErrTooManyConstraints,
    /* fi-0165 */ &ErrExpectedType,
    /* fi-0166 */ &ErrUnexpectedConstraint,
    /* fi-0167 */ &ErrCannotConstrainTwice,
    /* fi-0168 */ &ErrProtocolConstraintRequired,
    /* fi-0169 */ &ErrBoxCannotBeOptional,
    /* fi-0170 */ &ErrBoxedTypeCannotBeOptional,
    /* fi-0171 */ &ErrCannotBeBoxedShouldBeOptional,
    /* fi-0172 */ &ErrResourceMustBeUint32Derived,
    /* fi-0173 */ &ErrResourceMissingSubtypeProperty,
    /* fi-0174 */ &ErrResourceMissingRightsProperty,
    /* fi-0175 */ &ErrResourceSubtypePropertyMustReferToEnum,
    /* fi-0176 */ &ErrHandleSubtypeMustReferToResourceSubtype,
    /* fi-0177 */ &ErrResourceRightsPropertyMustReferToBits,
    /* fi-0178 */ &ErrUnusedImport,
    /* fi-0179 */ &ErrNewTypeCannotHaveConstraint,
    /* fi-0180 */ &ErrExperimentalZxCTypesDisallowed,
    /* fi-0181 */ &ErrReferenceInLibraryAttribute,
    /* fi-0182 */ &ErrLegacyWithoutRemoval,
    /* fi-0183 */ &ErrLegacyConflictsWithParent,
    /* fi-0184 */ &ErrUnexpectedControlCharacter,
    /* fi-0185 */ &ErrUnicodeEscapeMissingBraces,
    /* fi-0186 */ &ErrUnicodeEscapeUnterminated,
    /* fi-0187 */ &ErrUnicodeEscapeEmpty,
    /* fi-0188 */ &ErrUnicodeEscapeTooLong,
    /* fi-0189 */ &ErrUnicodeEscapeTooLarge,
    /* fi-0190 */ &ErrSimpleProtocolMustBeClosed,
    /* fi-0191 */ &ErrMethodMustDefineStrictness,
    /* fi-0192 */ &ErrProtocolMustDefineOpenness,
    /* fi-0193 */ &ErrCannotBeBoxedNorOptional,
    /* fi-0194 */ &ErrEmptyPayloadStructsWhenResultUnion,
    /* fi-0195 */ &ErrExperimentalOverflowingAttributeMissingExperimentalFlag,
    /* fi-0196 */ &ErrExperimentalOverflowingIncorrectUsage,
    /* fi-0197 */ &ErrOverlayMustBeStrict,
    /* fi-0198 */ &ErrOverlayMustBeValue,
    /* fi-0199 */ &ErrOverlayMemberMustBeValue,
    /* fi-0200 */ &ErrOverlayMustNotContainReserved,
    /* fi-0201 */ &ErrPlatformVersionNotSelected,
    /* fi-0202 */ &ErrTransitionalNotAllowed,
    /* fi-0203 */ &ErrRemovedAndReplaced,
    /* fi-0204 */ &ErrLibraryReplaced,
    /* fi-0205 */ &ErrInvalidRemoved,
    /* fi-0206 */ &ErrInvalidReplaced,
    /* fi-0207 */ &ErrTypeShapeIntegerOverflow,
    /* fi-0208 */ &ErrReservedPlatform,
    /* fi-0209 */ &ErrReservedNotAllowed,
    /* fi-0210 */ &ErrInvalidDiscoverableLocation,
    /* fi-0211 */ &ErrCannotBeRenamed,
    /* fi-0212 */ &ErrRenamedWithoutReplacedOrRemoved,
    /* fi-0213 */ &ErrRenamedToSameName,
    /* fi-0214 */ &ErrInvalidRemovedAndRenamed,
    /* fi-0215 */ &ErrInvalidReplacedAndRenamed,
    /* fi-0216 */ &ErrInvalidRemovedAbi,
    /* fi-0217 */ &ErrInvalidReplacedAbi,
    /* fi-0218 */ &ErrInvalidModifierAvailableArgument,
    /* fi-0219 */ &ErrCannotChangeMethodStrictness,
    /* fi-0220 */ &ErrNameNotFoundInVersionRange,
    /* fi-0221 */ &ErrResourceForbiddenHere,
    /* fi-0222 */ &ErrExperimentalNoResource,
    /* fi-0223 */ &ErrNoResourceForbidsCompose,
};

// In reporter.h we assert that reported error IDs are <= kNumDiagnosticDefs.
// This combined with the assert below ensures kAllDiagnosticDefs is complete.
static constexpr size_t kNumDiagnosticDefs =
    sizeof(kAllDiagnosticDefs) / sizeof(kAllDiagnosticDefs[0]);

// If anything is missing or out of order in kAllDiagnosticDefs, this assertion
// will fail and the message will include the position where it happened.
static_assert([] {
  ErrorId expected = 1;
  for (auto def : kAllDiagnosticDefs) {
    if (def->id != expected) {
      return expected;
    }
    expected++;
  }
  return 0u;
}() == 0);

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_DIAGNOSTICS_H_
