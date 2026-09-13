#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --release
/usr/bin/python3 -m py_compile ./*.py ./src/native/recording/check.py
/usr/bin/python3 -m unittest discover -p 'test_*.py'
