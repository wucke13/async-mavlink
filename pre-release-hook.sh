#!/usr/bin/env bash

cd "$(dirname "$0")"

set -e
cargo readme --output README.md
git add README.md
git commit --amend --no-edit
