// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;
use chrono::naive::NaiveDateTime;
use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use xml::EventWriter;
use xml::attribute::OwnedAttribute;
use xml::name::OwnedName;
use xml::reader::{ParserConfig, XmlEvent};
use xml::writer::{EmitterConfig, XmlEvent as XmlWriteEvent};

use crate::error::Error;
use crate::{Builder, Parser};

/// Element names
const FILE_ELEM: &str = "file";
const FOLDER_ELEM: &str = "folder";
const PARENT_FOLDER_ELEM: &str = "parent-folder";
const FOLDER_LISTING_ELEM: &str = "folder-listing";

const VERSION_ATTR: &str = "version";
const NAME_ATTR: &str = "name";
const SIZE_ATTR: &str = "size";
const MODIFIED_ATTR: &str = "modified";
const CREATED_ATTR: &str = "created";
const ACCESSED_ATTR: &str = "accessed";
const USER_PERM_ATTR: &str = "user-perm";
const GROUP_PERM_ATTR: &str = "group-perm";
const OTHER_PERM_ATTR: &str = "other-perm";
const OWNER_ATTR: &str = "owner";
const GROUP_ATTR: &str = "group";
const TYPE_ATTR: &str = "type";
const XML_LANG_ATTR: &str = "xml:lang";

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Permission : u8 {
        const READ = 0x1;
        const WRITE = 0x2;
        const DELETE = 0x4;
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.contains(Self::READ) {
            write!(f, "R")?;
        }
        if self.contains(Self::WRITE) {
            write!(f, "W")?;
        }
        if self.contains(Self::DELETE) {
            write!(f, "D")?;
        }
        Ok(())
    }
}

impl FromStr for Permission {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut perm = Permission::empty();
        s.chars().try_for_each(|c| {
            match c {
                'R' => perm = perm | Permission::READ,
                'W' => perm = perm | Permission::WRITE,
                'D' => perm = perm | Permission::DELETE,
                _ => return Err(Error::InvalidData(s.to_string())),
            };
            Ok(())
        })?;
        Ok(perm)
    }
}

// Naive date time object with format string to use for formatting.
#[derive(Clone, Debug, PartialEq)]
pub struct FormattedDateTimeObj(NaiveDateTime, String);

impl FormattedDateTimeObj {
    /// The ISO 8601 time format used in the Time Header packet.
    /// The format is YYYYMMDDTHHMMSS where "T" delimits the date from the time. It is assumed that
    /// the time is the local time, but per OBEX 2.2.5, a suffix of "Z" can be included to indicate
    /// UTC time.
    const ISO_8601_UTC_TIME_FORMAT: &'static str = "%Y%m%dT%H%M%SZ";
    const ISO_8601_TIME_FORMAT: &'static str = "%Y%m%dT%H%M%S";

    fn new(dt: NaiveDateTime) -> Self {
        Self(dt, Self::ISO_8601_TIME_FORMAT.to_string())
    }
    fn new_utc(dt: NaiveDateTime) -> Self {
        Self(dt, Self::ISO_8601_UTC_TIME_FORMAT.to_string())
    }

    fn parse_datetime(dt: String) -> Result<Self, Error> {
        let Ok(datetime) =
            NaiveDateTime::parse_from_str(dt.as_str(), Self::ISO_8601_UTC_TIME_FORMAT)
        else {
            return NaiveDateTime::parse_from_str(dt.as_str(), Self::ISO_8601_TIME_FORMAT)
                .map(|t| Self::new(t))
                .map_err(|_| Error::InvalidData(dt));
        };

        // UTC timestamp.
        Ok(Self::new_utc(datetime))
    }
}

impl fmt::Display for FormattedDateTimeObj {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.format(self.1.as_str()))
    }
}

#[derive(Clone, Debug)]
pub enum FolderListingAttribute {
    Name(String),
    Size(String),
    Modified(FormattedDateTimeObj),
    Created(FormattedDateTimeObj),
    Accessed(FormattedDateTimeObj),
    UserPerm(Permission),
    GroupPerm(Permission),
    OtherPerm(Permission),
    Owner(String),
    Group(String),
    XmlLang(String),
    Type(String),
}

