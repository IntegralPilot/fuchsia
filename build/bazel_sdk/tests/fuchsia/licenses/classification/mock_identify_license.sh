#!/bin/bash
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This script mocks https://github.com/google/licenseclassifier/tree/main/tools/identify_license
# and verifies it is invoked as expected by the 'fuchsia_licenses_classification' rule
# when applied to 'input.spdx.json'.
# It then outputs a mock json output that is processed by the rule.

echoerr() { echo "ERROR in mock_identify_license.sh: $@" 1>&2; }

verify_arg () {
    local arg_title=$1
    local actual_value=$2
    local expected_value=$3
    if [ "$actual_value" != "$expected_value" ]; then
        echoerr "Expected $arg_title to be '$expected_value' but got '$actual_value'"
        exit -1
    fi
}

read_arg_value () {
    local arg_title=$1
    local actual_value=$2
    local prefix_value=$3

    if [[ "$actual_value" =~ $prefix_value* ]];
    then
        local prefix_len=${#prefix_value}
        local value="${actual_value:$prefix_len}"
        echo $value
    else
        echoerr "Expected $arg_title to begin with '$prefix_value' but got '$actual_value'"
        exit -1
    fi
}

verify_arg "1st argument" "$1" "-headers"
output_json_path=$(read_arg_value "2st argument" "$2" "-json=")
verify_arg "3rd argument" "$3" "-include_text=true"
verify_arg "4th argument" "$4" "-ignorable=true"
verify_arg "5th argument" "$5" "-copyright=true"
verify_arg "6th argument" "$6" "input_licenses"

verify_file_exists () {
    local file_path=$1
    if [ ! -f "$file_path" ]; then
        echoerr "Expected $file_path doesn't exist"
        exit -1
    fi
}

# There are 5 unique license texts in input.spdx.json
verify_file_exists input_licenses/LicenseRef-A-known.txt
verify_file_exists input_licenses/LicenseRef-B-dedupped.txt
verify_file_exists input_licenses/LicenseRef-C-ignorable.txt
verify_file_exists input_licenses/LicenseRef-C-copyright.txt
verify_file_exists input_licenses/LicenseRef-D-multiple-conditions.txt
verify_file_exists input_licenses/LicenseRef-E-multiple-conditions-enough-overriden.txt
verify_file_exists input_licenses/LicenseRef-F-multiple-conditions-not-enough-overriden.txt

echo "Writing output to $output_json_path"

write () {
    echo "$1" >> $output_json_path
}

# Output mock classification.

write '['
write '    {'
write '        "Filepath": "input_licenses/LicenseRef-A-known.txt",'
write '        "Classifications": ['
write '            {'
write '                "Name": "License Class 1",'
write '                "Confidence": 1,'
write '                "StartLine": 1,'
write '                "EndLine": 2,'
write '                "Condition": "allowed-condition"'
write '            },'
write '            {'
write '                "Name": "License Class 2",'
write '                "Confidence": 0.5,'
write '                "StartLine": 2,'
write '                "EndLine": 3,'
write '                "Condition": "disallowed-condition"'
write '            }'
write '        ]'
write '    },'
write '    {'
write '        "Filepath": "input_licenses/LicenseRef-B-dedupped.txt",'
write '        "Classifications": ['
write '            {'
write '                "Name": "License Class 3",'
write '                "Confidence": 1,'
write '                "StartLine": 1,'
write '                "EndLine": 2,'
write '                "Condition": "disallowed-condition"'
write '            },'
write '            {'
write '                "Name": "Unclassified",'
write '                "Confidence": 1,'
write '                "StartLine": 3,'
write '                "EndLine": 3,'
write '                "Condition": ""'
write '            }'
write '        ]'
write '    },'
write '    {'
write '        "Filepath": "input_licenses/LicenseRef-C-ignorable.txt",'
write '        "Classifications": ['
write '            {'
write '                "Name": "Ignorable",'
write '                "Confidence": 1,'
write '                "StartLine": 1,'
write '                "EndLine": 2'
write '            }'
write '        ]'
write '    },'
write '    {'
write '        "Filepath": "input_licenses/LicenseRef-C-copyright.txt",'
write '        "Classifications": ['
write '            {'
write '                "Name": "Copyright",'
write '                "Confidence": 1,'
write '                "StartLine": 1,'
write '                "EndLine": 2'
write '            }'
write '        ]'
write '    },'
write '    {'
write '        "Filepath": "input_licenses/LicenseRef-D-multiple-conditions.txt",'
write '        "Classifications": ['
write '            {'
write '                "Name": "License Class 4",'
write '                "Confidence": 1,'
write '                "StartLine": 1,'
write '                "EndLine": 2,'
write '                "Condition": "allowed-condition disallowed-condition"'
write '            }'
write '        ]'
write '    },'
write '    {'
write '        "Filepath": "input_licenses/LicenseRef-E-multiple-conditions-enough-overriden.txt",'
write '        "Classifications": ['
write '            {'
write '                "Name": "License Class 5",'
write '                "Confidence": 1,'
write '                "StartLine": 1,'
write '                "EndLine": 2,'
write '                "Condition": "allowed-condition disallowed-condition"'
write '            }'
write '        ]'
write '    },'
write '    {'
write '        "Filepath": "input_licenses/LicenseRef-F-multiple-conditions-not-enough-overriden.txt",'
write '        "Classifications": ['
write '            {'
write '                "Name": "License Class 6",'
write '                "Confidence": 1,'
write '                "StartLine": 1,'
write '                "EndLine": 2,'
write '                "Condition": "disallowed-condition-1 disallowed-condition-2"'
write '            }'
write '        ]'
write '    }'
write ']'