// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{
    CompatibilityProblems, Flexibility, HandleRights, HandleType, Optionality, Path, Primitive,
    Protocol, Transport,
};
use crate::Configuration;

use flyweights::FlyStr;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

fn value_list(values: &BTreeSet<i128>) -> String {
    let v: Vec<String> = values.iter().sorted().map(|o| format!("{o}")).collect();
    v.join(", ")
}

fn member_list(members: &BTreeMap<u64, Type>) -> String {
    let members: Vec<String> =
        members.keys().sorted().map(|o| format!("{o}: {}", members[o])).collect();
    members.join(", ")
}

#[derive(PartialEq, PartialOrd, Debug, Clone)]
pub enum Type {
    Primitive(Path, Primitive),
    String(Path, u64, Optionality),
    Enum(Path, Flexibility, Primitive, BTreeSet<i128>, BTreeMap<i128, FlyStr>),
    Bits(Path, Flexibility, Primitive, BTreeSet<i128>, BTreeMap<i128, FlyStr>),
    Handle(Path, Option<HandleType>, Optionality, HandleRights),
    ClientEnd(Path, String, Transport, Optionality, Box<Protocol>),
    ServerEnd(Path, String, Transport, Optionality, Box<Protocol>),
    Array(Path, u64, Box<Type>),
    Vector(Path, u64, Box<Type>, Optionality),
    Struct(Path, Vec<Type>),
    Table(Path, BTreeMap<u64, Type>),
    Union(Path, Flexibility, BTreeMap<u64, Type>),
    Box(Path, Box<Type>),
    StringArray(Path, u64),
    /// Cycle is use to break cycles in recursive type declaration.
    /// `Path` is the path to where the cycle is being broken.
    /// `String` is the identifier used to point back to the start of the cycle.
    /// `usize` is how many path components are in the cycle.
    Cycle(Path, String, usize),
    // Internal types?
    FrameworkError(Path),
}

impl Type {
    pub fn path(&self) -> &Path {
        match self {
            Type::Primitive(path, _) => path,
            Type::String(path, _, _) => path,
            Type::Enum(path, _, _, _, _) => path,
            Type::Bits(path, _, _, _, _) => path,
            Type::Handle(path, _, _, _) => path,
            Type::ClientEnd(path, _, _, _, _) => path,
            Type::ServerEnd(path, _, _, _, _) => path,
            Type::Array(path, _, _) => path,
            Type::Vector(path, _, _, _) => path,
            Type::Struct(path, _) => path,
            Type::Table(path, _) => path,
            Type::Union(path, _, _) => path,
            Type::Box(path, _) => path,
            Type::StringArray(path, _) => path,
            Type::Cycle(path, _, _) => path,
            Type::FrameworkError(path) => path,
        }
    }
}

impl Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Primitive(_, primitive) => primitive.fmt(f),
            Type::String(_, size, opt) => match opt {
                Optionality::Optional => write!(f, "string:<{size},optional>"),
                Optionality::Required => write!(f, "string:{size}"),
            },
            Type::Enum(_, flex, primitive, members, _) => {
                write!(f, "{flex} enum : {primitive} {{ {} }}", value_list(members))
            }
            Type::Bits(_, flex, primitive, members, _) => {
                write!(f, "{flex} bits : {primitive} {{ {} }}", value_list(members))
            }
            Type::Handle(_, _, _, _) => f.write_str("zx.Handle:TODO"),
            Type::ClientEnd(_, protocol, _, _, _) => write!(f, "client_end:TODO({protocol})"),
            Type::ServerEnd(_, protocol, _, _, _) => write!(f, "server_end:TODO({protocol})"),
            Type::Array(_, _, _) => f.write_str("array:TODO"),
            Type::Vector(_, _, _, _) => f.write_str("vector:TODO"),
            Type::Struct(_, members) => {
                write!(f, "struct {{ {} }}", members.iter().map(|m| format!("{}", m)).join(", "))
            }
            Type::Table(_, members) => write!(f, "table {{ {} }}", member_list(members)),
            Type::Union(_, flex, members) => {
                write!(f, "{flex} union {{ {} }}", member_list(members))
            }
            Type::Box(_, t) => {
                write!(f, "box<{t}>")
            }
            Type::StringArray(_, element_count) => write!(f, "string_array<{element_count}>"),
            Type::Cycle(_, id, len) => write!(f, "(cycle id={id} len={len})"),
            Type::FrameworkError(_) => f.write_str("fidl.TransportError"),
        }
    }
}

