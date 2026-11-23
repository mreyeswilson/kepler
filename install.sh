#!/bin/sh
# Kepler installer for Linux and macOS (x86_64, aarch64).
#
#   curl -fsSL https://raw.githubusercontent.com/mreyeswilson/kepler/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/mreyeswilson/kepler/main/install.sh | sh -s -- --help
#
# Read this script before running it. What it does:
#   1. detects OS/arch and picks the matching GitHub Release archive,
#   2. downloads SHA256SUMS and the archive over HTTPS,
#   3. verifies the SHA-256 of the archive and ABORTS if it does not match,
#   4. extracts only the `kepler` binary into the install directory (no sudo),
#   5. runs `kepler version` once to confirm it starts.
# No telemetry. Nothing else downloaded is executed.
#
# Environment:
#   KEPLER_VERSION      release to install, e.g. 0.1.0 or v0.1.0 (default: latest)
#   KEPLER_INSTALL_DIR  install directory (default: $HOME/.local/bin)
#   KEPLER_REPO         GitHub repository owner/name (default: mreyeswilson/kepler)
#   KEPLER_BASE_URL     override of the release download base URL (https:// or file://).
#                       Intended for tests and mirrors; checksums are still verified.
#   KEPLER_GITHUB_TOKEN token for private repos. Downloads are anonymous first; the token is used when this is set,
#                       or as a fallback when the anonymous download is refused (then also GH_TOKEN, GITHUB_TOKEN or
#                       `gh auth token`). With a token, assets come from the GitHub API (api.github.com, HTTPS only; the
#                       token is never printed or stored).
#   KEPLER_API_URL      API base override (https://api.github.com or file://; tests only)
#   KEPLER_OS/KEPLER_ARCH  override `uname -s` / `uname -m` (testing only)

set -eu

REPO="${KEPLER_REPO:-mreyeswilson/kepler}"

say() { printf '%s\n' "$*"; }
err() { printf 'kepler-install: error: %s\n' "$*" >&2; }
die() { err "$*"; exit 1; }

usage() {
  cat <<'EOF'
Kepler installer

Usage: install.sh [--version X.Y.Z] [--dir DIR] [--uninstall] [--help]

  --version X.Y.Z  install this release (default: latest; or set KEPLER_VERSION)
  --dir DIR        install directory (default: $HOME/.local/bin; or set KEPLER_INSTALL_DIR)
  --uninstall      remove the installed `kepler` binary from the install directory
  --help           show this help

Supported: Linux and macOS on x86_64 and aarch64. Windows: use install.ps1.
The SHA-256 of the downloaded archive is always verified against the release SHA256SUMS.
EOF
}

have() { command -v "$1" >/dev/null 2>&1; }

fetch() { # fetch URL DEST
  case "$1" in
    https://*)
      if have curl; then
        curl -fsSL --proto '=https' --tlsv1.2 --retry 2 -o "$2" "$1"
      elif have wget; then
        wget -q --https-only -O "$2" "$1"
      else
        die "curl or wget is required"
      fi
      ;;
    file://*)
      have curl || die "curl is required for file:// base URLs"
      curl -fsSL -o "$2" "$1"
      ;;
    *) die "refusing non-HTTPS URL: $1" ;;
  esac
}

# fetch_status URL DEST : like fetch, but sets FETCH_CODE (HTTP status, "file" for a failed file://, 000 if unknown)
fetch_status() {
  FETCH_CODE=000
  case "$1" in
    https://*)
      if have curl; then
        FETCH_CODE="$(curl -sSL --proto '=https' --tlsv1.2 --retry 2 -o "$2" -w '%{http_code}' "$1" 2>/dev/null || true)"
        case "$FETCH_CODE" in 2??) return 0 ;; esac
        return 1
      fi
      fetch "$1" "$2" 2>/dev/null
      ;;
    file://*)
      if fetch "$1" "$2" 2>/dev/null; then return 0; fi
      FETCH_CODE="file"
      return 1
      ;;
    *) die "refusing non-HTTPS URL: $1" ;;
  esac
}

sha256_of() { # sha256_of FILE -> hex digest
  if have sha256sum; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif have shasum; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  elif have openssl; then
    openssl dgst -sha256 "$1" | sed 's/^.*= *//'
  else
    die "no SHA-256 tool found (need sha256sum, shasum or openssl)"
  fi
}

# --- Optional GitHub token (private repos) -------------------------------------------------------
# With a token, assets are downloaded through the GitHub API; without one, the public URLs are used.
# The token is only ever sent to https://api.github.com, via curl's stdin config (never in argv),
# and is never printed or stored.
API="${KEPLER_API_URL:-https://api.github.com}"
TOKEN=""

