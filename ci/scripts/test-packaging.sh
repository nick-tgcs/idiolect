#!/usr/bin/env bash
set -euo pipefail

package_name="idiolect"
package_version="0.1.0"
package_root="target/package/idiolect-deb"
package_file="target/package/${package_name}_${package_version}_amd64.deb"

cargo build --workspace --release
bash ci/scripts/test-fcitx5.sh

rm -rf "${package_root}"
install -d "${package_root}/DEBIAN"
install -d "${package_root}/usr/bin"
install -d "${package_root}/usr/lib/fcitx5"
install -d "${package_root}/usr/lib/systemd/user"
install -d "${package_root}/usr/share/fcitx5/addon"
install -d "${package_root}/usr/share/fcitx5/inputmethod"
install -d "${package_root}/usr/share/metainfo"
install -d "${package_root}/usr/share/icons/hicolor/scalable/apps"
install -d "${package_root}/usr/share/idiolect"
install -d "${package_root}/usr/share/doc/idiolect"

install -m 0644 packaging/debian/DEBIAN/control "${package_root}/DEBIAN/control"
install -m 0755 target/release/idiolect-cli "${package_root}/usr/bin/idiolect"
install -m 0755 target/release/idiolect-cli "${package_root}/usr/bin/idiolect-cli"
install -m 0755 target/release/idiolectd "${package_root}/usr/bin/idiolectd"
install -m 0755 target/release/idiolect-trainerctl "${package_root}/usr/bin/idiolect-train"
install -m 0755 fcitx5/idiolect-fcitx5/build/libidiolect-fcitx5.so "${package_root}/usr/lib/fcitx5/idiolect.so"
install -m 0644 fcitx5/idiolect-fcitx5/data/idiolect-addon.conf "${package_root}/usr/share/fcitx5/addon/idiolect.conf"
install -m 0644 fcitx5/idiolect-fcitx5/data/idiolect.conf "${package_root}/usr/share/fcitx5/inputmethod/idiolect.conf"
install -m 0644 fcitx5/idiolect-fcitx5/data/org.fcitx.Fcitx5.Addon.Idiolect.metainfo.xml "${package_root}/usr/share/metainfo/org.fcitx.Fcitx5.Addon.Idiolect.metainfo.xml"
install -m 0644 packaging/debian/usr/lib/systemd/user/idiolectd.service "${package_root}/usr/lib/systemd/user/idiolectd.service"
install -m 0644 packaging/debian/usr/share/icons/hicolor/scalable/apps/idiolect.svg "${package_root}/usr/share/icons/hicolor/scalable/apps/idiolect.svg"
install -m 0644 packaging/debian/usr/share/idiolect/config.example.toml "${package_root}/usr/share/idiolect/config.example.toml"
install -m 0644 packaging/debian/usr/share/doc/idiolect/README.md "${package_root}/usr/share/doc/idiolect/README.md"

rm -f "${package_file}"
dpkg-deb --build "${package_root}" "${package_file}"
test -f "${package_file}"

contents="$(dpkg-deb --contents "${package_file}")"
required_entries=(
  './usr/bin/idiolect'
  './usr/bin/idiolect-cli'
  './usr/bin/idiolectd'
  './usr/bin/idiolect-train'
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
