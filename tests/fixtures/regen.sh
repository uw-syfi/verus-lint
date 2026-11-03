#!/usr/bin/env bash
# Regenerate the fixtures of one Verus release:
#   tests/fixtures/fx/VERSION/{crate.vir,crate.impl_names}  from tests/fixtures/crate
#   tests/fixtures/report/VERSION.json                       from tests/fixtures/report/crate
#
# Usage: tests/fixtures/regen.sh VERSION IMAGE VSTD
#   VERSION  the release string `verus --version` prints, for example 0.2026.08.30.b432e82
#   IMAGE    a Docker image with that release's verus, cargo-verus and Rust toolchain on PATH
#   VSTD     the vstd dependency for that release as a TOML value, for example
#            '"=0.0.0-2026-08-30-0159"' (crates.io) or
#            '{ git = "https://github.com/verus-lang/verus.git", rev = "<commit>" }'
# The report crate uses syntax of 0.2026.08.30; set SKIP_REPORT=1 for an older release.
set -euo pipefail
if [ $# -ne 3 ]; then
  sed -n '2,13p' "$0" >&2
  exit 2
fi
version="$1" image="$2" vstd="$3"
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cache="${CARGO_VOL:-verus-lint-fixture-cargo}"
run() {
  docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp -e CARGO_HOME=/cargo \
    -v "$cache:/cargo" -v "$work:$work" -w "$1" "$image" "${@:2}"
}
if ! docker volume inspect "$cache" >/dev/null 2>&1; then
  docker volume create "$cache" >/dev/null
  docker run --rm -v "$cache:/cargo" "$image" chown "$(id -u):$(id -g)" /cargo
fi
got="$(run "$work" verus --version --output-json | python3 -c 'import json,sys; print(json.load(sys.stdin)["verus"]["version"])')"
if [ "$got" != "$version" ]; then
  echo "image $image has Verus $got, not $version" >&2
  exit 1
fi
setvstd() { sed -i "s|^vstd = .*|vstd = $vstd|" "$1/Cargo.toml"; }

mkdir -p "$work/fx"
cp -r "$here/crate/." "$work/fx/"
setvstd "$work/fx"
run "$work/fx" cargo verus build --fwd-verus-args-to roots -- --no-verify --log vir --log impl-names --log-dir "$work/fx/log"
mkdir -p "$here/fx/$version"
cp "$work/fx/log/crate.vir" "$work/fx/log/crate.impl_names" "$here/fx/$version/"

if [ "${SKIP_REPORT:-0}" != 1 ]; then
  mkdir -p "$work/report"
  cp -r "$here/report/crate/." "$work/report/"
  setvstd "$work/report"
  # The crate has one failing function on purpose, so cargo exits non-zero.
  run "$work/report" cargo verus build --fwd-verus-args-to roots -- --time-expanded --output-json \
    >"$work/report.out" || true
  python3 - "$work/report.out" "$here/report/$version.json" <<'PY'
import json, sys
text = open(sys.argv[1]).read()
obj = json.JSONDecoder().raw_decode(text, text.index("{"))[0]
obj.pop("func-details", None)  # large and unused
open(sys.argv[2], "w").write(json.dumps(obj, indent=1) + "\n")
PY
fi
echo "fixtures of Verus $version written"