resolve_token() {
  TOKEN="${KEPLER_GITHUB_TOKEN:-${GH_TOKEN:-${GITHUB_TOKEN:-}}}"
  if [ -z "$TOKEN" ] && have gh; then
    TOKEN="$(gh auth token 2>/dev/null || true)"
  fi
  TOKEN="$(printf '%s' "$TOKEN" | tr -d '[:space:]')"
  case "$TOKEN" in
    *[!A-Za-z0-9_.-]*) die "the GitHub token contains unexpected characters; refusing to use it" ;;
  esac
}

# api_fetch URL DEST ACCEPT : GET from the API base only; Authorization only for https://api.github.com
api_fetch() {
  case "$1" in
    "$API"/*) ;;
    *) die "refusing to fetch outside the API base: $1" ;;
  esac
  have curl || die "curl is required to use a GitHub token"
  case "$1" in
    https://api.github.com/*)
      printf 'header = "Authorization: Bearer %s"\n' "$TOKEN" |
        curl -fsSL --proto '=https' --tlsv1.2 --retry 2 -K - -H "Accept: $3" -H 'X-GitHub-Api-Version: 2022-11-28' -o "$2" "$1"
      ;;
    file://*) curl -fsSL -o "$2" "$1" ;;
    *) die "refusing non-HTTPS API URL: $1" ;;
  esac
}

# asset_url NAME : print the API URL of release asset NAME from the release JSON in $tmp/release.json
asset_url() {
  awk -v want="$1" 'BEGIN { RS = "," }
    { if (match($0, /"url" *: *"[^"]*\/releases\/assets\/[0-9]+"/)) {
        u = substr($0, RSTART, RLENGTH); sub(/^"url" *: *"/, "", u); sub(/"$/, "", u); url = u; next }
      if (url != "" && match($0, /"name" *: *"[^"]*"/)) {
        n = substr($0, RSTART, RLENGTH); sub(/^"name" *: *"/, "", n); sub(/"$/, "", n)
        if (n == want) { print url; exit }
        url = "" } }' "$tmp/release.json"
}

# get_asset NAME DEST : download release asset NAME (API mode) or "$base/NAME" (public mode)
get_asset() {
  if [ -n "$TOKEN" ]; then
    u="$(asset_url "$1")"
    [ -n "$u" ] || return 1
    api_fetch "$u" "$2" "application/octet-stream"
  else
    fetch "$base/$1" "$2"
  fi
}

detect_target() {
  os="${KEPLER_OS:-$(uname -s)}"
  arch="${KEPLER_ARCH:-$(uname -m)}"
  case "$os" in
    Linux) os_part="unknown-linux-gnu" ;;
    Darwin) os_part="apple-darwin" ;;
    MINGW* | MSYS* | CYGWIN*) die "Windows detected: use install.ps1 (see the README)" ;;
    *) die "unsupported operating system: $os (supported: Linux, macOS)" ;;
  esac
  case "$arch" in
    x86_64 | amd64) arch_part="x86_64" ;;
    aarch64 | arm64) arch_part="aarch64" ;;
    *) die "unsupported architecture: $arch (supported: x86_64, aarch64)" ;;
  esac
  # Rosetta: an x86_64 shell on Apple Silicon should get the native binary.
  if [ -z "${KEPLER_ARCH:-}" ] && [ "$os" = "Darwin" ] && [ "$arch_part" = "x86_64" ] && have sysctl; then
    if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
      arch_part="aarch64"
    fi
  fi
  if [ "$os" = "Linux" ] && have ldd && ldd --version 2>&1 | grep -qi musl; then
    err "warning: musl libc detected; release binaries are built for glibc and may not run"
  fi
  TARGET="${arch_part}-${os_part}"
}

install_main() {
  version="${KEPLER_VERSION:-}"
  dir="${KEPLER_INSTALL_DIR:-${HOME:-}/.local/bin}"
  uninstall=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --help | -h) usage; return 0 ;;
      --uninstall) uninstall=1 ;;
      --version)
        [ $# -ge 2 ] || die "--version needs a value"
        version="$2"; shift ;;
      --dir)
        [ $# -ge 2 ] || die "--dir needs a value"
        dir="$2"; shift ;;
      *) usage >&2; die "unknown option: $1" ;;
    esac
    shift
  done
  if [ -z "$dir" ] || [ "$dir" = "/.local/bin" ]; then
    die "cannot determine the install directory (HOME is unset); set KEPLER_INSTALL_DIR"
  fi

  if [ "$uninstall" -eq 1 ]; then
    if [ -f "$dir/kepler" ]; then
      rm -f "$dir/kepler"
      say "Removed $dir/kepler. User data (~/.kepler or your configured data dir) was not touched."
    else
      say "Nothing to remove: $dir/kepler does not exist."
    fi
    return 0
  fi

  detect_target

  version="${version#v}"
  if [ -n "$version" ]; then
    case "$version" in
      *[!0-9A-Za-z.+-]* | "") die "invalid version: $version" ;;
    esac
  fi
  if [ -n "${KEPLER_BASE_URL:-}" ]; then
    base="${KEPLER_BASE_URL%/}"
  elif [ -n "$version" ]; then
    base="https://github.com/${REPO}/releases/download/v${version}"
  else
    base="https://github.com/${REPO}/releases/latest/download"
  fi

  tmp="$(mktemp -d "${TMPDIR:-/tmp}/kepler-install.XXXXXX")" || die "cannot create a temporary directory"
  trap 'rm -rf "$tmp"' EXIT INT TERM HUP

  say "Detected target: $TARGET"
  # Anonymous download first. A token is used only when KEPLER_GITHUB_TOKEN is set explicitly (and no
  # KEPLER_BASE_URL mirror is given), or as a fallback when the anonymous download is refused (401/403/404).
  MODE=public
  if [ -z "${KEPLER_BASE_URL:-}" ] && [ -n "${KEPLER_GITHUB_TOKEN:-}" ]; then resolve_token; MODE=api; fi
  if [ "$MODE" = public ]; then
    say "Downloading SHA256SUMS from $base"
    if ! fetch_status "$base/SHA256SUMS" "$tmp/SHA256SUMS"; then
      case "$FETCH_CODE" in
        401 | 403 | 404 | file) ;;
        *) die "could not download SHA256SUMS (does the release exist?)" ;;
      esac
      resolve_token
      [ -n "$TOKEN" ] || die "could not download SHA256SUMS (the release does not exist or the repository is private); set KEPLER_GITHUB_TOKEN to use the GitHub API"
      say "Anonymous download was refused; retrying with a GitHub token through the API"
      MODE=api
    fi
  fi
  if [ "$MODE" = api ]; then
    case "$API" in
      https://api.github.com | file://*) API="${API%/}" ;;
      *) die "KEPLER_API_URL must be https://api.github.com (or file:// for tests)" ;;
    esac
    if [ -n "$version" ]; then rel_url="$API/repos/${REPO}/releases/tags/v${version}"; else rel_url="$API/repos/${REPO}/releases/latest"; fi
    say "Using a GitHub token: downloading through the GitHub API ($API)"
    api_fetch "$rel_url" "$tmp/release.json" "application/vnd.github+json" || die "could not read the release metadata (does the release exist, and can the token read the repo?)"
    get_asset SHA256SUMS "$tmp/SHA256SUMS" || die "could not download SHA256SUMS (does the release exist?)"
  fi

  # Lines look like "<sha256>  kepler-<version>-<target>.tar.gz"
  if [ -n "$version" ]; then
    archive="kepler-${version}-${TARGET}.tar.gz"
  else
    archive="$(awk -v t="-${TARGET}.tar.gz" '{ f=$2; sub(/^\*/, "", f); n=length(f); m=length(t); if (n>m && substr(f,1,7)=="kepler-" && substr(f,n-m+1)==t) { print f; exit } }' "$tmp/SHA256SUMS")"
    [ -n "$archive" ] || die "SHA256SUMS lists no archive for $TARGET"
  fi
  expected="$(awk -v a="$archive" '{ f=$2; sub(/^\*/, "", f); if (f==a) { print $1; exit } }' "$tmp/SHA256SUMS")"
  [ -n "$expected" ] || die "SHA256SUMS has no entry for $archive"
  case "$expected" in
    *[!0-9a-fA-F]* | "") die "malformed checksum for $archive" ;;
  esac
  [ "${#expected}" -eq 64 ] || die "malformed checksum for $archive"

  say "Downloading $archive"
  get_asset "$archive" "$tmp/$archive" || die "could not download $archive"

  actual="$(sha256_of "$tmp/$archive")"
  expected_lc="$(printf '%s' "$expected" | tr 'A-F' 'a-f')"
  if [ "$actual" != "$expected_lc" ]; then
    die "SHA-256 mismatch for $archive (expected $expected_lc, got $actual); aborting, nothing installed"
  fi
  say "SHA-256 verified."

  have tar || die "tar is required"
  stem="${archive%.tar.gz}"
  mkdir "$tmp/x"
  tar -xzf "$tmp/$archive" -C "$tmp/x" "$stem/kepler" || die "archive does not contain $stem/kepler"
  [ -f "$tmp/x/$stem/kepler" ] || die "archive does not contain a regular file $stem/kepler"

  mkdir -p "$dir" || die "cannot create $dir"
  cp "$tmp/x/$stem/kepler" "$dir/.kepler.new.$$" || die "cannot write to $dir"
  chmod 755 "$dir/.kepler.new.$$"
  mv -f "$dir/.kepler.new.$$" "$dir/kepler"
  say "Installed $dir/kepler"

  "$dir/kepler" version || die "installed binary failed to run 'kepler version'"

  case ":${PATH:-}:" in
    *":$dir:"*) ;;
    *)
      say ""
      say "Note: $dir is not in your PATH. Add it, e.g.:"
      say "  export PATH=\"$dir:\$PATH\""
      ;;
  esac
}

install_main "$@"
