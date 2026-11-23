# Local test of install.ps1 against a simulated release directory (file:// base URL).
# Run with Windows PowerShell 5.1:  powershell -NoProfile -ExecutionPolicy Bypass -File tests\installers\test-install-ps1.ps1
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$installer = Join-Path $root 'install.ps1'
$work = Join-Path ([IO.Path]::GetTempPath()) ('kepler-ps1-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
$script:pass = 0; $script:fail = 0
function Ok([string]$m) { $script:pass++; Write-Host "ok   - $m" }
function Ko([string]$m) { $script:fail++; Write-Host "FAIL - $m" }

# A tiny real kepler.exe that prints a version and exits 0.
$src = 'public static class P { public static int Main(string[] a) { System.Console.WriteLine("kepler 9.9.9 (fake)"); return 0; } }'
$fakeExe = Join-Path $work 'fake-kepler.exe'
Add-Type -TypeDefinition $src -OutputAssembly $fakeExe -OutputType ConsoleApplication

$target = 'x86_64-pc-windows-msvc'
$stem = "kepler-9.9.9-$target"
$rel = Join-Path $work 'release'; New-Item -ItemType Directory -Path $rel | Out-Null
$pkg = Join-Path $work "pkg\$stem"; New-Item -ItemType Directory -Path $pkg -Force | Out-Null
Copy-Item $fakeExe (Join-Path $pkg 'kepler.exe'); 'MIT' | Set-Content (Join-Path $pkg 'LICENSE')
Compress-Archive -Path (Join-Path $work "pkg\$stem") -DestinationPath (Join-Path $rel "$stem.zip")
$h = (Get-FileHash (Join-Path $rel "$stem.zip") -Algorithm SHA256).Hash.ToLower()
[IO.File]::WriteAllText((Join-Path $rel 'SHA256SUMS'), "$h  $stem.zip`n")
$url = ([Uri]$rel.ToString()).AbsoluteUri.TrimEnd('/')
# Compress-Archive in 5.1 may write backslash entry names; the installer normalizes them.

$shim = Join-Path $work 'bin'; New-Item -ItemType Directory -Path $shim | Out-Null; Set-Content -LiteralPath (Join-Path $shim 'gh.cmd') -Value '@exit /b 1' -Encoding ASCII
function Invoke-Installer([string]$base, [string]$dir, [hashtable]$envs = @{}, [string[]]$more = @()) {
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = (Get-Command powershell.exe).Source
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $installer) + $more
    $psi.Arguments = ($argList | ForEach-Object { '"' + $_ + '"' }) -join ' '
    $psi.UseShellExecute = $false; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    if ($base) { $psi.EnvironmentVariables['KEPLER_BASE_URL'] = $base }
    $psi.EnvironmentVariables['KEPLER_INSTALL_DIR'] = $dir
    $psi.EnvironmentVariables['PATH'] = $shim + ';' + $psi.EnvironmentVariables['PATH']
    foreach ($k in $envs.Keys) { $psi.EnvironmentVariables[$k] = $envs[$k] }
    $p = [Diagnostics.Process]::Start($psi)
    $out = $p.StandardOutput.ReadToEnd() + $p.StandardError.ReadToEnd()
    $p.WaitForExit()
    [pscustomobject]@{ Code = $p.ExitCode; Out = $out }
}

