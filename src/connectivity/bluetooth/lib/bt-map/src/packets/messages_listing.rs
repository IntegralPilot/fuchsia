// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use chrono::DateTime;
use chrono::naive::NaiveDateTime;
use objects::{Builder, ObexObjectError as Error, Parser};
use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use xml::EventWriter;
use xml::attribute::OwnedAttribute;
use xml::reader::{ParserConfig, XmlEvent};
use xml::writer::{EmitterConfig, XmlEvent as XmlWriteEvent};

use crate::MessageType;
use crate::packets::{ISO_8601_TIME_FORMAT, bool_to_string, str_to_bool, truncate_string};

// From MAP v1.4.2 section 3.1.6 Message-Listing Object:
//
// <!DTD for the MAP Messages-Listing Object-->
// <!DOCTYPE MAP-msg-listing [
// <!ELEMENT MAP-msg-listing ( msg )* >
// <!ATTLIST MAP-msg-listing version CDATA #FIXED "1.1">
// <!ELEMENT msg EMPTY>
// <!ATTLIST msg
//     handle CDATA #REQUIRED
//     subject CDATA #REQUIRED
//     datetime CDATA #REQUIRED
//     sender_name CDATA #IMPLIED
//     sender_addressing CDATA #IMPLIED
//     replyto_addressing CDATA #IMPLIED
//     recipient_name CDATA #IMPLIED
//     recipient_addressing CDATA #REQUIRED
//     type CDATA #REQUIRED
//     size CDATA #REQUIRED
//     text (yes|no) "no"
//     reception_status CDATA #REQUIRED
//     attachment_size CDATA #REQUIRED
//     priority (yes|no) "no"
//     read (yes|no) "no"
//     sent (yes|no) "no"
//     protected (yes|no) "no"
//     delivery_status CDATA #IMPLIED
//     conversation_id CDATA #REQUIRED
//     conversation_name CDATA #IMPLIED
//     direction CDATA #REQUIRED
//     attachment_mime_types CDATA #IMPLIED
// >
// ]>

const MESSAGE_ELEM: &str = "msg";
const MESSAGES_LISTING_ELEM: &str = "MAP-msg-listing";

const VERSION_ATTR: &str = "version";

const HANDLE_ATTR: &str = "handle";
const SUBJECT_ATTR: &str = "subject";
const DATETIME_ATTR: &str = "datetime";
const SENDER_NAME_ATTR: &str = "sender_name";
const SENDER_ADDRESSING_ATTR: &str = "sender_addressing";
const REPLYTO_ADDRESSING_ATTR: &str = "relyto_addressing";
const RECIPIENT_NAME_ATTR: &str = "recipient_name";
const RECIPIENT_ADDRESSING_ATTR: &str = "recipient_addressing";
const TYPE_ATTR: &str = "type";

// Must have one of these attributes.
const SIZE_ATTR: &str = "size";
const TEXT_ATTR: &str = "text";
const RECEPTION_STATUS_ATTR: &str = "reception_status";
const ATTACHMENT_SIZE_ATTR: &str = "attachment_size";
const PRIORITY_ATTR: &str = "priority";
const READ_ATTR: &str = "read";
const SENT_ATTR: &str = "sent";
const PROTECTED_ATTR: &str = "protected";

// V1.1 specific attributes.
const DELIVERY_STATUS_ATTR: &str = "delivery_status";
const CONVERSATION_ID_ATTR: &str = "conversation_id";
const CONVERSATION_NAME_ATTR: &str = "conversation_name";
const DIRECTION_ATTR: &str = "direction";
const ATTACHMENT_MIME_TYPES_ATTR: &str = "attachment_mime_types";

#[derive(Debug, PartialEq)]
pub enum ReceptionStatus {
    // Complete message has been received by the MSE
    Complete,
    // Only a part of the message has been received by the MSE (e.g. fractioned email of
    // push-service)
    Fractioned,
    // Only a notification of the message has been received by the MSE
    Notification,
}

impl fmt::Display for ReceptionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Complete => write!(f, "complete"),
            Self::Fractioned => write!(f, "fractioned"),
            Self::Notification => write!(f, "notification"),
        }
    }
}

impl FromStr for ReceptionStatus {
    type Err = Error;
    fn from_str(src: &str) -> Result<Self, Self::Err> {
        match src {
            "complete" => Ok(Self::Complete),
            "fractioned" => Ok(Self::Fractioned),
            "notification" => Ok(Self::Notification),
            v => Err(Error::invalid_data(v)),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum DeliveryStatus {
    Unknown,
    Delivered,
    Sent,
}

impl fmt::Display for DeliveryStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => write!(f, "unknown"),
            Self::Delivered => write!(f, "delivered"),
            Self::Sent => write!(f, "sent"),
        }
    }
}

