// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::test::*;
use super::*;
use crate::Version;
use Optionality::*;
use StringPattern::*;
use Type::*;

#[test]
fn primitive() {
    assert!(compare_fidl_type(
        "PrimitiveStruct",
        "
    type PrimitiveStruct = struct {
        @available(replaced=2)
        value bool;
        @available(added=2)
        value bool;
    };",
    )
    .is_compatible());

    assert!(compare_fidl_type(
        "PrimitiveStruct",
        "
    type PrimitiveStruct = struct {
        @available(replaced=2)
        value bool;
        @available(added=2)
        value int8;
    };",
    )
    .has_problems(vec![
        ProblemPattern::error()
            .message("Incompatible primitive types, sender(@1):bool, receiver(@2):int8"),
        ProblemPattern::error()
            .message("Incompatible primitive types, sender(@2):int8, receiver(@1):bool")
    ]));
}

#[test]
fn string() {
    assert!(compare_fidl_type(
        "Strings",
        "
        type Strings = struct {
            // Lengths
            same_length string:10;
            @available(replaced=2)
            change_length string:10;
            @available(added=2)
            change_length string:20;

            // Optionality
            @available(replaced=2)
            become_optional string:10;
            @available(added=2)
            become_optional string:<10,optional>;
        };"
    )
    .has_problems(vec![
        ProblemPattern::error()
            .message("Incompatible string lengths, sender(@2):20, receiver(@1):10")
            .path(Ends(".change_length")),
        ProblemPattern::error()
            .message("Sender(@2) string is optional but receiver(@1) is required")
            .path(Ends(".become_optional"))
    ]));
}

#[test]
fn enums() {
    // Primitive type
    assert!(compare_fidl_type(
        "Enum",
        "
        @available(replaced=2)
        type Enum = enum : uint32 { M = 1; };
        @available(added=2)
        type Enum = enum : int32 { M = 1; };"
    )
    .has_problems(vec![
        ProblemPattern::error()
            .message("Incompatible enum types, sender(@1):uint32, receiver(@2):int32"),
        ProblemPattern::error()
            .message("Incompatible enum types, sender(@2):int32, receiver(@1):uint32")
    ]));

    // Strictness difference
    assert!(compare_fidl_type(
        "Enum",
        "
        @available(replaced=2)
        type Enum = strict enum {
            A = 1;
        };
        @available(added=2)
        type Enum = flexible enum {
            A = 1;
        };
        "
    )
    .is_compatible());

    // Strict member difference
    assert!(compare_fidl_type(
        "Enum",
        "
        type Enum = strict enum {
            A = 1;
            @available(added=2)
            B = 2;
        };"
    )
    .has_problems(vec![
        ProblemPattern::error().message("Extra strict enum members in sender(@2): B=2")
    ]));

    // Flexible member difference
    assert!(compare_fidl_type(
        "Enum",
        "
                type Enum = flexible enum {
                    A = 1;
                    @available(added=2)
                    B = 2;
                };"
    )
    .has_problems(vec![
        ProblemPattern::warning().message("Extra flexible enum members in sender(@2): B=2")
    ]));

    // Strictness difference, member difference.
    assert!(compare_fidl_type(
        "Enum",
        "
        @available(replaced=2)
        type Enum = strict enum {
            A = 1;
            B = 2;
        };
        @available(added=2)
        type Enum = flexible enum {
            A = 1;
            C = 3;
        };
        "
    )
    .has_problems(vec![
        ProblemPattern::warning().message("Extra flexible enum members in sender(@1): B=2"),
        ProblemPattern::error().message("Extra strict enum members in sender(@2): C=3")
    ]));

    // Primitive conversion
    assert!(compare_fidl_type(
        "Wrapper",
        "
        @available(replaced=2)
        type Enum = enum : uint32 { M = 1; };
        @available(added=2)
        alias Enum = uint32;
        type Wrapper = struct { e Enum; };
        "
    )
    .has_problems(vec![
        ProblemPattern::error().message(
            "Incompatible types, sender(@1):flexible enum : uint32 { 1 }, receiver(@2):uint32"
        ),
        ProblemPattern::error().message(
            "Incompatible types, sender(@2):uint32, receiver(@1):flexible enum : uint32 { 1 }"
        )
    ]));
}

