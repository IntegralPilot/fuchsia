// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::{NonZeroU32, NonZeroU64};

use derivative::Derivative;
use futures::StreamExt;

use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_ndp as fnet_ndp;

/// Errors regarding the length of an NDP option body observed in an
/// [`OptionWatchEntry`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum BodyLengthError {
    /// The maximum possible length of an NDP option body
    /// ([`fidl_fuchsia_net_ndp::MAX_OPTION_BODY_LENGTH`]) was exceeded.
    MaxLengthExceeded,
    /// The body's length is not a multiple of 8 octets (after adding two bytes
    /// for the type and length).
    NotMultipleOf8,
}

/// The body of an NDP option.
///
/// The raw bytes of the NDP option excluding the leading two bytes for the type
/// and the length according to [RFC 4861 section
/// 4.6](https://datatracker.ietf.org/doc/html/rfc4861#section-4.6). The body's
/// length is guaranteed to be such that if it were prepended with a type octet
/// and a length octet to match the format described in RFC 4861 section 4.6,
/// its length would be a multiple of 8 octets (as required by the RFC).
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Derivative)]
#[derivative(Debug)]
pub struct OptionBody<B = Vec<u8>> {
    // Avoid including this in debug logs in order to avoid defeating privacy
    // redaction.
    #[derivative(Debug = "ignore")]
    bytes: B,
}

impl<B> OptionBody<B> {
    /// Returns the internal bytes storage of the `OptionBody`.
    pub fn into_inner(self) -> B {
        self.bytes
    }
}

impl<B: AsRef<[u8]>> OptionBody<B> {
    pub fn new(bytes: B) -> Result<Self, BodyLengthError> {
        let len = bytes.as_ref().len();
        if len > fnet_ndp::MAX_OPTION_BODY_LENGTH as usize {
            return Err(BodyLengthError::MaxLengthExceeded);
        }
        if (len + 2) % 8 != 0 {
            return Err(BodyLengthError::NotMultipleOf8);
        }
        Ok(Self { bytes })
    }

    fn as_ref(&self) -> &[u8] {
        self.bytes.as_ref()
    }

    pub fn to_owned(&self) -> OptionBody {
        let Self { bytes } = self;
        OptionBody { bytes: bytes.as_ref().to_vec() }
    }
}

pub type OptionBodyRef<'a> = OptionBody<&'a [u8]>;

/// Errors observed while converting from FIDL types to this extension crate's
/// domain types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FidlConversionError {
    /// A required field was not set.
    #[error("required field not set: {0}")]
    MissingField(&'static str),
    /// The option's body length does not conform to spec.
    #[error("body length error: {0:?}")]
    BodyLength(BodyLengthError),
    /// The interface ID was zero.
    #[error("interface ID must be non-zero")]
    ZeroInterfaceId,
}

/// The result of attempting to parse an [`OptionWatchEntry`] as a specific NDP
/// option.
#[derive(Debug, PartialEq, Eq)]
pub enum TryParseAsOptionResult<O> {
    /// The option was successfully parsed from the option body.
    Parsed(O),
    /// The [`OptionWatchEntry`]'s `option_type` did not match that of the
    /// desired option type.
    OptionTypeMismatch,
    /// The option type did match the desired option type, but there was an
    /// error parsing the body.
    ParseErr(packet::records::options::OptionParseErr),
}

/// An entry representing a single option received in an NDP message.
///
/// The `option_type` and `body` are not guaranteed to be validated in any way
/// other than the `body` conforming to length requirements as specified in [RFC
/// 4861 section
/// 4.6](https://datatracker.ietf.org/doc/html/rfc4861#section-4.6).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OptionWatchEntry {
    /// The interface on which the NDP message containing the option was
    /// received.
    pub interface_id: NonZeroU64,
    /// The source address of the IPv6 packet containing the NDP message in
    /// which the option was received.
    pub source_address: net_types::ip::Ipv6Addr,
    /// The NDP option type.
    pub option_type: fnet_ndp::OptionType,
    /// The body of the NDP option.
    pub body: OptionBody,
}

impl OptionWatchEntry {
    /// Tries to parse this entry as a Recursive DNS Server option.
    pub fn try_parse_as_rdnss(
        &self,
    ) -> TryParseAsOptionResult<packet_formats::icmp::ndp::options::RecursiveDnsServer<'_>> {
        if self.option_type
            != u8::from(packet_formats::icmp::ndp::options::NdpOptionType::RecursiveDnsServer)
        {
            return TryParseAsOptionResult::OptionTypeMismatch;
        }
        packet_formats::icmp::ndp::options::RecursiveDnsServer::parse(self.body.as_ref())
            .map_or_else(TryParseAsOptionResult::ParseErr, TryParseAsOptionResult::Parsed)
    }
}

impl TryFrom<fnet_ndp::OptionWatchEntry> for OptionWatchEntry {
    type Error = FidlConversionError;

