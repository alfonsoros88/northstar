#!/usr/bin/env bash

binary_name=`basename $1`

if [[ $binary_name =~ ^(northstar|tests-[a-z0-9]{16}|test_launcher)$ ]]; then
    sudo -E --preserve-env=PATH $@
else
    eval $@
fi
