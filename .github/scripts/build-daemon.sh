#!/usr/bin/env bash
# Builds the fan daemon, thermite-fand, as a static musl binary, and puts it where
# src-tauri/tauri.release.conf.json expects it. That config runs this script before each
# release build, and puts the binary at /usr/libexec/thermite/thermite-fand in each package.
#
# A static binary needs no glibc and no loader, so the copy in the packages runs on every
# distribution, whatever its glibc. Needs the musl target, for example:
#   rustup target add x86_64-unknown-linux-musl
set -euo pipefail

cd "$(dirname "$0")/../../src-tauri"
host=$(rustc -vV | sed -n 's/^host: //p')
# x86_64-unknown-linux-gnu -> x86_64-unknown-linux-musl
musl=${host/-gnu/-musl}
cargo build --release --locked -p thermite-core --bin thermite-fand --target "$musl"

target_dir=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
mkdir -p binaries
cp "$target_dir/$musl/release/thermite-fand" binaries/thermite-fand