impl Hash for FolderListingAttribute {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.xml_attribute_name().hash(state);
    }
}

impl PartialEq for FolderListingAttribute {
    fn eq(&self, other: &FolderListingAttribute) -> bool {
        self.xml_attribute_name() == other.xml_attribute_name()
    }
}

impl Eq for FolderListingAttribute {}

pub type Attributes = HashSet<FolderListingAttribute>;

impl FolderListingAttribute {
    fn try_from_xml_attribute(a: &OwnedAttribute) -> Result<Self, Error> {
        let v = a.value.clone();
        // For now ignore all attributes that aren't recognized.
        Ok(match a.name.local_name.as_str() {
            NAME_ATTR => Self::Name(v),
            SIZE_ATTR => Self::Size(v),
            MODIFIED_ATTR => Self::Modified(FormattedDateTimeObj::parse_datetime(v)?),
            CREATED_ATTR => Self::Created(FormattedDateTimeObj::parse_datetime(v)?),
            ACCESSED_ATTR => Self::Accessed(FormattedDateTimeObj::parse_datetime(v)?),
            USER_PERM_ATTR => Self::UserPerm(str::parse(v.as_str())?),
            GROUP_PERM_ATTR => Self::GroupPerm(str::parse(v.as_str())?),
            OTHER_PERM_ATTR => Self::OtherPerm(str::parse(v.as_str())?),
            OWNER_ATTR => Self::Owner(v),
            GROUP_ATTR => Self::Group(v),
            XML_LANG_ATTR => Self::XmlLang(v),
            TYPE_ATTR => Self::Type(v),
            _ => return Err(Error::InvalidData(format!("{a:?}"))),
        })
    }

    const fn xml_attribute_name(&self) -> &'static str {
        match self {
            Self::Name(_) => NAME_ATTR,
            Self::Size(_) => SIZE_ATTR,
            Self::Modified(_) => MODIFIED_ATTR,
            Self::Created(_) => CREATED_ATTR,
            Self::Accessed(_) => ACCESSED_ATTR,
            Self::UserPerm(_) => USER_PERM_ATTR,
            Self::GroupPerm(_) => GROUP_PERM_ATTR,
            Self::OtherPerm(_) => OTHER_PERM_ATTR,
            Self::Owner(_) => OWNER_ATTR,
            Self::Group(_) => GROUP_ATTR,
            Self::XmlLang(_) => XML_LANG_ATTR,
            Self::Type(_) => TYPE_ATTR,
        }
    }

    fn xml_attribute_value(&self) -> String {
        match self {
            Self::Name(v)
            | Self::Size(v)
            | Self::Owner(v)
            | Self::Group(v)
            | Self::XmlLang(v)
            | Self::Type(v) => v.clone(),
            Self::Modified(dt) | Self::Created(dt) | Self::Accessed(dt) => dt.to_string(),
            Self::UserPerm(p) | Self::GroupPerm(p) | Self::OtherPerm(p) => p.to_string(),
        }
    }
}

/// See OBEX v1.4 section 9.1.1.2.2 for attributes for File elements.
#[derive(Clone, Debug, PartialEq)]
pub struct File {
    data: Option<String>,
    attributes: Attributes,
}

impl File {
    fn write<W: std::io::prelude::Write>(&self, writer: &mut EventWriter<W>) -> Result<(), Error> {
        // Build the file XML element.
        let mut builder = XmlWriteEvent::start_element(FILE_ELEM);
        let attrs: Vec<(&str, String)> = self
            .attributes
            .iter()
            .map(|a| (a.xml_attribute_name(), a.xml_attribute_value()))
            .collect();
        for a in &attrs {
            builder = builder.attr(a.0, &a.1);
        }
        writer.write(builder)?;

        if let Some(data) = &self.data {
            writer.write(data.as_str())?;
        }

        // Write the end element.
        writer.write(XmlWriteEvent::end_element()).map_err(Into::into)
    }

