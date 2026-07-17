#!/usr/bin/env bash
set -euo pipefail

package_name="idiolect"
package_file="target/package/${package_name}_0.1.0_amd64.deb"
smoke_root="target/package/smoke-root"
workspace_version_toml="Cargo.toml"

get_workspace_package_version() {
  local file="$1"
  local section="$2"
  awk '
    BEGIN { in_section = 0 }
    $0 == "[" section "]" { in_section = 1; next }
    in_section && /^\[/ { in_section = 0 }
    in_section && $0 ~ /^version[[:space:]]*=/ {
      gsub(/^[[:space:]]*version[[:space:]]*=[[:space:]]*"/, "", $0)
      gsub(/"[[:space:]]*$/, "", $0)
      print $0
      exit
    }
  ' section="$2" "$1"
}

workspace_version="$(get_workspace_package_version "${workspace_version_toml}" "workspace.package")"
if [[ -z "${workspace_version}" ]]; then
  workspace_version="$(get_workspace_package_version "${workspace_version_toml}" "package")"
fi
if [[ -z "${workspace_version}" ]]; then
  workspace_version="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p; t; d' crates/idiolect-cli/Cargo.toml | head -n1)"
fi

if [[ -z "${workspace_version}" ]]; then
  echo "Unable to determine package version from workspace or crate metadata" >&2
  exit 1
fi

bash ci/scripts/test-packaging.sh

if [[ ! -f "${package_file}" ]]; then
  echo "Expected package not found: ${package_file}" >&2
  exit 1
fi

package_version="$(dpkg-deb --field "${package_file}" Version)"
if [[ "${package_version}" != "${workspace_version}" ]]; then
  echo "Debian control Version (${package_version}) does not match workspace version (${workspace_version})" >&2
  exit 1
fi

rm -rf "${smoke_root}"
install -d "${smoke_root}"
dpkg-deb --extract "${package_file}" "${smoke_root}"

"${smoke_root}/usr/bin/idiolect" doctor --json
"${smoke_root}/usr/bin/idiolect-cli" doctor --json
"${smoke_root}/usr/bin/idiolectd" --version --json
trainer_output="$(${smoke_root}/usr/bin/idiolect-train)"
if [[ "${trainer_output}" != "idiolect-trainerctl" ]]; then
  echo "Unexpected idiolect-train output: ${trainer_output}" >&2
  exit 1
fi

contents="$(dpkg-deb --contents "${package_file}")"
required_entries=(
  './usr/bin/idiolect'
  './usr/bin/idiolect-cli'
  './usr/bin/idiolectd'
  './usr/bin/idiolect-train'
  './usr/bin/idiolect-crash-notify'
  './usr/lib/fcitx5/idiolect.so'
  './usr/lib/systemd/user/idiolectd.service'
  './usr/share/fcitx5/addon/idiolect.conf'
  './usr/share/fcitx5/inputmethod/idiolect.conf'
  './usr/share/metainfo/org.fcitx.Fcitx5.Addon.Idiolect.metainfo.xml'
  './usr/share/icons/hicolor/scalable/apps/idiolect.svg'
  './usr/share/idiolect/config.example.toml'
  './usr/share/doc/idiolect/README.md'
)
for entry in "${required_entries[@]}"; do
  printf '%s\n' "${contents}" | grep -F "${entry}" >/dev/null
done
