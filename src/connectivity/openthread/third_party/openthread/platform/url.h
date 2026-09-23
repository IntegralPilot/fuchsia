// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_URL_H_
#define SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_URL_H_

#include <stdint.h>

#include <openthread/error.h>

/**
 * @struct otUrl
 *
 * Represents a URL.
 *
 */
struct otUrl {
  const char *mProtocol;  ///< The URL protocol.
  const char *mPath;      ///< The path.
  const char *mQuery;     ///< The start of the URL query string.
  const char *mEnd;       ///< The end of the URL buffer.
};

namespace ot {
namespace Url {

/**
 * Implements the URL processing.
 *
 */
class Url : public otUrl {
 public:
  Url(void);

  /**
   * Initializes the URL.
   *
   * @param[in]   aUrl    A buffer stores the null-terminated URL string.
   *
   * @retval  OT_ERROR_NONE   Successfully parsed the URL.
   * @retval  OT_ERROR_PARSE  Failed to parse the string as a URL.
   *
   */
  otError Init(char *aUrl);

  /**
   * Gets the path in URL.
   *
   * @returns The path in URL.
   *
   */
  const char *GetPath(void) const { return mPath; }

  /**
   * Gets the value of parameter @p aName.
   *
   * @param[in] aName       The parameter name.
   * @param[in] aLastValue  The last iterated parameter value, nullptr for the first value.
   *
   * @returns The parameter value.
   *
   */
  const char *GetValue(const char *aName, const char *aLastValue = nullptr) const;

  /**
   * Returns the URL protocol.
   *
   * @returns The URL protocol.
   *
   */
  const char *GetProtocol(void) const { return mProtocol; }

  /**
   * Indicates whether or not the url contains the parameter.
   *
   * @param[in]  aName  A pointer to the parameter name.
   *
   * @retval TRUE   The url contains the parameter.
   * @retval FALSE  The url doesn't support the parameter.
   *
   */
  bool HasParam(const char *aName) const { return (GetValue(aName) != nullptr); }

  /**
   * Parses a `uint32_t` parameter value.
   *
   * The parameter value in string is parsed as decimal or hex format (if contains `0x` or `0X`
   * prefix).
   *
   * @param[in]  aName    A pointer to the parameter name.
   * @param[out] aValue   A reference to an `uint32_t` variable to output the parameter value.
   *                      The original value of @p aValue won't change if failed to get the value.
   *
   * @retval OT_ERROR_NONE          The parameter value was parsed successfully.
   * @retval OT_ERROR_NOT_FOUND     The parameter name was not found.
   * @retval OT_ERROR_INVALID_ARGS  The parameter value was not contain valid number (e.g., value
   * out of range).
   *
   */
  otError ParseUint32(const char *aName, uint32_t &aValue) const;

  /**
   * Parses a `uint16_t` parameter value.
   *
   * The parameter value in string is parsed as decimal or hex format (if contains `0x` or `0X`
   * prefix).
   *
   * @param[in]  aName    A pointer to the parameter name.
   * @param[out] aValue   A reference to an `uint16_t` variable to output the parameter value.
   *                      The original value of @p aValue won't change if failed to get the value.
   *
   * @retval OT_ERROR_NONE          The parameter value was parsed successfully.
   * @retval OT_ERROR_NOT_FOUND     The parameter name was not found.
   * @retval OT_ERROR_INVALID_ARGS  The parameter value was not contain valid number (e.g., value
   * out of range).
   *
   */
  otError ParseUint16(const char *aName, uint16_t &aValue) const;

  /**
   * Parses a `uint8_t` parameter value.
   *
   * The parameter value in string is parsed as decimal or hex format (if contains `0x` or `0X`
   * prefix).
   *
   * @param[in]  aName    A pointer to the parameter name.
   * @param[out] aValue   A reference to an `uint16_t` variable to output the parameter value.
   *                      The original value of @p aValue won't change if failed to get the value.
   *
   * @retval OT_ERROR_NONE          The parameter value was parsed successfully.
   * @retval OT_ERROR_NOT_FOUND     The parameter name was not found.
   * @retval OT_ERROR_INVALID_ARGS  The parameter value was not contain valid number (e.g., value
   * out of range).
   *
   */
  otError ParseUint8(const char *aName, uint8_t &aValue) const;

  /**
   * Parses a `int32_t` parameter value.
   *
   * The parameter value in string is parsed as decimal or hex format (if contains `0x` or `0X`
   * prefix). The string can start with `+`/`-` sign.
   *
   * @param[in]  aName    A pointer to the parameter name.
   * @param[out] aValue   A reference to an `int32_t` variable to output the parameter value.
   *                      The original value of @p aValue won't change if failed to get the value.
   *
   * @retval OT_ERROR_NONE          The parameter value was parsed successfully.
   * @retval OT_ERROR_NOT_FOUND     The parameter name was not found.
   * @retval OT_ERROR_INVALID_ARGS  The parameter value was not contain valid number (e.g., value
   * out of range).
   *
   */
  otError ParseInt32(const char *aName, int32_t &aValue) const;

  /**
   * Parses a `int16_t` parameter value.
   *
   * The parameter value in string is parsed as decimal or hex format (if contains `0x` or `0X`
   * prefix). The string can start with `+`/`-` sign.
   *
   * @param[in]  aName    A pointer to the parameter name.
   * @param[out] aValue   A reference to an `int16_t` variable to output the parameter value.
   *                      The original value of @p aValue won't change if failed to get the value.
   *
   * @retval OT_ERROR_NONE          The parameter value was parsed successfully.
   * @retval OT_ERROR_NOT_FOUND     The parameter name was not found.
   * @retval OT_ERROR_INVALID_ARGS  The parameter value was not contain valid number (e.g., value
   * out of range).
   *
   */
  otError ParseInt16(const char *aName, int16_t &aValue) const;

  /**
   * Parses a `int8_t` parameter value.
   *
   * The parameter value in string is parsed as decimal or hex format (if contains `0x` or `0X`
   * prefix). The string can start with `+`/`-` sign.
   *
   * @param[in]  aName    A pointer to the parameter name.
   * @param[out] aValue   A reference to an `int8_t` variable to output the parameter value.
   *                      The original value of @p aValue won't change if failed to get the value.
   *
   * @retval OT_ERROR_NONE          The parameter value was parsed successfully.
   * @retval OT_ERROR_NOT_FOUND     The parameter name was not found.
   * @retval OT_ERROR_INVALID_ARGS  The parameter value was not contain valid number (e.g., value
   * out of range).
   *
   */
  otError ParseInt8(const char *aName, int8_t &aValue) const;
};

}  // namespace Url
}  // namespace ot

#endif  // SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_URL_H_