    fn validate(&self) -> Result<(), Error> {
        // Name is a required field.
        if !self.attributes.contains(&FolderListingAttribute::Name("".to_string())) {
            return Err(Error::MissingData(NAME_ATTR.to_string()));
        }
        Ok(())
    }
}

impl TryFrom<XmlEvent> for File {
    type Error = Error;
    fn try_from(src: XmlEvent) -> Result<Self, Error> {
        let XmlEvent::StartElement { ref name, ref attributes, .. } = src else {
            return Err(Error::InvalidData(format!("{:?}", src)));
        };
        if name.local_name.as_str() != FILE_ELEM {
            return Err(Error::InvalidData(name.local_name.clone()));
        }
        let mut attrs = HashSet::new();
        for a in attributes {
            if !attrs.insert(FolderListingAttribute::try_from_xml_attribute(a)?) {
                return Err(Error::InvalidData(format!("duplicate \"{}\"", a.name.local_name)));
            }
        }

        let file = File { data: None, attributes: attrs };
        file.validate()?;
        Ok(file)
    }
}

/// See OBEX v1.4 section 9.1.1.2.2 for attributes for Folder elements.
#[derive(Clone, Debug, PartialEq)]
pub struct Folder {
    data: Option<String>,
    attributes: Attributes,
}

impl Folder {
    fn write<W: std::io::prelude::Write>(&self, writer: &mut EventWriter<W>) -> Result<(), Error> {
        // Build the file XML element.
        let mut builder = XmlWriteEvent::start_element(FOLDER_ELEM);
        let attrs: Vec<(&str, String)> = self
            .attributes
            .iter()
            .map(|a| (a.xml_attribute_name(), a.xml_attribute_value()))
            .collect();
        for a in &attrs {
            builder = builder.attr(a.0, a.1.as_str());
        }
        writer.write(builder)?;

        if let Some(data) = &self.data {
            writer.write(data.as_str())?;
        }

        // Write the end element.
        Ok(writer.write(XmlWriteEvent::end_element())?)
    }

    fn validate(&self) -> Result<(), Error> {
        // Name is a required field.
        if !self.attributes.contains(&FolderListingAttribute::Name("".to_string())) {
            return Err(Error::MissingData(NAME_ATTR.to_string()));
        }
        // Type field only applies to File elements.
        if self.attributes.contains(&FolderListingAttribute::Type("".to_string())) {
            return Err(Error::InvalidData(TYPE_ATTR.to_string()));
        }
        Ok(())
    }
}

impl TryFrom<XmlEvent> for Folder {
    type Error = Error;
    fn try_from(src: XmlEvent) -> Result<Self, Error> {
        let XmlEvent::StartElement { ref name, ref attributes, .. } = src else {
            return Err(Error::InvalidData(format!("{:?}", src)));
        };
        if name.local_name.as_str() != FOLDER_ELEM {
            return Err(Error::InvalidData(name.local_name.clone()));
        }
        let mut attrs = HashSet::new();
        attributes.iter().try_for_each(|a| {
            if !attrs.insert(FolderListingAttribute::try_from_xml_attribute(a)?) {
                return Err(Error::InvalidData(format!("duplicate \"{}\"", a.name.local_name)));
            }
            Ok(())
        })?;

        let folder = Folder { data: None, attributes: attrs };
        folder.validate()?;
        Ok(folder)
    }
}

enum ParsedXmlEvent {
    DocumentStart,
    FolderListingElement,
    ParentFolderElement,
    FolderElement(Folder),
    FileElement(File),
}

/// FolderListing struct represents the list of the objects in the current folder.
/// See OBEX v1.4 section 9.1.4 for XML Document Definition for OBEX folder listing.
/// Version field is not stated explicitly since we only support version 1.0.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderListing {
    // Whether or current folder has a parent folder.
    parent_folder: bool,
    files: Vec<File>,
    folders: Vec<Folder>,
}