try {
    # 1. valid checksum
    $d1 = Join-Path $work 'i1'
    $r = Invoke-Installer $url $d1
    if ($r.Code -eq 0 -and (Test-Path "$d1\kepler.exe") -and $r.Out -match 'SHA-256 verified' -and $r.Out -match 'kepler 9.9.9') { Ok 'valid checksum installs and runs' } else { Ko "valid checksum: $($r.Out)" }
    if ($r.Out -match 'not in your PATH') { Ok 'PATH untouched by default, instructions printed' } else { Ko "PATH note: $($r.Out)" }

    # 2. explicit version
    $d2 = Join-Path $work 'i2'
    $r = Invoke-Installer $url $d2 @{} @('-Version', '9.9.9')
    if ($r.Code -eq 0 -and (Test-Path "$d2\kepler.exe")) { Ok '-Version 9.9.9' } else { Ko "-Version: $($r.Out)" }

    # 3. wrong checksum
    $bad = Join-Path $work 'bad'; Copy-Item $rel $bad -Recurse
    [IO.File]::WriteAllText((Join-Path $bad 'SHA256SUMS'), ('0' * 64) + "  $stem.zip`n")
    $d3 = Join-Path $work 'i3'
    $r = Invoke-Installer ([Uri]$bad).AbsoluteUri $d3
    if ($r.Code -ne 0 -and -not (Test-Path "$d3\kepler.exe") -and $r.Out -match 'mismatch') { Ok 'wrong checksum fails, nothing installed' } else { Ko "wrong checksum: code=$($r.Code) $($r.Out)" }

    # 4. tampered archive
    $tam = Join-Path $work 'tam'; Copy-Item $rel $tam -Recurse
    Add-Content -LiteralPath (Join-Path $tam "$stem.zip") -Value 'junk'
    $d4 = Join-Path $work 'i4'
    $r = Invoke-Installer ([Uri]$tam).AbsoluteUri $d4
    if ($r.Code -ne 0 -and -not (Test-Path "$d4\kepler.exe")) { Ok 'tampered archive fails' } else { Ko "tampered: $($r.Out)" }

    # 5. unsupported architecture
    $d5 = Join-Path $work 'i5'
    $r = Invoke-Installer $url $d5 @{ KEPLER_ARCH = 'ARM64' }
    if ($r.Code -ne 0 -and $r.Out -match 'unsupported architecture') { Ok 'unsupported architecture fails' } else { Ko "arch: $($r.Out)" }

    # 6. non-HTTPS refused
    $r = Invoke-Installer 'http://example.invalid/x' $d5
    if ($r.Code -ne 0 -and $r.Out -match 'non-HTTPS') { Ok 'http:// refused' } else { Ko "http: $($r.Out)" }

    # 7. help and uninstall
    $r = Invoke-Installer $url $d1 @{} @('-Help')
    if ($r.Code -eq 0 -and $r.Out -match 'Kepler installer') { Ok '-Help' } else { Ko "-Help: $($r.Out)" }
    $r = Invoke-Installer $url $d1 @{ KEPLER_NO_MODIFY_PATH = '1' } @('-Uninstall')
    if ($r.Code -eq 0 -and -not (Test-Path "$d1\kepler.exe")) { Ok '-Uninstall removes binary' } else { Ko "-Uninstall: $($r.Out)" }

    # 8. token mode: assets via a simulated GitHub API (file:// API base; no KEPLER_BASE_URL)
    function New-FakeApi([string]$apiDir, [string]$relDir, [string]$skip = '') {
        $ru = ([Uri]$apiDir).AbsoluteUri.TrimEnd('/')
        $rr = Join-Path $apiDir 'repos\mreyeswilson\kepler\releases'
        New-Item -ItemType Directory -Path (Join-Path $rr 'tags'), (Join-Path $rr 'assets') -Force | Out-Null
        $assets = @(); $id = 100
        foreach ($f in Get-ChildItem $relDir -File) {
            $id++
            if ($f.Name -eq $skip) { continue }
            Copy-Item $f.FullName (Join-Path $rr "assets\$id")
            $assets += @{ url = "$ru/repos/mreyeswilson/kepler/releases/assets/$id"; id = $id; name = $f.Name }
        }
        $json = @{ tag_name = 'v9.9.9'; assets = $assets } | ConvertTo-Json -Depth 5
        [IO.File]::WriteAllText((Join-Path $rr 'latest'), $json)
        [IO.File]::WriteAllText((Join-Path $rr 'tags\v9.9.9'), $json)
        $ru
    }
    function Invoke-TokenInstaller([string]$apiUrl, [string]$dir, [hashtable]$envs = @{}, [string[]]$more = @()) {
        $e = @{ KEPLER_API_URL = $apiUrl; KEPLER_GITHUB_TOKEN = 'ghp_TESTTOKEN123' }
        foreach ($k in $envs.Keys) { $e[$k] = $envs[$k] }
        Invoke-Installer '' $dir $e $more
    }
    $apiUrl = New-FakeApi (Join-Path $work 'api') $rel
    $d8 = Join-Path $work 'i8'
    $r = Invoke-TokenInstaller $apiUrl $d8
    if ($r.Code -eq 0 -and (Test-Path "$d8\kepler.exe") -and $r.Out -match 'GitHub API' -and $r.Out -match 'SHA-256 verified') { Ok 'token: latest via API installs' } else { Ko "token latest: $($r.Out)" }
    if ($r.Out -notmatch 'ghp_TESTTOKEN123') { Ok 'token never printed' } else { Ko 'token printed' }
    $d8b = Join-Path $work 'i8b'
    $r = Invoke-TokenInstaller $apiUrl $d8b @{} @('-Version', '9.9.9')
    if ($r.Code -eq 0 -and (Test-Path "$d8b\kepler.exe")) { Ok 'token: -Version via API' } else { Ko "token -Version: $($r.Out)" }
    # anonymous first: a token in the environment must not push a public release through the API
    $d8c = Join-Path $work 'i8c'
    $r = Invoke-Installer $url $d8c @{ GH_TOKEN = 'ghp_OTHER'; KEPLER_API_URL = 'https://api.invalid' }
    if ($r.Code -eq 0 -and (Test-Path "$d8c\kepler.exe") -and $r.Out -notmatch 'GitHub API') { Ok 'public release + GH_TOKEN installs anonymously' } else { Ko "anonymous first: $($r.Out)" }
    # refused anonymous download (empty mirror) + GH_TOKEN => retries through the API
    $empty = Join-Path $work 'empty'; New-Item -ItemType Directory -Path $empty | Out-Null
    $d8h = Join-Path $work 'i8h'
    $r = Invoke-Installer ([Uri]$empty).AbsoluteUri $d8h @{ GH_TOKEN = 'ghp_OTHER'; KEPLER_API_URL = $apiUrl }
    if ($r.Code -eq 0 -and (Test-Path "$d8h\kepler.exe") -and $r.Out -match 'retrying with a GitHub token') { Ok 'refused anonymous download falls back to the token' } else { Ko "fallback: $($r.Out)" }
    # refused and no token => clear error
    $d8i = Join-Path $work 'i8i'
    $r = Invoke-Installer ([Uri]$empty).AbsoluteUri $d8i
    if ($r.Code -ne 0 -and $r.Out -match 'set KEPLER_GITHUB_TOKEN') { Ok 'refused anonymous download without token fails clearly' } else { Ko "no token: $($r.Out)" }
    # non-web exception (invalid URL) must not blow up on .Response under StrictMode
    $d8j = Join-Path $work 'i8j'
    $r = Invoke-Installer 'https://bad host/x' $d8j @{ GH_TOKEN = 'ghp_OTHER' }
    if ($r.Code -ne 0 -and $r.Out -match 'could not download SHA256SUMS' -and $r.Out -notmatch "Response") { Ok 'non-web exception reported cleanly' } else { Ko "non-web: $($r.Out)" }
    # checksum still mandatory in token mode
    $badRel = Join-Path $work 'badrel'; Copy-Item $rel $badRel -Recurse
    Add-Content -LiteralPath (Join-Path $badRel "$stem.zip") -Value 'junk'
    $apiBad = New-FakeApi (Join-Path $work 'apibad') $badRel
    $d8d = Join-Path $work 'i8d'
    $r = Invoke-TokenInstaller $apiBad $d8d
    if ($r.Code -ne 0 -and -not (Test-Path "$d8d\kepler.exe") -and $r.Out -match 'mismatch') { Ok 'token: tampered archive fails' } else { Ko "token tampered: $($r.Out)" }
    # missing asset
    $apiMiss = New-FakeApi (Join-Path $work 'apimiss') $rel 'SHA256SUMS'
    $d8e = Join-Path $work 'i8e'
    $r = Invoke-TokenInstaller $apiMiss $d8e
    if ($r.Code -ne 0 -and -not (Test-Path "$d8e\kepler.exe") -and $r.Out -match 'no asset named SHA256SUMS') { Ok 'token: missing SHA256SUMS asset fails' } else { Ko "token missing asset: $($r.Out)" }
    # a non-GitHub https API base is refused
    $d8f = Join-Path $work 'i8f'
    $r = Invoke-TokenInstaller 'https://evil.invalid' $d8f
    if ($r.Code -ne 0 -and $r.Out -match 'KEPLER_API_URL must be') { Ok 'token: foreign API host refused' } else { Ko "foreign API: $($r.Out)" }
    # KEPLER_BASE_URL set => token ignored (public path)
    $d8g = Join-Path $work 'i8g'
    $r = Invoke-Installer $url $d8g @{ KEPLER_GITHUB_TOKEN = 'ghp_T' }
    if ($r.Code -eq 0 -and $r.Out -notmatch 'GitHub API') { Ok 'token ignored with KEPLER_BASE_URL' } else { Ko "base url + token: $($r.Out)" }
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host "passed=$($script:pass) failed=$($script:fail)"
if ($script:fail -gt 0) { exit 1 }
