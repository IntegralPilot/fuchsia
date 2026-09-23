// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use chrono::naive::NaiveDateTime;
use fidl_fuchsia_bluetooth_map::{Audience, Notification, NotificationType};
use objects::{Builder, ObexObjectError as Error, Parser};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use xml::EventWriter;
use xml::attribute::OwnedAttribute;
use xml::reader::{ParserConfig, XmlEvent};
use xml::writer::{EmitterConfig, XmlEvent as XmlWriteEvent};

use crate::MessageType;
use crate::packets::{ISO_8601_TIME_FORMAT, bool_to_string, str_to_bool, truncate_string};

// From MAP v1.4.2 section 3.1.7 Event-Report Object:
//
// <!DTD for the OBEX MAP-Event-Report object--> <!DOCTYPE MAP-eventreport [
// <!ELEMENT MAP-event-report ( event ) >
// <!ATTLIST MAP-event-report version CDATA #FIXED "1.1">
// <!ELEMENT event EMPTY>
// <!ATTLIST event
// type CDATA #REQUIRED
// handle CDATA #IMPLIED
// folder CDATA #IMPLIED
// old_folder CDATA #IMPLIED
// msg_type CDATA #IMPLIED
// datetime CDATA #IMPLIED
// subject CDATA #IMPLIED
// sender_name CDATA #IMPLIED
// priority CDATA #IMPLIED
// >
// ]>

const EVENT_ELEM: &str = "event";
const EVENT_REPORT_ELEM: &str = "MAP-event-report";

const VERSION_ATTR: &str = "version";

const TYPE_ATTR: &str = "type";
const HANDLE_ATTR: &str = "handle";
const FOLDER_ATTR: &str = "folder";
const OLD_FOLDER_ATTR: &str = "old_folder";
const MSG_TYPE_ATTR: &str = "msg_type";

// V1.1 specific attributes.
const DATETIME_ATTR: &str = "datetime";
const SUBJECT_ATTR: &str = "subject";
const SENDER_NAME_ATTR: &str = "sender_name";
const PRIORITY_ATTR: &str = "priority";

/// Type of MAP event report. See MAP v1.4.2 Table 3.2 for details.
/// Not all event report types form the spec are represented since
/// we don't expect to observe the events.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Type {
    NewMessage,
    DeliverySuccess,
    SendingSuccess,
    DeliveryFailure,
    SendingFailure,
    MessageDeleted,
    MessageShift,
    ReadStatusChanged,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let str_value = match self {
            Type::NewMessage => "NewMessage",
            Type::DeliverySuccess => "DeliverySuccess",
            Type::SendingSuccess => "SendingSuccess",
            Type::DeliveryFailure => "DeliveryFailure",
            Type::SendingFailure => "SendingFailure",
            Type::MessageDeleted => "MessageDeleted",
            Type::MessageShift => "MessageShift",
            Type::ReadStatusChanged => "ReadStatusChanged",
        };

        write!(f, "{}", str_value)
    }
}

impl FromStr for Type {
    type Err = Error;
    fn from_str(src: &str) -> Result<Self, Self::Err> {
        let val = match src {
            "NewMessage" => Self::NewMessage,
            "DeliverySuccess" => Self::DeliverySuccess,
            "SendingSuccess" => Self::SendingSuccess,
            "SendingFailure" => Self::DeliveryFailure,
            "MessageDeleted" => Self::MessageDeleted,
            "MessageShift" => Self::MessageShift,
            "ReadStatusChanged" => Self::ReadStatusChanged,
            v => return Err(Error::invalid_data(v)),
        };
        Ok(val)
    }
}