impl FolderListing {
    const DEFAULT_VERSION: &'static str = "1.0";

    fn new_empty() -> Self {
        Self { parent_folder: false, files: Vec::new(), folders: Vec::new() }
    }

    // Given the XML StartElement, checks whether or not it is a valid folder
    // listing element.
    fn validate_folder_listing_element(element: XmlEvent) -> Result<(), Error> {
        let XmlEvent::StartElement { ref name, ref attributes, .. } = element else {
            return Err(Error::InvalidData(format!("{:?}", element)));
        };

        if name.local_name != FOLDER_LISTING_ELEM {
            return Err(Error::InvalidData(name.local_name.clone()));
        }
        let default_version: OwnedAttribute = OwnedAttribute::new(
            OwnedName { local_name: VERSION_ATTR.to_string(), namespace: None, prefix: None },
            FolderListing::DEFAULT_VERSION,
        );
        // If the version attribute was missing, assume 1.0.
        let version = &attributes
            .iter()
            .find(|a| a.name.local_name == VERSION_ATTR)
            .unwrap_or(&default_version)
            .value;
        if version != FolderListing::DEFAULT_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        Ok(())
    }
}

impl Parser for FolderListing {
    type Error = Error;

    /// Parses FolderListing from raw bytes of XML data.
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
        let _ = Self::validate_folder_listing_element(xml_event)?;

        prev.push(ParsedXmlEvent::FolderListingElement);
        let mut folder_listing = Self::new_empty();

        // Process remaining elements elements.
        let mut finished_document = false;
        let mut finished_folder_listing = false;
        while !finished_document {
            // Could be either end of folder listing element,
            let e = reader.next()?;
            let invalid_elem_err = Err(Error::InvalidData(format!("{:?}", e)));
            match e {
                XmlEvent::StartElement { ref name, .. } => {
                    match name.local_name.as_str() {
                        PARENT_FOLDER_ELEM => prev.push(ParsedXmlEvent::ParentFolderElement),
                        FOLDER_ELEM => prev.push(ParsedXmlEvent::FolderElement(e.try_into()?)),
                        FILE_ELEM => prev.push(ParsedXmlEvent::FileElement(e.try_into()?)),
                        other_value => return Err(Error::InvalidData(other_value.to_string())),
                    };
                }
                XmlEvent::EndElement { ref name } => {
                    let Some(parsed_elem) = prev.pop() else {
                        return invalid_elem_err;
                    };
                    match name.local_name.as_str() {
                        FOLDER_LISTING_ELEM => {
                            let ParsedXmlEvent::FolderListingElement = parsed_elem else {
                                return Err(Error::MissingData(format!(
                                    "closing {FOLDER_LISTING_ELEM}"
                                )));
                            };
                            finished_folder_listing = true;
                        }
                        PARENT_FOLDER_ELEM => {
                            let ParsedXmlEvent::ParentFolderElement = parsed_elem else {
                                return Err(Error::MissingData(format!(
                                    "closing {PARENT_FOLDER_ELEM}"
                                )));
                            };
                            folder_listing.parent_folder = true;
                        }
                        FOLDER_ELEM => {
                            let ParsedXmlEvent::FolderElement(f) = parsed_elem else {
                                return Err(Error::MissingData(format!("closing {FOLDER_ELEM}")));
                            };
                            folder_listing.folders.push(f);
                        }
                        FILE_ELEM => {
                            let ParsedXmlEvent::FileElement(f) = parsed_elem else {
                                return Err(Error::MissingData(format!("closing {FILE_ELEM}")));
                            };
                            folder_listing.files.push(f);
                        }
                        _ => return invalid_elem_err,
                    };
                }
                XmlEvent::Characters(data) => {
                    let err = Err(Error::InvalidData(data.clone()));
                    if let Some(mut event) = prev.pop() {
                        match &mut event {
                            ParsedXmlEvent::FolderElement(f) => f.data = Some(data),
                            ParsedXmlEvent::FileElement(f) => f.data = Some(data),
                            _ => return err,
                        };
                        prev.push(event);
                    } else {
                        return err;
                    }
                }
                XmlEvent::EndDocument => {
                    if !finished_folder_listing {
                        return Err(Error::MissingData(format!("closing {FOLDER_LISTING_ELEM}")));
                    }
                    finished_document = true;
                }
                XmlEvent::Doctype { .. } | XmlEvent::Comment(_) => {}
                _ => return invalid_elem_err,
            }
        }
        Ok(folder_listing)
    }
}

