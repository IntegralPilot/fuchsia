// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/encoding/dns_message.h"

namespace mdns {

DnsName::DnsName(std::string dotted_string) : dotted_string_(std::move(dotted_string)) {
  if (dotted_string_.empty()) {
    return;
  }

  if (!dotted_string_.ends_with(".")) {
    dotted_string_ += ".";
  }

  first_label_size_ = dotted_string_.find('.');
}

DnsName::DnsName(std::string dotted_string, size_t first_label_size)
    : dotted_string_(std::move(dotted_string)), first_label_size_(first_label_size) {
  if (!dotted_string_.ends_with(".")) {
    dotted_string_ += ".";
  }
  FX_DCHECK(first_label_size < dotted_string_.size());
}

DnsName DnsName::append(const DnsLabel& label) const {
  DnsName result(*this);
  result.push_back(label);
  return result;
}

DnsName DnsName::append(const DnsName& name) const {
  DnsName result(*this);
  for (auto label = name.first_label_view(); !label.empty(); label = name.next_label_view(label)) {
    result.push_back(DnsLabel(label));
  }
  return result;
}

void DnsName::push_back(const DnsLabel& label) {
  if (dotted_string_.empty()) {
    dotted_string_ = label + ".";
    first_label_size_ = label.size();
    return;
  }

  dotted_string_ += label + ".";
}

std::string_view DnsName::next_label_view(std::string_view current_label_view) const {
  if (current_label_view.empty()) {
    return std::string_view();
  }

  size_t pos = current_label_view.data() - dotted_string_.data();
  FX_DCHECK(pos < dotted_string_.size());

  pos += current_label_view.size();
  if (pos >= dotted_string_.size()) {
    return std::string_view();
  }
  pos += 1;
  if (pos == dotted_string_.size()) {
    return std::string_view();
  }

  auto data = dotted_string_.data() + pos;

  auto end_pos = dotted_string_.find('.', pos);
  FX_DCHECK(end_pos != std::string::npos);

  return std::string_view(data, end_pos - pos);
}

void DnsHeader::SetResponse(bool value) {
  if (value) {
    flags_ |= kQueryResponseMask;
  } else {
    flags_ &= ~kQueryResponseMask;
  }
}

void DnsHeader::SetOpCode(DnsOpCode op_code) {
  flags_ &= ~kOpCodeMask;
  flags_ |= static_cast<uint16_t>(op_code) << kOpCodeShift;
}

void DnsHeader::SetAuthoritativeAnswer(bool value) {
  if (value) {
    flags_ |= kAuthoritativeAnswerMask;
  } else {
    flags_ &= ~kAuthoritativeAnswerMask;
  }
}

void DnsHeader::SetTruncated(bool value) {
  if (value) {
    flags_ |= kTruncationMask;
  } else {
    flags_ &= ~kTruncationMask;
  }
}

void DnsHeader::SetRecursionDesired(bool value) {
  if (value) {
    flags_ |= kRecursionDesiredMask;
  } else {
    flags_ &= ~kRecursionDesiredMask;
  }
}

void DnsHeader::SetRecursionAvailable(bool value) {
  if (value) {
    flags_ |= kRecursionAvailableMask;
  } else {
    flags_ &= ~kRecursionAvailableMask;
  }
}

void DnsHeader::SetResponseCode(DnsResponseCode response_code) {
  flags_ &= ~kResponseCodeMask;
  flags_ |= static_cast<uint16_t>(response_code);
}

DnsQuestion::DnsQuestion() = default;

DnsQuestion::DnsQuestion(DnsName name, DnsType type) : name_(std::move(name)), type_(type) {}

DnsQuestion::DnsQuestion(DnsName name, DnsType type, bool request_unicast_response)
    : name_(std::move(name)), type_(type), unicast_response_(request_unicast_response) {}

DnsResource::DnsResource() {}

DnsResource::DnsResource(DnsName name, DnsType type) : name_(std::move(name)), type_(type) {
  switch (type_) {
    case DnsType::kA:
      new (&a_) DnsResourceDataA();
      time_to_live_ = kShortTimeToLive;
      // cache_flush_ false, because multiple A resources can apply to the same name.
      break;
    case DnsType::kNs:
      new (&ns_) DnsResourceDataNs();
      time_to_live_ = kLongTimeToLive;
      cache_flush_ = true;
      break;
    case DnsType::kCName:
      new (&cname_) DnsResourceDataCName();
      time_to_live_ = kLongTimeToLive;
      cache_flush_ = true;
      break;
    case DnsType::kPtr:
      new (&ptr_) DnsResourceDataPtr();
      time_to_live_ = kLongTimeToLive;
      // cache_flush_ false, because multiple PTR resources can apply to the same name.
      break;
    case DnsType::kTxt:
      new (&txt_) DnsResourceDataTxt();
      time_to_live_ = kLongTimeToLive;
      cache_flush_ = true;
      break;
    case DnsType::kAaaa:
      new (&aaaa_) DnsResourceDataAaaa();
      time_to_live_ = kShortTimeToLive;
      // cache_flush_ false, because multiple AAAA resources can apply to the same name.
      break;
    case DnsType::kSrv:
      new (&srv_) DnsResourceDataSrv();
      time_to_live_ = kShortTimeToLive;
      cache_flush_ = true;
      break;
    case DnsType::kOpt:
      new (&opt_) DnsResourceDataOpt();
      time_to_live_ = kShortTimeToLive;
      cache_flush_ = true;
      break;
    case DnsType::kNSec:
      new (&nsec_) DnsResourceDataNSec();
      time_to_live_ = kLongTimeToLive;
      cache_flush_ = true;
      break;
    default:
      break;
  }
}

DnsResource::DnsResource(DnsName name, inet::IpAddress address, bool cache_flush)
    : name_(std::move(name)), cache_flush_(cache_flush) {
  if (address.is_v4()) {
    type_ = DnsType::kA;
    new (&a_) DnsResourceDataA();
    time_to_live_ = kShortTimeToLive;
    a_.address_.address_ = address;
  } else {
    type_ = DnsType::kAaaa;
    new (&aaaa_) DnsResourceDataAaaa();
    time_to_live_ = kShortTimeToLive;
    aaaa_.address_.address_ = address;
  }
}

DnsResource::DnsResource(const DnsResource& other) {
  name_ = other.name_;
  type_ = other.type_;
  class_ = other.class_;
  cache_flush_ = other.cache_flush_;
  time_to_live_ = other.time_to_live_;

  switch (type_) {
    case DnsType::kA:
      new (&a_) DnsResourceDataA();
      a_ = other.a_;
      break;
    case DnsType::kNs:
      new (&ns_) DnsResourceDataNs();
      ns_ = other.ns_;
      break;
    case DnsType::kCName:
      new (&cname_) DnsResourceDataCName();
      cname_ = other.cname_;
      break;
    case DnsType::kPtr:
      new (&ptr_) DnsResourceDataPtr();
      ptr_ = other.ptr_;
      break;
    case DnsType::kTxt:
      new (&txt_) DnsResourceDataTxt();
      txt_ = other.txt_;
      break;
    case DnsType::kAaaa:
      new (&aaaa_) DnsResourceDataAaaa();
      aaaa_ = other.aaaa_;
      break;
    case DnsType::kSrv:
      new (&srv_) DnsResourceDataSrv();
      srv_ = other.srv_;
      break;
    case DnsType::kOpt:
      new (&opt_) DnsResourceDataOpt();
      opt_ = other.opt_;
      break;
    case DnsType::kNSec:
      new (&nsec_) DnsResourceDataNSec();
      nsec_ = other.nsec_;
      break;
    default:
      break;
  }
}

DnsResource::~DnsResource() {
  switch (type_) {
    case DnsType::kA:
      a_.~DnsResourceDataA();
      break;
    case DnsType::kNs:
      ns_.~DnsResourceDataNs();
      break;
    case DnsType::kCName:
      cname_.~DnsResourceDataCName();
      break;
    case DnsType::kPtr:
      ptr_.~DnsResourceDataPtr();
      break;
    case DnsType::kTxt:
      txt_.~DnsResourceDataTxt();
      break;
    case DnsType::kAaaa:
      aaaa_.~DnsResourceDataAaaa();
      break;
    case DnsType::kSrv:
      srv_.~DnsResourceDataSrv();
      break;
    case DnsType::kOpt:
      opt_.~DnsResourceDataOpt();
      break;
    case DnsType::kNSec:
      nsec_.~DnsResourceDataNSec();
      break;
    default:
      break;
  }
}

}  // namespace mdns
