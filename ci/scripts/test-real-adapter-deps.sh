#!/usr/bin/env bash
set -euo pipefail

found=0

while IFS= read -r manifest; do
  while IFS= read -r version; do
    found=1
    case "$version" in
      ""|'*'|^*|~*)
        echo "unsupported version requirement in ${manifest}: ${version:-<empty>}" >&2
        exit 1
        ;;
    esac
  done < <(sed -n 's/.*version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$manifest")
done < <(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"manifest_path":"\([^"]*\)".*/\1/p')

if [ "$found" -eq 0 ]; then
  echo "no version requirements found in cargo manifests" >&2
  exit 1
fi

dependency_tree="$(cargo tree --workspace --target all --all-features --prefix none)"

if printf '%s\n' "$dependency_tree" | rg -n '^(numpy|pyo3|pyo3-build-config|pyo3-ffi|pyo3-macros|pyo3-macros-backend) v'; then
  echo "python-related crates are not allowed in v1 required dependency paths" >&2
  exit 1
fi
