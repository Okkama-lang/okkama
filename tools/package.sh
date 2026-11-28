#!/usr/bin/env bash
#
# Релизный архив Okkama для одной платформы.
#
#     tools/package.sh                                  # платформа этой машины
#     tools/package.sh --target x86_64-apple-darwin     # кросс-сборка cargo
#     tools/package.sh --no-build --out dist --version 0.1.0-nightly.20261029
#     tools/package.sh --man-pages                      # добавить страницы std(3ok)
#     tools/package.sh --installer                      # ещё .pkg (macOS) или .deb и .rpm (Linux)
#
# Версия по умолчанию — из okkama-rs/Cargo.toml. Результат —
# dist/okkama-<версия>-<платформа>.tar.gz (+ .pkg или .deb и .rpm, + -symbols.tar.gz
# с бинарниками до strip, #1109) и .sha256 к
# каждому. `.pkg` ставит в /usr/local, `.deb` и `.rpm` — в /usr; `.rpm`
# собирается, если есть `rpmbuild` (#313). Архив
# раскладывается как установка, поэтому распакованный `bin/okkama` сразу
# находит std и нативную библиотеку относительно себя:
#
#     okkama-<версия>-<платформа>/
#         bin/okkama bin/sparrow bin/blade
#         lib/okkama/std/
#         install.sh README.md LICENSE

set -euo pipefail
# macOS: без AppleDouble-файлов `._*` и расширенных атрибутов в tar и pkg.
export COPYFILE_DISABLE=1

ROOT="$(cd -P "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET=""
OUT="$ROOT/dist"
BUILD=1
INSTALLER=0
MAN_PAGES=0
VERSION=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target) TARGET="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --no-build) BUILD=0; shift ;;
        --installer) INSTALLER=1; shift ;;
        --man-pages) MAN_PAGES=1; shift ;;
        --version) VERSION="$2"; shift 2 ;;
        -h|--help) sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "package.sh: unknown option $1" >&2; exit 2 ;;
    esac
done

# Платформа архива: из тройки cargo или с этой машины.
triple="${TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
case "$triple" in
    aarch64-apple-darwin) PLATFORM=macos-aarch64 ;;
    x86_64-apple-darwin) PLATFORM=macos-x86_64 ;;
    x86_64-unknown-linux-gnu) PLATFORM=linux-x86_64 ;;
    aarch64-unknown-linux-gnu) PLATFORM=linux-aarch64 ;;
    *) echo "package.sh: unsupported target $triple" >&2; exit 1 ;;
esac

if [[ -n "$TARGET" ]]; then
    RELEASE="${CARGO_TARGET_DIR:-$ROOT/okkama-rs/target}/$TARGET/release"
    CARGO_TARGET=(--target "$TARGET")
else
    RELEASE="${CARGO_TARGET_DIR:-$ROOT/okkama-rs/target}/release"
    CARGO_TARGET=()
fi

if [[ $BUILD -eq 1 ]]; then
    # Release builds compile the development instrumentation out (okkama-devtrace).
    (cd "$ROOT/okkama-rs" && RUSTFLAGS="${RUSTFLAGS:-} --cfg okkama_release -C force-frame-pointers=yes" cargo build --release --locked \
        "${CARGO_TARGET[@]}" -p okkama-cli -p sparrow-cli -p blade-cli)
fi

VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/okkama-rs/Cargo.toml" | head -1)}"
NAME="okkama-$VERSION-$PLATFORM"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
DEST="$STAGE/$NAME"

mkdir -p "$DEST/bin" "$DEST/lib/okkama"
# #1109: the binaries ship without their symbol tables; the unstripped ones go into
# dist/okkama-<version>-<platform>-symbols.tar.gz, which resolves a crash address
# (`atos -o okkama -l <load address> <address>` on macOS, `addr2line -f -e okkama <address>` on Linux).
SYMBOLS="$STAGE/$NAME-symbols"
mkdir -p "$SYMBOLS"
for tool in okkama sparrow blade; do
    install -m 755 "$RELEASE/$tool" "$DEST/bin/$tool"
    cp "$RELEASE/$tool" "$SYMBOLS/$tool"
    case "$PLATFORM" in
        # Stripping invalidates the linker's ad-hoc signature, which Apple silicon requires.
        macos-*) strip "$DEST/bin/$tool" && codesign --force --sign - "$DEST/bin/$tool" ;;
        linux-*) strip --strip-all "$DEST/bin/$tool" ;;
    esac