impl From<Type> for NotificationType {
    fn from(value: Type) -> Self {
        match value {
            Type::NewMessage => NotificationType::NewMessage,
            Type::DeliverySuccess => NotificationType::DeliverySuccess,
            Type::SendingSuccess => NotificationType::SendingSuccess,
            Type::DeliveryFailure => NotificationType::DeliveryFailure,
            Type::SendingFailure => NotificationType::SendingFailure,
            Type::MessageDeleted => NotificationType::MessageDeleted,
            Type::MessageShift => NotificationType::MessageShift,
            Type::ReadStatusChanged => NotificationType::ReadStatusChanged,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EventReportVersion {
    V1_0,
    V1_1,
}

impl fmt::Display for EventReportVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V1_0 => write!(f, "1.0"),
            Self::V1_1 => write!(f, "1.1"),
        }
    }
}

impl FromStr for EventReportVersion {
    type Err = Error;
    fn from_str(src: &str) -> Result<Self, Self::Err> {
        match src {
            "1.0" => Ok(Self::V1_0),
            "1.1" => Ok(Self::V1_1),
            _v => Err(Error::UnsupportedVersion),
        }
    }
}

/// List of attributes available for event-report objects.
/// See MAP v1.4.2 section 3.1.7 for details.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum EventAttribute {
    // Attributes that exist in EventReport v1.0 and above.
    Type(Type),
    Handle(u64),
    Folder(String),
    OldFolder(String),
    MessageType(MessageType),
    // Attributes that exist in EventReport v1.1 and above.
    Datetime(NaiveDateTime),
    Subject(String),
    SenderName(String),
    Priority(bool),
}

impl EventAttribute {
    fn xml_attribute_name(&self) -> &'static str {
        match self {
            EventAttribute::Type(_) => TYPE_ATTR,
            EventAttribute::Handle(_) => HANDLE_ATTR,
            EventAttribute::Folder(_) => FOLDER_ATTR,
            EventAttribute::OldFolder(_) => OLD_FOLDER_ATTR,
            EventAttribute::MessageType(_) => MSG_TYPE_ATTR,
            EventAttribute::Datetime(_) => DATETIME_ATTR,
            EventAttribute::Subject(_) => SUBJECT_ATTR,
            EventAttribute::SenderName(_) => SENDER_NAME_ATTR,
            EventAttribute::Priority(_) => PRIORITY_ATTR,
        }
    }

    fn xml_attribute_value(&self) -> String {
        match self {
            EventAttribute::Type(v) => v.to_string(),
            EventAttribute::Handle(v) => hex::encode(v.to_be_bytes()),
            EventAttribute::Folder(v) => v.into(),
            EventAttribute::OldFolder(v) => v.into(),
            EventAttribute::MessageType(v) => v.to_string(),
            EventAttribute::Datetime(v) => v.format(ISO_8601_TIME_FORMAT).to_string(),
            EventAttribute::Subject(v) => v.to_string(),
            EventAttribute::SenderName(v) => v.to_string(),
            EventAttribute::Priority(v) => bool_to_string(*v),
        }
    }

    /// Adds this attribute to notification.
    fn add_to_notification(&self, notification: &mut Notification) -> Result<(), Error> {
        match self {
            EventAttribute::Type(type_) => notification.type_ = Some((*type_).into()),
            EventAttribute::Handle(h) => notification.message_handle = Some(*h),
            EventAttribute::Folder(name) => notification.folder = Some(name.clone()),
            // Attributes that aren't represented in FIDL struct are ignored.
            EventAttribute::MessageType(type_) => {
                notification.message_type = Some((*type_).into());
            }
            EventAttribute::Datetime(datetime) => {
                let timestamp_nanos = datetime
                    .and_utc()
                    .timestamp_nanos_opt()
                    .ok_or_else(|| Error::invalid_data(datetime))?;
                notification.timestamp = Some(timestamp_nanos);
            }
            EventAttribute::Subject(value) => notification.subject = Some(value.to_string()),
            EventAttribute::SenderName(value) => {
                notification.sender =
                    Some(Audience { name: Some(value.to_string()), ..Default::default() })
            }
            EventAttribute::Priority(value) => notification.priority = Some(*value),
            _ => {}
        };
        Ok(())
    }

    /// Returns the event report version the attribute was introduced in.
    fn version(&self) -> EventReportVersion {
        match self {
            &EventAttribute::Datetime(_)
            | &EventAttribute::Subject(_)
            | &EventAttribute::SenderName(_)
            | &EventAttribute::Priority(_) => EventReportVersion::V1_1,
            _other => EventReportVersion::V1_0,
        }
    }

    /// Returns whether or not the event attribute is an attribute that's
    /// only allowed to exist for NewMessage event type.
    // See MAP v1.4.2 section 3.1.7 for details.
    fn is_new_message_attribute(&self) -> bool {
        if self.version() == EventReportVersion::V1_1 {
            return true;
        }
        false
    }

    /// Validate that all the attributes are valid for event report v1.0.
    fn validate_v1_0(attrs: &Vec<EventAttribute>) -> Result<(), Error> {
        // Make sure there are no v1.1 attributes.
        if let Some(a) = attrs.iter().find(|a| a.version() == EventReportVersion::V1_1) {
            return Err(Error::invalid_data(a.xml_attribute_name()));
        }
        Ok(())
    }
}