impl Builder for FolderListing {
    type Error = Error;

    // Returns the MIME type of the raw bytes of data.
    fn mime_type(&self) -> String {
        "application/xml".to_string()
    }

    /// Builds self into raw bytes of the specific Document Type.
    fn build<W: std::io::Write>(&self, buf: W) -> Result<(), Self::Error> {
        let mut w = EmitterConfig::new()
            .write_document_declaration(true)
            .perform_indent(true)
            .create_writer(buf);

        // Begin `folder-listing` element.
        let folder_listing = XmlWriteEvent::start_element(FOLDER_LISTING_ELEM)
            .attr(VERSION_ATTR, Self::DEFAULT_VERSION);
        w.write(folder_listing)?;

        if self.parent_folder {
            w.write(XmlWriteEvent::start_element(PARENT_FOLDER_ELEM))?;
            w.write(XmlWriteEvent::end_element())?;
        }
        self.folders.iter().try_for_each(|f| f.write(&mut w))?;
        self.files.iter().try_for_each(|f| f.write(&mut w))?;

        // End `folder-listing` element.
        w.write(XmlWriteEvent::end_element())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveTime};

    use std::fs;
    use std::io::Cursor;

    #[fuchsia::test]
    fn parse_empty_folder_listing_success() {
        const EMPTY_FOLDER_LISTING_TEST_FILE: &str = "/pkg/data/sample_folder_listing_1.xml";
        let bytes = fs::read(EMPTY_FOLDER_LISTING_TEST_FILE).expect("should be ok");
        let folder_listing = FolderListing::parse(Cursor::new(bytes)).expect("should be ok");
        assert_eq!(folder_listing, FolderListing::new_empty());
    }

    #[fuchsia::test]
    fn parse_simple_folder_listing_success() {
        const FOLDER_LISTING_TEST_FILE: &str = "/pkg/data/sample_folder_listing_2.xml";
        let bytes = fs::read(FOLDER_LISTING_TEST_FILE).expect("should be ok");
        let folder_listing = FolderListing::parse(Cursor::new(bytes)).expect("should be ok");
        assert_eq!(
            folder_listing,
            FolderListing {
                parent_folder: true,
                files: vec![
                    File {
                        data: Some("Jumar Handling Guide".to_string()),
                        attributes: HashSet::from([
                            FolderListingAttribute::Name("Jumar.txt".to_string()),
                            FolderListingAttribute::Size("6672".to_string()),
                        ]),
                    },
                    File {
                        data: Some("OBEX Specification v1.0".to_string()),
                        attributes: HashSet::from([
                            FolderListingAttribute::Name("Obex.doc".to_string()),
                            FolderListingAttribute::Type("application/msword".to_string()),
                        ]),
                    },
                ],
                folders: vec![
                    Folder {
                        data: None,
                        attributes: HashSet::from([FolderListingAttribute::Name(
                            "System".to_string()
                        ),])
                    },
                    Folder {
                        data: None,
                        attributes: HashSet::from([FolderListingAttribute::Name(
                            "IR Inbox".to_string()
                        ),])
                    },
                ],
            }
        );
    }