#[test]
fn bits() {
    // Primitive type
    assert!(compare_fidl_type(
        "Bits",
        "
        @available(replaced=2)
        type Bits = bits : uint32 { M = 1; };
        @available(added=2)
        type Bits = bits : uint64 { M = 1; };"
    )
    .has_problems(vec![
        ProblemPattern::error()
            .message("Incompatible bits types, sender(@1):uint32, receiver(@2):uint64"),
        ProblemPattern::error()
            .message("Incompatible bits types, sender(@2):uint64, receiver(@1):uint32")
    ]));

    // Strictness difference
    assert!(compare_fidl_type(
        "Bits",
        "
        @available(replaced=2)
        type Bits = strict bits {
            A = 1;
        };
        @available(added=2)
        type Bits = flexible bits {
            A = 1;
        };
        "
    )
    .is_compatible());

    // Strict member difference
    assert!(compare_fidl_type(
        "Bits",
        "
        type Bits = strict bits {
            A = 1;
            @available(added=2)
            B = 2;
        };"
    )
    .has_problems(vec![
        ProblemPattern::error().message("Extra strict bits members in sender(@2): B=2")
    ]));

    // Flexible member difference
    assert!(compare_fidl_type(
        "Bits",
        "
                type Bits = flexible bits {
                    A = 1;
                    @available(added=2)
                    B = 2;
                };"
    )
    .has_problems(vec![
        ProblemPattern::warning().message("Extra flexible bits members in sender(@2): B=2")
    ]));

    // Strictness difference, member difference.
    assert!(compare_fidl_type(
        "Bits",
        "
        @available(replaced=2)
        type Bits = strict bits {
            A = 1;
            B = 2;
        };
        @available(added=2)
        type Bits = flexible bits {
            A = 1;
            C = 4;
        };
        "
    )
    .has_problems(vec![
        ProblemPattern::warning().message("Extra flexible bits members in sender(@1): B=2"),
        ProblemPattern::error().message("Extra strict bits members in sender(@2): C=4")
    ]));

    // Primitive conversion
    assert!(compare_fidl_type(
        "Wrapper",
        "
        @available(replaced=2)
        type Bits = bits : uint32 { M = 1; };
        @available(added=2)
        alias Bits = uint32;
        type Wrapper = struct { e Bits; };
        "
    )
    .has_problems(vec![
        ProblemPattern::error().message(
            "Incompatible types, sender(@1):flexible bits : uint32 { 1 }, receiver(@2):uint32"
        ),
        ProblemPattern::error().message(
            "Incompatible types, sender(@2):uint32, receiver(@1):flexible bits : uint32 { 1 }"
        )
    ]));
}

