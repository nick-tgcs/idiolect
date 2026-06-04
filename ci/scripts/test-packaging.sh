#!/usr/bin/env bash
set -euo pipefail

package_root="target/package/idiolect-deb"
package_file="target/package/idiolect_0.1.0_amd64.deb"

cargo build --workspace --release
bash ci/scripts/test-fcitx5.sh

install -d "${package_root}/DEBIAN"
install -d "${package_root}/usr/bin"
install -d "${package_root}/usr/lib/fcitx5"
install -d "${package_root}/usr/share/doc/idiolect"

install -m 0644 packaging/debian/DEBIAN/control "${package_root}/DEBIAN/control"
install -m 0755 target/release/idiolect-cli "${package_root}/usr/bin/idiolect-cli"
install -m 0755 target/release/idiolectd "${package_root}/usr/bin/idiolectd"
install -m 0755 fcitx5/idiolect-fcitx5/build/libidiolect-fcitx5.so "${package_root}/usr/lib/fcitx5/libidiolect-fcitx5.so"
install -m 0644 packaging/debian/usr/share/doc/idiolect/README.md "${package_root}/usr/share/doc/idiolect/README.md"

rm -f "${package_file}"
dpkg-deb --build "${package_root}" "${package_file}"
test -f "${package_file}"

contents="$(dpkg-deb --contents "${package_file}")"
printf '%s\n' "${contents}" | grep -F './usr/bin/idiolect-cli' >/dev/null
printf '%s\n' "${contents}" | grep -F './usr/bin/idiolectd' >/dev/null
printf '%s\n' "${contents}" | grep -F './usr/lib/fcitx5/libidiolect-fcitx5.so' >/dev/null