impl TryFrom<&OwnedAttribute> for EventAttribute {
    type Error = Error;

    fn try_from(src: &OwnedAttribute) -> Result<Self, Error> {
        let attr_name = src.name.local_name.as_str();
        let attribute = match attr_name {
            TYPE_ATTR => Self::Type(str::parse(src.value.as_str())?),
            HANDLE_ATTR => {
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
            // Shall be restricted to 512 bytes
            FOLDER_ATTR => Self::Folder(truncate_string(&src.value, 512)),
            OLD_FOLDER_ATTR => Self::OldFolder(truncate_string(&src.value, 512)),
            MSG_TYPE_ATTR => Self::MessageType(str::parse(src.value.as_str())?),
            // TODO(b/348051261): Support UTC Offset Timestamp Format feature.
            DATETIME_ATTR => Self::Datetime(
                NaiveDateTime::parse_from_str(src.value.as_str(), ISO_8601_TIME_FORMAT)
                    .map_err(|e| Error::invalid_data(e))?,
            ),
            SUBJECT_ATTR => Self::Subject(truncate_string(&src.value, 256)),
            SENDER_NAME_ATTR => Self::SenderName(truncate_string(&src.value, 256)),
            PRIORITY_ATTR => Self::Priority(str_to_bool(&src.value)?),
            val => return Err(Error::invalid_data(val)),
        };
        Ok(attribute)
    }
}

/// Represents a MAP-Event-Report object as defined in MAP v1.4.2 section 3.1.7.
/// The object contains one event element which contains attributes that
/// describes the event.
#[derive(Debug, PartialEq)]
pub struct EventReport {
    version: EventReportVersion,
    attrs: Vec<EventAttribute>,
}

impl EventReport {
    /// Creates a new v1.0 message.
    pub fn v1_0(attrs: Vec<EventAttribute>) -> Result<Self, Error> {
        EventReport::new(EventReportVersion::V1_0, attrs)
    }

    /// Creates a new v1.1 message.
    pub fn v1_1(attrs: Vec<EventAttribute>) -> Result<Self, Error> {
        EventReport::new(EventReportVersion::V1_1, attrs)
    }

    fn new(version: EventReportVersion, attrs: Vec<EventAttribute>) -> Result<Self, Error> {
        let event_report = EventReport { version, attrs };
        event_report.validate()?;
        Ok(event_report)
    }

    pub fn version(&self) -> EventReportVersion {
        self.version
    }

