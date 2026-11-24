<#
.SYNOPSIS
  Functional acceptance suite for the Kepler release binary (Windows).

.DESCRIPTION
  Drives the real `kepler.exe` in a throw-away KEPLER_HOME: version, doctor, emit
  (passthrough / enrich / deny), memory, sentinel, inspect, MCP over stdio, Vision against a
  real browser and a local fixture server, and Orbit on a temporary git repository.
  Prints a PASS / FAIL / SKIP table and exits non-zero if anything FAILED.
  A SKIP is never a PASS: it is reported with its reason, and -RequireVision turns a skipped
  Vision section into a failure.

.PARAMETER Kepler
  Path to kepler.exe (default: target\release\kepler.exe next to this repository).
.PARAMETER RequireVision
  Fail (instead of skip) when Node.js, npm or an installed Edge/Chrome is missing.
.PARAMETER WithClaude
  Also run one real, non-destructive `claude -p` task through Orbit and one real `claude -p` that calls a Kepler MCP tool (needs `claude` on PATH).
#>
param(
  [string]$Kepler = '',
  [switch]$RequireVision,
  [switch]$WithClaude
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if ($Kepler -eq '') { $Kepler = Join-Path $repoRoot 'target\release\kepler.exe' }
if (-not (Test-Path $Kepler)) { Write-Host "kepler binary not found: $Kepler (run cargo build --release)"; exit 2 }
$Kepler = (Resolve-Path $Kepler).Path

$work = Join-Path ([IO.Path]::GetTempPath()) ("kepler-acceptance-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $work | Out-Null
$env:KEPLER_HOME = Join-Path $work 'home'
$env:KEPLER_IPC_NAME = 'acc' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
$env:KEPLER_IDLE_TIMEOUT_SECS = '3'
# Hermetic: the suite never calls a real model. The Forge model-assist step enables a fake one.
$env:KEPLER_FORGE_MODEL = 'off'
$env:GIT_AUTHOR_NAME = 't'; $env:GIT_AUTHOR_EMAIL = 't@t'; $env:GIT_COMMITTER_NAME = 't'; $env:GIT_COMMITTER_EMAIL = 't@t'

$results = New-Object System.Collections.ArrayList
$cleanups = New-Object System.Collections.ArrayList

function Add-Result($name, $status, $detail) {
  [void]$results.Add([pscustomobject]@{ Step = $name; Status = $status; Detail = $detail })
  $color = switch ($status) { 'PASS' { 'Green' } 'FAIL' { 'Red' } default { 'Yellow' } }
  Write-Host ("[{0}] {1}  {2}" -f $status, $name, $detail) -ForegroundColor $color
}

class SkipException : System.Exception { SkipException([string]$m) : base($m) {} }

function Step($name, [scriptblock]$body) {
  try {
    $detail = & $body
    Add-Result $name 'PASS' ([string]$detail)
  } catch [SkipException] {
    Add-Result $name 'SKIP' $_.Exception.Message
  } catch {
    Add-Result $name 'FAIL' ($_.Exception.Message -replace "\s+", ' ')
  }
}

function Assert($cond, $msg) { if (-not $cond) { throw $msg } }

function Quote-Arg([string]$a) {
  if ($a -eq '') { return '""' }
  if ($a -notmatch '[\s"]') { return $a }
  $s = $a -replace '(\\*)"', '$1$1\"'
  $s = $s -replace '(\\+)$', '$1$1'
  return '"' + $s + '"'
}

# Run a process, capture stdout/stderr/exit code, with a hard timeout (kills the tree).
function Invoke-Proc([string]$exe, [string[]]$argv, [string]$stdin = $null, [int]$timeoutSec = 120, [string]$cwd = $null) {
  $inFile = $null
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $exe
  $psi.Arguments = ($argv | ForEach-Object { Quote-Arg $_ }) -join ' '
  if ($null -ne $stdin) {
    # Feed stdin from a file through cmd: a redirected .NET stdin writer may prepend a BOM.
    $inFile = Join-Path ([IO.Path]::GetTempPath()) ("kepler-acc-in-" + [Guid]::NewGuid().ToString('N') + '.txt')
    [IO.File]::WriteAllBytes($inFile, (New-Object Text.UTF8Encoding($false)).GetBytes($stdin))
    $psi.Arguments = '/d /s /c ""' + $exe + '" ' + $psi.Arguments + ' < "' + $inFile + '""'
    $psi.FileName = $env:ComSpec
  }
  $psi.UseShellExecute = $false
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.RedirectStandardInput = $false
  $psi.CreateNoWindow = $true
  if ($cwd) { $psi.WorkingDirectory = $cwd }
  $p = New-Object System.Diagnostics.Process
  $p.StartInfo = $psi
  [void]$p.Start()
  $outTask = $p.StandardOutput.ReadToEndAsync()
  $errTask = $p.StandardError.ReadToEndAsync()
  if (-not $p.WaitForExit($timeoutSec * 1000)) {
    & taskkill /PID $p.Id /T /F | Out-Null
    throw "timeout after ${timeoutSec}s: $exe $($psi.Arguments)"
  }
  $p.WaitForExit()
  if ($inFile) { Remove-Item $inFile -ErrorAction SilentlyContinue }
  return [pscustomobject]@{ Code = $p.ExitCode; Out = $outTask.Result; Err = $errTask.Result }
}

function K([string[]]$argv, [string]$stdin = $null, [int]$timeoutSec = 120, [string]$cwd = $null) {
  Invoke-Proc $Kepler $argv $stdin $timeoutSec $cwd
}

function KJson([string[]]$argv, [int]$timeoutSec = 120) {
  $r = K $argv $null $timeoutSec
  try { $v = $r.Out | ConvertFrom-Json } catch { throw "not JSON (exit $($r.Code)): $($r.Out) $($r.Err)" }
  return [pscustomobject]@{ Code = $r.Code; V = $v; Err = $r.Err }
}

function Git([string]$dir, [string[]]$argv) {
  $r = Invoke-Proc 'git' (@('-C', $dir) + $argv) $null 60
  if ($r.Code -ne 0) { throw "git $($argv -join ' '): $($r.Err)" }
  return $r.Out
}

function New-Repo([string]$name) {
  $d = Join-Path $work $name
  New-Item -ItemType Directory -Path $d | Out-Null
  Git $d @('init', '-q', '-b', 'main') | Out-Null
  Git $d @('config', 'user.email', 't@t') | Out-Null
  Git $d @('config', 'user.name', 't') | Out-Null
  Set-Content -Path (Join-Path $d 'base.txt') -Value 'base'
  Git $d @('add', '.') | Out-Null
  Git $d @('commit', '-q', '-m', 'init') | Out-Null
  return $d
}

function Write-Json([string]$path, $obj) {
  [IO.File]::WriteAllText($path, ($obj | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding($false)))
}

function Event-Json($type, $payload) {
  @{ version = 1; id = "evt-$type-$([Guid]::NewGuid().ToString('N').Substring(0,6))"; type = $type
     timestamp = '2026-01-02T03:04:05.123Z'; harness = 'pi'; project_id = 'proj-demo'
     session_id = 'sess-001'; correlation_id = 'task-001'; payload = $payload }
}

# ------------------------------------------------------------------ core
Step 'version' {
  $r = K @('version', '--json')
  Assert ($r.Code -eq 0) "exit $($r.Code)"
  $v = $r.Out | ConvertFrom-Json
  Assert ($v.version) "no version field: $($r.Out)"
  "kepler $($v.version)"
}

Step 'doctor' {
  $r = KJson @('doctor', '--json')
  Assert ($r.Code -eq 0) "exit $($r.Code)"
  Assert ($r.V.runtime.llm_or_network_calls -eq $false) 'runtime must report no LLM/network calls'
  Assert (-not (Test-Path (Join-Path $env:KEPLER_HOME 'kepler.db'))) 'doctor must not create the database'
  'read-only, no database created'
}

Step 'emit passthrough' {
  $f = Join-Path $work 'e1.json'
  Write-Json $f (Event-Json 'input.received' @{ text = 'Fix the failing test in parser.rs' })
  $r = KJson @('emit', '--in-process', '--file', $f)
  Assert ($r.V.action -eq 'passthrough') "action=$($r.V.action)"
  "action=$($r.V.action)"
}

Step 'emit deny (read .env)' {
  $f = Join-Path $work 'e2.json'
  Write-Json $f (Event-Json 'tool.call.requested' @{ tool = 'bash'; command = 'cat .env' })
  $r = KJson @('emit', '--in-process', '--file', $f)
  Assert ($r.V.action -eq 'deny') "action=$($r.V.action)"
  "action=$($r.V.action)"
}

# ------------------------------------------------------------------ memory (+ emit enrich)
Step 'memory add/search/export' {
  $a = K @('memory', 'add', 'parser tests live in tests/parser and use golden fixtures; token api_key=sk-abcdefghijklmnopqrstuvwxyz0123456789', '--project', 'proj-demo')
  Assert ($a.Code -eq 0) "add exit $($a.Code): $($a.Err)"
  $s = K @('memory', 'search', 'where are the parser tests', '--project', 'proj-demo')
  Assert ($s.Out -match 'tests/parser') "search did not find the memory: $($s.Out)"
  $e = K @('memory', 'export', '--project', 'proj-demo')
  Assert ($e.Out -match 'tests/parser') 'export lacks the memory'
  Assert ($e.Out -notmatch 'sk-abcdefgh') 'export leaked the secret'
  'found, secret redacted in export'
}

Step 'emit enrich (memory in context)' {
  $f = Join-Path $work 'e3.json'
  Write-Json $f (Event-Json 'context.requested' @{ budget_tokens = 2000; query = 'where are the parser tests' })
  $r = KJson @('emit', '--file', $f)
  Assert ($r.V.action -eq 'enrich') "action=$($r.V.action) reason=$($r.V.reason)"
  "action=$($r.V.action)"
}

Step 'inspect' {
  $r = KJson @('inspect', '--limit', '5')
  Assert ($r.Code -eq 0) "exit $($r.Code)"
  Assert ($r.V.memory.total -ge 1) 'memory counters missing'
  "memory.total=$($r.V.memory.total)"
}

Step 'mcp serve (initialize, tools/list, tools/call)' {
  $reqs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"acc","version":"0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/list"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"kepler_memory_search","arguments":{"query":"where are the parser tests","project":"proj-demo"}}}'
  ) -join "`n"
  $r = K @('mcp', 'serve') ($reqs + "`n") 60
  Assert ($r.Code -eq 0) "exit $($r.Code): $($r.Err)"
  $lines = @($r.Out -split "`n" | Where-Object { $_.Trim() -ne '' } | ForEach-Object { $_ | ConvertFrom-Json })
  Assert ($lines.Count -eq 3) "expected 3 replies, got $($lines.Count)"
  Assert ($lines[0].result.protocolVersion) "no protocolVersion in initialize: $($r.Out.Substring(0, [Math]::Min(300, $r.Out.Length)))"
  Assert ($lines[1].result.tools.Count -ge 3) 'tools/list returned < 3 tools'
  Assert ($lines[2].result.content[0].text -match 'tests/parser') 'tools/call did not return the memory'
  "$($lines[1].result.tools.Count) tools; tools/call returned the memory"
}

