#!/usr/bin/env bash
# shellcheck disable=SC2015  # "A && B || ko" is the intended pass/fail idiom here
# Local test of install.sh against a simulated release directory (file:// base URL).
# Usage: tests/installers/test-install-sh.sh   (needs bash, curl, tar, sha256sum/shasum)
set -u
root="$(cd "$(dirname "$0")/../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"; printf "#!/bin/sh
exit 1
" > "$work/bin/gh"; chmod +x "$work/bin/gh"; PATH="$work/bin:$PATH"
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


# 9. token mode: assets via a simulated GitHub API (file:// API base; no KEPLER_BASE_URL)
fileurl() { case "$(uname -s)" in MINGW* | MSYS*) echo "file:///$(cd "$1" && pwd -W)" ;; *) echo "file://$1" ;; esac; }
mkapi() { # mkapi DIR RELDIR [skip-asset] : write repos/mreyeswilson/kepler/releases/{latest,tags/v9.9.9} + assets
  a="$1"; r="$2"; mkdir -p "$a"; au="$(fileurl "$a")"
  mkdir -p "$a/repos/mreyeswilson/kepler/releases/tags" "$a/repos/mreyeswilson/kepler/releases/assets"
  id=100; json='{"url": "'"$au"'/repos/mreyeswilson/kepler/releases/1", "name": "Kepler v9.9.9", "tag_name": "v9.9.9", "assets": ['
  sep=""
  for f in "$r"/*; do
    n="$(basename "$f")"; id=$((id + 1))
    [ "${3:-}" = "$n" ] && continue
    cp "$f" "$a/repos/mreyeswilson/kepler/releases/assets/$id"
    json="$json$sep"'
  {"url": "'"$au"'/repos/mreyeswilson/kepler/releases/assets/'"$id"'", "id": '"$id"', "node_id": "X", "name": "'"$n"'", "label": "", "uploader": {"login": "u", "url": "'"$au"'/users/u"}, "content_type": "application/octet-stream"}'
    sep=","
  done
  printf '%s\n]}\n' "$json" > "$a/repos/mreyeswilson/kepler/releases/latest"
  cp "$a/repos/mreyeswilson/kepler/releases/latest" "$a/repos/mreyeswilson/kepler/releases/tags/v9.9.9"
}
api="$work/api"; mkapi "$api" "$rel"; apiurl="$(fileurl "$api")"
runtok() { env -u KEPLER_BASE_URL KEPLER_API_URL="$apiurl" KEPLER_GITHUB_TOKEN="ghp_TESTTOKEN123" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Linux KEPLER_ARCH=x86_64 sh "$root/install.sh" "$@"; }
inst="$work/i9"; out="$(runtok 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ -x "$inst/kepler" ] && echo "$out" | grep -q "GitHub API" && echo "$out" | grep -q "SHA-256 verified"; then ok "token: latest via API installs"; else ko "token latest: rc=$rc $out"; fi
echo "$out" | grep -q "ghp_TESTTOKEN123" && ko "token is printed" || ok "token never printed"
inst="$work/i9b"; out="$(runtok --version 9.9.9 2>&1)"; rc=$?
[ $rc -eq 0 ] && [ -x "$inst/kepler" ] && ok "token: --version via API" || ko "token --version: $out"
# checksum still mandatory in token mode
apibad="$work/apibad"; badrel="$work/badrel"; cp -r "$rel" "$badrel"; echo junk >> "$badrel/kepler-9.9.9-x86_64-unknown-linux-gnu.tar.gz"
mkapi "$apibad" "$badrel"; inst="$work/i9d"
out="$(env -u KEPLER_BASE_URL KEPLER_API_URL="$(fileurl "$apibad")" KEPLER_GITHUB_TOKEN="ghp_T" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Linux KEPLER_ARCH=x86_64 sh "$root/install.sh" 2>&1)"; rc=$?
[ $rc -ne 0 ] && [ ! -e "$inst/kepler" ] && echo "$out" | grep -q "mismatch" && ok "token: tampered archive fails" || ko "token tampered: $out"
# missing asset fails clearly
apimiss="$work/apimiss"; mkapi "$apimiss" "$rel" SHA256SUMS; inst="$work/i9e"
out="$(env -u KEPLER_BASE_URL KEPLER_API_URL="$(fileurl "$apimiss")" KEPLER_GITHUB_TOKEN="ghp_T" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Linux KEPLER_ARCH=x86_64 sh "$root/install.sh" 2>&1)"; rc=$?
[ $rc -ne 0 ] && [ ! -e "$inst/kepler" ] && ok "token: missing SHA256SUMS asset fails" || ko "token missing asset: $out"
# a non-GitHub https API base is refused (token must never go elsewhere)
inst="$work/i9f"
out="$(env -u KEPLER_BASE_URL KEPLER_API_URL="https://evil.invalid" KEPLER_GITHUB_TOKEN="ghp_T" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Linux KEPLER_ARCH=x86_64 sh "$root/install.sh" 2>&1)"; rc=$?
[ $rc -ne 0 ] && echo "$out" | grep -q "KEPLER_API_URL must be" && ok "token: foreign API host refused" || ko "foreign API: $out"
# KEPLER_BASE_URL set => token ignored, public path
inst="$work/i9g"; out="$(KEPLER_GITHUB_TOKEN="ghp_T" run 2>&1)"; rc=$?
[ $rc -eq 0 ] && ! echo "$out" | grep -q "GitHub API" && ok "token ignored with KEPLER_BASE_URL" || ko "base url + token: $out"

# 10. anonymous first: a token in the environment must not push a public release through the API
inst="$work/i10"; out="$(GH_TOKEN="ghp_T" KEPLER_API_URL="https://api.invalid" run 2>&1)"; rc=$?
[ $rc -eq 0 ] && [ -x "$inst/kepler" ] && ! echo "$out" | grep -q "GitHub API" && ok "public release + GH_TOKEN installs anonymously" || ko "anonymous first: $out"
# 11. fallback: anonymous download refused (empty mirror) + GH_TOKEN => retries through the API
empty="$work/empty"; mkdir -p "$empty"; inst="$work/i11"
out="$(KEPLER_BASE_URL="$(fileurl "$empty")" GH_TOKEN="ghp_T" KEPLER_API_URL="$apiurl" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Linux KEPLER_ARCH=x86_64 sh "$root/install.sh" 2>&1)"; rc=$?
[ $rc -eq 0 ] && [ -x "$inst/kepler" ] && echo "$out" | grep -q "retrying with a GitHub token" && ok "refused anonymous download falls back to the token" || ko "fallback: rc=$rc $out"
# 12. refused and no token => clear error (gh hidden from PATH)
inst="$work/i12";
out="$(env -u GH_TOKEN -u GITHUB_TOKEN -u KEPLER_GITHUB_TOKEN KEPLER_BASE_URL="$(fileurl "$empty")" KEPLER_INSTALL_DIR="$inst" KEPLER_OS=Linux KEPLER_ARCH=x86_64 sh "$root/install.sh" 2>&1)"; rc=$?
[ $rc -ne 0 ] && [ ! -e "$inst/kepler" ] && ok "refused anonymous download without token fails" || ko "no token: $out"

echo "passed=$pass failed=$fail"
[ "$fail" -eq 0 ]