#[test]
fn handle() {
    use HandleType::*;

    // Can't use compare_fidl_type because we don't have access to library zx.
    pub fn do_compare_types(sender: &Type, receiver: &Type) -> CompatibilityProblems {
        compare_types(sender, receiver, &Default::default())
    }

    let untyped = Handle(Path::empty(), None, Required, HandleRights::default());
    let channel = Handle(Path::empty(), Some(Channel), Required, HandleRights::default());
    let vmo = Handle(Path::empty(), Some(VMO), Required, HandleRights::default());

    // Handle types
    assert!(do_compare_types(&untyped, &untyped).is_compatible());
    assert!(do_compare_types(&channel, &untyped).is_compatible());
    assert!(do_compare_types(&channel, &channel).is_compatible());
    assert!(do_compare_types(&untyped, &channel)
        .has_problems(vec![ProblemPattern::error().message(
            "Incompatible handle types, sender(@0):zx.Handle:CHANNEL, receiver(@0):zx.Handle"
        )]));
    assert!(do_compare_types(&vmo, &channel).has_problems(vec![ProblemPattern::error().message(
        "Incompatible handle types, sender(@0):zx.Handle:CHANNEL, receiver(@0):zx.Handle:VMO"
    )]));

    // Optionality
    assert!(do_compare_types(
        &Handle(Path::empty(), Some(Channel), Required, HandleRights::default()),
        &Handle(Path::empty(), Some(Channel), Required, HandleRights::default())
    )
    .is_compatible());
    assert!(do_compare_types(
        &Handle(Path::empty(), Some(Channel), Optional, HandleRights::default()),
        &Handle(Path::empty(), Some(Channel), Optional, HandleRights::default())
    )
    .is_compatible());
    assert!(do_compare_types(
        &Handle(Path::empty(), Some(Channel), Required, HandleRights::default()),
        &Handle(Path::empty(), Some(Channel), Optional, HandleRights::default())
    )
    .is_compatible());

    assert!(do_compare_types(
        &Handle(Path::empty(), Some(Channel), Optional, HandleRights::default()),
        &Handle(Path::empty(), Some(Channel), Required, HandleRights::default())
    )
    .has_problems(vec![ProblemPattern::error()
        .message("Sender handle(@0) is optional but receiver(@0) is required")]));

    // Handle rights
    assert!(compare_fidl_type(
        "HandleRights",
        "
        using zx;
        type HandleRights = resource struct {
            @available(replaced=2)
            no_rights_to_some_rights zx.Handle:CHANNEL;
            @available(added=2)
            no_rights_to_some_rights zx.Handle:<CHANNEL, zx.Rights.WRITE | zx.Rights.READ>;

            @available(replaced=2)
            reduce_rights zx.Handle:<CHANNEL, zx.Rights.WRITE | zx.Rights.READ>;
            @available(added=2)
            reduce_rights zx.Handle:<CHANNEL, zx.Rights.WRITE>;
        };
    "
    )
    .has_problems(vec![
        ProblemPattern::warning()
            .path(Ends("HandleRights.no_rights_to_some_rights"))
            .message("Sender(@1) doesn't specify handle rights but receiver(@2) does: HandleRights(READ | WRITE)"),
        ProblemPattern::error()
            .path(Ends("HandleRights.reduce_rights"))
            .message("Incompatible handle rights, sender(@2):HandleRights(WRITE), receiver(@1):HandleRights(READ | WRITE)"),
    ]));
}

#[test]
fn array() {
    assert!(compare_fidl_type(
        "Arrays",
        "
        type Arrays = table {
            @available(replaced=2)
            1: size_changed array<uint32, 10>;
            @available(added=2)
            1: size_changed array<uint32, 20>;

            @available(replaced=2)
            2: member_incompatible array<uint32, 10>;
            @available(added=2)
            2: member_incompatible array<float32, 10>;

            @available(replaced=2)
            3: member_soft_change array<flexible enum { M = 1; }, 10>;
            @available(added=2)
            3: member_soft_change array<flexible enum { M = 1; N = 2; }, 10>;
        };
    "
    )
    .has_problems(vec![
        ProblemPattern::error()
            .path(Ends("Arrays.size_changed"))
            .message("Array length mismatch, sender(@1):10, receiver(@2):20"),
        ProblemPattern::error()
            .path(Ends("Arrays.size_changed"))
            .message("Array length mismatch, sender(@2):20, receiver(@1):10"),
        ProblemPattern::error()
            .path(Ends("Arrays.member_incompatible[]"))
            .message("Incompatible primitive types, sender(@1):uint32, receiver(@2):float32"),
        ProblemPattern::error()
            .path(Ends("Arrays.member_incompatible[]"))
            .message("Incompatible primitive types, sender(@2):float32, receiver(@1):uint32"),
        ProblemPattern::warning()
            .path(Ends("Arrays.member_soft_change[]"))
            .message("Extra flexible enum members in sender(@2): N=2")
    ]));
}

