# Kepler installer for Windows (x86_64), PowerShell 5.1+. No administrator rights needed.
#
#   irm https://raw.githubusercontent.com/mreyeswilson/kepler/main/install.ps1 | iex
#
# Read this script before running it. What it does:
#   1. picks the Windows x86_64 archive of the GitHub Release (latest, or KEPLER_VERSION),
#   2. downloads SHA256SUMS and the .zip over HTTPS (TLS 1.2+),
#   3. verifies the SHA-256 of the zip and ABORTS if it does not match,
#   4. extracts only kepler.exe into the install directory,
#   5. runs `kepler.exe version` once to confirm it starts.
# No telemetry. Nothing else downloaded is executed.
#
# Environment variables (a piped script cannot take parameters):
#   KEPLER_VERSION         release to install, e.g. 0.1.0 or v0.1.0 (default: latest)
#   KEPLER_INSTALL_DIR     install directory (default: $env:LOCALAPPDATA\Programs\kepler\bin)
#   KEPLER_ADD_TO_PATH=1   opt in to adding the install directory to your USER PATH (default: no change)
#   KEPLER_NO_MODIFY_PATH=1  never touch PATH (wins over KEPLER_ADD_TO_PATH)
#   KEPLER_UNINSTALL=1     remove kepler.exe instead of installing
#   KEPLER_REPO            GitHub owner/name (default: mreyeswilson/kepler)
#   KEPLER_BASE_URL        override of the release download base URL (https:// or file://);
#                          for tests and mirrors. Checksums are still verified.
#   KEPLER_GITHUB_TOKEN    token for private repos (also GH_TOKEN, GITHUB_TOKEN, or `gh auth token`). With a
#                          token, assets come from the GitHub API (api.github.com, HTTPS only; the token is
#                          never printed or stored). Ignored when KEPLER_BASE_URL is set.
#   KEPLER_API_URL         API base override (https://api.github.com or file://; tests only)
#   KEPLER_ARCH            override of the detected architecture (testing only)
# When run as a file you can also pass: -Help  -Uninstall  -Version X.Y.Z  -Dir <path>