    fn validate(&self) -> Result<(), Error> {
        // See MAP v1.4.2 section 3.1.7 for details.
        // Type is a required attribute.
        let type_ = self
            .attrs
            .iter()
            .find_map(|a| if let EventAttribute::Type(t) = a { Some(t) } else { None })
            .ok_or_else(|| Error::missing_data(TYPE_ATTR))?;

        // Old folder attribute is only allowed if event type is message shift.
        if type_ != &Type::MessageShift
            && self.attrs.iter().any(|a| a.xml_attribute_name() == OLD_FOLDER_ATTR)
        {
            return Err(Error::invalid_data(
                "old_folder attribute is only allowed for MessageShift event",
            ));
        }
        match self.version {
            EventReportVersion::V1_0 => EventAttribute::validate_v1_0(&self.attrs)?,
            EventReportVersion::V1_1 => {
                // If new message event, we have no further checks.
                if type_ == &Type::NewMessage {
                    return Ok(());
                }
                // As per MAP v1.4.2 section 3.1.7 certain attributes are
                // not allowed if not for new message event.
                if self.attrs.iter().any(|a| a.is_new_message_attribute()) {
                    return Err(Error::invalid_data(
                        "contains attribute only allowed in NewMessage event",
                    ));
                }
            }
        }
        Ok(())
    }

    // Given the XML StartElement, checks whether or not it is a valid
    // `MAP-event-report` element.
    fn check_map_event_report_element(element: XmlEvent) -> Result<EventReportVersion, Error> {
        let XmlEvent::StartElement { ref name, ref attributes, .. } = element else {
            return Err(Error::invalid_data(format!("{element:?}")));
        };

        if name.local_name != EVENT_REPORT_ELEM {
            return Err(Error::invalid_data(&name.local_name));
        }

        let version_attr = &attributes
            .iter()
            .find(|a| a.name.local_name == VERSION_ATTR)
            .ok_or_else(|| Error::MissingData(VERSION_ATTR.to_string()))?
            .value;
        str::parse(version_attr.as_str())
    }

    fn write<W: std::io::Write>(&self, writer: &mut EventWriter<W>) -> Result<(), Error> {
        let mut builder = XmlWriteEvent::start_element(EVENT_ELEM);
        let attr_pairs: Vec<_> =
            self.attrs.iter().map(|a| (a.xml_attribute_name(), a.xml_attribute_value())).collect();
        for a in &attr_pairs {
            builder = builder.attr(a.0, a.1.as_str());
        }
        writer.write(builder)?;
        Ok(writer.write(XmlWriteEvent::end_element())?)
    }
}

impl TryFrom<(XmlEvent, EventReportVersion)> for EventReport {
    type Error = Error;
    fn try_from(src: (XmlEvent, EventReportVersion)) -> Result<Self, Error> {
        let XmlEvent::StartElement { ref name, ref attributes, .. } = src.0 else {
            return Err(Error::InvalidData(format!("{:?}", src)));
        };
        if name.local_name.as_str() != EVENT_ELEM {
            return Err(Error::invalid_data(&name.local_name));
        }
        let mut seen_attrs: HashSet<_> = HashSet::new();
        let mut attrs = vec![];

        attributes.iter().try_for_each(|a| {
            let attr = EventAttribute::try_from(a)?;
            if !seen_attrs.insert(std::mem::discriminant(&attr)) {
                return Err(Error::DuplicateData(a.name.local_name.to_string()));
            }
            attrs.push(attr);
            Ok(())
        })?;

        let event_report = EventReport { version: src.1, attrs };
        let _ = event_report.validate()?;
        Ok(event_report)
    }
}

impl TryFrom<&EventReport> for Notification {
    type Error = Error;
    fn try_from(value: &EventReport) -> Result<Self, Error> {
        let mut notification = Notification::default();

        for a in &value.attrs {
            let _ = a.add_to_notification(&mut notification)?;
        }
        Ok(notification)
    }
}

impl Parser for EventReport {
    type Error = Error;