#[test]
fn vector() {
    assert!(compare_fidl_type(
        "Vectors",
        "
        type Vectors = struct {
            @available(replaced=2)
            size_changed vector<uint32>:10;
            @available(added=2)
            size_changed vector<uint32>:20;

            @available(replaced=2)
            member_incompatible vector<uint32>:10;
            @available(added=2)
            member_incompatible vector<float32>:10;

            @available(replaced=2)
            member_soft_change vector<flexible enum { M = 1; }>:10;
            @available(added=2)
            member_soft_change vector<flexible enum { M = 1; N = 2; }>:10;
        };
    "
    )
    .has_problems(vec![
        ProblemPattern::error().path(Ends("Vectors.size_changed")).message(
            "Sender vector is larger than receiver vector, sender(@2):20, receiver(@1):10"
        ),
        ProblemPattern::error()
            .path(Ends("Vectors.member_incompatible[]"))
            .message("Incompatible primitive types, sender(@1):uint32, receiver(@2):float32"),
        ProblemPattern::error()
            .path(Ends("Vectors.member_incompatible[]"))
            .message("Incompatible primitive types, sender(@2):float32, receiver(@1):uint32"),
        ProblemPattern::warning()
            .path(Ends("Vectors.member_soft_change[]"))
            .message("Extra flexible enum members in sender(@2): N=2")
    ]));
}

