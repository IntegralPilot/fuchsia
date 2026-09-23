// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Macros for parsing and serialization of ICMP packets.

/// Implement `IcmpMessage` for a type.
///
/// The arguments are:
/// - `$ip` - `Ipv4` or `Ipv6`
/// - `$type` - the type to implement for
/// - `$msg_variant` - the variant of `Icmpv4MessageType` or `icmpv6::MessageType`
///   associated with this message type
/// - `$code` - the type to use for `IcmpMessage::Code`; if `()` is used, 0 will
///   be the only valid code
/// - `$has_body` - `true` or `false` depending on whether this message type
///   supports a body
macro_rules! impl_icmp_message {
    ($ip:ident, $type:ident, $msg_variant:ident, $code:tt, $body_type:ty, $expects_body:ident) => {
        impl crate::icmp::IcmpMessage<$ip> for $type {
            const EXPECTS_BODY: bool = $expects_body;

            type Code = $code;

            type Body<B: SplitByteSlice> = $body_type;

            const TYPE: <$ip as IcmpIpExt>::IcmpMessageType =
                impl_icmp_message_inner_message_type!($ip, $msg_variant);

            fn code_from_u8(u: u8) -> Option<Self::Code> {
                impl_icmp_message_inner_code_from_u8!($code, u)
            }
        }
    };

    ($ip:ident, $type:ident, $msg_variant:ident, $code:tt, $body_type:ty) => {
        impl_icmp_message!($ip, $type, $msg_variant, $code, $body_type, true);
    };

    ($ip:ident, $type:ident, $msg_variant:ident, $code:tt) => {
        impl_icmp_message!($ip, $type, $msg_variant, $code, crate::icmp::EmptyMessage<B>, false);
    };
}

macro_rules! impl_common_icmp_message {
    ($type:ident, $icmp_type:ident, $code:tt, $body_type:ty, $expects_body:ident) => {
        impl<I: crate::icmp::IcmpIpExt> crate::icmp::IcmpMessage<I> for $type {
            const EXPECTS_BODY: bool = $expects_body;

            type Code = $code;

            type Body<B: zerocopy::SplitByteSlice> = $body_type;

            const TYPE: I::IcmpMessageType = I::$icmp_type;

            fn code_from_u8(u: u8) -> Option<Self::Code> {
                impl_icmp_message_inner_code_from_u8!($code, u)
            }
        }
    };

    ($type:ident, $icmp_type:ident, $code:tt, $body_type:ty) => {
        impl_common_icmp_message!($type, $icmp_type, $code, $body_type, true);
    };

    ($type:ident, $icmp_type:ident, $code:tt) => {
        impl_common_icmp_message!($type, $icmp_type, $code, crate::icmp::EmptyMessage<B>, false);
    };
}

macro_rules! impl_icmp_message_inner_message_type {
    (Ipv4, $msg_variant:ident) => {
        crate::icmp::icmpv4::Icmpv4MessageType::$msg_variant
    };
    (Ipv6, $msg_variant:ident) => {
        crate::icmp::icmpv6::Icmpv6MessageType::$msg_variant
    };
}

macro_rules! impl_icmp_message_inner_code_from_u8 {
    ($code:tt, $var:ident) => {
        $code::try_from($var).ok()
    };
}