impl FromStr for DeliveryStatus {
    type Err = Error;
    fn from_str(src: &str) -> Result<Self, Self::Err> {
        match src {
            "unknown" => Ok(Self::Unknown),
            "delivered" => Ok(Self::Delivered),
            "sent" => Ok(Self::Sent),
            v => Err(Error::invalid_data(v)),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Direction {
    Incoming,
    Outgoing,
    OutgoingDraft,
    OutgoingPending,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incoming => write!(f, "incoming"),
            Self::Outgoing => write!(f, "outgoing"),
            Self::OutgoingDraft => write!(f, "outgoingdraft"),
            Self::OutgoingPending => write!(f, "outgoingpending"),
        }
    }
}

impl FromStr for Direction {
    type Err = Error;
    fn from_str(src: &str) -> Result<Self, Self::Err> {
        match src {
            "incoming" => Ok(Self::Incoming),
            "outgoing" => Ok(Self::Outgoing),
            "outgoingdraft" => Ok(Self::OutgoingDraft),
            "outgoingpending" => Ok(Self::OutgoingPending),
            v => Err(Error::invalid_data(v)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MessagesListingVersion {
    V1_0,
    V1_1,
}

impl fmt::Display for MessagesListingVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V1_0 => write!(f, "1.0"),
            Self::V1_1 => write!(f, "1.1"),
        }
    }
}

impl FromStr for MessagesListingVersion {
    type Err = Error;
    fn from_str(src: &str) -> Result<Self, Self::Err> {
        match src {
            "1.0" => Ok(Self::V1_0),
            "1.1" => Ok(Self::V1_1),
            _v => Err(Error::UnsupportedVersion),
        }
    }
}

/// List of attributes available for messages-listing v1.0 objects.
/// See MAP v1.4.2 section 3.1.6.1 for details.
#[derive(Debug)]
pub enum AttributeV1_0 {
    Handle(u64),
    Subject(String),
    Datetime(NaiveDateTime),
    SenderName(String),
    SenderAddressing(String),
    ReplyToAddressing(String),
    RecipientName(String),
    RecipientAddressing(Vec<String>),
    Type(MessageType),
    // At least one of the below attributes shall be supported by the MSE.
    Size(u64),
    Text(bool),
    ReceiptionStatus(ReceptionStatus),
    AttachmentSize(u64),
    Priority(bool),
    Read(bool),
    Sent(bool),
    Protected(bool),
}

impl Hash for AttributeV1_0 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
    }
}

/// Note that partial equality for AttributeV1_0 indicates that two attributes
/// are of the same type, but not necessarily the same value.
impl PartialEq for AttributeV1_0 {
    fn eq(&self, other: &AttributeV1_0) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

impl Eq for AttributeV1_0 {}

impl TryFrom<&OwnedAttribute> for AttributeV1_0 {
    type Error = Error;

    fn try_from(src: &OwnedAttribute) -> Result<Self, Error> {
        let attr_name = src.name.local_name.as_str();
        let attribute = match attr_name {
            HANDLE_ATTR => {
                // MAP v1.4.2 section 3.1.6.1 - "handle" is the message handle in hexadecimal representation with up to 16
                // digits; leading zero digits may be used so the MCE shall accept both handles with and without leading zeros.
                if src.value.len() > 16 {
                    return Err(Error::invalid_data(&src.value));
                }
                Self::Handle(u64::from_be_bytes(
                    hex::decode(format!("{:0>16}", src.value.as_str()))
                        .map_err(|e| Error::invalid_data(e))?
                        .as_slice()
                        .try_into()
                        .unwrap(),
                ))
            }
            SUBJECT_ATTR => {
                // MAP v1.4.2 section 3.1.6.1 - "subject" parameter shall not exceed 256 bytes.
                if src.value.len() > 256 {
                    return Err(Error::invalid_data(&src.value));
                }
                Self::Subject(src.value.clone())
            }
            DATETIME_ATTR => Self::Datetime(
                NaiveDateTime::parse_from_str(src.value.as_str(), ISO_8601_TIME_FORMAT)
                    .map_err(|e| Error::invalid_data(e))?,
            ),
            // Below attributes have max byte data length 256 limit. See MAP v1.5.2 section 3.1.6.1 for details.
            SENDER_NAME_ATTR => Self::SenderName(truncate_string(&src.value, 256)),
            SENDER_ADDRESSING_ATTR => Self::SenderAddressing(truncate_string(&src.value, 256)),
            REPLYTO_ADDRESSING_ATTR => Self::ReplyToAddressing(truncate_string(&src.value, 256)),
            RECIPIENT_NAME_ATTR => Self::RecipientName(truncate_string(&src.value, 256)),
            RECIPIENT_ADDRESSING_ATTR => {
                // Recipients shall be separated by semicolon.
                let mut recipients = Vec::new();
                src.value.split(";").for_each(|r| recipients.push(r.to_string()));
                Self::RecipientAddressing(recipients)
            }
            TYPE_ATTR => Self::Type(str::parse(src.value.as_str())?),
            SIZE_ATTR | ATTACHMENT_SIZE_ATTR => {
                let value =
                    src.value.parse::<u64>().map_err(|_| Error::invalid_data(&src.value))?;
                if attr_name == SIZE_ATTR {
                    Self::Size(value)
                } else
                /* attr_name == ATTACHMENT_SIZE_ATTR */
                {
                    Self::AttachmentSize(value)
                }
            }
            TEXT_ATTR => Self::Text(str_to_bool(&src.value)?),
            PRIORITY_ATTR => Self::Priority(str_to_bool(&src.value)?),
            READ_ATTR => Self::Read(str_to_bool(&src.value)?),
            SENT_ATTR => Self::Sent(str_to_bool(&src.value)?),
            PROTECTED_ATTR => Self::Protected(str_to_bool(&src.value)?),
            RECEPTION_STATUS_ATTR => Self::ReceiptionStatus(str::parse(src.value.as_str())?),
            val => return Err(Error::invalid_data(val)),
        };
        Ok(attribute)
    }
}

impl AttributeV1_0 {
    // Validate the attribute against the message listing version.
    fn validate(&self) -> Result<(), Error> {
        if let Self::RecipientAddressing(v) = self {
            if v.len() == 0 {
                return Err(Error::MissingData(RECIPIENT_ADDRESSING_ATTR.to_string()));
            }
        }
        Ok(())
    }