done
cp -R "$ROOT/std" "$DEST/lib/okkama/std"
if [[ $MAN_PAGES -eq 1 ]]; then
    # A cross package needs a matching host compiler; never execute target code
    # implicitly through a shell or assume emulation is installed.
    GENERATOR="${OKKAMA_DOC_GENERATOR:-$RELEASE/okkama}"
    bash "$ROOT/tools/install-man.sh" "$GENERATOR" "$ROOT/std" "$DEST"
fi
printf '%s\n' "$VERSION" > "$DEST/lib/okkama/VERSION"
# The compiler the binaries were built with (#41).
(cd "$ROOT/okkama-rs" && rustc -Vv) > "$DEST/lib/okkama/TOOLCHAIN"
cp "$ROOT/README.md" "$ROOT/LICENSE" "$DEST/"

cat > "$DEST/install.sh" <<'EOF'
#!/usr/bin/env bash
# Копирует Okkama из этого архива в префикс (по умолчанию ~/.local):
#     ./install.sh                  # ~/.local/bin, ~/.local/lib/okkama
#     ./install.sh --prefix /opt/okkama
set -euo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="$HOME/.local"
case "${1:-}" in
    --prefix) PREFIX="${2:?--prefix needs a directory}" ;;
    --prefix=*) PREFIX="${1#--prefix=}" ;;
    "") ;;
    *) echo "install.sh: unknown option $1" >&2; exit 2 ;;
esac
PREFIX="${PREFIX%/}"
mkdir -p "$PREFIX/bin" "$PREFIX/lib"
if [[ -f "$PREFIX/lib/okkama/man-pages.txt" ]]; then
    while IFS= read -r page; do rm -f "$PREFIX/share/man/man3/$page"; done < "$PREFIX/lib/okkama/man-pages.txt"
fi
rm -rf "$PREFIX/lib/okkama"
cp -R "$HERE/lib/okkama" "$PREFIX/lib/okkama"
for tool in okkama sparrow blade; do
    # Через временный файл: замена работающего бинарника ломает подпись на macOS.
    cp "$HERE/bin/$tool" "$PREFIX/bin/.$tool.tmp"
    mv -f "$PREFIX/bin/.$tool.tmp" "$PREFIX/bin/$tool"
done
if [[ -d "$HERE/share/man/man3" ]]; then
    mkdir -p "$PREFIX/share/man/man3"
    while IFS= read -r page; do
        install -m 644 "$HERE/share/man/man3/$page" "$PREFIX/share/man/man3/$page"
    done < "$HERE/lib/okkama/man-pages.txt"
fi
cat > "$PREFIX/lib/okkama/uninstall.sh" <<UNINSTALL
#!/usr/bin/env bash
rm -f "$PREFIX/bin/okkama" "$PREFIX/bin/sparrow" "$PREFIX/bin/blade"
if [[ -f "$PREFIX/lib/okkama/man-pages.txt" ]]; then
    while IFS= read -r page; do rm -f "$PREFIX/share/man/man3/\$page"; done < "$PREFIX/lib/okkama/man-pages.txt"
fi
rm -rf "$PREFIX/lib/okkama"
echo "Okkama removed from $PREFIX"
UNINSTALL
chmod 755 "$PREFIX/lib/okkama/uninstall.sh"
echo "Okkama $(cat "$HERE/lib/okkama/VERSION") installed into $PREFIX"
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "add it to PATH: export PATH=\"$PREFIX/bin:\$PATH\"" ;;
esac
EOF
chmod 755 "$DEST/install.sh"

if command -v xattr >/dev/null 2>&1; then
    xattr -cr "$STAGE"
fi

mkdir -p "$OUT"
ARCHIVE="$OUT/$NAME.tar.gz"
tar -C "$STAGE" -czf "$ARCHIVE" "$NAME"
echo "$ARCHIVE"
tar -C "$STAGE" -czf "$OUT/$NAME-symbols.tar.gz" "$NAME-symbols"
echo "$OUT/$NAME-symbols.tar.gz"

