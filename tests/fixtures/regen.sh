#!/usr/bin/env bash
# Regenerate tests/fixtures/fx/{crate.vir,crate.impl_names} from tests/fixtures/crate.
# Needs Docker and the verus-oracle:local image (Verus 0.2026.07.18.3a4d30b, see DESIGN.md section 3).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
cp -r "$here/crate/." "$work/"
docker run --rm --user "$(id -u):$(id -g)" -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/target \
  -e RUSTUP_HOME=/opt/rustup -e HOME=/tmp -v "$work:$work" -v "${CARGO_VOL:-verus-rcs-cargo}:/cargo" \
  -v "${TARGET_VOL:-verus-lint-fixture-target}:/target" -w "$work" verus-oracle:local \
  cargo verus build --fwd-verus-args-to roots -- --no-verify --log vir --log impl-names --log-dir "$work/log"
cp "$work/log/crate.vir" "$work/log/crate.impl_names" "$here/fx/"
rm -rf "$work"