    /// Parses EventReport from raw bytes of XML data.
    fn parse<R: std::io::prelude::Read>(buf: R) -> Result<Self, Self::Error> {
        let mut reader = ParserConfig::new()
            .ignore_comments(true)
            .whitespace_to_characters(true)
            .cdata_to_characters(true)
            .trim_whitespace(true)
            .create_reader(buf);

        // Process start of document.
        match reader.next() {
            Ok(XmlEvent::StartDocument { .. }) => {}
            Ok(element) => return Err(Error::InvalidData(format!("{:?}", element))),
            Err(e) => return Err(Error::ReadXml(e)),
        };

        // Process start of `MAP-event-report` element.
        let mut xml_event = reader.next()?;
        while matches!(xml_event, XmlEvent::Doctype { .. } | XmlEvent::Comment(_)) {
            xml_event = reader.next()?;
        }
        let version = EventReport::check_map_event_report_element(xml_event)?;

        // Process start of `event` element.
        let mut xml_event: XmlEvent = reader.next()?;
        while matches!(xml_event, XmlEvent::Doctype { .. } | XmlEvent::Comment(_)) {
            xml_event = reader.next()?;
        }
        let invalid_elem_err = Err(Error::InvalidData(format!("{:?}", xml_event)));
        let event_report: EventReport = match xml_event {
            XmlEvent::StartElement { ref name, .. } => match name.local_name.as_str() {
                EVENT_ELEM => (xml_event, version).try_into()?,
                _ => return invalid_elem_err,
            },
            _ => return invalid_elem_err,
        };

        // Process end of `event` element.
        let xml_event = reader.next()?;
        match xml_event {
            XmlEvent::EndElement { ref name } => {
                if name.local_name.as_str() != EVENT_ELEM {
                    return Err(Error::InvalidData(format!("{:?}", xml_event)));
                }
            }
            _ => return Err(Error::InvalidData(format!("{:?}", xml_event))),
        };

        // Process end of `MAP-event-report` element.
        let xml_event = reader.next()?;
        match xml_event {
            XmlEvent::EndElement { ref name } => {
                if name.local_name.as_str() != EVENT_REPORT_ELEM {
                    return Err(Error::InvalidData(format!("{:?}", xml_event)));
                }
            }
            _ => return Err(Error::InvalidData(format!("{:?}", xml_event))),
        };

        // Process end of document.
        let xml_event = reader.next()?;
        match xml_event {
            XmlEvent::EndDocument { .. } => {}
            _ => return Err(Error::InvalidData(format!("{:?}", xml_event))),
        };

        Ok(event_report)
    }
}

impl Builder for EventReport {
    type Error = Error;

    // Returns the document type of the raw bytes of data.
    fn mime_type(&self) -> String {
        "application/xml".to_string()
    }