#[test]
fn structs() {
    // Number of members
    assert!(compare_fidl_type(
        "NumMembers",
        "
        type NumMembers = struct {
            one int32;
            @available(replaced=2, renamed=\"four\")
            two int16;
            @available(removed=2)
            three int16;
            @available(added=2)
            four int32;
        };
    "
    )
    .has_problems(vec![
        ProblemPattern::error()
            .message("Struct has different number of members, sender(@1):3, receiver(@2):2"),
        ProblemPattern::error()
            .message("Struct has different number of members, sender(@2):2, receiver(@1):3"),
        ProblemPattern::error()
            .message("Incompatible primitive types, sender(@1):int16, receiver(@2):int32"),
        ProblemPattern::error()
            .message("Incompatible primitive types, sender(@2):int32, receiver(@1):int16"),
    ]));

    // Member Compatibility
    assert!(compare_fidl_type(
        "MemberCompat",
        "
        type MemberCompat = struct {
            @available(replaced=2)
            weak flexible enum { M = 1; };
            @available(added=2)
            weak flexible enum { M = 1; N = 2; };

            @available(replaced=2)
            strong int32;
            @available(added=2)
            strong float32;
        };
    "
    )
    .has_problems(vec![
        ProblemPattern::error()
            .path(Ends(".strong"))
            .message("Incompatible primitive types, sender(@1):int32, receiver(@2):float32"),
        ProblemPattern::error()
            .path(Ends(".strong"))
            .message("Incompatible primitive types, sender(@2):float32, receiver(@1):int32"),
        ProblemPattern::warning()
            .path(Ends(".weak"))
            .message("Extra flexible enum members in sender(@2): N=2")
    ]));
}

#[test]
fn table() {
    assert!(compare_fidl_type(
        "Table",
        "
        type Table = table {
            // unchanged
            1: one uint32;

            // defined <-> absent
            @available(removed=2)
            2: two uint32;

            // incompatible types
            @available(replaced=2)
            3: three string;
            @available(added=2)
            3: three int32;

            // weakly compatible types
            @available(replaced=2)
            4: four flexible enum { M = 1; };
            @available(added=2)
            4: four flexible enum { M = 1; N = 2; };

            // missing member
            @available(removed=2)
            5: five float64;
        };
    "
    )
    .has_problems(vec![
        ProblemPattern::warning()
            .path(Ends("/Table"))
            .message("Table in sender(@1) has members that receiver(@2) does not have."),
        ProblemPattern::error()
            .path(Ends("Table.three"))
            .message("Incompatible types, sender(@1):string:65535, receiver(@2):int32"),
        ProblemPattern::error()
            .path(Ends("Table.three"))
            .message("Incompatible types, sender(@2):int32, receiver(@1):string:65535"),
        ProblemPattern::warning()
            .path(Ends("Table.four"))
            .message("Extra flexible enum members in sender(@2): N=2"),
    ]));
}

#[test]
fn union() {
    // TODO: strict unions

    assert!(compare_fidl_type(
        "Union",
        "
        type Union = union {
            // unchanged
            1: one uint32;

            // defined <-> absent
            @available(removed=2)
            2: two uint32;

            // incompatible types
            @available(replaced=2)
            3: three string;
            @available(added=2)
            3: three int32;

            // weakly compatible types
            @available(replaced=2)
            4: four flexible enum { M = 1; };
            @available(added=2)
            4: four flexible enum { M = 1; N = 2; };

            // missing member
            @available(removed=2)
            5: five float64;
        };
    "
    )
    .has_problems(vec![
        ProblemPattern::warning()
            .path(Ends("/Union"))
            .message("Union in sender(@1) has members that union in receiver(@2) does not have."),
        ProblemPattern::error()
            .path(Ends("Union.three"))
            .message("Incompatible types, sender(@1):string:65535, receiver(@2):int32"),
        ProblemPattern::error()
            .path(Ends("Union.three"))
            .message("Incompatible types, sender(@2):int32, receiver(@1):string:65535"),
        ProblemPattern::warning()
            .path(Ends("Union.four"))
            .message("Extra flexible enum members in sender(@2): N=2"),
    ]));
}

#[test]
fn similar() {
    assert!(compare_fidl_type(
        "Similar",
        "
    @available(replaced=2)
    type Similar = flexible enum : uint32 {};
    @available(added=2)
    type Similar = flexible bits : uint32 {};
    "
    )
    .has_problems(vec![
        ProblemPattern::error().message("Incompatible types, sender(@2):flexible bits : uint32 {  }, receiver(@1):flexible enum : uint32 {  }"),
        ProblemPattern::error().message("Incompatible types, sender(@1):flexible enum : uint32 {  }, receiver(@2):flexible bits : uint32 {  }")
    ]));
    assert!(compare_fidl_type(
        "Similar",
        "
    @available(replaced=2)
    type Similar = table {};
    @available(added=2)
    type Similar = flexible union {};
    "
    )
    .has_problems(vec![
        ProblemPattern::error()
            .message("Incompatible types, sender(@2):flexible union {  }, receiver(@1):table {  }"),
        ProblemPattern::error()
            .message("Incompatible types, sender(@1):table {  }, receiver(@2):flexible union {  }")
    ]));
}

#[test]
fn endpoints() {
    let source = "
    closed protocol Incompatible {
        @available(removed=2)
        strict Foo();
        @available(added=2)
        strict Bar();
    };

    type ClientIncompatible = resource table {
        1: ep client_end:Incompatible;
    };

    type ServerIncompatible = resource table {
        1: ep server_end:Incompatible;
    };

    closed protocol AddMethod {
        @available(added=2)
        strict M();
    };

    type ClientAddMethod = resource table {
        1: ep client_end:AddMethod;
    };

    type ServerAddMethod = resource table {
        1: ep server_end:AddMethod;
    };

    closed protocol RemoveMethod {
        @available(added=2)
        strict M();
    };

    type ClientRemoveMethod = resource table {
        1: ep client_end:RemoveMethod;
    };

    type ServerRemoveMethod = resource table {
        1: ep server_end:RemoveMethod;
    };
    ";

    let versions = vec![TypeVersions { send: Version::new("1"), recv: Version::new("2") }];

    assert!(compare_fidl_type_between("ClientIncompatible", &versions, source)
        .has_problems(vec![ProblemPattern::error()
            .message(Equals("Server(@1) missing method fuchsia.compat.test/Incompatible.Bar"))]));

    assert!(compare_fidl_type_between("ServerIncompatible", &versions, source)
        .has_problems(vec![ProblemPattern::error()
            .message(Equals("Server(@2) missing method fuchsia.compat.test/Incompatible.Foo"))]));

    assert!(compare_fidl_type_between("ClientAddMethod", &versions, source)
        .has_problems(vec![ProblemPattern::error()
            .message(Equals("Server(@1) missing method fuchsia.compat.test/AddMethod.M"))]));

    assert!(compare_fidl_type_between("ServerAddMethod", &versions, source).has_problems(vec![]));

    assert!(compare_fidl_type_between("ClientRemoveMethod", &versions, source)
        .has_problems(vec![ProblemPattern::error()
            .message(Equals("Server(@1) missing method fuchsia.compat.test/RemoveMethod.M"))]));

    assert!(compare_fidl_type_between("ServerRemoveMethod", &versions, source).has_problems(vec![]));
}
