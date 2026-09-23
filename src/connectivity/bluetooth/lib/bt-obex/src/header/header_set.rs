// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use packet_encoding::{Decodable, Encodable};
use std::collections::BTreeMap;

use crate::error::{Error, PacketError};
use crate::header::{ConnectionIdentifier, Header, HeaderIdentifier, SingleResponseMode};

/// Maintains a collection of OBEX Headers that are sent & received in an OBEX packet.
/// See OBEX 1.5 Section 3.4 for the ordering requirements and definitions of the supported Header
/// types.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeaderSet {
    ids: BTreeMap<HeaderIdentifier, Header>,
}

impl HeaderSet {
    pub fn new() -> Self {
        Self { ids: BTreeMap::new() }
    }

    pub fn from_headers(headers: Vec<Header>) -> Result<Self, Error> {
        let mut set = Self::new();
        for header in headers {
            set.add(header)?;
        }
        Ok(set)
    }

    pub fn from_header(header: Header) -> Self {
        Self::from_headers(vec![header]).expect("single header always valid")
    }

    /// Returns true if the provided `id` must be encoded in a particular order in the OBEX packet.
    fn is_special_encoding_id(id: &HeaderIdentifier) -> bool {
        use HeaderIdentifier::*;
        match id {
            ConnectionId | Target | Body | EndOfBody => true,
            _ => false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn contains_header(&self, id: &HeaderIdentifier) -> bool {
        self.ids.contains_key(id)
    }

    #[cfg(test)]
    pub fn contains_headers(&self, ids: &Vec<HeaderIdentifier>) -> bool {
        for id in ids {
            if !self.contains_header(id) {
                return false;
            }
        }
        true
    }

    pub fn get(&self, id: &HeaderIdentifier) -> Option<&Header> {
        self.ids.get(id)
    }

    /// Attempts to add the `header` to the HeaderSet.
    /// Returns `Ok` if the header was successfully added, Error if the header is incompatible with
    /// the existing headers in the set or if a different value already exists for the requested
    /// `header`.
    pub fn add(&mut self, header: Header) -> Result<(), Error> {
        let id = header.identifier();
        // Cannot add the same header to the collection. If the underlying value is the same then
        // `add` is a no-op.
        match self.get(&id) {
            Some(h) if *h == header => return Ok(()),
            Some(_h) => return Err(Error::AlreadyExists(id)),
            None => {}
        }

        // Check for any incompatibilities.
        // Listed on a per-header bases in OBEX 1.5 Section 2.2.
        use HeaderIdentifier::*;
        match id {
            ConnectionId if self.contains_header(&Target) => {
                return Err(Error::IncompatibleHeaders(ConnectionId, Target));
            }
            Target if self.contains_header(&ConnectionId) => {
                return Err(Error::IncompatibleHeaders(Target, ConnectionId));
            }
            Body if self.contains_header(&EndOfBody) => {
                return Err(Error::IncompatibleHeaders(Body, EndOfBody));
            }
            EndOfBody if self.contains_header(&Body) => {
                return Err(Error::IncompatibleHeaders(EndOfBody, Body));
            }
            _ => {}
        }
        let _ = self.ids.insert(id, header);
        Ok(())
    }

    /// Attempts to combine `HeaderSet`s together by modifying the current collection. Returns Ok on
    /// success, Error otherwise.
    pub fn try_append(&mut self, other: HeaderSet) -> Result<(), Error> {
        for (_, header) in other.ids.into_iter() {
            self.add(header)?;
        }
        Ok(())
    }

    /// Removes and returns the payload for the Body Header from the set.
    /// If `final_` is set, then the EndOfBody header payload is returned.
    /// Returns Error if the expected Header is not present in the collection.
    pub fn remove_body(&mut self, final_: bool) -> Result<Vec<u8>, Error> {
        if final_ {
            let Some(Header::EndOfBody(end_of_body)) = self.remove(&HeaderIdentifier::EndOfBody)
            else {
                return Err(PacketError::data("missing end of body header").into());
            };
            Ok(end_of_body)
        } else {
            let Some(Header::Body(body)) = self.remove(&HeaderIdentifier::Body) else {
                return Err(PacketError::data("missing body header").into());
            };
            Ok(body)
        }
    }

    /// Removes and returns the specified Header from the collection. Returns None if the Header is
    /// not present.
    pub fn remove(&mut self, id: &HeaderIdentifier) -> Option<Header> {
        self.ids.remove(id)
    }

    /// Attempts to add the `SingleResponseMode` Header to the current `HeaderSet`.
    /// `local` is the supported mode of the local transport.
    /// Returns the SingleResponseMode value that was added to the set on success, Error if it
    /// couldn't be added or there was an incompatibility between the current set and `local`
    /// preferences.
    pub fn try_add_srm(&mut self, local: SingleResponseMode) -> Result<SingleResponseMode, Error> {
        // The current set has a preference for SRM. Verify it is compatible with the `local` mode.
        if let Some(Header::SingleResponseMode(srm)) =
            self.get(&HeaderIdentifier::SingleResponseMode)
        {
            // Current set requests to enable SRM, but it is not supported locally.
            if *srm == SingleResponseMode::Enable && local != SingleResponseMode::Enable {
                return Err(Error::SrmNotSupported);
            }
            // Otherwise, the mode currently specified in the headers is compatible.
            return Ok(*srm);
        }

        // Current set has no preference - default to the `local` preference.
        if local == SingleResponseMode::Enable {
            self.add(SingleResponseMode::Enable.into())?;
        }
        Ok(local)
    }

    pub fn try_add_connection_id(
        &mut self,
        id: &Option<ConnectionIdentifier>,
    ) -> Result<(), Error> {
        if let Some(id) = id {
            self.add(Header::ConnectionId(*id))?;
        }
        Ok(())
    }
}

impl Encodable for HeaderSet {
    type Error = PacketError;

    fn encoded_len(&self) -> usize {
        self.ids.iter().map(|(_, h)| h.encoded_len()).sum()
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }

        let mut start_idx = 0;
        // The ConnectionId and Target headers must be the first in the packet.
        // See OBEX 1.5 Sections 2.2.7 & 2.2.11.
        if let Some(header) = self.get(&HeaderIdentifier::ConnectionId) {
            header.encode(&mut buf[start_idx..])?;
            start_idx += header.encoded_len();
        }
        if let Some(header) = self.get(&HeaderIdentifier::Target) {
            header.encode(&mut buf[start_idx..])?;
            start_idx += header.encoded_len();
        }

        // TODO(https://fxbug.dev/42076319): Encode ActionId immediately after ConnectionId when the ACTION
        // operation is supported. See OBEX 1.5 Section 2.2.20.

        // TODO(https://fxbug.dev/42076318): Encode Session Parameters before other headers when Reliable
        // Sessions are supported. See OBEX 1.5 Section 2.2.18.

        // TODO(https://fxbug.dev/42079359): Encode SRMP after the SRM header when the SRMP feature is
        // supported. See OBEX 1.5 Sections 2.2.23, 2.2.24.

        // All other headers can be encoded in any order.
        for (id, header) in &self.ids {
            if !Self::is_special_encoding_id(&id) {
                header.encode(&mut buf[start_idx..])?;
                start_idx += header.encoded_len();
            }
        }

        // The Body/EndOfBody header must be the last header in the packet.
        if let Some(header) = self.get(&HeaderIdentifier::Body) {
            header.encode(&mut buf[start_idx..])?;
            start_idx += header.encoded_len();
        }
        if let Some(header) = self.get(&HeaderIdentifier::EndOfBody) {
            header.encode(&mut buf[start_idx..])?;
        }

        Ok(())
    }
}

impl Decodable for HeaderSet {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> Result<Self, Self::Error> {
        let mut headers = Self::new();
        let mut start_idx = 0;
        while start_idx < buf.len() {
            let header = Header::decode(&buf[start_idx..])?;
            start_idx += header.encoded_len();
            headers.add(header).map_err(|e| PacketError::data(format!("{e:?}")))?;
        }
        Ok(headers)
    }
}

#[cfg(test)]
#[track_caller]
pub fn expect_body(headers: &HeaderSet, expected: Vec<u8>) {
    match headers.get(&HeaderIdentifier::Body).expect("contains body") {
        Header::Body(v) => assert_eq!(v, &expected),
        x => panic!("Expected body, got: {x:?}"),
    }
}

#[cfg(test)]
#[track_caller]
pub fn expect_end_of_body(headers: &HeaderSet, expected: Vec<u8>) {
    match headers.get(&HeaderIdentifier::EndOfBody).expect("contains end of body") {
        Header::EndOfBody(v) => assert_eq!(v, &expected),
        x => panic!("Expected end of body, got: {x:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;

    #[fuchsia::test]
    fn add_duplicate_header_is_ok() {
        let mut headers = HeaderSet::new();
        let header = Header::ConnectionId(ConnectionIdentifier(1));
        headers.add(header.clone()).expect("can add header");
        assert!(headers.contains_header(&HeaderIdentifier::ConnectionId));
        // Adding the same exact header (ID & Value) is OK. It's a no-op.
        assert_matches!(headers.add(header), Ok(_));
        assert!(headers.contains_header(&HeaderIdentifier::ConnectionId));
    }

    #[fuchsia::test]
    fn add_existing_header_is_error() {
        let mut headers = HeaderSet::new();
        headers.add(Header::ConnectionId(ConnectionIdentifier(2))).expect("can add header");
        assert!(headers.contains_header(&HeaderIdentifier::ConnectionId));
        // Adding the same header but with a different value is an error.
        assert_matches!(
            headers.add(Header::ConnectionId(ConnectionIdentifier(3))),
            Err(Error::AlreadyExists(HeaderIdentifier::ConnectionId))
        );
    }

    #[fuchsia::test]
    fn try_append_success() {
        let mut headers1 = HeaderSet::from_header(Header::name("foo"));
        let headers2 = HeaderSet::from_header(Header::Description("bar".into()));
        let () = headers1.try_append(headers2).expect("valid headers");
        assert!(headers1.contains_header(&HeaderIdentifier::Name));
        assert!(headers1.contains_header(&HeaderIdentifier::Description));
    }

    #[fuchsia::test]
    fn try_append_error() {
        let mut headers1 = HeaderSet::from_header(Header::name("foo"));
        let headers2 = HeaderSet::from_header(Header::name("bar"));
        assert_matches!(headers1.try_append(headers2), Err(Error::AlreadyExists(_)));
    }

    #[fuchsia::test]
    fn add_incompatible_header_is_error() {
        // Target cannot be added when ConnectionId exists.
        let mut headers = HeaderSet::from_header(Header::ConnectionId(ConnectionIdentifier(2)));
        assert_matches!(
            headers.add(Header::Target("123".into())),
            Err(Error::IncompatibleHeaders(..))
        );

        // Body cannot be added when EndOfBody exists.
        let mut headers = HeaderSet::from_header(Header::EndOfBody(vec![1]));
        assert_matches!(headers.add(Header::Body(vec![2])), Err(Error::IncompatibleHeaders(..)));

        // EndOfBody cannot be added when Body exists.
        let mut headers = HeaderSet::from_header(Header::Body(vec![1]));
        assert_matches!(
            headers.add(Header::EndOfBody(vec![2])),
            Err(Error::IncompatibleHeaders(..))
        );
    }

    #[fuchsia::test]
    fn remove_headers() {
        let mut headers =
            HeaderSet::from_headers(vec![Header::Count(123), Header::name("123")]).unwrap();
        assert!(headers.contains_header(&HeaderIdentifier::Count));
        assert!(headers.contains_header(&HeaderIdentifier::Name));
        assert!(headers.remove(&HeaderIdentifier::Count).is_some());
        assert!(!headers.contains_header(&HeaderIdentifier::Count));
        assert!(headers.remove(&HeaderIdentifier::Count).is_none());
        assert!(headers.remove(&HeaderIdentifier::Name).is_some());
        assert!(!headers.contains_header(&HeaderIdentifier::Name));
    }

    #[fuchsia::test]
    fn remove_body_headers() {
        let mut body_header = HeaderSet::from_header(Header::Body(vec![1, 2]));
        let mut end_of_body_header = HeaderSet::from_header(Header::EndOfBody(vec![7, 8, 9]));

        let eob = end_of_body_header.remove_body(true).expect("end of body exists");
        assert_eq!(eob, vec![7, 8, 9]);
        // Trying to get it again is an Error since it no longer exists in the collection.
        assert_matches!(
            end_of_body_header.remove_body(true),
            Err(Error::Packet(PacketError::Data(_)))
        );

        let b = body_header.remove_body(false).expect("body exists");
        assert_eq!(b, vec![1, 2]);
        // Trying to get it again is an Error since it no longer exists in the collection.
        assert_matches!(body_header.remove_body(false), Err(Error::Packet(PacketError::Data(_))));

        // Body exists, but user wants EOB.
        let mut headers = HeaderSet::from_headers(vec![Header::Body(vec![1])]).unwrap();
        assert_matches!(headers.remove_body(true), Err(Error::Packet(PacketError::Data(_))));

        // EOB exists, but user wants Body.
        let mut headers = HeaderSet::from_headers(vec![Header::EndOfBody(vec![1])]).unwrap();
        assert_matches!(headers.remove_body(false), Err(Error::Packet(PacketError::Data(_))));
    }

    #[fuchsia::test]
    fn try_add_srm_success() {
        // Trying to add SRM when it is supported locally should add the SRM Header to the
        // collection.
        let mut headers = HeaderSet::new();
        let result = headers.try_add_srm(SingleResponseMode::Enable).expect("can add SRM");
        assert_eq!(result, SingleResponseMode::Enable);
        assert_matches!(
            headers.get(&HeaderIdentifier::SingleResponseMode),
            Some(Header::SingleResponseMode(SingleResponseMode::Enable))
        );
        // Trying to add SRM when it isn't supported locally shouldn't add the SRM to the
        // collection.
        let mut headers = HeaderSet::new();
        let result = headers.try_add_srm(SingleResponseMode::Disable).expect("can add SRM");
        assert_eq!(result, SingleResponseMode::Disable);
        assert_matches!(headers.get(&HeaderIdentifier::SingleResponseMode), None);
        // Trying to add SRM when it is already enabled in the collection and is supported locally
        // should be a no-op.
        let mut headers = HeaderSet::from_header(SingleResponseMode::Enable.into());
        let result = headers.try_add_srm(SingleResponseMode::Enable).expect("can add SRM");
        assert_eq!(result, SingleResponseMode::Enable);
        assert_matches!(
            headers.get(&HeaderIdentifier::SingleResponseMode),
            Some(Header::SingleResponseMode(SingleResponseMode::Enable))
        );
        // Trying to add SRM when it already disabled in the collection and isn't supported locally
        // should be a no-op.
        let mut headers = HeaderSet::from_header(SingleResponseMode::Disable.into());
        let result = headers.try_add_srm(SingleResponseMode::Disable).expect("can add SRM");
        assert_eq!(result, SingleResponseMode::Disable);
        assert_matches!(
            headers.get(&HeaderIdentifier::SingleResponseMode),
            Some(Header::SingleResponseMode(SingleResponseMode::Disable))
        );
        // Trying to add SRM when it already disabled in the collection and is supported locally
        // should default to disabled.
        let mut headers = HeaderSet::from_header(SingleResponseMode::Disable.into());
        let result = headers.try_add_srm(SingleResponseMode::Enable).expect("can add SRM");
        assert_eq!(result, SingleResponseMode::Disable);
        assert_matches!(
            headers.get(&HeaderIdentifier::SingleResponseMode),
            Some(Header::SingleResponseMode(SingleResponseMode::Disable))
        );
    }

    #[fuchsia::test]
    fn try_add_srm_error() {
        // Trying to add SRM when it already enabled in the collection and isn't supported locally
        // is an Error. The collection itself is not modified.
        let mut headers = HeaderSet::from_header(SingleResponseMode::Enable.into());
        let result = headers.try_add_srm(SingleResponseMode::Disable);
        assert_matches!(result, Err(Error::SrmNotSupported));
        assert_matches!(
            headers.get(&HeaderIdentifier::SingleResponseMode),
            Some(Header::SingleResponseMode(SingleResponseMode::Enable))
        );
    }

    #[fuchsia::test]
    fn try_add_connection_id_success() {
        let mut headers = HeaderSet::new();

        // No ID is a no-op.
        let () = headers.try_add_connection_id(&None).expect("success");
        assert!(!headers.contains_header(&HeaderIdentifier::ConnectionId));

        let () = headers.try_add_connection_id(&Some(ConnectionIdentifier(11))).expect("success");
        assert!(headers.contains_header(&HeaderIdentifier::ConnectionId));
    }

    #[fuchsia::test]
    fn try_add_connection_id_error() {
        // Trying to add the ID to a set that already contains one is an error.
        let mut headers = HeaderSet::from_header(Header::ConnectionId(ConnectionIdentifier(10)));
        assert_matches!(
            headers.try_add_connection_id(&Some(ConnectionIdentifier(11))),
            Err(Error::AlreadyExists(_))
        );

        // Trying to add the ID to a set with a Target header is an error.
        let mut headers = HeaderSet::from_header(Header::Target("foo".into()));
        assert_matches!(
            headers.try_add_connection_id(&Some(ConnectionIdentifier(1))),
            Err(Error::IncompatibleHeaders(..))
        );
    }

    #[fuchsia::test]
    fn encode_header_set() {
        let headers = HeaderSet::from_headers(vec![
            Header::ConnectionId(ConnectionIdentifier(1)),
            Header::EndOfBody(vec![1, 2, 3]),
        ])
        .expect("can build header set");

        // The total length should be the sum of the lengths of each Header in the set.
        assert_eq!(headers.encoded_len(), 11);
        let mut buf = vec![0; headers.encoded_len()];
        headers.encode(&mut buf[..]).expect("can encode headers");
        let expected_buf = [0xcb, 0x00, 0x00, 0x00, 0x01, 0x49, 0x00, 0x06, 0x01, 0x02, 0x03];
        assert_eq!(buf, expected_buf);
    }

    #[fuchsia::test]
    fn encode_header_set_enforces_ordering() {
        let headers = HeaderSet::from_headers(vec![
            Header::Body(vec![1, 2, 3]),
            Header::name("2"),
            Header::ConnectionId(ConnectionIdentifier(1)),
            Header::SingleResponseMode(SingleResponseMode::Enable),
        ])
        .expect("can build header set");

        assert_eq!(headers.encoded_len(), 20);
        let mut buf = vec![0; headers.encoded_len()];
        headers.encode(&mut buf[..]).expect("can encode headers");
        // The higher priority headers will always be encoded first. For all others, the order is
        // arbitrary - currently based on `HeaderIdentifier` integer value to make encoding
        // deterministic. The lower priority headers will always be encoded last.
        let expected_buf = [
            0xcb, 0x00, 0x00, 0x00, 0x01, // ConnectionId
            0x01, 0x00, 0x07, 0x00, 0x32, 0x00, 0x00, // Name
            0x97, 0x01, // SingleResponseMode
            0x48, 0x00, 0x06, 0x01, 0x02, 0x03, // Body
        ];
        assert_eq!(buf, expected_buf);
    }

    #[fuchsia::test]
    fn decode_header_set() {
        let buf = [
            0x05, 0x00, 0x09, 0x00, 0x68, 0x00, 0x65, 0x00,
            0x00, // Description = "he" (String)
            0xd6, 0x00, 0x00, 0x00, 0x05, // Permissions = 5 (u32)
            0x97, 0x01, // SRM = Enabled = 1 (u8)
        ];
        let headers = HeaderSet::decode(&buf[..]).expect("can decode into headers");
        let expected_body = Header::Description("he".into());
        let expected_permissions = Header::Permissions(5);
        let expected_srm = Header::SingleResponseMode(SingleResponseMode::Enable);
        let expected_headers =
            HeaderSet::from_headers(vec![expected_body, expected_permissions, expected_srm])
                .unwrap();
        assert_eq!(headers, expected_headers);
    }

    #[fuchsia::test]
    fn decode_partial_header_set_error() {
        // Decoding should fail if one of the headers is invalidly formatted.
        let buf = [
            0xd6, 0x00, 0x00, 0x00, 0x09, // Permissions = 9 (u32)
            0x97, 0x01, // SRM = 1 (u8)
            0xc4, 0x00, // Time4Byte but missing remaining bytes.
        ];
        let headers = HeaderSet::decode(&buf[..]);
        assert_matches!(headers, Err(PacketError::BufferTooSmall));
    }
}