    fn build<W: std::io::Write>(&self, buf: W) -> core::result::Result<(), Self::Error> {
        // Only build the XML if the event report is a valid event report.
        let _ = self.validate()?;

        let mut w = EmitterConfig::new()
            .write_document_declaration(true)
            .perform_indent(true)
            .create_writer(buf);

        // Begin `MAP-event-report` element.
        let version = self.version().to_string();
        let event_report =
            XmlWriteEvent::start_element(EVENT_REPORT_ELEM).attr(VERSION_ATTR, version.as_str());
        w.write(event_report)?;

        // Write `event` element.
        self.write(&mut w)?;

        // End `MAP-event-report` element.
        Ok(w.write(XmlWriteEvent::end_element())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{NaiveDate, NaiveTime};
    use fidl_fuchsia_bluetooth_map as fidl_bt_map;

    use std::fs;
    use std::io::Cursor;

    #[fuchsia::test]
    fn event_report() {
        let _event_report = EventReport::v1_0(vec![
            EventAttribute::Type(Type::NewMessage),
            EventAttribute::Handle(0x12345678),
            EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
            EventAttribute::MessageType(MessageType::SmsCdma),
        ])
        .expect("should be created");

        let _event_report = EventReport::v1_1(vec![
            EventAttribute::Type(Type::NewMessage),
            EventAttribute::Handle(0x12345678),
            EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
            EventAttribute::MessageType(MessageType::SmsCdma),
            EventAttribute::Subject("Hello".to_string()),
        ])
        .expect("should be created");
    }

    #[fuchsia::test]
    fn event_report_fail() {
        // Old folder attr not allowed.
        let _ = EventReport::v1_0(vec![
            EventAttribute::Type(Type::DeliveryFailure),
            EventAttribute::Handle(0x12345678),
            EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
            EventAttribute::OldFolder("TELECOM/MSG/SOME".to_string()),
            EventAttribute::MessageType(MessageType::SmsCdma),
        ])
        .expect_err("should fail");

        // Subject attr not allowed for non NewMessage type.
        let _ = EventReport::v1_1(vec![
            EventAttribute::Type(Type::DeliveryFailure),
            EventAttribute::Handle(0x12345678),
            EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
            EventAttribute::MessageType(MessageType::SmsCdma),
            EventAttribute::Subject("Hello".to_string()),
        ])
        .expect_err("should fail");
    }

    #[fuchsia::test]
    fn parse_event_report_v1_0_success() {
        const V1_0_TEST_FILE: &str = "/pkg/data/sample_event_report_v1_0.xml";
        let bytes = fs::read(V1_0_TEST_FILE).expect("should be ok");
        let report = EventReport::parse(Cursor::new(bytes)).expect("should be ok");
        assert_eq!(report.version(), EventReportVersion::V1_0);
        assert_eq!(
            report.attrs,
            vec![
                EventAttribute::Type(Type::NewMessage),
                EventAttribute::Handle(0x12345678),
                EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
                EventAttribute::MessageType(MessageType::SmsCdma),
            ]
        );
    }

    #[fuchsia::test]
    fn parse_event_report_v1_1_success() {
        const V1_1_TEST_FILE: &str = "/pkg/data/sample_event_report_v1_1.xml";
        let bytes = fs::read(V1_1_TEST_FILE).expect("should be ok");
        let report = EventReport::parse(Cursor::new(bytes)).expect("should be ok");
        assert_eq!(report.version(), EventReportVersion::V1_1);
        assert_eq!(
            report.attrs,
            vec![
                EventAttribute::Type(Type::NewMessage),
                EventAttribute::Handle(0x12345678),
                EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
                EventAttribute::MessageType(MessageType::SmsCdma),
                EventAttribute::Subject("Hello".to_string()),
                EventAttribute::Datetime(NaiveDateTime::new(
                    NaiveDate::from_ymd_opt(2011, 02, 21).unwrap(),
                    NaiveTime::from_hms_opt(13, 05, 10).unwrap(),
                )),
                EventAttribute::SenderName("Jamie".to_string()),
                EventAttribute::Priority(true),
            ]
        );
    }

    #[fuchsia::test]
    fn parse_event_report_fail() {
        let bytes = fs::read("/pkg/data/bad_sample.xml").unwrap();
        let Error::InvalidData(_) =
            EventReport::parse(Cursor::new(bytes)).expect_err("should have failed")
        else {
            panic!("expected failure due to invalid data");
        };

        let bytes = fs::read("/pkg/data/sample_event_report_v1_2.xml").unwrap();
        let Error::UnsupportedVersion =
            EventReport::parse(Cursor::new(bytes)).expect_err("should have failed")
        else {
            panic!("expected failure due to unsupported version");
        };

        let bytes = fs::read("/pkg/data/bad_sample_event_report_v1_0_1.xml").unwrap();
        let Error::ReadXml(_) =
            EventReport::parse(Cursor::new(bytes)).expect_err("should have failed")
        else {
            panic!("expected failure due to read xml error");
        };

        let bytes = fs::read("/pkg/data/bad_sample_event_report_v1_0_2.xml").unwrap();
        let Error::MissingData(_) =
            EventReport::parse(Cursor::new(bytes)).expect_err("should have failed")
        else {
            panic!("expected failure due to missing data");
        };

        let bytes = fs::read("/pkg/data/bad_sample_event_report_v1_1.xml").unwrap();
        let Error::InvalidData(_) =
            EventReport::parse(Cursor::new(bytes)).expect_err("should have failed")
        else {
            panic!("expected failure due to invalid data");
        };
    }

    #[fuchsia::test]
    fn build_event_report_v1_0_success() {
        let event_report = EventReport {
            version: EventReportVersion::V1_0,
            attrs: vec![
                EventAttribute::Type(Type::NewMessage),
                EventAttribute::Handle(12345678),
                EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
                EventAttribute::MessageType(MessageType::SmsCdma),
            ],
        };

        let mut buf = Vec::new();
        let _ = event_report.build(&mut buf).expect("should have succeeded");
        assert_eq!(
            event_report,
            EventReport::parse(Cursor::new(buf)).expect("should be valid xml")
        );
    }

    #[fuchsia::test]
    fn build_event_report_v1_1_success() {
        let event_report = EventReport {
            version: EventReportVersion::V1_1,
            attrs: vec![
                EventAttribute::Type(Type::NewMessage),
                EventAttribute::Handle(12345678),
                EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
                EventAttribute::MessageType(MessageType::SmsCdma),
                EventAttribute::Subject("Hello".to_string()),
                EventAttribute::Datetime(NaiveDateTime::new(
                    NaiveDate::from_ymd_opt(2011, 02, 21).unwrap(),
                    NaiveTime::from_hms_opt(13, 05, 10).unwrap(),
                )),
                EventAttribute::SenderName("Jamie".to_string()),
                EventAttribute::Priority(true),
            ],
        };

        let mut buf = Vec::new();
        event_report.build(&mut buf).expect("should have succeeded");
        assert_eq!(
            event_report,
            EventReport::parse(Cursor::new(buf)).expect("should be valid xml")
        );
    }

    #[fuchsia::test]
    fn build_event_report_fail() {
        // Invalid v1.0 since missing Type attr.
        let event_report = EventReport {
            version: EventReportVersion::V1_0,
            attrs: vec![
                EventAttribute::Handle(12345678),
                EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
                EventAttribute::MessageType(MessageType::SmsCdma),
            ],
        };
        let mut buf = Vec::new();
        let _ = event_report.build(&mut buf).expect_err("should have failed");

        // Invalid v1.1 since incompatible Type and Subject attrs.
        let event_report = EventReport {
            version: EventReportVersion::V1_1,
            attrs: vec![
                EventAttribute::Type(Type::DeliveryFailure),
                EventAttribute::Handle(12345678),
                EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
                EventAttribute::MessageType(MessageType::SmsCdma),
                EventAttribute::Subject("Hello".to_string()),
            ],
        };
        let mut buf = Vec::new();
        let _ = event_report.build(&mut buf).expect_err("should have failed");
    }

    #[fuchsia::test]
    fn notification() {
        let event_report = EventReport::v1_1(vec![
            EventAttribute::Type(Type::NewMessage),
            EventAttribute::Handle(12345678),
            EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
            EventAttribute::MessageType(MessageType::SmsCdma),
            EventAttribute::Datetime(NaiveDateTime::new(
                NaiveDate::from_ymd_opt(2000, 01, 09).unwrap(),
                NaiveTime::from_hms_opt(01, 09, 19).unwrap(),
            )),
        ])
        .unwrap();

        // Should be converted successfully.
        let notification: Notification = (&event_report).try_into().expect("should succeed");
        assert_eq!(
            notification,
            Notification {
                type_: Some(NotificationType::NewMessage),
                message_handle: Some(12345678),
                folder: Some("TELECOM/MSG/INBOX".to_string()),
                message_type: Some(fidl_bt_map::MessageType::SMS_CDMA),
                timestamp: Some(947380159000000000),
                ..Default::default()
            }
        );
    }

    #[fuchsia::test]
    fn notification_fail() {
        let event_report = EventReport::v1_1(vec![
            EventAttribute::Type(Type::NewMessage),
            EventAttribute::Handle(12345678),
            EventAttribute::Folder("TELECOM/MSG/INBOX".to_string()),
            EventAttribute::MessageType(MessageType::SmsCdma),
            EventAttribute::Datetime(NaiveDateTime::new(
                // Out of range date time.
                NaiveDate::from_ymd_opt(1500, 02, 21).unwrap(),
                NaiveTime::from_hms_opt(13, 05, 10).unwrap(),
            )),
        ])
        .unwrap();

        // Conversion should fail.
        let res: Result<Notification, _> = (&event_report).try_into();
        let _ = res.expect_err("should have failed");
    }
}
