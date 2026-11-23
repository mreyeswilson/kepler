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

function Invoke-Installer([string]$base, [string]$dir, [hashtable]$envs = @{}, [string[]]$more = @()) {
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = (Get-Command powershell.exe).Source
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $installer) + $more
    $psi.Arguments = ($argList | ForEach-Object { '"' + $_ + '"' }) -join ' '
    $psi.UseShellExecute = $false; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $psi.EnvironmentVariables['KEPLER_BASE_URL'] = $base
    $psi.EnvironmentVariables['KEPLER_INSTALL_DIR'] = $dir
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
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host "passed=$($script:pass) failed=$($script:fail)"
if ($script:fail -gt 0) { exit 1 }