# Установщик системы: раскладка установки, но в системном префиксе.
# `okkama` находит std и native относительно себя (<prefix>/lib/okkama).
if [[ $INSTALLER -eq 1 ]]; then
    case "$PLATFORM" in
        macos-*)
            ROOTFS="$STAGE/pkgroot"
            mkdir -p "$ROOTFS/usr/local"
            cp -R "$DEST/bin" "$DEST/lib" "$ROOTFS/usr/local/"
            if [[ -d "$DEST/share" ]]; then cp -R "$DEST/share" "$ROOTFS/usr/local/"; fi
            # Версия pkg — только цифры и точки.
            PKG_VERSION="$(printf '%s' "$VERSION" | sed 's/[^0-9.].*//')"
            pkgbuild --quiet --root "$ROOTFS" --identifier dev.okkama.okkama \
                --version "${PKG_VERSION:-0}" --install-location / "$OUT/$NAME.pkg"
            echo "$OUT/$NAME.pkg"
            ;;
        linux-*)
            case "$PLATFORM" in
                linux-x86_64) DEB_ARCH=amd64 ;;
                linux-aarch64) DEB_ARCH=arm64 ;;
            esac
            ROOTFS="$STAGE/debroot"
            mkdir -p "$ROOTFS/usr" "$ROOTFS/DEBIAN"
            cp -R "$DEST/bin" "$DEST/lib" "$ROOTFS/usr/"
            if [[ -d "$DEST/share" ]]; then cp -R "$DEST/share" "$ROOTFS/usr/"; fi
            mkdir -p "$ROOTFS/usr/share/doc/okkama"
            cp "$ROOT/LICENSE" "$ROOTFS/usr/share/doc/okkama/copyright"
            # Debian: `-` в версии — разделитель ревизии, пре-релиз — через `~`.
            DEB_VERSION="$(printf '%s' "$VERSION" | sed 's/-/~/')"
            cat > "$ROOTFS/DEBIAN/control" <<CONTROL
Package: okkama
Version: $DEB_VERSION
Architecture: $DEB_ARCH
Maintainer: Alexander Ponomarev <shootnix@users.noreply.github.com>
Section: devel
Priority: optional
Depends: libc6 (>= 2.17)
Suggests: libsqlite3-0 (>= 3.41)
Homepage: https://github.com/Okkama-lang/okkama
Description: Okkama programming language
 Compiler and runner for Okkama (Sparrow VM with JIT), the standard library, the language server, the formatter
 and the blade project tool.
CONTROL
            # xz, not the zstd Ubuntu's dpkg-deb picks by default: dpkg before
            # Debian 12 cannot read zstd members (#485).
            dpkg-deb -Zxz --root-owner-group --build "$ROOTFS" "$OUT/$NAME.deb" >/dev/null
            echo "$OUT/$NAME.deb"
            # RPM from the same tree (#313). Release binaries are built against
            # glibc 2.17 (manylinux2014, #485): every glibc distribution since
            # 2014 installs them — CentOS 7, Amazon Linux 2, RHEL 8 and newer.
            if command -v rpmbuild >/dev/null 2>&1; then
                case "$PLATFORM" in
                    linux-x86_64) RPM_ARCH=x86_64 ;;
                    linux-aarch64) RPM_ARCH=aarch64 ;;
                esac
                # RPM: no `-` in a version; `~` sorts a pre-release first.
                RPM_VERSION="$(printf '%s' "$VERSION" | sed 's/-/~/')"
                MAN_RPM_FILES=""
                if [[ $MAN_PAGES -eq 1 ]]; then MAN_RPM_FILES='/usr/share/man/man3/*.3ok'; fi
                TOPDIR="$STAGE/rpmbuild"
                mkdir -p "$TOPDIR/SPECS"
                cat > "$TOPDIR/SPECS/okkama.spec" <<SPEC
Name: okkama
Version: $RPM_VERSION
Release: 1
Summary: Okkama programming language
License: GPL-3.0-only
URL: https://github.com/Okkama-lang/okkama
BuildArch: $RPM_ARCH
Requires: glibc >= 2.17
Recommends: sqlite-libs
AutoReqProv: no
%global debug_package %{nil}
%global __os_install_post %{nil}
%global _build_id_links none

%description
Compiler and runner for Okkama (Sparrow VM with JIT), the standard library, the language server, the formatter
and the blade project tool.

%install
mkdir -p %{buildroot}
cp -a $ROOTFS/usr %{buildroot}/

%files
/usr/bin/okkama
/usr/bin/sparrow
/usr/bin/blade
/usr/lib/okkama
$MAN_RPM_FILES
%doc /usr/share/doc/okkama/copyright
SPEC
                rpmbuild --quiet --define "_topdir $TOPDIR" -bb "$TOPDIR/SPECS/okkama.spec"
                cp "$TOPDIR/RPMS/$RPM_ARCH/okkama-$RPM_VERSION-1.$RPM_ARCH.rpm" "$OUT/$NAME.rpm"
                echo "$OUT/$NAME.rpm"
            fi
            ;;
    esac
fi

(cd "$OUT" && for file in "$NAME".tar.gz "$NAME"-symbols.tar.gz "$NAME".pkg "$NAME".deb "$NAME".rpm; do
    [[ -f "$file" ]] && shasum -a 256 "$file" > "$file.sha256"
done; true)