Step 'memory purge' {
  $d = K @('memory', 'purge', '--project', 'proj-demo')
  Assert ($d.Code -ne 0) 'purge without --yes must be refused'
  $r = K @('memory', 'purge', '--project', 'proj-demo', '--yes')
  Assert ($r.Code -eq 0) "exit $($r.Code): $($r.Err)"
  $st = KJson @('memory', 'stats', '--json')
  Assert ($st.V.total -eq 0) "total=$($st.V.total)"
  'refused without --yes; purged'
}

# ------------------------------------------------------------------ sentinel
Step 'sentinel audit (suspicious change)' {
  $repo = New-Repo 'sentinel-repo'
  Set-Content -Path (Join-Path $repo 'a.py') -Value 'x = 1'
  Git $repo @('add', '.') | Out-Null; Git $repo @('commit', '-q', '-m', 'a') | Out-Null
  Set-Content -Path (Join-Path $repo 'a.py') -Value @('x = 1', 'eval(user_input)')
  $r = KJson @('sentinel', 'audit', '--repo', $repo, '--json')
  Assert ($r.V.findings[0].rule -eq 'py.eval') "rule=$($r.V.findings[0].rule)"
  $g = K @('sentinel', 'audit', '--repo', $repo, '--json', '--fail-on', 'high')
  Assert ($g.Code -eq 1) "--fail-on high should exit 1, got $($g.Code)"
  'py.eval found; --fail-on high exits 1'
}

