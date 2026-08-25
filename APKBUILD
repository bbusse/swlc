# Maintainer: Björn Busse <bj.rn@baerlin.eu>
pkgname=swlc
pkgver=0.1.0
pkgrel=0
pkgdesc="Stupid Wayland client - no external dependencies"
url="https://github.com/bbusse/swlc"
arch="all"
license="BSD-3-Clause"
makedepends="cargo rust"
srcdir="$startdir/.abuild-src"
builddir="$startdir"

build() {
	cd "$builddir"
	cargo build --release --locked
}

package() {
	cd "$builddir"
	install -Dm755 target/release/swlc "$pkgdir"/usr/bin/swlc
}