    fn xml_attribute_name(&self) -> &'static str {
        match self {
            Self::Handle(_) => HANDLE_ATTR,
            Self::Subject(_) => SUBJECT_ATTR,
            Self::Datetime(_) => DATETIME_ATTR,
            Self::SenderName(_) => SENDER_NAME_ATTR,
            Self::SenderAddressing(_) => SENDER_ADDRESSING_ATTR,
            Self::ReplyToAddressing(_) => REPLYTO_ADDRESSING_ATTR,
            Self::RecipientName(_) => RECIPIENT_NAME_ATTR,
            Self::RecipientAddressing(_) => RECIPIENT_ADDRESSING_ATTR,
            Self::Type(_) => TYPE_ATTR,
            Self::Size(_) => SIZE_ATTR,
            Self::Text(_) => TEXT_ATTR,
            Self::ReceiptionStatus(_) => RECEPTION_STATUS_ATTR,
            Self::AttachmentSize(_) => ATTACHMENT_SIZE_ATTR,
            Self::Priority(_) => PRIORITY_ATTR,
            Self::Read(_) => READ_ATTR,
            Self::Sent(_) => SENT_ATTR,
            Self::Protected(_) => PROTECTED_ATTR,
        }
    }

    fn xml_attribute_value(&self) -> String {
        match self {
            Self::Handle(v) => hex::encode(v.to_be_bytes()),
            Self::Subject(v) => truncate_string(v, 256),
            Self::Datetime(v) => v.format(ISO_8601_TIME_FORMAT).to_string(),
            Self::SenderName(v) => v.clone(),
            Self::SenderAddressing(v) => v.clone(),
            Self::ReplyToAddressing(v) => v.clone(),
            Self::RecipientName(v) => v.clone(),
            Self::RecipientAddressing(v) => v.join(";"),
            Self::Type(v) => v.to_string(),
            Self::Size(v) => v.to_string(),
            Self::Text(v)
            | Self::Priority(v)
            | Self::Read(v)
            | Self::Sent(v)
            | Self::Protected(v) => bool_to_string(*v),
            Self::ReceiptionStatus(v) => v.to_string(),
            Self::AttachmentSize(v) => v.to_string(),
        }
    }

    fn validate_all(attrs: &HashSet<AttributeV1_0>) -> Result<(), Error> {
        // See MAP v1.4.2 section 3.1.6.1 for details.
        // We are checking if the attribute types exist at all in the hashset, so
        // the values are irrelevant.
        let required_attrs = vec![
            AttributeV1_0::Handle(0),
            AttributeV1_0::Subject(String::new()),
            AttributeV1_0::Datetime(DateTime::from_timestamp(0, 0).unwrap().naive_utc()),
            AttributeV1_0::RecipientAddressing(Vec::new()),
            AttributeV1_0::Type(MessageType::Email),
        ];
        for a in &required_attrs {
            if !attrs.contains(&a) {
                return Err(Error::MissingData(a.xml_attribute_name().to_string()));
            }
        }

        // At least one of these types shall be supported by the MSE.
        let required_oneof: HashSet<AttributeV1_0> = HashSet::from([
            AttributeV1_0::Size(0),
            AttributeV1_0::Text(false),
            AttributeV1_0::ReceiptionStatus(ReceptionStatus::Complete),
            AttributeV1_0::AttachmentSize(0),
            AttributeV1_0::Priority(false),
            AttributeV1_0::Read(false),
            AttributeV1_0::Sent(false),
            AttributeV1_0::Protected(false),
        ]);
        if attrs.intersection(&required_oneof).next().is_none() {
            return Err(Error::MissingData(format!(
                "should have one of {:?}",
                vec![
                    SIZE_ATTR,
                    TEXT_ATTR,
                    RECEPTION_STATUS_ATTR,
                    ATTACHMENT_SIZE_ATTR,
                    PRIORITY_ATTR,
                    READ_ATTR,
                    SENT_ATTR,
                    PROTECTED_ATTR,
                ]
            )));
        }

        for a in attrs {
            a.validate()?;
        }
        Ok(())
    }
}

/// Additional information unique to messages-listing v1.1.
/// See MAP v1.4.2 section 3.1.6.2.
#[derive(Debug)]
pub enum AttributeV1_1 {
    DeliveryStatus(DeliveryStatus),
    ConversationId(u128),
    ConversationName(String),
    Direction(Direction),
    AttachmentMimeTypes(Vec<String>),
}

impl Hash for AttributeV1_1 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
    }
}

/// Note that partial equality for AttributeV1_1 indicates that two attributes
/// are of the same type, but not necessarily the same value.
impl PartialEq for AttributeV1_1 {
    fn eq(&self, other: &AttributeV1_1) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

impl Eq for AttributeV1_1 {}

impl TryFrom<&OwnedAttribute> for AttributeV1_1 {
    type Error = Error;

    fn try_from(src: &OwnedAttribute) -> Result<Self, Error> {
        let attr_name = src.name.local_name.as_str();
        // See MAP v1.4.2 section 3.1.6.1 Messages-Listing Object.
        match attr_name {
            DELIVERY_STATUS_ATTR => Ok(Self::DeliveryStatus(str::parse(src.value.as_str())?)),
            CONVERSATION_ID_ATTR => {
                let id = hex::decode(src.value.as_str()).map_err(|e| Error::invalid_data(e))?;
                if id.len() != 16 {
                    return Err(Error::invalid_data(&src.value));
                }
                let bytes: &[u8; 16] = id[..].try_into().unwrap();
                Ok(Self::ConversationId(u128::from_be_bytes(*bytes)))
            }
            CONVERSATION_NAME_ATTR => Ok(Self::ConversationName(src.value.to_string())),
            DIRECTION_ATTR => Ok(Self::Direction(str::parse(src.value.as_str())?)),
            ATTACHMENT_MIME_TYPES_ATTR => {
                // Mime type shall be separated by comma.
                let mut mime_types = Vec::new();
                src.value.split(",").for_each(|t| mime_types.push(t.to_string()));
                Ok(Self::AttachmentMimeTypes(mime_types))
            }
            val => Err(Error::invalid_data(val)),
        }
    }
}

impl AttributeV1_1 {
    fn xml_attribute_name(&self) -> &'static str {
        match self {
            Self::DeliveryStatus(_) => DELIVERY_STATUS_ATTR,
            Self::ConversationId(_) => CONVERSATION_ID_ATTR,
            Self::ConversationName(_) => CONVERSATION_NAME_ATTR,
            Self::Direction(_) => DIRECTION_ATTR,
            Self::AttachmentMimeTypes(_) => ATTACHMENT_MIME_TYPES_ATTR,
        }
    }

