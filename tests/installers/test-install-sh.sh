#!/usr/bin/env bash
# Local test of install.sh against a simulated release directory (file:// base URL).
# Usage: tests/installers/test-install-sh.sh   (needs bash, curl, tar, sha256sum/shasum)
set -u
root="$(cd "$(dirname "$0")/../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
pass=0; fail=0
ok() { pass=$((pass + 1)); echo "ok   - $1"; }
ko() { fail=$((fail + 1)); echo "FAIL - $1"; }

sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

# Build a fake release for version 9.9.9 for the four Unix targets.
rel="$work/release"; mkdir -p "$rel"
for t in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin; do
  stem="kepler-9.9.9-$t"
  mkdir -p "$work/pkg/$stem"
  printf '#!/bin/sh\necho "kepler 9.9.9 (fake)"\n' > "$work/pkg/$stem/kepler"
  chmod +x "$work/pkg/$stem/kepler"
  echo MIT > "$work/pkg/$stem/LICENSE"
  tar -C "$work/pkg" -czf "$rel/$stem.tar.gz" "$stem"
done
( cd "$rel" && for f in kepler-*; do echo "$(sha "$f")  $f"; done > SHA256SUMS )
url="file://$rel"
# curl on Git Bash/Windows wants a drive-style path
case "$(uname -s)" in MINGW* | MSYS*) url="file:///$(cd "$rel" && pwd -W)" ;; esac

run() { # run NAME ARGS... (uses $inst); env KEPLER_* passed by caller
  KEPLER_BASE_URL="$url" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Linux KEPLER_ARCH=x86_64 sh "$root/install.sh" "$@"
}

# 1. good checksum installs and runs
inst="$work/i1"; out="$(run 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ -x "$inst/kepler" ] && echo "$out" | grep -q "SHA-256 verified" && echo "$out" | grep -q "kepler 9.9.9"; then ok "valid checksum installs"; else ko "valid checksum installs: $out"; fi
echo "$out" | grep -q "not in your PATH" && ok "warns when dir not in PATH" || ko "PATH warning"

# 2. explicit version
inst="$work/i2"; out="$(run --version 9.9.9 2>&1)"; rc=$?
[ $rc -eq 0 ] && [ -x "$inst/kepler" ] && ok "--version 9.9.9" || ko "--version: $out"

# 3. wrong checksum aborts and installs nothing
bad="$work/bad"; cp -r "$rel" "$bad"
sed -i 's/^[0-9a-f]\{64\}/0000000000000000000000000000000000000000000000000000000000000000/' "$bad/SHA256SUMS"
inst="$work/i3"
out="$(KEPLER_BASE_URL="file://$bad" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Linux KEPLER_ARCH=x86_64 sh "$root/install.sh" 2>&1)"; rc=$?
case "$(uname -s)" in MINGW* | MSYS*) out="$(KEPLER_BASE_URL="file:///$(cd "$bad" && pwd -W)" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Linux KEPLER_ARCH=x86_64 sh "$root/install.sh" 2>&1)"; rc=$? ;; esac
if [ $rc -ne 0 ] && [ ! -e "$inst/kepler" ] && echo "$out" | grep -q "mismatch"; then ok "wrong checksum fails, nothing installed"; else ko "wrong checksum: rc=$rc $out"; fi

# 4. tampered archive (checksum file honest) fails
tam="$work/tam"; cp -r "$rel" "$tam"; echo junk >> "$tam/kepler-9.9.9-x86_64-unknown-linux-gnu.tar.gz"
inst="$work/i4"; tamurl="file://$tam"; case "$(uname -s)" in MINGW* | MSYS*) tamurl="file:///$(cd "$tam" && pwd -W)" ;; esac
out="$(KEPLER_BASE_URL="$tamurl" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Linux KEPLER_ARCH=x86_64 sh "$root/install.sh" 2>&1)"; rc=$?
[ $rc -ne 0 ] && [ ! -e "$inst/kepler" ] && ok "tampered archive fails" || ko "tampered archive: $out"

# 5. unsupported arch / OS
inst="$work/i5"
out="$(KEPLER_BASE_URL="$url" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Linux KEPLER_ARCH=riscv64 sh "$root/install.sh" 2>&1)"; rc=$?
[ $rc -ne 0 ] && echo "$out" | grep -q "unsupported architecture" && ok "unsupported arch fails" || ko "unsupported arch: $out"
out="$(KEPLER_BASE_URL="$url" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=FreeBSD KEPLER_ARCH=x86_64 sh "$root/install.sh" 2>&1)"; rc=$?
[ $rc -ne 0 ] && echo "$out" | grep -q "unsupported operating system" && ok "unsupported OS fails" || ko "unsupported OS: $out"

# 6. non-HTTPS base URL refused
out="$(KEPLER_BASE_URL="http://example.invalid/x" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Linux KEPLER_ARCH=x86_64 sh "$root/install.sh" 2>&1)"; rc=$?
[ $rc -ne 0 ] && echo "$out" | grep -q "non-HTTPS" && ok "http:// refused" || ko "http refused: $out"

# 7. macOS arm64 picks the right archive
inst="$work/i7"
out="$(KEPLER_BASE_URL="$url" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Darwin KEPLER_ARCH=arm64 sh "$root/install.sh" 2>&1)"
echo "$out" | grep -q "aarch64-apple-darwin" && [ -x "$inst/kepler" ] && ok "macOS arm64 target" || ko "macOS arm64: $out"

# 8. help and uninstall
sh "$root/install.sh" --help | grep -q "Usage:" && ok "--help" || ko "--help"
inst="$work/i1"; run --uninstall >/dev/null 2>&1; [ ! -e "$inst/kepler" ] && ok "--uninstall removes binary" || ko "--uninstall"

echo "passed=$pass failed=$fail"
[ "$fail" -eq 0 ]