function Install-Kepler {
    param([string[]]$CliArgs)

    Set-StrictMode -Version Latest
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'

    function Write-Say([string]$m) { Write-Host $m }

    $usage = @'
Kepler installer (Windows x86_64)

Run:  irm https://raw.githubusercontent.com/mreyeswilson/kepler/main/install.ps1 | iex
File: .\install.ps1 [-Help] [-Uninstall] [-Version X.Y.Z] [-Dir <path>]

Environment: KEPLER_VERSION, KEPLER_INSTALL_DIR, KEPLER_ADD_TO_PATH=1 (opt-in, user PATH),
KEPLER_NO_MODIFY_PATH=1, KEPLER_UNINSTALL=1, KEPLER_REPO, KEPLER_BASE_URL.
The SHA-256 of the downloaded zip is always verified against the release SHA256SUMS.
'@

    $version = $env:KEPLER_VERSION
    $dir = $env:KEPLER_INSTALL_DIR
    $uninstall = ($env:KEPLER_UNINSTALL -eq '1')
    $i = 0
    $a = @($CliArgs)
    while ($i -lt $a.Count) {
        switch -Regex ($a[$i]) {
            '^-{1,2}(Help|h|\?)$' { Write-Say $usage; return }
            '^-{1,2}Uninstall$' { $uninstall = $true }
            '^-{1,2}Version$' { $i++; if ($i -ge $a.Count) { throw '-Version needs a value' }; $version = $a[$i] }
            '^-{1,2}Dir$' { $i++; if ($i -ge $a.Count) { throw '-Dir needs a value' }; $dir = $a[$i] }
            default { throw "unknown option: $($a[$i])" }
        }
        $i++
    }

    if (-not $dir) {
        if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is not set; set KEPLER_INSTALL_DIR' }
        $dir = Join-Path $env:LOCALAPPDATA 'Programs\kepler\bin'
    }
    $exePath = Join-Path $dir 'kepler.exe'
    $modifyPath = ($env:KEPLER_ADD_TO_PATH -eq '1') -and ($env:KEPLER_NO_MODIFY_PATH -ne '1')

    function Test-InUserPath([string]$d) {
        $p = [Environment]::GetEnvironmentVariable('Path', 'User')
        if (-not $p) { return $false }
        foreach ($e in $p.Split(';')) { if ($e.TrimEnd('\') -ieq $d.TrimEnd('\')) { return $true } }
        return $false
    }

    if ($uninstall) {
        if (Test-Path -LiteralPath $exePath) {
            Remove-Item -LiteralPath $exePath -Force
            Write-Say "Removed $exePath. User data was not touched."
        } else {
            Write-Say "Nothing to remove: $exePath does not exist."
        }
        if ($env:KEPLER_NO_MODIFY_PATH -ne '1' -and (Test-InUserPath $dir)) {
            $kept = ([Environment]::GetEnvironmentVariable('Path', 'User')).Split(';') |
                Where-Object { $_ -and ($_.TrimEnd('\') -ine $dir.TrimEnd('\')) }
            [Environment]::SetEnvironmentVariable('Path', ($kept -join ';'), 'User')
            Write-Say "Removed $dir from your user PATH."
        }
        return
    }

    # TLS 1.2 (and 1.3 when the runtime knows it); HTTPS only.
    $proto = [Net.SecurityProtocolType]::Tls12
    try { $proto = $proto -bor [Net.SecurityProtocolType]'Tls13' } catch { }
    [Net.ServicePointManager]::SecurityProtocol = $proto

    $arch = $env:KEPLER_ARCH
    if (-not $arch) {
        $arch = $env:PROCESSOR_ARCHITEW6432
        if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    }
    switch -Regex ($arch) {
        '^(AMD64|x86_64|x64)$' { $target = 'x86_64-pc-windows-msvc' }
        default { throw "unsupported architecture: $arch (supported: x86_64)" }
    }

    $repo = $env:KEPLER_REPO
    if (-not $repo) { $repo = 'mreyeswilson/kepler' }
    if ($version) {
        $version = $version.TrimStart('v')
        if ($version -notmatch '^[0-9A-Za-z.+-]+$') { throw "invalid version: $version" }
    }
    if ($env:KEPLER_BASE_URL) { $base = $env:KEPLER_BASE_URL.TrimEnd('/') }
    elseif ($version) { $base = "https://github.com/$repo/releases/download/v$version" }
    else { $base = "https://github.com/$repo/releases/latest/download" }

    function Get-Remote([string]$url, [string]$dest) {
        if ($url -like 'https://*') {
            Invoke-WebRequest -Uri $url -OutFile $dest -UseBasicParsing -MaximumRedirection 5
        } elseif ($url -like 'file://*') {
            Copy-Item -LiteralPath ([Uri]$url).LocalPath -Destination $dest
        } else {
            throw "refusing non-HTTPS URL: $url"
        }
    }

    # Optional GitHub token (private repos). Only sent to https://api.github.com; never printed or stored.
    $token = $null
    if (-not $env:KEPLER_BASE_URL) {
        foreach ($n in 'KEPLER_GITHUB_TOKEN', 'GH_TOKEN', 'GITHUB_TOKEN') {
            $v = [Environment]::GetEnvironmentVariable($n)
            if ($v -and -not $token) { $token = $v }
        }
        if (-not $token -and (Get-Command gh -ErrorAction SilentlyContinue)) {
            try { $token = (& gh auth token 2>$null | Select-Object -First 1) } catch { $token = $null }
        }
        if ($token) {
            $token = ([string]$token) -replace '\s', ''
            if ($token -notmatch '^[A-Za-z0-9_.-]+$') { throw 'the GitHub token contains unexpected characters; refusing to use it' }
        }
    }
    $api = $env:KEPLER_API_URL
    if (-not $api) { $api = 'https://api.github.com' }
    $api = $api.TrimEnd('/')
    $script:releaseJson = $null

    function Get-Api([string]$url, [string]$dest, [string]$accept) {
        if (-not $url.StartsWith("$api/")) { throw "refusing to fetch outside the API base: $url" }
        if ($url -like 'file://*') {
            Copy-Item -LiteralPath ([Uri]$url).LocalPath -Destination $dest
        } elseif ($url -like 'https://api.github.com/*') {
            $h = @{ Authorization = "Bearer $token"; Accept = $accept; 'X-GitHub-Api-Version' = '2022-11-28'; 'User-Agent' = 'kepler-install' }
            # No automatic redirects: the Authorization header must never follow a redirect to another host.
            $loc = $null
            try {
                Invoke-WebRequest -Uri $url -Headers $h -OutFile $dest -UseBasicParsing -MaximumRedirection 0 | Out-Null
            } catch {
                $resp = $_.Exception.Response
                $code = 0; if ($resp) { $code = [int]$resp.StatusCode }
                if ($code -ge 300 -and $code -lt 400) {
                    $loc = [string]($resp.Headers['Location'])
                    if (-not $loc -and $resp.Headers.Location) { $loc = [string]$resp.Headers.Location }
                } else { throw }
            }
            if ($loc) {
                if ($loc -notlike 'https://*') { throw 'refusing a non-HTTPS redirect' }
                Invoke-WebRequest -Uri $loc -OutFile $dest -UseBasicParsing -MaximumRedirection 5 -Headers @{ Accept = 'application/octet-stream'; 'User-Agent' = 'kepler-install' }
            }
        } else {
            throw "refusing non-HTTPS API URL: $url"
        }
    }

    function Get-Asset([string]$name, [string]$dest) {
        if ($token) {
            $a = @($script:releaseJson.assets | Where-Object { $_.name -eq $name } | Select-Object -First 1)
            if ($a.Count -eq 0) { throw "release has no asset named $name" }
            Get-Api ([string]$a[0].url) $dest 'application/octet-stream'
        } else {
            Get-Remote "$base/$name" $dest
        }
    }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('kepler-install-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Say "Detected target: $target"
        if ($token) {
            if ($api -ne 'https://api.github.com' -and $api -notlike 'file://*') { throw 'KEPLER_API_URL must be https://api.github.com (or file:// for tests)' }
            if ($version) { $relUrl = "$api/repos/$repo/releases/tags/v$version" } else { $relUrl = "$api/repos/$repo/releases/latest" }
            Write-Say "Using a GitHub token: downloading through the GitHub API ($api)"
            $rj = Join-Path $tmp 'release.json'
            try { Get-Api $relUrl $rj 'application/vnd.github+json' } catch { throw "could not read the release metadata (does the release exist, and can the token read the repo?): $($_.Exception.Message)" }
            $script:releaseJson = Get-Content -LiteralPath $rj -Raw | ConvertFrom-Json
        } else {
            Write-Say "Downloading SHA256SUMS from $base"
        }
        $sums = Join-Path $tmp 'SHA256SUMS'
        try { Get-Asset 'SHA256SUMS' $sums } catch { throw "could not download SHA256SUMS (does the release exist?): $($_.Exception.Message)" }

        # Lines look like "<sha256>  kepler-<version>-<target>.zip"
        $entries = @()
        foreach ($line in Get-Content -LiteralPath $sums) {
            if ($line -match '^([0-9A-Fa-f]{64})\s+\*?(\S+)$') { $entries += [pscustomobject]@{ Hash = $Matches[1].ToLowerInvariant(); File = $Matches[2] } }
        }
        if ($version) { $archive = "kepler-$version-$target.zip" }
        else {
            $m = @($entries | Where-Object { $_.File -like "kepler-*-$target.zip" } | Select-Object -First 1)
            if ($m.Count -eq 0) { throw "SHA256SUMS lists no archive for $target" }
            $archive = $m[0].File
        }
        $entry = @($entries | Where-Object { $_.File -eq $archive } | Select-Object -First 1)
        if ($entry.Count -eq 0) { throw "SHA256SUMS has no entry for $archive" }
        $expected = $entry[0].Hash

        Write-Say "Downloading $archive"
        $zip = Join-Path $tmp $archive
        try { Get-Asset $archive $zip } catch { throw "could not download ${archive}: $($_.Exception.Message)" }

        $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $expected) {
            throw "SHA-256 mismatch for $archive (expected $expected, got $actual); aborting, nothing installed"
        }
        Write-Say 'SHA-256 verified.'

        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $stem = [IO.Path]::GetFileNameWithoutExtension($archive)
        $staged = Join-Path $tmp 'kepler.exe'
        $z = [IO.Compression.ZipFile]::OpenRead($zip)
        try {
            $e = $z.Entries | Where-Object { ($_.FullName -replace '\\', '/') -eq "$stem/kepler.exe" } | Select-Object -First 1
            if (-not $e) { throw "archive does not contain $stem/kepler.exe" }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($e, $staged, $true)
        } finally { $z.Dispose() }

        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        Copy-Item -LiteralPath $staged -Destination $exePath -Force
        Write-Say "Installed $exePath"
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    & $exePath version
    if ($LASTEXITCODE -ne 0) { throw "installed binary failed to run 'kepler version'" }

    if (-not (Test-InUserPath $dir)) {
        if ($modifyPath) {
            $cur = [Environment]::GetEnvironmentVariable('Path', 'User')
            $new = if ($cur) { $cur.TrimEnd(';') + ';' + $dir } else { $dir }
            [Environment]::SetEnvironmentVariable('Path', $new, 'User')
            $env:Path = $env:Path.TrimEnd(';') + ';' + $dir
            Write-Say "Added $dir to your user PATH (open a new terminal to pick it up everywhere)."
        } else {
            Write-Say ''
            Write-Say "Note: $dir is not in your PATH. Add it for this session with:"
            Write-Say "  `$env:Path += ';$dir'"
            Write-Say 'or re-run with KEPLER_ADD_TO_PATH=1 to add it to your user PATH.'
        }
    }
}

$script:KeplerFailed = $false
try {
    Install-Kepler -CliArgs $args
} catch {
    $script:KeplerFailed = $true
    [Console]::Error.WriteLine("kepler-install: error: $($_.Exception.Message)")
}
# Exit non-zero only when run as a file; never close the user's shell when piped to iex.
if ($script:KeplerFailed -and $PSCommandPath) { exit 1 }
