#!/usr/bin/env bash
set -euo pipefail

package_name="idiolect"
package_file="target/package/${package_name}_0.1.0_amd64.deb"
lifecycle_root="target/package/lifecycle-root"
upgrade_root="target/package/lifecycle-upgrade-root"
user_data_rel="home/test/.local/share/idiolect/id.sqlite"

bash ci/scripts/test-package-smoke.sh

if [[ ! -f "${package_file}" ]]; then
  echo "Expected package not found: ${package_file}" >&2
  exit 1
fi

rm -rf "${lifecycle_root}" "${upgrade_root}"
install -d "${lifecycle_root}" "${upgrade_root}"
dpkg-deb --extract "${package_file}" "${lifecycle_root}"
dpkg-deb --extract "${package_file}" "${upgrade_root}"

require_file() {
  local root="$1"
  local path="$2"
  if [[ ! -f "${root}${path}" ]]; then
    echo "Missing package lifecycle payload: ${path}" >&2
    exit 1
  fi
}

require_executable() {
  local root="$1"
  local path="$2"
  require_file "${root}" "${path}"
  if [[ ! -x "${root}${path}" ]]; then
    echo "Package payload is not executable: ${path}" >&2
    exit 1
  fi
}

require_executable "${lifecycle_root}" /usr/bin/idiolect
require_executable "${lifecycle_root}" /usr/bin/idiolect-cli
require_executable "${lifecycle_root}" /usr/bin/idiolectd
require_executable "${lifecycle_root}" /usr/bin/idiolect-train
require_file "${lifecycle_root}" /usr/lib/fcitx5/idiolect.so
require_file "${lifecycle_root}" /usr/share/fcitx5/addon/idiolect.conf
require_file "${lifecycle_root}" /usr/share/fcitx5/inputmethod/idiolect.conf
require_file "${lifecycle_root}" /usr/share/metainfo/org.fcitx.Fcitx5.Addon.Idiolect.metainfo.xml
require_file "${lifecycle_root}" /usr/share/icons/hicolor/scalable/apps/idiolect.svg
require_file "${lifecycle_root}" /usr/share/idiolect/config.example.toml
require_file "${lifecycle_root}" /usr/share/doc/idiolect/README.md
require_file "${lifecycle_root}" /usr/lib/systemd/user/idiolectd.service

service="${lifecycle_root}/usr/lib/systemd/user/idiolectd.service"
grep -F 'Description=Idiolect local speech-to-text daemon' "${service}" >/dev/null
grep -F 'After=graphical-session.target' "${service}" >/dev/null
# The daemon exits code 2 without --config; the unit must pass the user config path.
grep -F 'ExecStart=/usr/bin/idiolectd run --config %h/.config/idiolect/config.toml' "${service}" >/dev/null
grep -F 'Restart=on-failure' "${service}" >/dev/null
# Bad/missing config (exit 2) must fail ONCE, never crash-loop; genuine crashes
# get a bounded number of restarts before systemd gives up.
grep -F 'RestartPreventExitStatus=2' "${service}" >/dev/null
grep -F 'StartLimitIntervalSec=300' "${service}" >/dev/null
grep -F 'StartLimitBurst=5' "${service}" >/dev/null
# User-session daemon with a tray: start with the graphical session, not at any login.
grep -F 'WantedBy=graphical-session.target' "${service}" >/dev/null

addon="${lifecycle_root}/usr/share/fcitx5/addon/idiolect.conf"
input_method="${lifecycle_root}/usr/share/fcitx5/inputmethod/idiolect.conf"
grep -F 'Name=idiolect' "${addon}" >/dev/null
grep -F 'Name=Idiolect' "${input_method}" >/dev/null
grep -F 'Addon=idiolect' "${input_method}" >/dev/null
grep -F 'Icon=idiolect' "${input_method}" >/dev/null

wants_dir="${lifecycle_root}/home/test/.config/systemd/user/graphical-session.target.wants"
install -d "${wants_dir}"
ln -s /usr/lib/systemd/user/idiolectd.service "${wants_dir}/idiolectd.service"
test -L "${wants_dir}/idiolectd.service"
rm "${wants_dir}/idiolectd.service"
test ! -e "${wants_dir}/idiolectd.service"

install -d "$(dirname "${lifecycle_root}/${user_data_rel}")"
printf 'user-owned sqlite placeholder\n' > "${lifecycle_root}/${user_data_rel}"

# Upgrade simulation: extracting a same-version package over an existing staged root must not touch user data.
dpkg-deb --extract "${package_file}" "${lifecycle_root}"
test -f "${lifecycle_root}/${user_data_rel}"
grep -F 'user-owned sqlite placeholder' "${lifecycle_root}/${user_data_rel}" >/dev/null

# Uninstall simulation: package-owned files are removed, user data is preserved. This is not clean-VM evidence.
rm -f \
  "${lifecycle_root}/usr/bin/idiolect" \
  "${lifecycle_root}/usr/bin/idiolect-cli" \
  "${lifecycle_root}/usr/bin/idiolectd" \
  "${lifecycle_root}/usr/bin/idiolect-train" \
  "${lifecycle_root}/usr/lib/fcitx5/idiolect.so" \
  "${lifecycle_root}/usr/lib/systemd/user/idiolectd.service" \
  "${lifecycle_root}/usr/share/fcitx5/addon/idiolect.conf" \
  "${lifecycle_root}/usr/share/fcitx5/inputmethod/idiolect.conf" \
  "${lifecycle_root}/usr/share/metainfo/org.fcitx.Fcitx5.Addon.Idiolect.metainfo.xml" \
  "${lifecycle_root}/usr/share/icons/hicolor/scalable/apps/idiolect.svg" \
  "${lifecycle_root}/usr/share/idiolect/config.example.toml" \
  "${lifecycle_root}/usr/share/doc/idiolect/README.md"

test ! -e "${lifecycle_root}/usr/bin/idiolect"
test ! -e "${lifecycle_root}/usr/bin/idiolectd"
test -f "${lifecycle_root}/${user_data_rel}"

# Clean-VM install/uninstall remains a separate release evidence requirement.