    fn try_from(fidl_entry: fnet_ndp::OptionWatchEntry) -> Result<Self, Self::Error> {
        let fnet_ndp::OptionWatchEntry {
            interface_id,
            source_address,
            option_type,
            body,
            __source_breaking,
        } = fidl_entry;

        let interface_id = interface_id.ok_or(FidlConversionError::MissingField("interface_id"))?;
        let source_address =
            source_address.ok_or(FidlConversionError::MissingField("source_address"))?;
        let option_type = option_type.ok_or(FidlConversionError::MissingField("option_type"))?;
        let body = OptionBody::new(body.ok_or(FidlConversionError::MissingField("body"))?)
            .map_err(FidlConversionError::BodyLength)?;
        Ok(Self {
            interface_id: NonZeroU64::new(interface_id)
                .ok_or(FidlConversionError::ZeroInterfaceId)?,
            source_address: fnet_ext::FromExt::from_ext(source_address),
            option_type,
            body,
        })
    }
}

impl From<OptionWatchEntry> for fnet_ndp::OptionWatchEntry {
    fn from(value: OptionWatchEntry) -> Self {
        let OptionWatchEntry { interface_id, source_address, option_type, body } = value;
        Self {
            interface_id: Some(interface_id.get()),
            source_address: Some(fnet_ext::FromExt::from_ext(source_address)),
            option_type: Some(option_type),
            body: Some(body.into_inner()),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

/// An item in a stream of NDP option-watching hanging-get results.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OptionWatchStreamItem {
    /// An entry observed in the stream.
    Entry(OptionWatchEntry),
    /// Options have been dropped from the stream due to the hanging-get
    /// consumer falling behind.
    Dropped(NonZeroU32),
}

impl OptionWatchStreamItem {
    /// Tries to convert into an [`OptionWatchEntry`], yielding `self` otherwise.
    pub fn try_into_entry(self) -> Result<OptionWatchEntry, Self> {
        match self {
            Self::Entry(entry) => Ok(entry),
            Self::Dropped(_) => Err(self),
        }
    }
}

/// An error in a stream of NDP option-watching hanging-get results.
#[derive(Debug, Clone, thiserror::Error)]
pub enum OptionWatchStreamError {
    #[error(transparent)]
    Fidl(#[from] fidl::Error),
    #[error(transparent)]
    Conversion(#[from] FidlConversionError),
}

/// Creates an option watcher and a stream of its hanging-get results.
///
/// Awaits a probe of the watcher returning (indicating that it has been
/// registered with the netstack) before returning the stream. If the probe
/// fails with ClientChannelClosed, this is interpreted to mean the
/// RouterAdvertisementOptionWatcherProvider protocol is not present. In that
/// event, this returns None. Otherwise, any error encountered is
/// yielded as Some(Err(...)).
pub async fn create_watcher_stream(
    provider: &fnet_ndp::RouterAdvertisementOptionWatcherProviderProxy,
    params: &fnet_ndp::RouterAdvertisementOptionWatcherParams,
) -> Option<
    Result<
        impl futures::Stream<Item = Result<OptionWatchStreamItem, OptionWatchStreamError>> + use<>,
        fidl::Error,
    >,
> {
    let (proxy, server_end) = fidl::endpoints::create_proxy::<fnet_ndp::OptionWatcherMarker>();
    if let Err(e) = provider.new_router_advertisement_option_watcher(server_end, &params) {
        return Some(Err(e));
    }
    proxy
        .probe()
        .await
        .map_err(|e| match e {
            // Indicates that this protocol isn't present on the
            // system, so the caller shouldn't use this protocol.
            fidl::Error::ClientChannelClosed { .. } => return None,
            err => return Some(err),
        })
        .ok()?;

    Some(Ok(futures::stream::try_unfold(proxy, |proxy| async move {
        Ok(Some((proxy.watch_options().await?, proxy)))
    })
    .flat_map(|result: Result<_, fidl::Error>| match result {
        Err(e) => {
            futures::stream::once(futures::future::ready(Err(OptionWatchStreamError::Fidl(e))))
                .left_stream()
        }
        Ok((batch, dropped)) => futures::stream::iter(
            NonZeroU32::new(dropped).map(|dropped| Ok(OptionWatchStreamItem::Dropped(dropped))),
        )
        .chain(futures::stream::iter(batch.into_iter().map(|entry| {
            OptionWatchEntry::try_from(entry)
                .map(OptionWatchStreamItem::Entry)
                .map_err(OptionWatchStreamError::Conversion)
        })))
        .right_stream(),
    })))
}

#[cfg(test)]
mod test {
    use super::*;

    use packet::records::options::OptionParseErr;
    use packet_formats::icmp::ndp::options::RecursiveDnsServer;
    use test_case::test_case;

    use fidl_fuchsia_net as fnet;

    const INTERFACE_ID: NonZeroU64 = NonZeroU64::new(1).unwrap();
    const NET_SOURCE_ADDRESS: net_types::ip::Ipv6Addr = net_declare::net_ip_v6!("fe80::1");
    const FIDL_SOURCE_ADDRESS: fnet::Ipv6Address = net_declare::fidl_ip_v6!("fe80::1");
    const OPTION_TYPE: u8 = 1;
    const BODY: [u8; 6] = [1, 2, 3, 4, 5, 6];

    fn valid_fidl_entry() -> fnet_ndp::OptionWatchEntry {
        fnet_ndp::OptionWatchEntry {
            interface_id: Some(INTERFACE_ID.get()),
            source_address: Some(FIDL_SOURCE_ADDRESS),
            option_type: Some(OPTION_TYPE),
            body: Some(BODY.to_vec()),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }

    fn valid_ext_entry() -> OptionWatchEntry {
        OptionWatchEntry {
            interface_id: INTERFACE_ID,
            source_address: NET_SOURCE_ADDRESS,
            option_type: OPTION_TYPE,
            body: OptionBody::new(BODY.to_vec()).expect("should be valid option body"),
        }
    }

    #[test_case(valid_fidl_entry() => Ok(valid_ext_entry()))]
    #[test_case(fnet_ndp::OptionWatchEntry {
        interface_id: None,
        ..valid_fidl_entry()
    } => Err(FidlConversionError::MissingField("interface_id")))]
    #[test_case(fnet_ndp::OptionWatchEntry {
        source_address: None,
        ..valid_fidl_entry()
    } => Err(FidlConversionError::MissingField("source_address")))]
    #[test_case(fnet_ndp::OptionWatchEntry {
        option_type: None,
        ..valid_fidl_entry()
    } => Err(FidlConversionError::MissingField("option_type")))]
    #[test_case(fnet_ndp::OptionWatchEntry {
        body: None,
        ..valid_fidl_entry()
    } => Err(FidlConversionError::MissingField("body")))]
    #[test_case(fnet_ndp::OptionWatchEntry {
        interface_id: Some(0),
        ..valid_fidl_entry()
    } => Err(FidlConversionError::ZeroInterfaceId))]
    #[test_case(fnet_ndp::OptionWatchEntry {
        body: Some(vec![1; fnet_ndp::MAX_OPTION_BODY_LENGTH as usize + 1]),
        ..valid_fidl_entry()
    } => Err(FidlConversionError::BodyLength(BodyLengthError::MaxLengthExceeded)))]
    #[test_case(fnet_ndp::OptionWatchEntry {
        body: Some(vec![1; 7]),
        ..valid_fidl_entry()
    } => Err(FidlConversionError::BodyLength(BodyLengthError::NotMultipleOf8)))]
    fn convert_option_watch_entry(
        entry: fnet_ndp::OptionWatchEntry,
    ) -> Result<OptionWatchEntry, FidlConversionError> {
        OptionWatchEntry::try_from(entry)
    }

    fn recursive_dns_server_option_and_bytes() -> (RecursiveDnsServer<'static>, Vec<u8>) {
        const ADDRESSES: [net_types::ip::Ipv6Addr; 2] =
            [net_declare::net_ip_v6!("2001:db8::1"), net_declare::net_ip_v6!("2001:db8::2")];
        let option = RecursiveDnsServer::new(u32::MAX, &ADDRESSES);
        let builder = packet_formats::icmp::ndp::options::NdpOptionBuilder::RecursiveDnsServer(
            option.clone(),
        );
        let len = packet::records::options::OptionBuilder::serialized_len(&builder);
        let mut data = vec![0u8; len];
        packet::records::options::OptionBuilder::serialize_into(&builder, &mut data);
        (option, data)
    }

    #[test]
    fn try_parse_as_rdnss_succeeds() {
        let (option, bytes) = recursive_dns_server_option_and_bytes();
        let entry = OptionWatchEntry {
            interface_id: INTERFACE_ID,
            source_address: NET_SOURCE_ADDRESS,
            option_type: u8::from(
                packet_formats::icmp::ndp::options::NdpOptionType::RecursiveDnsServer,
            ),
            body: OptionBody::new(bytes).unwrap(),
        };
        assert_eq!(entry.try_parse_as_rdnss(), TryParseAsOptionResult::Parsed(option));
    }

    #[test]
    fn try_parse_as_rdnss_option_type_mismatch() {
        let (_option, bytes) = recursive_dns_server_option_and_bytes();
        let entry = OptionWatchEntry {
            interface_id: INTERFACE_ID,
            source_address: NET_SOURCE_ADDRESS,
            option_type: u8::from(packet_formats::icmp::ndp::options::NdpOptionType::Nonce),
            body: OptionBody::new(bytes).unwrap(),
        };
        assert_eq!(entry.try_parse_as_rdnss(), TryParseAsOptionResult::OptionTypeMismatch);
    }

    #[test]
    fn try_parse_as_rdnss_fails() {
        let (_option, bytes) = recursive_dns_server_option_and_bytes();
        let entry = OptionWatchEntry {
            interface_id: INTERFACE_ID,
            source_address: NET_SOURCE_ADDRESS,
            option_type: u8::from(
                packet_formats::icmp::ndp::options::NdpOptionType::RecursiveDnsServer,
            ),
            body: OptionBody::new(vec![0u8; bytes.len()]).unwrap(),
        };
        assert_eq!(entry.try_parse_as_rdnss(), TryParseAsOptionResult::ParseErr(OptionParseErr));
    }
}
