// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/config.h"

#include <lib/syslog/cpp/macros.h>

#include <sstream>

#include "src/connectivity/network/mdns/service/common/mdns_names.h"
#include "src/connectivity/network/mdns/service/common/type_converters.h"
#include "src/connectivity/network/mdns/service/encoding/dns_formatting.h"
#include "src/lib/json_parser/rapidjson_validation.h"

namespace mdns {
namespace {

const char kSchema[] = R"({
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "perform_host_name_probe": {
      "type": "boolean"
    },
    "publications": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "properties": {
          "service": {
            "type": "string",
            "minLength": 8,
            "maxLength": 22
          },
          "instance": {
            "type": "string",
            "minLength": 1,
            "maxLength": 63
          },
          "port": {
            "type": "integer",
            "minimum": 1,
            "maximum": 65535
          },
          "text": {
            "type": "array",
            "items": {
              "type": "string",
              "maxLength": 255
            }
          },
          "perform_probe": {
            "type": "boolean"
          },
          "media": {
            "type": "string",
            "enum": ["wired", "wireless", "both"]
          },
          "include_serial": {
            "type": "boolean"
          }
        },
        "required": ["service","port"]
      }
    },
    "alt_services": {
      "type": "array",
      "items": {
        "type": "string",
        "minLength": 8,
        "maxLength": 22
      }
    }
  }
})";

const char kPortKey[] = "port";
const char kPerformHostNameProbeKey[] = "perform_host_name_probe";
const char kPublicationsKey[] = "publications";
const char kIncludeSerialKey[] = "include_serial";
const char kServiceKey[] = "service";
const char kInstanceKey[] = "instance";
const char kTextKey[] = "text";
const char kPerformProbeKey[] = "perform_probe";
const char kMediaKey[] = "media";
const char kMediaValueWired[] = "wired";
const char kMediaValueWireless[] = "wireless";
const char kMediaValueBoth[] = "both";

// TODO(https://fxbug.dev/42065146): Remove this when alt_services is no longer needed.
const char kAltServicesKey[] = "alt_services";

}  // namespace

//  static
const char Config::kConfigDir[] = "/config/data";
const char Config::kBootConfigDir[] = "/boot/data/mdns";

void Config::ReadConfigFiles(const DnsName& local_host_name, const std::string& serial,
                             const std::string& boot_config_dir, const std::string& config_dir) {
  FX_DCHECK(MdnsNames::IsValidHostName(local_host_name));

  auto schema_result = json_parser::InitSchema(kSchema);
  FX_CHECK(schema_result.is_ok()) << schema_result.error_value().ToString();
  auto schema = std::move(schema_result.value());

  // Order of parsing matters here. The boot configuration is read first, and has precedence over
  // the default configuration which is part of the image assembly.
  // |ParseFromDirectory| treats a non-existent directory the same as an empty directory, which
  // is what we want.

  // Read boot configuration first. This enables the emulator to pass along additional txt records
  // withe the _fuchsia record.
  parser_.ParseFromDirectory(
      boot_config_dir, [this, &schema, &local_host_name, &serial](rapidjson::Document document) {
        auto validation_result = json_parser::ValidateSchema(document, schema);
        if (validation_result.is_error()) {
          parser_.ReportError(validation_result.error_value());
          return;
        }

        IntegrateDocument(document, local_host_name, serial);
      });

  // Read the default configuration.
  parser_.ParseFromDirectory(
      config_dir, [this, &schema, &local_host_name, &serial](rapidjson::Document document) {
        auto validation_result = json_parser::ValidateSchema(document, schema);
        if (validation_result.is_error()) {
          parser_.ReportError(validation_result.error_value());
          return;
        }

        IntegrateDocument(document, local_host_name, serial);
      });
}

void Config::IntegrateDocument(const rapidjson::Document& document, const DnsName& local_host_name,
                               const std::string& serial) {
  FX_DCHECK(document.IsObject());

  if (document.HasMember(kPerformHostNameProbeKey)) {
    FX_DCHECK(document[kPerformHostNameProbeKey].IsBool());
    SetPerformHostNameProbe(document[kPerformHostNameProbeKey].GetBool());
    if (parser_.HasError()) {
      return;
    }
  }

  if (document.HasMember(kPublicationsKey)) {
    FX_DCHECK(document[kPublicationsKey].IsArray());
    for (const auto& item : document[kPublicationsKey].GetArray()) {
      IntegratePublication(item, local_host_name, serial);
      if (parser_.HasError()) {
        return;
      }
    }
  }

  if (document.HasMember(kAltServicesKey)) {
    FX_DCHECK(document[kAltServicesKey].IsArray());
    for (const auto& item : document[kAltServicesKey].GetArray()) {
      FX_DCHECK(item.IsString());
      DnsName item_as_name = DnsName(item.GetString());
      if (!MdnsNames::IsValidServiceName(item_as_name)) {
        parser_.ReportError((std::stringstream() << kAltServicesKey << " item value "
                                                 << item_as_name << " is not a valid service type.")
                                .str());
        return;
      }

      alt_services_.push_back(item_as_name);
    }
  }
}