    #[fuchsia::test]
    fn parse_detailed_folder_listing_success() {
        const DETAILED_FOLDER_LISTING_TEST_FILE: &str = "/pkg/data/sample_folder_listing_3.xml";
        let bytes = fs::read(DETAILED_FOLDER_LISTING_TEST_FILE).expect("should be ok");
        let folder_listing = FolderListing::parse(Cursor::new(bytes)).expect("should be ok");
        assert_eq!(
            folder_listing,
            FolderListing {
                parent_folder: true,
                files: vec![
                    File {
                        data: None,
                        attributes: HashSet::from([
                            FolderListingAttribute::Name("Jumar.txt".to_string()),
                            FolderListingAttribute::Created(FormattedDateTimeObj(
                                NaiveDateTime::new(
                                    NaiveDate::from_ymd_opt(1997, 12, 09).unwrap(),
                                    NaiveTime::from_hms_opt(09, 03, 00).unwrap(),
                                ),
                                FormattedDateTimeObj::ISO_8601_TIME_FORMAT.to_string(),
                            )),
                            FolderListingAttribute::Size("6672".to_string()),
                            FolderListingAttribute::Modified(FormattedDateTimeObj(
                                NaiveDateTime::new(
                                    NaiveDate::from_ymd_opt(1997, 12, 22).unwrap(),
                                    NaiveTime::from_hms_opt(16, 41, 00).unwrap(),
                                ),
                                FormattedDateTimeObj::ISO_8601_TIME_FORMAT.to_string(),
                            )),
                            FolderListingAttribute::UserPerm(Permission::READ | Permission::WRITE),
                        ]),
                    },
                    File {
                        data: None,
                        attributes: HashSet::from([
                            FolderListingAttribute::Name("Obex.doc".to_string()),
                            FolderListingAttribute::Created(FormattedDateTimeObj(
                                NaiveDateTime::new(
                                    NaiveDate::from_ymd_opt(1997, 01, 22).unwrap(),
                                    NaiveTime::from_hms_opt(10, 23, 00).unwrap(),
                                ),
                                FormattedDateTimeObj::ISO_8601_UTC_TIME_FORMAT.to_string(),
                            )),
                            FolderListingAttribute::Size("41042".to_string()),
                            FolderListingAttribute::Type("application/msword".to_string()),
                            FolderListingAttribute::Modified(FormattedDateTimeObj(
                                NaiveDateTime::new(
                                    NaiveDate::from_ymd_opt(1997, 01, 22).unwrap(),
                                    NaiveTime::from_hms_opt(10, 23, 00).unwrap(),
                                ),
                                FormattedDateTimeObj::ISO_8601_UTC_TIME_FORMAT.to_string(),
                            )),
                        ]),
                    },
                ],
                folders: vec![
                    Folder {
                        data: None,
                        attributes: HashSet::from([
                            FolderListingAttribute::Name("System".to_string()),
                            FolderListingAttribute::Created(FormattedDateTimeObj(
                                NaiveDateTime::new(
                                    NaiveDate::from_ymd_opt(1996, 11, 03).unwrap(),
                                    NaiveTime::from_hms_opt(14, 15, 00).unwrap(),
                                ),
                                FormattedDateTimeObj::ISO_8601_TIME_FORMAT.to_string(),
                            )),
                        ]),
                    },
                    Folder {
                        data: None,
                        attributes: HashSet::from([
                            FolderListingAttribute::Name("IR Inbox".to_string()),
                            FolderListingAttribute::Created(FormattedDateTimeObj(
                                NaiveDateTime::new(
                                    NaiveDate::from_ymd_opt(1995, 03, 30).unwrap(),
                                    NaiveTime::from_hms_opt(10, 50, 00).unwrap(),
                                ),
                                FormattedDateTimeObj::ISO_8601_UTC_TIME_FORMAT.to_string(),
                            )),
                        ]),
                    },
                ],
            }
        );
    }

