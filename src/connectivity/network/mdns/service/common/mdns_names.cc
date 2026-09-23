// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/common/mdns_names.h"

#include <lib/stdcompat/string_view.h>
#include <lib/syslog/cpp/macros.h>

#include <algorithm>

namespace mdns {

namespace {

static const DnsLabel kLocalDomainName = "local";
static const std::string kSubtypeSeparatorLabel = "_sub";
static const std::string kLabelSeparator = ".";
static const std::string kTcpSuffix = "_tcp";
static const std::string kUdpSuffix = "_udp";

static constexpr size_t kMaxHostNameLength = 254 - 6;  // 6 for "local." domain.
static constexpr size_t kMaxTextStringLength = 255;
static constexpr size_t kMaxLabelLength = 63;

}  // namespace

// static
const DnsName MdnsNames::kAnyServiceFullName = DnsName("_services._dns-sd._udp.local.");

// static
DnsName MdnsNames::HostFullName(const DnsName& host_name) {
  FX_DCHECK(IsValidHostName(host_name));

  return host_name.append(kLocalDomainName);
}

// static
DnsName MdnsNames::HostNameFromFullName(const DnsName& host_full_name) {
  // Copy all but the last label.
  DnsName result;
  std::string_view prev_label_view;
  for (auto label_view = host_full_name.first_label_view(); !label_view.empty();
       label_view = host_full_name.next_label_view(label_view)) {
    if (!prev_label_view.empty()) {
      result.push_back(DnsLabel(prev_label_view));
    }

    prev_label_view = label_view;
  }

  FX_DCHECK(!result.empty()) << "`host_full_name` must be a valid host full name";

  return result;
}

// static
DnsName MdnsNames::ServiceFullName(const DnsName& service_name) {
  FX_DCHECK(IsValidServiceName(service_name));

  return service_name.append(kLocalDomainName);
}

// static
DnsName MdnsNames::ServiceSubtypeFullName(const DnsName& service_name, const DnsLabel& subtype) {
  FX_DCHECK(IsValidServiceName(service_name));
  FX_DCHECK(IsValidSubtypeName(subtype));

  return DnsName(subtype)
      .append(kSubtypeSeparatorLabel)
      .append(service_name)
      .append(kLocalDomainName);
}

// static
DnsName MdnsNames::InstanceFullName(const DnsLabel& instance_name, const DnsName& service_name) {
  FX_DCHECK(IsValidInstanceName(instance_name));
  FX_DCHECK(IsValidServiceName(service_name));

  return DnsName(instance_name, instance_name.size()).append(service_name).append(kLocalDomainName);
}

// static
bool MdnsNames::SplitInstanceFullName(const DnsName& instance_full_name,
                                      DnsLabel* instance_name_out, DnsName* service_name_out) {
  FX_DCHECK(instance_name_out);
  FX_DCHECK(service_name_out);

  // instance_name service_type service_protocol kLocalDomainName

  auto instance_name_view = instance_full_name.first_label_view();
  auto service_type_view = instance_full_name.next_label_view(instance_name_view);
  auto service_protocol_view = instance_full_name.next_label_view(service_type_view);
  auto local_domain_view = instance_full_name.next_label_view(service_protocol_view);
  if (local_domain_view != kLocalDomainName ||
      !instance_full_name.next_label_view(local_domain_view).empty()) {
    return false;
  }

  DnsLabel instance_name = DnsLabel(instance_name_view);
  DnsName service_name((DnsLabel(service_type_view)));
  service_name.push_back(DnsLabel(service_protocol_view));
  if (!IsValidInstanceName(instance_name) || !IsValidServiceName(service_name)) {
    return false;
  }

  *instance_name_out = std::move(instance_name);
  *service_name_out = service_name;

  return true;
}

// static
bool MdnsNames::MatchServiceName(const DnsName& name, const DnsName& service_name,
                                 DnsLabel* subtype_out) {
  FX_DCHECK(IsValidServiceName(service_name));
  FX_DCHECK(subtype_out);

  auto expected_service_type = service_name.first_label_view();
  auto expected_service_protocol = service_name.next_label_view(expected_service_type);

  // [ subtype kSubtypeSeparatorLabel ] service_type service_protocol kLocalDomainName

  auto first_label_view = name.first_label_view();
  if (first_label_view.empty()) {
    return false;
  }

  auto second_label_view = name.next_label_view(first_label_view);
  if (second_label_view.empty()) {
    return false;
  }

  if (second_label_view == kSubtypeSeparatorLabel) {
    // subtype kSubtypeSeparatorLabel service_type service_protocol kLocalDomainName
    DnsLabel subtype(first_label_view);
    if (!IsValidSubtypeName(subtype)) {
      return false;
    }

    auto service_type_view = name.next_label_view(second_label_view);
    auto service_protocol_view = name.next_label_view(service_type_view);
    if (service_type_view != expected_service_type ||
        service_protocol_view != expected_service_protocol) {
      return false;
    }

    auto local_domain_view = name.next_label_view(service_protocol_view);
    if (local_domain_view != kLocalDomainName || !name.next_label_view(local_domain_view).empty()) {
      return false;
    }

    *subtype_out = subtype;
  } else {
    // service_type service_protocol kLocalDomainName
    if (first_label_view != expected_service_type ||
        second_label_view != expected_service_protocol) {
      return false;
    }

    auto local_domain_view = name.next_label_view(second_label_view);
    if (local_domain_view != kLocalDomainName || !name.next_label_view(local_domain_view).empty()) {
      return false;
    }

    *subtype_out = DnsLabel();
  }

  return true;
}

// static
bool MdnsNames::IsValidHostName(const DnsName& host_name) {
  // A host name has one or more labels. A complete host name with separators must be at most
  // 247 characters long (254 minus 6 to accommodate a "local." suffix).
  return !host_name.empty() && host_name.length() <= kMaxHostNameLength;
}

// static
bool MdnsNames::IsValidServiceName(const DnsName& service_name) {
  // A service name is two labels, both terminated with '.'. The first label
  // must be [1..16] characters, and the first character must be '_'. The
  // second label must be "_tcp" or "_udp".

  auto service_name_label_view = service_name.first_label_view();
  if (service_name_label_view.empty()) {
    return false;
  }

  auto protocol_label_view = service_name.next_label_view(service_name_label_view);
  if (protocol_label_view.empty()) {
    return false;
  }

  if (!service_name.next_label_view(protocol_label_view).empty()) {
    return false;
  }

  return service_name_label_view.length() >= 1 && service_name_label_view.length() <= 16 &&
         service_name_label_view[0] == '_' &&
         (protocol_label_view == kTcpSuffix || protocol_label_view == kUdpSuffix);
}

// static
bool MdnsNames::IsValidInstanceName(const DnsLabel& instance_name) {
  // Instance names consist of a single label.
  return instance_name.length() > 0 && instance_name.length() <= kMaxLabelLength;
}

// static
bool MdnsNames::IsValidSubtypeName(const DnsLabel& subtype_name) {
  // Subtype names consist of a single label.
  return subtype_name.length() > 0 && subtype_name.length() <= kMaxLabelLength &&
         !cpp23::contains(subtype_name, kLabelSeparator);
}

// static
bool MdnsNames::IsValidTextString(const std::string& text_string) {
  // Text strings must be at most 255 characters long.
  return text_string.length() <= kMaxTextStringLength;
}

// static
bool MdnsNames::IsValidTextString(const std::vector<uint8_t>& text_string) {
  // Text strings must be at most 255 characters long.
  return text_string.size() <= kMaxTextStringLength;
}

// static
DnsName MdnsNames::AltHostName(const DnsName& host_name) {
  static constexpr size_t kExpectedUnmodifiedHostNameSize = 22;
  static constexpr size_t kBlock0Pos = 8;
  static constexpr size_t kBlock1Pos = 13;
  static constexpr size_t kBlock2Pos = 18;
  static constexpr size_t kBlockSize = 4;

  // "fuchsia-1234-5678-9abc" becomes "12345678ABC".

  // `host_name` should contain exactly one label.
  auto label = host_name.first_label_view();
  if (label.empty() || !host_name.next_label_view(label).empty()) {
    return host_name;
  }

  // Make sure the label is in the right format.
  if (label.size() != kExpectedUnmodifiedHostNameSize || !label.starts_with("fuchsia-") ||
      label[kBlock1Pos - 1] != '-' || label[kBlock2Pos - 1] != '-') {
    return host_name;
  }

  DnsLabel result;
  result.reserve(kBlockSize * 3);
  result.append(label.substr(kBlock0Pos, kBlockSize));
  result.append(label.substr(kBlock1Pos, kBlockSize));
  result.append(label.substr(kBlock2Pos, kBlockSize));
  std::transform(result.begin(), result.end(), result.begin(), ::toupper);

  return DnsName(result);
}

}  // namespace mdns