void Config::IntegratePublication(const rapidjson::Value& value, const DnsName& local_host_name,
                                  const std::string& serial) {
  FX_DCHECK(value.IsObject());
  FX_DCHECK(value.HasMember(kServiceKey));
  FX_DCHECK(value[kServiceKey].IsString());
  FX_DCHECK(value.HasMember(kPortKey));
  FX_DCHECK(value[kPortKey].IsUint());
  const unsigned int port = value[kPortKey].GetUint();
  FX_DCHECK(port >= 1);
  FX_DCHECK(port <= std::numeric_limits<uint16_t>::max()) << port << " doesn't fit in a uint16";

  auto service = DnsName(value[kServiceKey].GetString());
  if (!MdnsNames::IsValidServiceName(service)) {
    parser_.ReportError((std::stringstream()
                         << kServiceKey << " value " << service << " is not a valid service name.")
                            .str());
    return;
  }

  DnsLabel instance;
  if (value.HasMember(kInstanceKey)) {
    instance = DnsLabel(value[kInstanceKey].GetString());
    if (!MdnsNames::IsValidInstanceName(instance)) {
      parser_.ReportError((std::stringstream() << kInstanceKey << " value " << instance
                                               << " is not a valid instance name.")
                              .str());
      return;
    }
  } else {
    instance = local_host_name.first_label();
    if (!MdnsNames::IsValidInstanceName(instance)) {
      parser_.ReportError((std::stringstream()
                           << "Publication of service " << service
                           << " specifies that the host name should be "
                              "used as the instance name, but "
                           << local_host_name << "is not a valid instance name.")
                              .str());
      return;
    }
  }

  FX_LOGS(INFO) << "Publishing service " << service << " instance " << instance
                << " per configuration.";

  std::vector<std::string> text;
  if (value.HasMember(kTextKey)) {
    FX_DCHECK(value[kTextKey].IsArray());
    for (const auto& item : value[kTextKey].GetArray()) {
      FX_DCHECK(item.IsString());
      if (!MdnsNames::IsValidTextString(item.GetString())) {
        parser_.ReportError((std::stringstream() << kTextKey << " item value " << item.GetString()
                                                 << " is not a valid text string.")
                                .str());
        return;
      }
      text.push_back(item.GetString());
    }
  }

  bool perform_probe = true;
  if (value.HasMember(kPerformProbeKey)) {
    FX_DCHECK(value[kPerformProbeKey].IsBool());
    perform_probe = value[kPerformProbeKey].GetBool();
  }

  bool include_serial = false;
  if (value.HasMember(kIncludeSerialKey)) {
    FX_DCHECK(value[kIncludeSerialKey].IsBool());
    include_serial = value[kIncludeSerialKey].GetBool();
  }

  if (include_serial && serial != "") {
    text.push_back("serial=" + serial);
  } else if (include_serial) {
    FX_LOGS(WARNING) << "Configuration indicated to include the serial number in mDNS "
                     << "messages, but no serial number was provided. Continuing without.";
  } else {
    FX_LOGS(INFO) << "Not including device serial number " << serial
                  << " in mDNS messages, per configuration.";
  }

  Media media = Media::kBoth;
  if (value.HasMember(kMediaKey)) {
    FX_DCHECK(value[kMediaKey].IsString());
    std::string media_string = value[kMediaKey].GetString();
    if (media_string == kMediaValueWired) {
      media = Media::kWired;
    } else if (media_string == kMediaValueWireless) {
      media = Media::kWireless;
    } else {
      FX_DCHECK(media_string == kMediaValueBoth);
    }
  }

  for (const auto& existing : publications_) {
    if (existing.service_ == service && existing.instance_ == instance) {
      if (existing.media_ == media || existing.media_ == Media::kBoth || media == Media::kBoth) {
        auto media_to_string = [](Media m) {
          switch (m) {
            case Media::kBoth:
              return "both";
            case Media::kWired:
              return "wired";
            case Media::kWireless:
              return "wireless";
          }
        };
        FX_LOGS(WARNING) << "Duplicate publication detected in configuration for service "
                         << service << " and instance " << instance << " on media "
                         << media_to_string(media) << " (conflicts with existing on "
                         << media_to_string(existing.media_) << ")";
      }
    }
  }

  publications_.emplace_back(Publication{
      .service_ = service,
      .instance_ = instance,
      .publication_ =
          Mdns::Publication::Create(inet::IpPort::From_uint16_t(static_cast<uint16_t>(port)),
                                    fidl::To<std::vector<std::vector<uint8_t>>>(text)),
      .perform_probe_ = perform_probe,
      .media_ = media});
}

void Config::SetPerformHostNameProbe(bool perform_host_name_probe) {
  if (perform_host_name_probe_.has_value() &&
      perform_host_name_probe_.value() != perform_host_name_probe) {
    parser_.ReportError(
        (std::stringstream() << "Conflicting " << kPerformHostNameProbeKey << " value.").str());
    return;
  }

  perform_host_name_probe_ = perform_host_name_probe;
}

}  // namespace mdns