pub fn compare_types(
    sender: &Type,
    receiver: &Type,
    config: &Configuration,
) -> CompatibilityProblems {
    use Flexibility::*;
    use Optionality::*;
    use Type::*;

    let mut problems = CompatibilityProblems::default();

    match (sender, receiver) {
        (Primitive(send_path, s), Primitive(recv_path, r)) => {
            let send_level = send_path.api_level();
            let recv_level = recv_path.api_level();
            if s != r {
                problems.error(
                    [send_path,
                    recv_path],
                    format!(
                        "Incompatible primitive types, sender(@{send_level}):{s}, receiver(@{recv_level}):{r}"
                    ),
                )
            }
        }
        (String(send_path, send_len, send_opt), String(recv_path, recv_len, recv_opt)) => {
            if *recv_opt == Required && *send_opt == Optional {
                let send_level = send_path.api_level();
                let recv_level = recv_path.api_level();
                problems.error(
                    [send_path,
                    recv_path],
                    format!("Sender(@{send_level}) string is optional but receiver(@{recv_level}) is required"),
                );
            }
            if send_len > recv_len {
                let send_level = send_path.api_level();
                let recv_level = recv_path.api_level();
                problems.error(
                    [send_path,
                    recv_path],
                    format!("Incompatible string lengths, sender(@{send_level}):{send_len}, receiver(@{recv_level}):{recv_len}"),
                );
            }
        }
        (
            Enum(send_path, _, send_type, send_values, send_names),
            Enum(recv_path, recv_flexibility, recv_type, recv_values, _recv_names),
        ) => {
            let send_level = send_path.api_level();
            if send_type != recv_type {
                let recv_level = recv_path.api_level();
                problems.error(
                    [send_path,
                    recv_path],
                    format!("Incompatible enum types, sender(@{send_level}):{send_type}, receiver(@{recv_level}):{recv_type}"),
                )
            } else {
                let missing_values: Vec<_> = send_values
                    .difference(recv_values)
                    .sorted()
                    .map(|v| format!("{}={v}", send_names.get(v).unwrap()))
                    .collect();
                if !missing_values.is_empty() {
                    let missing_values_str = missing_values.join(", ");
                    if recv_flexibility == &Flexible {
                        problems.warning(
                            [send_path,
                            recv_path],
                            format!("Extra flexible enum members in sender(@{send_level}): {missing_values_str}"),
                        );
                    } else {
                        problems.error(
                            [send_path,
                            recv_path],
                            format!("Extra strict enum members in sender(@{send_level}): {missing_values_str}"),
                        );
                    }
                }
            }
        }
        (
            Bits(send_path, _, send_type, send_values, send_names),
            Bits(recv_path, recv_flexibility, recv_type, recv_values, _recv_names),
        ) => {
            let send_level = send_path.api_level();
            if send_type != recv_type {
                let recv_level = recv_path.api_level();
                problems.error(
                    [send_path,
                    recv_path],
                    format!("Incompatible bits types, sender(@{send_level}):{send_type}, receiver(@{recv_level}):{recv_type}"),
                )
            } else {
                let missing_values: Vec<_> = send_values
                    .difference(recv_values)
                    .sorted()
                    .map(|v| format!("{}={v}", send_names.get(v).unwrap()))
                    .collect();
                if !missing_values.is_empty() {
                    let missing_values_str = missing_values.join(", ");
                    if recv_flexibility == &Flexible {
                        problems.warning(
                            [send_path,
                            recv_path],
                            format!("Extra flexible bits members in sender(@{send_level}): {missing_values_str}"),
                        );
                    } else {
                        problems.error(
                            [send_path,
                            recv_path],
                            format!("Extra strict bits members in sender(@{send_level}): {missing_values_str}"),
                        );
                    }
                }
            }
        }
        (
            Handle(send_path, send_type, send_opt, send_rights),
            Handle(recv_path, recv_type, recv_opt, recv_rights),
        ) => {
            let send_level = send_path.api_level();
            let recv_level = recv_path.api_level();
            if *send_opt == Optional && *recv_opt == Required {
                problems.error(
                    [send_path,
                    recv_path],
                    format!("Sender handle(@{send_level}) is optional but receiver(@{recv_level}) is required"),
                );
            }
            if recv_type.is_some() && recv_type != send_type {
                problems.error(
                    [send_path,
                    recv_path],
                    format!(
                        "Incompatible handle types, sender(@{send_level}):{}, receiver(@{recv_level}):{}",
                        recv_type.unwrap(),
                        match send_type {
                            Some(handle_type) => format!("{handle_type}"),
                            None => format!("zx.Handle"),
                        }
                    ),
                );
            }
            if HandleRights::SAME_RIGHTS.intersects(*recv_rights) {
                // The receiver doesn't care what rights the sender has
                assert_eq!(*recv_rights, HandleRights::SAME_RIGHTS);
            } else if HandleRights::SAME_RIGHTS.intersects(*send_rights) {
                assert_eq!(*send_rights, HandleRights::SAME_RIGHTS);
                // The sender can send any kinds of rights.
                problems.warning(
                    [send_path,
                    recv_path],
                    format!(
                        "Sender(@{send_level}) doesn't specify handle rights but receiver(@{recv_level}) does: {:?}",
                        recv_rights
                    ),
                );
            } else {
                // Both send_rights and recv_rights are specified.
                if send_rights.contains(*recv_rights) {
                    // Receive rights are the same or a subset of send rights. This is fine.
                } else {
                    problems.error(
                        [send_path,
                        recv_path],
                        format!(
                            "Incompatible handle rights, sender(@{send_level}):{:?}, receiver(@{recv_level}):{:?}",
                            send_rights, recv_rights
                        ),
                    );
                }
            }
        }
        (Array(send_path, send_size, send_type), Array(recv_path, recv_size, recv_type)) => {
            if send_size != recv_size {
                let send_level = send_path.api_level();
                let recv_level: &str = recv_path.api_level();
                problems.error(
                    [send_path,
                    recv_path],
                    format!(
                        "Array length mismatch, sender(@{send_level}):{}, receiver(@{recv_level}):{}",
                        send_size, recv_size
                    ),
                );
            }
            problems.append(compare_types(send_type, recv_type, config));
        }
        (
            Vector(send_path, send_size, send_type, send_opt),
            Vector(recv_path, recv_size, recv_type, recv_opt),
        ) => {
            let send_level = send_path.api_level();
            let recv_level = recv_path.api_level();
            if *send_opt == Optional && *recv_opt == Required {
                problems.error(
                    [send_path,
                    recv_path],
                    format!("Sender(@{send_level}) vector is optional but receiver(@{recv_level}) is required"),
                );
            }

            if send_size > recv_size {
                problems.error([send_path, recv_path], format!("Sender vector is larger than receiver vector, sender(@{send_level}):{send_size}, receiver(@{recv_level}):{recv_size}"));
            }
            problems.append(compare_types(send_type, recv_type, config));
        }
        (Struct(send_path, send_members), Struct(recv_path, recv_members)) => {
            if send_members.len() != recv_members.len() {
                let send_level = send_path.api_level();
                let recv_level = recv_path.api_level();
                problems.error(
                    [send_path,
                    recv_path],
                    format!(
                        "Struct has different number of members, sender(@{send_level}):{}, receiver(@{recv_level}):{}",
                        send_members.len(),
                        recv_members.len()
                    ),
                )
            }
            for (send, recv) in send_members.iter().zip(recv_members.iter()) {
                problems.append(compare_types(send, recv, config));
            }
        }
        (Table(send_path, send_members), Table(recv_path, recv_members)) => {
            let send_ordinals = BTreeSet::from_iter(send_members.keys().cloned());
            let recv_ordinals = BTreeSet::from_iter(recv_members.keys().cloned());
            let common_ordinals = BTreeSet::from_iter(send_ordinals.intersection(&recv_ordinals));
            let send_only = BTreeSet::from_iter(send_ordinals.difference(&recv_ordinals));

            for o in common_ordinals {
                problems.append(compare_types(&send_members[o], &recv_members[o], config));
            }

            if !send_only.is_empty() {
                let send_level = send_path.api_level();
                let recv_level = recv_path.api_level();
                problems.warning(
                    [send_path,
                    recv_path],
                    format!(
                        "Table in sender(@{send_level}) has members that receiver(@{recv_level}) does not have."
                    ),
                );
            }
        }

        (Union(send_path, _, send_members), Union(recv_path, recv_flex, recv_members)) => {
            let send_ordinals = BTreeSet::from_iter(send_members.keys().cloned());
            let recv_ordinals = BTreeSet::from_iter(recv_members.keys().cloned());
            let common_ordinals = BTreeSet::from_iter(send_ordinals.intersection(&recv_ordinals));
            let send_only = BTreeSet::from_iter(send_ordinals.difference(&recv_ordinals));

            for o in common_ordinals {
                problems.append(compare_types(&send_members[o], &recv_members[o], config));
            }

            if !send_only.is_empty() {
                let send_level = send_path.api_level();
                let recv_level = recv_path.api_level();
                if recv_flex == &Flexibility::Flexible {
                    problems.warning(
                        [send_path,
                        recv_path],
                        format!(
                            "Union in sender(@{send_level}) has members that union in receiver(@{recv_level}) does not have."
                        ),
                    );
                } else {
                    problems.error(
                        [send_path,
                        recv_path],
                        format!("Union in sender(@{send_level}) has members that strict union in receiver(@{recv_level}) does not have."),
                );
                }
            }
        }

        (
            ClientEnd(send_path, _send_name, send_transport, send_optional, send_protocol),
            ClientEnd(recv_path, _recv_name, recv_transport, recv_optional, recv_protocol),
        ) => {
            let send_level = send_path.api_level();
            let recv_level = recv_path.api_level();
            if send_transport != recv_transport {
                problems.error([send_path, recv_path], format!("client_end transports don't match sender(@{send_level}):{send_transport}, receiver(@{recv_level}):{recv_transport}"))
            }

            if config.tear_off {
                problems.append(Protocol::client_server_compatible(
                    recv_protocol,
                    send_protocol,
                    &config,
                ));
            }

            if send_optional == &Optionality::Optional && recv_optional == &Optionality::Required {
                // TODO: reword
                problems
                    .error([send_path, recv_path], format!("client_end optionality incompatible"))
            }
        }
        (
            ServerEnd(send_path, _send_name, send_transport, send_optional, send_protocol),
            ServerEnd(recv_path, _recv_name, recv_transport, recv_optional, recv_protocol),
        ) => {
            let send_level = send_path.api_level();
            let recv_level = recv_path.api_level();
            if send_transport != recv_transport {
                problems.error([send_path, recv_path], format!("server_end transports don't match sender(@{send_level}):{send_transport}, receiver(@{recv_level}):{recv_transport}"))
            }

            if config.tear_off {
                problems.append(Protocol::client_server_compatible(
                    send_protocol,
                    recv_protocol,
                    &config,
                ));
            }

            if send_optional == &Optionality::Optional && recv_optional == &Optionality::Required {
                // TODO: reword
                problems
                    .error([send_path, recv_path], format!("server_end optionality incompatible"))
            }
        }

        (FrameworkError(_), FrameworkError(_)) => (),

        (Cycle(send_path, _, send_cycle), Cycle(recv_path, _, recv_cycle)) => {
            if send_cycle != recv_cycle {
                let send_level = send_path.api_level();
                let recv_level: &str = recv_path.api_level();
                problems.error([send_path, recv_path], format!("Cycle length differs between sender(@{send_level}):{send_cycle} and receiver(@{recv_level}):{recv_cycle}"))
            }
        }

        (Box(_, send), Box(_, recv)) => problems.append(compare_types(send, recv, &config)),

        (send, recv) => {
            let send_level = send.path().api_level();
            let recv_level = recv.path().api_level();
            problems.error(
                [send.path(),
                recv.path()],
                format!("Incompatible types, sender(@{send_level}):{send}, receiver(@{recv_level}):{recv}"),
            );
        }
    };
    problems
}