    #[fuchsia::test]
    fn parse_folder_listing_fail() {
        let bad_sample_xml_files = vec![
            "/pkg/data/bad_sample.xml",
            "/pkg/data/bad_sample_folder_listing_1.xml",
            "/pkg/data/bad_sample_folder_listing_2.xml",
            "/pkg/data/bad_sample_folder_listing_3.xml",
            "/pkg/data/bad_sample_folder_listing_4.xml",
            "/pkg/data/bad_sample_folder_listing_5.xml",
            "/pkg/data/bad_sample_folder_listing_6.xml",
            "/pkg/data/bad_sample_folder_listing_7.xml",
            "/pkg/data/bad_sample_folder_listing_8.xml",
        ];

        bad_sample_xml_files.iter().for_each(|f| {
            let bytes = fs::read(f).expect("should be ok");
            let _ = FolderListing::parse(Cursor::new(bytes)).expect_err("should have failed");
        });
    }

    #[fuchsia::test]
    fn build_empty_folder_listing_success() {
        // Empty folder listing example.
        let empty_folder_listing = FolderListing::new_empty();
        let mut buf = Vec::new();
        assert_eq!(empty_folder_listing.mime_type(), "application/xml");
        empty_folder_listing.build(&mut buf).expect("should have succeeded");
        assert_eq!(
            empty_folder_listing,
            FolderListing::parse(Cursor::new(buf)).expect("should be valid xml")
        );
    }

    #[fuchsia::test]
    fn build_simple_folder_listing_success() {
        let folder_listing = FolderListing {
            parent_folder: true,
            files: vec![File {
                data: Some("Jumar Handling Guide".to_string()),
                attributes: HashSet::from([
                    FolderListingAttribute::Name("Jumar.txt".to_string()),
                    FolderListingAttribute::Size("6672".to_string()),
                ]),
            }],
            folders: vec![Folder {
                data: None,
                attributes: HashSet::from([FolderListingAttribute::Name("System".to_string())]),
            }],
        };
        let mut buf = Vec::new();
        folder_listing.build(&mut buf).expect("should have succeeded");
        assert_eq!(
            folder_listing,
            FolderListing::parse(Cursor::new(buf)).expect("should be valid xml")
        );
    }

    #[fuchsia::test]
    fn build_detailed_folder_listing_success() {
        let detailed_folder_listing = FolderListing {
            parent_folder: true,
            files: vec![File {
                data: None,
                attributes: HashSet::from([
                    FolderListingAttribute::Name("Jumar.txt".to_string()),
                    FolderListingAttribute::Created(FormattedDateTimeObj(
                        NaiveDateTime::new(
                            NaiveDate::from_ymd_opt(1997, 12, 09).unwrap(),
                            NaiveTime::from_hms_opt(09, 03, 00).unwrap(),
                        ),
                        FormattedDateTimeObj::ISO_8601_TIME_FORMAT.to_string(),
                    )),
                    FolderListingAttribute::Size("6672".to_string()),
                    FolderListingAttribute::Modified(FormattedDateTimeObj(
                        NaiveDateTime::new(
                            NaiveDate::from_ymd_opt(1997, 12, 22).unwrap(),
                            NaiveTime::from_hms_opt(16, 41, 00).unwrap(),
                        ),
                        FormattedDateTimeObj::ISO_8601_TIME_FORMAT.to_string(),
                    )),
                    FolderListingAttribute::UserPerm(Permission::READ | Permission::WRITE),
                ]),
            }],
            folders: vec![Folder {
                data: None,
                attributes: HashSet::from([
                    FolderListingAttribute::Name("System".to_string()),
                    FolderListingAttribute::Created(FormattedDateTimeObj(
                        NaiveDateTime::new(
                            NaiveDate::from_ymd_opt(1996, 11, 03).unwrap(),
                            NaiveTime::from_hms_opt(14, 15, 00).unwrap(),
                        ),
                        FormattedDateTimeObj::ISO_8601_TIME_FORMAT.to_string(),
                    )),
                ]),
            }],
        };
        let mut buf = Vec::new();
        assert_eq!(detailed_folder_listing.mime_type(), "application/xml");
        detailed_folder_listing.build(&mut buf).expect("should have succeeded");

        assert_eq!(
            detailed_folder_listing,
            FolderListing::parse(Cursor::new(buf)).expect("should be valid xml")
        );
    }
}