    fn xml_attribute_value(&self) -> String {
        match self {
            Self::DeliveryStatus(v) => v.to_string(),
            Self::ConversationId(v) => hex::encode_upper(v.to_be_bytes()),
            Self::ConversationName(v) => v.clone(),
            Self::Direction(v) => v.to_string(),
            Self::AttachmentMimeTypes(vals) => vals.join(","),
        }
    }

    fn validate_all(attrs: &HashSet<AttributeV1_1>) -> Result<(), Error> {
        // See MAP v1.4.2 section 3.1.6.2 for details.
        let required_attrs: HashSet<AttributeV1_1> = HashSet::from([
            AttributeV1_1::ConversationId(0),
            AttributeV1_1::Direction(Direction::Incoming),
        ]);
        for a in required_attrs {
            if !attrs.contains(&a) {
                return Err(Error::MissingData(a.xml_attribute_name().to_string()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub enum Message {
    V1_0 { attrs: HashSet<AttributeV1_0> },
    V1_1 { attrs_v1_0: HashSet<AttributeV1_0>, attrs_v1_1: HashSet<AttributeV1_1> },
}

impl Message {
    /// Creates a new v1.0 message.
    fn v1_0(attrs: HashSet<AttributeV1_0>) -> Self {
        Self::V1_0 { attrs }
    }

    /// Creates a new v1.1 message.
    fn v1_1(attrs_v1_0: HashSet<AttributeV1_0>, attrs_v1_1: HashSet<AttributeV1_1>) -> Self {
        Self::V1_1 { attrs_v1_0, attrs_v1_1 }
    }

    fn version(&self) -> MessagesListingVersion {
        match self {
            Message::V1_0 { .. } => MessagesListingVersion::V1_0,
            Message::V1_1 { .. } => MessagesListingVersion::V1_1,
        }
    }

    fn write<W: std::io::Write>(&self, writer: &mut EventWriter<W>) -> Result<(), Error> {
        let mut builder = XmlWriteEvent::start_element(MESSAGE_ELEM);
        let mut attributes: Vec<(&str, String)> = Vec::new();
        match self {
            Message::V1_0 { attrs } => {
                attrs.iter().for_each(|a| {
                    attributes.push((a.xml_attribute_name(), a.xml_attribute_value()))
                });
            }
            Message::V1_1 { attrs_v1_0, attrs_v1_1 } => {
                attrs_v1_0.iter().for_each(|a| {
                    attributes.push((a.xml_attribute_name(), a.xml_attribute_value()))
                });
                attrs_v1_1.iter().for_each(|a| {
                    attributes.push((a.xml_attribute_name(), a.xml_attribute_value()))
                });
            }
        };
        for a in &attributes {
            builder = builder.attr(a.0, a.1.as_str());
        }
        writer.write(builder)?;
        Ok(writer.write(XmlWriteEvent::end_element())?)
    }

    fn validate(&self) -> Result<(), Error> {
        match self {
            Message::V1_0 { attrs } => {
                AttributeV1_0::validate_all(attrs)?;
                match attrs.get(&AttributeV1_0::Type(MessageType::Im)).unwrap() {
                    AttributeV1_0::Type(t) => {
                        if *t == MessageType::Im {
                            return Err(Error::invalid_data(t));
                        }
                    }
                    _ => unreachable!(),
                };
            }
            Message::V1_1 { attrs_v1_0, attrs_v1_1 } => {
                AttributeV1_0::validate_all(attrs_v1_0)?;
                AttributeV1_1::validate_all(attrs_v1_1)?;
            }
        };
        Ok(())
    }
}

impl TryFrom<(XmlEvent, MessagesListingVersion)> for Message {
    type Error = Error;
    fn try_from(src: (XmlEvent, MessagesListingVersion)) -> Result<Self, Error> {
        let XmlEvent::StartElement { ref name, ref attributes, .. } = src.0 else {
            return Err(Error::InvalidData(format!("{:?}", src)));
        };
        if name.local_name.as_str() != MESSAGE_ELEM {
            return Err(Error::invalid_data(&name.local_name));
        }
        let mut attrs_v1_0 = HashSet::new();
        let mut attrs_v1_1 = HashSet::new();
        attributes.iter().try_for_each(|a| {
            let new_insert = match AttributeV1_0::try_from(a) {
                Ok(attr) => attrs_v1_0.insert(attr),
                Err(e) => match src.1 {
                    MessagesListingVersion::V1_0 => return Err(e),
                    MessagesListingVersion::V1_1 => attrs_v1_1.insert(AttributeV1_1::try_from(a)?),
                },
            };
            if !new_insert {
                return Err(Error::DuplicateData(a.name.local_name.to_string()));
            }
            Ok(())
        })?;
        Ok(match src.1 {
            MessagesListingVersion::V1_0 => Message::v1_0(attrs_v1_0),
            MessagesListingVersion::V1_1 => Message::v1_1(attrs_v1_0, attrs_v1_1),
        })
    }
}

enum ParsedXmlEvent {
    DocumentStart,
    MessagesListingElement,
    MessageElement(Message),
}

#[derive(Debug, PartialEq)]
pub struct MessagesListing {
    version: MessagesListingVersion,
    messages: Vec<Message>,
}

impl MessagesListing {
    fn new(version: MessagesListingVersion) -> MessagesListing {
        MessagesListing { version, messages: Vec::new() }
    }

    // Given the XML StartElement, checks whether or not it is a valid folder
    // listing element.
    fn validate_messages_listing_element(
        element: XmlEvent,
    ) -> Result<MessagesListingVersion, Error> {
        let XmlEvent::StartElement { ref name, ref attributes, .. } = element else {
            return Err(Error::InvalidData(format!("{:?}", element)));
        };

        if name.local_name != MESSAGES_LISTING_ELEM {
            return Err(Error::invalid_data(&name.local_name));
        }

        let version_attr = &attributes
            .iter()
            .find(|a| a.name.local_name == VERSION_ATTR)
            .ok_or_else(|| Error::MissingData(VERSION_ATTR.to_string()))?
            .value;
        str::parse(version_attr.as_str())
    }
}

impl Parser for MessagesListing {
    type Error = Error;

    /// Parses MessagesListing from raw bytes of XML data.
    fn parse<R: std::io::prelude::Read>(buf: R) -> Result<Self, Self::Error> {
        let mut reader = ParserConfig::new()
            .ignore_comments(true)
            .whitespace_to_characters(true)
            .cdata_to_characters(true)
            .trim_whitespace(true)
            .create_reader(buf);
        let mut prev = Vec::new();

        // Process start of document.
        match reader.next() {
            Ok(XmlEvent::StartDocument { .. }) => {
                prev.push(ParsedXmlEvent::DocumentStart);
            }
            Ok(element) => return Err(Error::InvalidData(format!("{:?}", element))),
            Err(e) => return Err(Error::ReadXml(e)),
        };

        // Process start of folder listing element.
        let mut xml_event = reader.next()?;
        while matches!(xml_event, XmlEvent::Doctype { .. } | XmlEvent::Comment(_)) {
            xml_event = reader.next()?;
        }
        let version = MessagesListing::validate_messages_listing_element(xml_event)?;

        prev.push(ParsedXmlEvent::MessagesListingElement);
        let mut messages_listing = MessagesListing::new(version);

        // Process remaining elements.
        let mut finished_document = false;
        let mut finished_messages_listing = false;
        while !finished_document {
            // Could be either end of folder listing element,
            let e = reader.next()?;
            let invalid_elem_err = Err(Error::InvalidData(format!("{:?}", e)));
            match e {
                XmlEvent::StartElement { ref name, .. } => {
                    match name.local_name.as_str() {
                        MESSAGE_ELEM => prev.push(ParsedXmlEvent::MessageElement(
                            (e, messages_listing.version).try_into()?,
                        )),
                        _ => return invalid_elem_err,
                    };
                }
                XmlEvent::EndElement { ref name } => {
                    let Some(parsed_elem) = prev.pop() else {
                        return invalid_elem_err;
                    };
                    match name.local_name.as_str() {
                        MESSAGES_LISTING_ELEM => {
                            let ParsedXmlEvent::MessagesListingElement = parsed_elem else {
                                return Err(Error::MissingData(format!(
                                    "closing {MESSAGES_LISTING_ELEM}"
                                )));
                            };
                            finished_messages_listing = true;
                        }
                        MESSAGE_ELEM => {
                            let ParsedXmlEvent::MessageElement(m) = parsed_elem else {
                                return Err(Error::MissingData(format!("closing {MESSAGE_ELEM}")));
                            };
                            let _ = m.validate()?;
                            messages_listing.messages.push(m);
                        }
                        _ => return invalid_elem_err,
                    };
                }
                XmlEvent::EndDocument => {
                    if !finished_messages_listing {
                        return Err(Error::MissingData(format!("closing {MESSAGES_LISTING_ELEM}")));
                    }
                    finished_document = true;
                }
                XmlEvent::Doctype { .. } | XmlEvent::Comment(_) => {}
                _ => return invalid_elem_err,
            }
        }
        Ok(messages_listing)
    }
}

impl Builder for MessagesListing {
    type Error = Error;

    // Returns the document type of the raw bytes of data.
    fn mime_type(&self) -> String {
        "application/xml".to_string()
    }

    /// Builds self into raw bytes of the specific Document Type.
    fn build<W: std::io::Write>(&self, buf: W) -> Result<(), Self::Error> {
        let mut w = EmitterConfig::new()
            .write_document_declaration(true)
            .perform_indent(true)
            .create_writer(buf);

        // Begin `MAP-msg-listing` element.
        let version = self.version.to_string();
        let messages_listing = XmlWriteEvent::start_element(MESSAGES_LISTING_ELEM)
            .attr(VERSION_ATTR, version.as_str());
        w.write(messages_listing)?;

        // Validate each message before writing it and ensure that all the messages have the same version.
        if self.messages.len() > 0 {
            let mut prev_version = self.messages[0].version();
            self.messages.iter().try_for_each(|m| {
                m.validate()?;
                if m.version() != prev_version {
                    return Err(Error::invalid_data(m.version()));
                }
                prev_version = m.version();
                Ok(())
            })?;
        }

        // Write each msg element.
        self.messages.iter().try_for_each(|m| m.write(&mut w))?;
        // End `MAP-msg-listing` element.
        Ok(w.write(XmlWriteEvent::end_element())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{NaiveDate, NaiveTime};
    use std::fs;
    use std::io::Cursor;

    #[fuchsia::test]
    fn safe_truncate_string() {
        const TEST_STR: &str = "Löwe 老虎 Léopard";

        // Case 1. string is less than or equal to max length.
        let res = truncate_string(&TEST_STR.to_string(), 200);
        assert_eq!(TEST_STR.to_string(), res);

        // Case 2. string is greater than the max length, but the max length is on the char boundary.
        let res = truncate_string(&TEST_STR.to_string(), 6);
        assert_eq!("Löwe ", res);
        assert_eq!(6, res.len());

        // Case 3. string is greater than max length, max length is not on the char boundary.
        let res = truncate_string(&TEST_STR.to_string(), 8);
        assert_eq!("Löwe ", res); // truncated to cloest char boundary from desired length.
        assert_eq!(6, res.len());
    }

    #[fuchsia::test]
    fn parse_empty_messages_listing_success() {
        const V1_0_TEST_FILE: &str = "/pkg/data/sample_messages_listing_v1_0_1.xml";
        let bytes = fs::read(V1_0_TEST_FILE).expect("should be ok");
        let messages_listing = MessagesListing::parse(Cursor::new(bytes)).expect("should be ok");
        assert_eq!(messages_listing, MessagesListing::new(MessagesListingVersion::V1_0));

        const V1_1_TEST_FILE: &str = "/pkg/data/sample_messages_listing_v1_1_1.xml";
        let bytes = fs::read(V1_1_TEST_FILE).expect("should be ok");
        let messages_listing = MessagesListing::parse(Cursor::new(bytes)).expect("should be ok");
        assert_eq!(messages_listing, MessagesListing::new(MessagesListingVersion::V1_1));
    }

    #[fuchsia::test]
    fn parse_messages_listing_success() {
        const V1_0_TEST_FILE: &str = "/pkg/data/sample_messages_listing_v1_0_2.xml";
        let bytes = fs::read(V1_0_TEST_FILE).expect("should be ok");
        let messages_listing = MessagesListing::parse(Cursor::new(bytes)).expect("should be ok");
        assert_eq!(messages_listing.version, MessagesListingVersion::V1_0);
        assert_eq!(messages_listing.messages.len(), 4);
        assert_eq!(
            messages_listing.messages[0],
            Message::v1_0(HashSet::from([
                AttributeV1_0::Handle(0x20000100001u64),
                AttributeV1_0::Subject("Hello".to_string()),
                AttributeV1_0::Datetime(NaiveDateTime::new(
                    NaiveDate::from_ymd_opt(2007, 12, 13).unwrap(),
                    NaiveTime::from_hms_opt(13, 05, 10).unwrap(),
                )),
                AttributeV1_0::SenderName("Jamie".to_string()),
                AttributeV1_0::SenderAddressing("+1-987-6543210".to_string()),
                AttributeV1_0::RecipientAddressing(vec!["+1-0123-456789".to_string()]),
                AttributeV1_0::Type(MessageType::SmsGsm),
                AttributeV1_0::Size(256u64),
                AttributeV1_0::AttachmentSize(0u64),
                AttributeV1_0::Priority(false),
                AttributeV1_0::Read(true),
                AttributeV1_0::Sent(false),
                AttributeV1_0::Protected(false),
            ]))
        );
        assert_eq!(
            messages_listing.messages[1],
            Message::v1_0(HashSet::from([
                AttributeV1_0::Handle(0x20000100002u64),
                AttributeV1_0::Subject("Guten Tag".to_string()),
                AttributeV1_0::Datetime(NaiveDateTime::new(
                    NaiveDate::from_ymd_opt(2007, 12, 14).unwrap(),
                    NaiveTime::from_hms_opt(09, 22, 00).unwrap(),
                )),
                AttributeV1_0::SenderName("Dmitri".to_string()),
                AttributeV1_0::SenderAddressing("8765432109".to_string()),
                AttributeV1_0::RecipientAddressing(vec!["+49-9012-345678".to_string()]),
                AttributeV1_0::Type(MessageType::SmsGsm),
                AttributeV1_0::Size(512u64),
                AttributeV1_0::AttachmentSize(3000u64),
                AttributeV1_0::Priority(false),
                AttributeV1_0::Read(false),
                AttributeV1_0::Sent(true),
                AttributeV1_0::Protected(false),
            ]))
        );
        assert_eq!(
            messages_listing.messages[2],
            Message::v1_0(HashSet::from([
                AttributeV1_0::Handle(0x20000100003u64),
                AttributeV1_0::Subject("Ohayougozaimasu".to_string()),
                AttributeV1_0::Datetime(NaiveDateTime::new(
                    NaiveDate::from_ymd_opt(2007, 12, 15).unwrap(),
                    NaiveTime::from_hms_opt(13, 43, 26).unwrap(),
                )),
                AttributeV1_0::SenderName("Andy".to_string()),
                AttributeV1_0::SenderAddressing("+49-7654-321098".to_string()),
                AttributeV1_0::RecipientAddressing(vec!["+49-89-01234567".to_string()]),
                AttributeV1_0::Type(MessageType::SmsGsm),
                AttributeV1_0::Size(256u64),
                AttributeV1_0::AttachmentSize(0u64),
                AttributeV1_0::Priority(false),
                AttributeV1_0::Read(true),
                AttributeV1_0::Sent(false),
                AttributeV1_0::Protected(false),
            ]))
        );
        assert_eq!(
            messages_listing.messages[3],
            Message::v1_0(HashSet::from([
                AttributeV1_0::Handle(0x20000100000u64),
                AttributeV1_0::Subject("Bonjour".to_string()),
                AttributeV1_0::Datetime(NaiveDateTime::new(
                    NaiveDate::from_ymd_opt(2007, 12, 15).unwrap(),
                    NaiveTime::from_hms_opt(17, 12, 04).unwrap(),
                )),
                AttributeV1_0::SenderName("Marc".to_string()),
                AttributeV1_0::SenderAddressing("marc@carworkinggroup.bluetooth".to_string()),
                AttributeV1_0::RecipientAddressing(vec![
                    "burch@carworkinggroup.bluetoot7".to_string()
                ]),
                AttributeV1_0::Type(MessageType::Email),
                AttributeV1_0::Size(1032u64),
                AttributeV1_0::AttachmentSize(0u64),
                AttributeV1_0::Priority(true),
                AttributeV1_0::Read(true),
                AttributeV1_0::Sent(false),
                AttributeV1_0::Protected(true),
            ]))
        );

        const V1_1_TEST_FILE: &str = "/pkg/data/sample_messages_listing_v1_1_2.xml";
        let bytes = fs::read(V1_1_TEST_FILE).expect("should be ok");
        let messages_listing = MessagesListing::parse(Cursor::new(bytes)).expect("should be ok");
        assert_eq!(messages_listing.version, MessagesListingVersion::V1_1);
        assert_eq!(messages_listing.messages.len(), 2);
        assert_eq!(
            messages_listing.messages[0],
            Message::v1_1(
                HashSet::from([
                    AttributeV1_0::Handle(0x20000100001u64),
                    AttributeV1_0::Subject("Welcome Clara Nicole".to_string()),
                    AttributeV1_0::Datetime(NaiveDateTime::new(
                        NaiveDate::from_ymd_opt(2014, 07, 06).unwrap(),
                        NaiveTime::from_hms_opt(09, 50, 00).unwrap(),
                    )),
                    AttributeV1_0::SenderName("Max".to_string()),
                    AttributeV1_0::SenderAddressing("4924689753@s.whateverapp.net".to_string()),
                    AttributeV1_0::RecipientAddressing(vec!["".to_string()]),
                    AttributeV1_0::Type(MessageType::Im),
                    AttributeV1_0::Size(256u64),
                    AttributeV1_0::AttachmentSize(0u64),
                    AttributeV1_0::Priority(false),
                    AttributeV1_0::Read(false),
                    AttributeV1_0::Sent(false),
                    AttributeV1_0::Protected(false),
                ]),
                HashSet::from([
                    AttributeV1_1::ConversationId(0xE1E2E3E4F1F2F3F4A1A2A3A4B1B2B3B4),
                    AttributeV1_1::Direction(Direction::Incoming),
                ]),
            )
        );
        assert_eq!(
            messages_listing.messages[1],
            Message::v1_1(
                HashSet::from([
                    AttributeV1_0::Handle(0x20000100002u64),
                    AttributeV1_0::Subject("What’s the progress Max?".to_string()),
                    AttributeV1_0::Datetime(NaiveDateTime::new(
                        NaiveDate::from_ymd_opt(2014, 07, 05).unwrap(),
                        NaiveTime::from_hms_opt(09, 22, 00).unwrap(),
                    )),
                    AttributeV1_0::SenderName("Jonas".to_string()),
                    AttributeV1_0::SenderAddressing("4913579864@s.whateverapp.net".to_string()),
                    AttributeV1_0::RecipientAddressing(vec!["".to_string()]),
                    AttributeV1_0::Type(MessageType::Im),
                    AttributeV1_0::Size(512u64),
                    AttributeV1_0::AttachmentSize(8671724u64),
                    AttributeV1_0::Priority(false),
                    AttributeV1_0::Read(true),
                    AttributeV1_0::Sent(true),
                    AttributeV1_0::Protected(false),
                ]),
                HashSet::from([
                    AttributeV1_1::ConversationId(0xE1E2E3E4F1F2F3F4A1A2A3A4B1B2B3B4),
                    AttributeV1_1::Direction(Direction::Incoming),
                    AttributeV1_1::AttachmentMimeTypes(vec!["video/mpeg".to_string()]),
                ]),
            )
        );
    }

    #[fuchsia::test]
    fn parse_messages_listing_fail() {
        let bad_sample_xml_files = vec![
            "/pkg/data/bad_sample.xml",
            "/pkg/data/bad_sample_messages_listing_v1_0_1.xml",
            "/pkg/data/bad_sample_messages_listing_v1_0_2.xml",
            "/pkg/data/bad_sample_messages_listing_v1_1_1.xml",
            "/pkg/data/bad_sample_messages_listing_v1_1_2.xml",
        ];

        bad_sample_xml_files.iter().for_each(|f| {
            let bytes = fs::read(f).expect("should be ok");
            let _ = MessagesListing::parse(Cursor::new(bytes)).expect_err("should have failed");
        });
    }

    #[fuchsia::test]
    fn build_empty_messages_listing_success() {
        // v1.0.
        let empty_messages_listing = MessagesListing::new(MessagesListingVersion::V1_0);
        let mut buf = Vec::new();
        assert_eq!(empty_messages_listing.mime_type(), "application/xml");
        empty_messages_listing.build(&mut buf).expect("should be ok");
        assert_eq!(
            empty_messages_listing,
            MessagesListing::parse(Cursor::new(buf)).expect("should be ok")
        );

        // v1.1.
        let empty_messages_listing = MessagesListing::new(MessagesListingVersion::V1_1);
        let mut buf = Vec::new();
        assert_eq!(empty_messages_listing.mime_type(), "application/xml");
        empty_messages_listing.build(&mut buf).expect("should be ok");
        assert_eq!(
            empty_messages_listing,
            MessagesListing::parse(Cursor::new(buf)).expect("should be ok")
        );
    }

    #[fuchsia::test]
    fn build_messages_listing_success() {
        // v1.0.
        let messages_listing = MessagesListing {
            version: MessagesListingVersion::V1_0,
            messages: vec![Message::v1_0(HashSet::from([
                AttributeV1_0::Handle(0x20000100001u64),
                AttributeV1_0::Subject("Hello".to_string()),
                AttributeV1_0::Datetime(NaiveDateTime::new(
                    NaiveDate::from_ymd_opt(2007, 12, 13).unwrap(),
                    NaiveTime::from_hms_opt(13, 05, 10).unwrap(),
                )),
                AttributeV1_0::SenderName("Jamie".to_string()),
                AttributeV1_0::SenderAddressing("+1-987-6543210".to_string()),
                AttributeV1_0::RecipientAddressing(vec!["+1-0123-456789".to_string()]),
                AttributeV1_0::Type(MessageType::SmsGsm),
                AttributeV1_0::Size(256u64),
                AttributeV1_0::AttachmentSize(0u64),
                AttributeV1_0::Priority(false),
                AttributeV1_0::Read(true),
                AttributeV1_0::Sent(false),
                AttributeV1_0::Protected(false),
            ]))],
        };
        let mut buf = Vec::new();
        assert_eq!(messages_listing.mime_type(), "application/xml");
        messages_listing.build(&mut buf).expect("should have succeeded");
        assert_eq!(
            messages_listing,
            MessagesListing::parse(Cursor::new(buf)).expect("should be valid xml")
        );

        // v1.1.
        let messages_listing = MessagesListing {
            version: MessagesListingVersion::V1_1,
            messages: vec![Message::v1_1(
                HashSet::from([
                    AttributeV1_0::Handle(0x20000100001u64),
                    AttributeV1_0::Subject("Welcome Clara Nicole".to_string()),
                    AttributeV1_0::Datetime(NaiveDateTime::new(
                        NaiveDate::from_ymd_opt(2014, 07, 06).unwrap(),
                        NaiveTime::from_hms_opt(09, 50, 00).unwrap(),
                    )),
                    AttributeV1_0::SenderName("Max".to_string()),
                    AttributeV1_0::SenderAddressing("4924689753@s.whateverapp.net".to_string()),
                    AttributeV1_0::RecipientAddressing(vec!["".to_string()]),
                    AttributeV1_0::Type(MessageType::Im),
                    AttributeV1_0::Size(256u64),
                    AttributeV1_0::AttachmentSize(0u64),
                    AttributeV1_0::Priority(false),
                    AttributeV1_0::Read(false),
                    AttributeV1_0::Sent(false),
                    AttributeV1_0::Protected(false),
                ]),
                HashSet::from([
                    AttributeV1_1::ConversationId(0xE1E2E3E4F1F2F3F4A1A2A3A4B1B2B3B4),
                    AttributeV1_1::Direction(Direction::Incoming),
                ]),
            )],
        };
        let mut buf = Vec::new();
        assert_eq!(messages_listing.mime_type(), "application/xml");
        messages_listing.build(&mut buf).expect("should be ok");
        assert_eq!(
            messages_listing,
            MessagesListing::parse(Cursor::new(buf)).expect("should be ok")
        );
    }

    #[fuchsia::test]
    fn build_messages_listing_fail() {
        // Inconsistent message versions.
        let messages_listing = MessagesListing {
            version: MessagesListingVersion::V1_0,
            messages: vec![
                Message::v1_0(HashSet::from([
                    AttributeV1_0::Handle(0x20000100001u64),
                    AttributeV1_0::Subject("Hello".to_string()),
                    AttributeV1_0::Datetime(NaiveDateTime::new(
                        NaiveDate::from_ymd_opt(2007, 12, 13).unwrap(),
                        NaiveTime::from_hms_opt(13, 05, 10).unwrap(),
                    )),
                    AttributeV1_0::SenderName("Jamie".to_string()),
                    AttributeV1_0::SenderAddressing("+1-987-6543210".to_string()),
                    AttributeV1_0::RecipientAddressing(vec!["+1-0123-456789".to_string()]),
                    AttributeV1_0::Type(MessageType::SmsGsm),
                    AttributeV1_0::Size(256u64),
                    AttributeV1_0::AttachmentSize(0u64),
                    AttributeV1_0::Priority(false),
                    AttributeV1_0::Read(true),
                    AttributeV1_0::Sent(false),
                    AttributeV1_0::Protected(false),
                ])),
                Message::v1_1(
                    HashSet::from([
                        AttributeV1_0::Handle(0x20000100001u64),
                        AttributeV1_0::Subject("Welcome Clara Nicole".to_string()),
                        AttributeV1_0::Datetime(NaiveDateTime::new(
                            NaiveDate::from_ymd_opt(2014, 07, 06).unwrap(),
                            NaiveTime::from_hms_opt(09, 50, 00).unwrap(),
                        )),
                        AttributeV1_0::SenderName("Max".to_string()),
                        AttributeV1_0::SenderAddressing("4924689753@s.whateverapp.net".to_string()),
                        AttributeV1_0::RecipientAddressing(vec!["".to_string()]),
                        AttributeV1_0::Type(MessageType::Im),
                        AttributeV1_0::Size(256u64),
                        AttributeV1_0::AttachmentSize(0u64),
                        AttributeV1_0::Priority(false),
                        AttributeV1_0::Read(false),
                        AttributeV1_0::Sent(false),
                        AttributeV1_0::Protected(false),
                    ]),
                    HashSet::from([
                        AttributeV1_1::ConversationId(0xE1E2E3E4F1F2F3F4A1A2A3A4B1B2B3B4),
                        AttributeV1_1::Direction(Direction::Incoming),
                    ]),
                ),
            ],
        };
        let mut buf = Vec::new();
        assert_eq!(messages_listing.mime_type(), "application/xml");
        let _ = messages_listing.build(&mut buf).expect_err("should have failed");
    }
}
