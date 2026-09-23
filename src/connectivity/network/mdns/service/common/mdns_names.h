// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_COMMON_MDNS_NAMES_H_
#define SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_COMMON_MDNS_NAMES_H_

#include <string>

#include "src/connectivity/network/mdns/service/encoding/dns_message.h"

namespace mdns {

struct MdnsNames {
  // Name used to query for any service on the subnet.
  static const DnsName kAnyServiceFullName;

  // Constructs a local host name from a simple host name. For example, produces
  // "host.local." from "host".
  static DnsName HostFullName(const DnsName& host_name);

  // Constructs a simple host name from a local host name. For example, produces
  // "host" from "host.local.".
  static DnsName HostNameFromFullName(const DnsName& host_full_name);

  // Constructs a local service name from a simple service name. For example,
  // produces "_foo._tcp.local." from "_foo._tcp.".
  static DnsName ServiceFullName(const DnsName& service_name);

  // Constructs a local service name from a simple service name and subtype.
  // For example, produces "_bar._sub_.foo._tcp.local." from "_foo._tcp." and
  // subtype "_bar".
  static DnsName ServiceSubtypeFullName(const DnsName& service_name, const DnsLabel& subtype);

  // Constructs a local service instance name from a simple instance name and
  // a simple service name. For example, produces "myfoo._foo._tcp.local." from
  // "myfoo" and "_foo._tcp.". The simple instance name does not include the trailing ".",
  // and the simple service name must end in ".".
  static DnsName InstanceFullName(const DnsLabel& instance_name, const DnsName& service_name);

  // Parses an instance full name to extract the simple instance name and the simple service
  // name.
  static bool SplitInstanceFullName(const DnsName& instance_full_name, DnsLabel* instance_name_out,
                                    DnsName* service_name_out);

  // Determines if |name| is a local service name matching |service_name| or
  // a subtype of |service_name|. If |name| does specify a subtype, the
  // subtype is returned via |subtype_out|. Otherwise |*subtype_out| is
  // cleared.
  static bool MatchServiceName(const DnsName& name, const DnsName& service_name,
                               DnsLabel* subtype_out);

  // Determines if |host_name| is a valid host name.
  static bool IsValidHostName(const DnsName& host_name);

  // Determines if |service_name| is a valid simple service name.
  static bool IsValidServiceName(const DnsName& service_name);

  // Determines if |instance_name| is a valid simple instance name.
  static bool IsValidInstanceName(const DnsLabel& instance_name);

  // Determines if |subtype_name| is a valid simple subtype name.
  static bool IsValidSubtypeName(const DnsLabel& subtype_name);

  // Determines if |text_string| is a valid text string.
  static bool IsValidTextString(const std::string& text_string);

  // Determines if |text_string| is a valid text string.
  static bool IsValidTextString(const std::vector<uint8_t>& text_string);

  // Returns the alternate host name for |host_name|. For example, if |host_name| is
  // "fuchsia-1234-5678-9abc", this method returns "123456789ABC". If |host_name| isn't
  // the expected size (22 characters), this method returns the |host_name| argument.
  // TODO(https://fxbug.dev/42065146): Remove this when alt_services is no longer needed.
  static DnsName AltHostName(const DnsName& host_name);
};

}  // namespace mdns

#endif  // SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_COMMON_MDNS_NAMES_H_