# ------------------------------------------------------------------ vision
$visionOk = $false
$nodeExe = (Get-Command node -ErrorAction SilentlyContinue)
$browserFound = ($null -ne ((@(
  "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe",
  "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe",
  "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
  "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1)))

function Skip-Or-Fail($why) {
  if ($RequireVision) { throw "Vision prerequisites missing: $why" }
  throw [SkipException]::new("skipped (not a pass): $why")
}

Step 'vision setup + status --probe (real browser)' {
  if (-not $nodeExe) { Skip-Or-Fail 'Node.js not on PATH' }
  if (-not $browserFound) { Skip-Or-Fail 'no Microsoft Edge or Google Chrome installed' }
  $s = K @('vision', 'setup') $null 300
  Assert ($s.Code -eq 0) "setup exit $($s.Code): $($s.Err) $($s.Out)"
  $p = K @('vision', 'status', '--probe') $null 120
  Assert ($p.Code -eq 0) "probe exit $($p.Code): $($p.Out)"
  Assert ($p.Out -match 'browser: (msedge|chrome|chromium)') "no browser reported: $($p.Out)"
  $script:visionOk = $true
  ($p.Out -split "`n" | Where-Object { $_ -match '^browser:' }) -join ''
}

$fx = $null
if ($visionOk) {
  $fxOut = Join-Path $work 'fx.out'
  $fx = Start-Process -FilePath $nodeExe.Source -ArgumentList @((Join-Path $PSScriptRoot 'fixture-server.mjs')) -PassThru -WindowStyle Hidden -RedirectStandardOutput $fxOut
  [void]$cleanups.Add({ if ($fx -and -not $fx.HasExited) { & taskkill /PID $fx.Id /T /F | Out-Null } })
  $port = $null
  for ($i = 0; $i -lt 50 -and -not $port; $i++) {
    Start-Sleep -Milliseconds 100
    if (Test-Path $fxOut) { $t = (Get-Content $fxOut -ErrorAction SilentlyContinue | Select-Object -First 1); if ($t -match 'listening (\d+)') { $port = $Matches[1] } }
  }
}

Step 'vision check (real findings + screenshot)' {
  if (-not $visionOk) { Skip-Or-Fail 'Vision worker/browser not available (see previous step)' }
  Assert $port 'fixture server did not start'
  $r = KJson @('vision', 'check', "http://localhost:$port/", '--viewport', '375x812', '--screenshot', '--json') 120
  Assert ($r.Code -eq 0) "exit $($r.Code): $($r.Err)"
  $vp = $r.V.viewports[0]
  Assert (($vp.console_errors -join '|') -match 'fixture console error') "console error not seen: $($vp.console_errors -join '|')"
  Assert (($vp.failed_requests -join '|') -match '404 .*missing\.css') "404 resource not seen: $($vp.failed_requests -join '|')"
  $broken = @($vp.issues | Where-Object { $_.rule -eq 'links.broken' })
  Assert ($broken.Count -eq 1 -and ($broken[0].samples -join '|') -match '404 /broken-link') 'broken link not seen'
  Assert (@($vp.issues | Where-Object { $_.rule -eq 'a11y.img_alt' }).Count -eq 1) 'img without alt not seen'
  Assert ($vp.screenshot -and (Test-Path $vp.screenshot)) 'no screenshot file'
  $len = (Get-Item $vp.screenshot).Length
  Assert ($len -gt 100 -and $len -le 2MB) "screenshot size out of bounds: $len"
  Assert (([IO.File]::ReadAllBytes($vp.screenshot)[1..3] | ForEach-Object { [char]$_ }) -join '' -eq 'PNG') 'screenshot is not a PNG'
  'console error, 404 resource, broken link, missing alt, PNG ' + $len + ' bytes'
}

Step 'vision allowlist (non-local host refused)' {
  if (-not $visionOk) { Skip-Or-Fail 'Vision worker/browser not available' }
  $r = K @('vision', 'check', 'http://example.com/') $null 60
  Assert ($r.Code -ne 0) 'a non-local host must be refused'
  Assert ($r.Err -match 'allowlist|refused') "unexpected message: $($r.Err)"
  'refused: ' + ($r.Err.Trim() -split "`n")[0]
}

Step 'vision time limit (hung page killed, no orphan browser)' {
  if (-not $visionOk) { Skip-Or-Fail 'Vision worker/browser not available' }
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $r = K @('vision', 'check', "http://localhost:$port/slow", '--timeout-secs', '3') $null 90
  $sw.Stop()
  Assert ($r.Code -ne 0) 'a hung page must fail'
  Assert ($sw.Elapsed.TotalSeconds -lt 40) "took $($sw.Elapsed.TotalSeconds)s"
  Start-Sleep -Milliseconds 500
  $orphans = @(Get-CimInstance Win32_Process -Filter "Name='msedge.exe' OR Name='chrome.exe'" | Where-Object { $_.CommandLine -match 'remote-debugging-pipe' -and $_.CommandLine -match [regex]::Escape($env:KEPLER_HOME) -or $_.CommandLine -match 'playwright_chromiumdev_profile' })
  Assert ($orphans.Count -eq 0) "$($orphans.Count) orphan browser process(es)"
  ('stopped after {0:N1}s: {1}' -f $sw.Elapsed.TotalSeconds, ($r.Err.Trim() -split "`n")[0])
}

foreach ($c in $cleanups) { try { & $c } catch {} }

# ------------------------------------------------------------------ orbit
function Orbit-Plan([string]$name, $tasks, [int]$par = 2) {
  $p = Join-Path $work "$name.json"
  Write-Json $p @{ name = $name; max_parallel = $par; tasks = $tasks }
  return $p
}

$nodeCmd = if ($nodeExe) { $nodeExe.Source } else { 'node' }

Step 'orbit run (deps, commit on task branch, approval, resume, cleanup)' {
  if (-not $nodeExe) { throw [SkipException]::new('skipped (not a pass): Node.js needed for the task commands') }
  $repo = New-Repo 'orbit-repo'
  $plan = Orbit-Plan 'acc-orbit' @(
    @{ id = 'a'; command = @($nodeCmd, '-e', "require('fs').writeFileSync('a.txt','from a')") },
    @{ id = 'b'; depends_on = @('a'); command = @($nodeCmd, '-e', "const fs=require('fs');fs.writeFileSync('b.txt',fs.readFileSync('a.txt','utf8')+' + b')") },
    @{ id = 'wipe'; depends_on = @('b'); command = @('git', 'clean', '-fd') }
  )
  $r = KJson @('orbit', 'run', $plan, '--repo', $repo, '--json') 180
  Assert ($r.Code -eq 3) "first run should stop for approval (exit 3), got $($r.Code): $($r.Err)"
  $run = $r.V.run
  $t = @{}; foreach ($x in $r.V.tasks) { $t[$x.id] = $x }
  Assert ($t['a'].status -eq 'done' -and $t['b'].status -eq 'done') 'a and b should be done'
  Assert ($t['wipe'].status -eq 'awaiting_approval') "wipe status=$($t['wipe'].status)"
  Assert ($t['a'].branch -eq "kepler/orbit/$run/a") "branch=$($t['a'].branch)"
  $content = Git $repo @('show', "kepler/orbit/$run/b:b.txt")
  Assert ($content.Trim() -eq 'from a + b') "b.txt=$content (dependency branch not merged?)"
  Assert ((Git $repo @('status', '--porcelain')).Trim() -eq '') 'the user repo must stay untouched'
  Assert ((Git $repo @('rev-parse', 'main')).Trim() -eq (Git $repo @('rev-parse', 'HEAD')).Trim()) 'main moved'
  $ap = K @('orbit', 'approve', $run, 'wipe'); Assert ($ap.Code -eq 0) "approve: $($ap.Err)"
  $r2 = KJson @('orbit', 'resume', $run, '--json') 180
  Assert ($r2.Code -eq 0 -and $r2.V.status -eq 'done') "resume exit $($r2.Code) status=$($r2.V.status)"
  $c = K @('orbit', 'cleanup', $run, '--yes'); Assert ($c.Code -eq 0) "cleanup: $($c.Err)"
  Assert ((Git $repo @('branch', '--list', 'kepler/orbit/*')).Trim() -eq '') 'task branches left after cleanup'
  Assert (@((Git $repo @('worktree', 'list')) -split "`n" | Where-Object { $_.Trim() }).Count -eq 1) 'worktrees left after cleanup'
  "run $run : a,b committed on kepler/orbit/$run/*; wipe waited, approved, resumed; cleanup left nothing"
}

Step 'orbit cancel kills the whole process tree + cleanup after cancel' {
  if (-not $nodeExe) { throw [SkipException]::new('skipped (not a pass): Node.js needed') }
  $repo = New-Repo 'orbit-cancel-repo'
  $mark = 'KMARK' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
  $js = "require('child_process').spawn(process.execPath,['-e','setInterval(()=>{},1000)','$mark'],{stdio:'ignore'});setInterval(()=>{},1000)"
  $plan = Orbit-Plan 'acc-cancel' @(
    @{ id = 'sleeper'; command = @($nodeCmd, '-e', $js) },
    @{ id = 'after'; depends_on = @('sleeper'); command = @('git', 'status') }
  )
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $Kepler; $psi.Arguments = (@('orbit', 'run', $plan, '--repo', $repo, '--json') | ForEach-Object { Quote-Arg $_ }) -join ' '
  $psi.UseShellExecute = $false; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true; $psi.CreateNoWindow = $true
  $ex = [Diagnostics.Process]::Start($psi)
  $so = $ex.StandardOutput.ReadToEndAsync(); $se = $ex.StandardError.ReadToEndAsync()
  function Marked { @(Get-CimInstance Win32_Process -Filter "Name='node.exe'" | Where-Object { $_.CommandLine -match $mark }).Count }
  $started = $false
  for ($i = 0; $i -lt 150; $i++) { if ((Marked) -ge 1) { $started = $true; break }; Start-Sleep -Milliseconds 200 }
  Assert $started 'the grandchild process never started'
  $run = ((K @('orbit', 'list', '--limit', '1')).Out -split ' ')[0]
  Assert ($run -like 'orb-*') "no run id: $run"
  $cn = K @('orbit', 'cancel', $run); Assert ($cn.Code -eq 0) "cancel: $($cn.Err)"
  if (-not $ex.WaitForExit(30000)) { & taskkill /PID $ex.Id /T /F | Out-Null; throw 'executor did not stop after cancel' }
  Start-Sleep -Milliseconds 500
  Assert ((Marked) -eq 0) 'grandchild process survived the cancel (tree not killed)'
  $st = KJson @('orbit', 'status', $run, '--json')
  Assert ($st.V.status -eq 'cancelled') "run status=$($st.V.status)"
  $m = @{}; foreach ($x in $st.V.tasks) { $m[$x.id] = $x.status }
  Assert ($m['sleeper'] -eq 'cancelled' -and $m['after'] -eq 'skipped') "tasks: sleeper=$($m['sleeper']) after=$($m['after'])"
  $c = K @('orbit', 'cleanup', $run, '--yes', '--delete-records'); Assert ($c.Code -eq 0) "cleanup: $($c.Err)"
  Assert ((Git $repo @('branch', '--list', 'kepler/orbit/*')).Trim() -eq '') 'branch leaked after cancel+cleanup'
  Assert (@((Git $repo @('worktree', 'list')) -split "`n" | Where-Object { $_.Trim() }).Count -eq 1) 'worktree leaked after cancel+cleanup'
  'tree killed, run cancelled, dependents skipped, nothing leaked'
}

Step 'orbit agent task (argv, redacted output)' {
  if (-not $nodeExe) { throw [SkipException]::new('skipped (not a pass): Node.js needed for the fake agent') }
  $repo = New-Repo 'orbit-agent-repo'
  $dir = Join-Path $work 'fake-agent'; New-Item -ItemType Directory -Path $dir | Out-Null
  # A stand-in for `claude`: records the argv it got, writes a file, prints a secret-shaped string.
  Set-Content -Path (Join-Path $dir 'agent.js') -Value @(
    "const fs=require('fs');",
    "fs.writeFileSync('agent-argv.json', JSON.stringify(process.argv.slice(2)));",
    "console.log('token ghp_' + 'a1B2'.repeat(9));",
    "console.log('done');"
  )
  Set-Content -Path (Join-Path $dir 'fake-agent.cmd') -Value "@echo off`r`n`"$nodeCmd`" `"%~dp0agent.js`" %*"
  $env:KEPLER_AGENT_BIN = Join-Path $dir 'fake-agent.cmd'
  try {
    $prompt = 'say hi; echo "$(whoami)" & dir'
    $plan = Orbit-Plan 'acc-agent' @(@{ id = 'ag'; agent = $prompt; agent_args = @('--flag', 'x'); timeout_secs = 60 })
    $r = KJson @('orbit', 'run', $plan, '--repo', $repo, '--json') 120
    Assert ($r.Code -eq 0) "exit $($r.Code): $($r.Err)"
    $run = $r.V.run
    $argv = (Git $repo @('show', "kepler/orbit/$run/ag:agent-argv.json")) | ConvertFrom-Json
    Assert ($argv[0] -eq '-p' -and $argv[1] -eq $prompt -and $argv[2] -eq '--flag') "argv=$($argv -join '|')"
    $log = (K @('orbit', 'logs', $run, 'ag')).Out
    Assert ($log -match 'done') 'output not captured'
    Assert ($log -notmatch 'ghp_a1B2') 'secret-shaped output was not redacted'
    K @('orbit', 'cleanup', $run, '--yes') | Out-Null
    'prompt reached the agent as one argv element (no shell); output captured and redacted'
  } finally { Remove-Item Env:\KEPLER_AGENT_BIN -ErrorAction SilentlyContinue }
}

Step 'orbit agent task with real claude (-WithClaude)' {
  if (-not $WithClaude) { throw [SkipException]::new('skipped (not a pass): pass -WithClaude to run it') }
  if (-not (Get-Command claude -ErrorAction SilentlyContinue)) { throw [SkipException]::new('skipped (not a pass): claude not on PATH') }
  $repo = New-Repo 'orbit-claude-repo'
  $plan = Orbit-Plan 'acc-claude' @(@{ id = 'hi'; agent = 'Reply with exactly the single word PONG and do not use any tools.'; timeout_secs = 180 })
  $r = KJson @('orbit', 'run', $plan, '--repo', $repo, '--json') 240
  Assert ($r.Code -eq 0) "exit $($r.Code): $($r.Err)"
  $log = (K @('orbit', 'logs', $r.V.run, 'hi')).Out
  Assert ($log -match 'PONG') "no PONG in output: $log"
  K @('orbit', 'cleanup', $r.V.run, '--yes') | Out-Null
  'claude -p answered PONG in the task worktree'
}

# ------------------------------------------------------------------ Claude Code hook (cold: no daemon running)
function Hook([string]$event, [string]$stdin) {
  K @('hook', 'claude-code', $event) $stdin 60
}

Step 'hook claude-code PreToolUse: rm -rf => ask' {
  $p = @{ session_id = 'acc-1'; cwd = $work; hook_event_name = 'PreToolUse'; tool_name = 'Bash'; tool_input = @{ command = 'rm -rf /' } } | ConvertTo-Json -Depth 5
  $r = Hook 'PreToolUse' $p
  Assert ($r.Code -eq 0) "exit $($r.Code): $($r.Err)"
  $o = $r.Out | ConvertFrom-Json
  Assert ($o.hookSpecificOutput.permissionDecision -eq 'ask') "decision=$($o.hookSpecificOutput.permissionDecision): $($r.Out)"
  'permissionDecision=ask'
}

Step 'hook claude-code PreToolUse: Read .env => deny' {
  $p = @{ session_id = 'acc-1'; cwd = $work; hook_event_name = 'PreToolUse'; tool_name = 'Read'; tool_input = @{ file_path = (Join-Path $work '.env') } } | ConvertTo-Json -Depth 5
  $r = Hook 'PreToolUse' $p
  Assert ($r.Code -eq 0) "exit $($r.Code): $($r.Err)"
  $o = $r.Out | ConvertFrom-Json
  Assert ($o.hookSpecificOutput.permissionDecision -eq 'deny') "decision=$($o.hookSpecificOutput.permissionDecision): $($r.Out)"
  'permissionDecision=deny'
}

Step 'hook claude-code PreToolUse: ls => silence' {
  $p = @{ session_id = 'acc-1'; cwd = $work; hook_event_name = 'PreToolUse'; tool_name = 'Bash'; tool_input = @{ command = 'ls' } } | ConvertTo-Json -Depth 5
  $r = Hook 'PreToolUse' $p
  Assert ($r.Code -eq 0) "exit $($r.Code): $($r.Err)"
  Assert ($r.Out.Trim() -eq '') "expected no output, got: $($r.Out)"
  'exit 0, empty stdout'
}

Step 'hook claude-code: garbage JSON => exit 0' {
  foreach ($ev in 'PreToolUse', 'UserPromptSubmit', 'PostToolUse', 'Stop') {
    $r = Hook $ev 'this is { not json'
    Assert ($r.Code -eq 0) "$ev exit $($r.Code): $($r.Err)"
  }
  'never fatal on invalid input'
}

Step 'hook claude-code --print-settings' {
  $r = K @('hook', 'claude-code', '--print-settings')
  Assert ($r.Code -eq 0) "exit $($r.Code): $($r.Err)"
  $o = $r.Out | ConvertFrom-Json
  Assert ($o.hooks.PreToolUse) "no PreToolUse hook: $($r.Out)"
  'settings block generated'
}

# ------------------------------------------------------------------ sentinel check
Step 'sentinel check (project command, recorded finding)' {
  $repo = New-Repo 'sentinel-check-repo'
  $ok = KJson @('sentinel', 'check', '--repo', $repo, '--cmd', 'git-status=git status', '--json') 120
  Assert ($ok.Code -eq 0) "passing command: exit $($ok.Code): $($ok.Err)"
  $bad = KJson @('sentinel', 'check', '--repo', $repo, '--cmd', 'bad=git no-such-subcommand', '--json') 120
  Assert ($bad.Code -ne 0 -or $bad.V) "failing command produced neither exit code nor report"
  $dn = K @('sentinel', 'check', '--repo', $repo, '--cmd', 'wipe=rm -rf /', '--json') 60
  Assert ($dn.Out -notmatch '"exit_code":\s*0' -or $dn.Code -ne 0) 'a destructive command must not run silently'
  'passing check ok; failing check reported; destructive command gated'
}

# ------------------------------------------------------------------ ui (Genesis local panel)
Step 'ui: starts with a token, refuses without one' {
  $so = Join-Path $work 'ui.out'; $se = Join-Path $work 'ui.err'
  $p = Start-Process -FilePath $Kepler -ArgumentList @('ui', '--json') -NoNewWindow -PassThru -RedirectStandardOutput $so -RedirectStandardError $se
  try {
    $line = $null
    for ($i = 0; $i -lt 50 -and -not $line; $i++) {
      Start-Sleep -Milliseconds 200
      if (Test-Path $so) { $line = Get-Content $so -ErrorAction SilentlyContinue | Select-Object -First 1 }
    }
    Assert $line "ui printed nothing: $(Get-Content $se -Raw -ErrorAction SilentlyContinue)"
    $info = $line | ConvertFrom-Json
    $base = "http://127.0.0.1:$($info.port)"
    $noTok = try { (Invoke-WebRequest -Uri "$base/api/overview" -UseBasicParsing -MaximumRedirection 0).StatusCode } catch { [int]$_.Exception.Response.StatusCode }
    Assert ($noTok -eq 401) "without token expected 401, got $noTok"
    $wrong = try { (Invoke-WebRequest -Uri "$base/?t=wrong" -UseBasicParsing -MaximumRedirection 0).StatusCode } catch { [int]$_.Exception.Response.StatusCode }
    Assert ($wrong -eq 401) "wrong token expected 401, got $wrong"
    $hdr = @{ Authorization = "Bearer $($info.token)" }
    $ov = Invoke-WebRequest -Uri "$base/api/overview" -Headers $hdr -UseBasicParsing
    Assert ($ov.StatusCode -eq 200) "with token expected 200, got $($ov.StatusCode)"
    "401 without/wrong token, 200 with token (port $($info.port))"
  } finally {
    if ($p -and -not $p.HasExited) { & taskkill /PID $p.Id /T /F | Out-Null }
  }
}

# ------------------------------------------------------------------ Forge model assist (fake claude, deterministic)
Step 'forge model assist (fake claude: note, cache, rules-first, breaker, no recursion)' {
  $fake = Join-Path (Split-Path -Parent $Kepler) 'examples\fake_claude.exe'
  if (-not (Test-Path $fake)) {
    $b = Invoke-Proc 'cargo' @('build', '--release', '--example', 'fake_claude', '-p', 'kepler-cli') $null 600 $repoRoot
    if ($b.Code -ne 0 -or -not (Test-Path $fake)) { throw ([SkipException]::new("fake_claude example could not be built: $($b.Err)")) }
  }
  $log = Join-Path $work 'fake-claude.log'
  $saved = @{ B = $env:KEPLER_CLAUDE_BIN; M = $env:KEPLER_FORGE_MODEL; L = $env:FAKE_CLAUDE_LOG; X = $env:FAKE_CLAUDE_MODE; H = $env:FAKE_CLAUDE_HOOK_EXE }
  try {
    $env:KEPLER_CLAUDE_BIN = $fake
    Remove-Item Env:\KEPLER_FORGE_MODEL -ErrorAction SilentlyContinue
    $env:FAKE_CLAUDE_LOG = $log
    $env:FAKE_CLAUDE_MODE = 'task'
    $calls = { if (Test-Path $log) { @(Get-Content $log | Where-Object { $_ -match '"args"' }).Count } else { 0 } }
    $prompt = { param($t) (Hook 'UserPromptSubmit' (@{ session_id = 'acc-fm'; cwd = $work; hook_event_name = 'UserPromptSubmit'; prompt = $t } | ConvertTo-Json -Depth 4)) }

    $r = & $prompt 'Elimina ese gk de todos lados'
    Assert ($r.Code -eq 0) "exit $($r.Code): $($r.Err)"
    Assert ($r.Out -match 'model-inferred') "no model-inferred note: $($r.Out)"
    Assert ((& $calls) -eq 1) "expected 1 model call, got $(& $calls)"
    $null = & $prompt 'elimina ese GK de todos lados'
    Assert ((& $calls) -eq 1) 'cache miss: the same prompt called the model again'
    $d = & $prompt 'Fix the failing test in parser.rs'
    $q = & $prompt 'hubo actividad?'
    Assert ((& $calls) -eq 1) 'a decisive prompt reached the model'
    $e = & $prompt 'Vamos a crear un driver nuevo en lab'
    Assert ($e.Out -match 'inferred' -and $e.Out -notmatch 'model-inferred' -and (& $calls) -eq 1) "conversational task must be handled by rules: $($e.Out)"

    $env:FAKE_CLAUDE_MODE = 'recurse'; $env:FAKE_CLAUDE_HOOK_EXE = $Kepler
    $null = & $prompt 'desinstala gitkraken por favor ya'
    Assert ((& $calls) -eq 2) "recursion: expected exactly 2 fake claude runs in total, got $(& $calls)"
    Assert ((Get-Content $log -Raw) -match '"nested_hook_exit":0') 'nested hook did not exit 0'

    $env:FAKE_CLAUDE_MODE = 'fail'
    $null = & $prompt 'una frase rara numero uno'
    $null = & $prompt 'una frase rara numero dos'
    $dj = (K @('doctor', '--json') $null 60).Out | ConvertFrom-Json
    Assert ($dj.forge_model.breaker.open -eq $true) "breaker not open: $($dj.forge_model | ConvertTo-Json -Depth 4)"
    $n = & $calls
    $null = & $prompt 'una frase rara numero tres'
    Assert ((& $calls) -eq $n) 'breaker open but the model was still called'
    '1 note, cache hit, rules-first (0 calls for decisive/conversational), no recursion, breaker opens after 2 failures'
  } finally {
    foreach ($p in @(@('KEPLER_CLAUDE_BIN', 'B'), @('KEPLER_FORGE_MODEL', 'M'), @('FAKE_CLAUDE_LOG', 'L'), @('FAKE_CLAUDE_MODE', 'X'), @('FAKE_CLAUDE_HOOK_EXE', 'H'))) {
      if ($null -ne $saved[$p[1]]) { Set-Item -Path ("Env:\" + $p[0]) -Value $saved[$p[1]] } else { Remove-Item ("Env:\" + $p[0]) -ErrorAction SilentlyContinue }
    }
  }
}

# ------------------------------------------------------------------ MCP tools (Orbit in the flow)
# One `mcp serve` session per call: initialize, then the given tools/call requests, then EOF.
# Returns one object per call: IsError + Text (+ Json when the text is JSON).
function Mcp-Session($calls, [int]$timeoutSec = 90) {
  $lines = New-Object System.Collections.ArrayList
  [void]$lines.Add('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"acc","version":"0"}}}')
  [void]$lines.Add('{"jsonrpc":"2.0","method":"notifications/initialized"}')
  $i = 10
  foreach ($c in $calls) {
    $i++
    [void]$lines.Add((@{ jsonrpc = '2.0'; id = $i; method = 'tools/call'; params = @{ name = $c.name; arguments = $c.args } } | ConvertTo-Json -Depth 20 -Compress))
  }
  $r = K @('mcp', 'serve') (($lines -join "`n") + "`n") $timeoutSec
  Assert ($r.Code -eq 0) "mcp serve exit $($r.Code): $($r.Err)"
  $replies = @($r.Out -split "`n" | Where-Object { $_.Trim() -ne '' } | ForEach-Object { $_ | ConvertFrom-Json })
  $res = @()
  foreach ($rep in ($replies | Select-Object -Skip 1)) {
    if ($rep.error) { $res += [pscustomobject]@{ IsError = $true; Text = "protocol error: $($rep.error.message)"; Json = $null; Protocol = $true }; continue }
    $t = [string]$rep.result.content[0].text
    $j = $null; try { $j = $t | ConvertFrom-Json } catch {}
    $res += [pscustomobject]@{ IsError = [bool]$rep.result.isError; Text = $t; Json = $j; Protocol = $false }
  }
  return $res
}

Step 'mcp tools: forge enrich, guardian ask/deny, orbit_plan cycle, strict schema' {
  $rs = Mcp-Session @(
    @{ name = 'kepler_forge_analyze'; args = @{ text = 'arregla el bug del login' } },
    @{ name = 'kepler_guardian_check'; args = @{ command = 'rm -rf /' } },
    @{ name = 'kepler_guardian_check'; args = @{ path = '.env' } },
    @{ name = 'kepler_orbit_plan'; args = @{ plan = @{ name = 'loop'; tasks = @(
        @{ id = 'a'; command = @('git', '--version'); depends_on = @('b') },
        @{ id = 'b'; command = @('git', '--version'); depends_on = @('a') }) } } },
    @{ name = 'kepler_orbit_plan'; args = @{ plan = @{ name = 'x'; tasks = @(@{ id = 'a'; command = @('git', '--version'); surprise = 1 }) } } }
  )
  Assert ($rs.Count -eq 5) "expected 5 replies, got $($rs.Count)"
  Assert ($rs[0].Json.mode -eq 'enrich') "forge: $($rs[0].Text)"
  Assert ($rs[1].Json.verdict -eq 'ask') "guardian rm -rf: $($rs[1].Text)"
  Assert ($rs[2].Json.verdict -eq 'deny') "guardian .env: $($rs[2].Text)"
  Assert ($rs[3].IsError -and $rs[3].Text -match 'cycle') "cycle: $($rs[3].Text)"
  Assert ($rs[4].Protocol -and $rs[4].Text -match 'surprise') "schema: $($rs[4].Text)"
  'forge=enrich, rm -rf /=ask, .env=deny, cycle rejected, unknown field rejected by schema'
}

Step 'mcp orbit_run returns at once; run survives the MCP server; approve needs confirm' {
  $repo = New-Repo 'mcp-orbit-repo'
  $dir = Join-Path $work 'mcp-agent'; New-Item -ItemType Directory -Path $dir | Out-Null
  # A stand-in sub-agent (no model): writes a file in its worktree and exits.
  Set-Content -Path (Join-Path $dir 'fake-agent.cmd') -Value "@echo off`r`necho agent was here> agent-out.txt`r`necho done"
  $env:KEPLER_AGENT_BIN = Join-Path $dir 'fake-agent.cmd'
  try {
    $plan = @{ name = 'acc-mcp'; max_parallel = 2; tasks = @(
      @{ id = 'one'; agent = 'first part'; timeout_secs = 60 },
      @{ id = 'two'; agent = 'second part'; timeout_secs = 60 },
      @{ id = 'wipe'; command = @('git', 'clean', '-fd'); depends_on = @('one', 'two') }) }
    $t0 = Get-Date
    $rs = Mcp-Session @(@{ name = 'kepler_orbit_run'; args = @{ plan = $plan; repo = $repo } })
    Assert (-not $rs[0].IsError) "orbit_run: $($rs[0].Text)"
    $run = $rs[0].Json.run
    Assert ($run -match '^orb-') "no run id: $($rs[0].Text)"
    # The MCP server already exited (EOF); the executor must keep going on its own.
    $deadline = (Get-Date).AddSeconds(90); $st = $null
    do {
      Start-Sleep -Milliseconds 500
      $st = (KJson @('orbit', 'status', $run, '--json')).V
    } while ($st.status -notin @('needs_approval', 'done', 'failed') -and (Get-Date) -lt $deadline)
    Assert ($st.status -eq 'needs_approval') "status $($st.status)"
    $wipe = $st.tasks | Where-Object id -eq 'wipe'
    Assert ($wipe.status -eq 'awaiting_approval') "wipe is $($wipe.status)"
    $one = Git $repo @('show', "kepler/orbit/$run/one:agent-out.txt")
    Assert ($one -match 'agent was here') 'agent output not committed on its branch'
    $rs = Mcp-Session @(
      @{ name = 'kepler_orbit_approve'; args = @{ run = $run; task = 'wipe'; confirm = $false } },
      @{ name = 'kepler_orbit_approve'; args = @{ run = $run; task = 'wipe'; confirm = $true } },
      @{ name = 'kepler_orbit_resume'; args = @{ run = $run } })
    Assert ($rs[0].IsError -and $rs[0].Text -match 'explicitly approved') "confirm=false: $($rs[0].Text)"
    Assert ($rs[1].Json.decision -eq 'approved') "approve: $($rs[1].Text)"
    Assert (-not $rs[2].IsError) "resume: $($rs[2].Text)"
    $deadline = (Get-Date).AddSeconds(90)
    do { Start-Sleep -Milliseconds 500; $st = (KJson @('orbit', 'status', $run, '--json')).V } while ($st.status -notin @('done', 'failed') -and (Get-Date) -lt $deadline)
    Assert ($st.status -eq 'done') "final status $($st.status)"
    $rs = Mcp-Session @(
      @{ name = 'kepler_orbit_cleanup'; args = @{ run = $run; confirm = $false } },
      @{ name = 'kepler_orbit_cleanup'; args = @{ run = $run; confirm = $true } })
    Assert ($rs[0].IsError) 'cleanup without confirm must be refused'
    Assert (-not $rs[1].IsError) "cleanup: $($rs[1].Text)"
    Assert (-not (Git $repo @('branch', '--list', 'kepler/orbit/*')).Trim()) 'task branches left behind'
    'run id returned in ' + [int]((Get-Date) - $t0).TotalSeconds + 's; executor outlived the MCP server; 2 agent tasks committed; approve needed confirm; cleanup removed branches'
  } finally { Remove-Item Env:\KEPLER_AGENT_BIN -ErrorAction SilentlyContinue }
}

Step 'mcp via the official MCP SDK for Node (third-party client)' {
  if (-not $nodeExe) { throw [SkipException]::new('skipped (not a pass): Node.js needed') }
  $npm = Get-Command npm.cmd -ErrorAction SilentlyContinue  # npm.ps1 cannot be started as a process
  if (-not $npm) { throw [SkipException]::new('skipped (not a pass): npm needed to install @modelcontextprotocol/sdk') }
  $sdk = Join-Path $work 'sdk'; New-Item -ItemType Directory -Path $sdk | Out-Null
  Set-Content -Path (Join-Path $sdk 'package.json') -Value '{"name":"t","private":true,"type":"module"}' -Encoding ascii
  $i = Invoke-Proc $npm.Source @('install', '@modelcontextprotocol/sdk@1.20.0', '--silent', '--no-audit', '--no-fund') $null 240 $sdk
  if ($i.Code -ne 0) { throw [SkipException]::new("skipped (not a pass): npm install failed (offline?): $($i.Err.Trim())") }
  Copy-Item (Join-Path $PSScriptRoot 'mcp-sdk-check.mjs') $sdk
  $repo = New-Repo 'sdk-repo'
  $r = Invoke-Proc $nodeExe.Source @('mcp-sdk-check.mjs', $Kepler, $repo) $null 180 $sdk
  Assert ($r.Code -eq 0) "sdk check failed: $($r.Out) $($r.Err)"
  $v = $r.Out | ConvertFrom-Json
  $bad = @($v.steps | Where-Object { -not $_.pass })
  Assert ($bad.Count -eq 0) "failed: $(($bad | ForEach-Object name) -join ', ')"
  "$($v.steps.Count) SDK checks passed ($($v.readOnlyTools) read-only + $($v.writeTools) write tools)"
}

Step 'mcp via real claude -p (-WithClaude)' {
  if (-not $WithClaude) { throw [SkipException]::new('skipped (not a pass): pass -WithClaude to run it') }
  if (-not (Get-Command claude -ErrorAction SilentlyContinue)) { throw [SkipException]::new('skipped (not a pass): claude not on PATH') }
  $cfg = Join-Path $work 'claude-mcp.json'
  Write-Json $cfg @{ mcpServers = @{ kepler = @{ command = $Kepler; args = @('mcp', 'serve'); env = @{ KEPLER_HOME = $env:KEPLER_HOME; KEPLER_IPC_NAME = $env:KEPLER_IPC_NAME } } } }
  $prompt = 'Call the kepler MCP tool kepler_guardian_check with command: rm -rf / and tell me its verdict word (allow, ask or deny). Be brief.'
  $r = Invoke-Proc 'claude' @('-p', '--mcp-config', $cfg, '--strict-mcp-config', '--allowedTools', 'mcp__kepler__kepler_guardian_check', '--max-turns', '6') $prompt 240
  Assert ($r.Code -eq 0) "claude exit $($r.Code): $($r.Err)"
  Assert ($r.Out -match '\bask\b') "no verdict in: $($r.Out)"
  'claude -p called kepler_guardian_check through --mcp-config and reported ask'
}

# ------------------------------------------------------------------ report
Start-Sleep -Milliseconds 300
try { Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue } catch {}
Write-Host ''
Write-Host '=== Kepler acceptance ===' -ForegroundColor Cyan
$results | Format-Table -AutoSize -Wrap Step, Status, Detail | Out-String -Width 200 | Write-Host
$fail = @($results | Where-Object Status -eq 'FAIL').Count
$skip = @($results | Where-Object Status -eq 'SKIP').Count
$pass = @($results | Where-Object Status -eq 'PASS').Count
Write-Host "PASS=$pass FAIL=$fail SKIP=$skip"
if ($fail -gt 0) { exit 1 }
exit 0
