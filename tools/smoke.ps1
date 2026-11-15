# Checks a built HomeLink.exe: its size against the budget, `--version` against the version in
# rust/app/Cargo.toml, and `--check-config` on a valid and on a missing settings file. Used by
# CI and by the release workflow.
#
#   pwsh tools/smoke.ps1 -Exe rust/target/release/HomeLink.exe -Config rust/app/config.example.toml -Budget 16777216

param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Config,
    [Parameter(Mandatory)] [long] $Budget
)

$ErrorActionPreference = 'Stop'

# Runs the program with the arguments and returns its exit code and standard output.
function Invoke-HomeLink([string[]] $Arguments) {
    $out = New-TemporaryFile
    $process = Start-Process -FilePath $Exe -ArgumentList $Arguments -Wait -PassThru `
        -NoNewWindow -RedirectStandardOutput $out
    $text = (Get-Content -Raw $out) -as [string]
    Remove-Item $out
    return @{ Code = $process.ExitCode; Text = "$text".Trim() }
}

$size = (Get-Item $Exe).Length
"HomeLink.exe: $size bytes (budget $Budget)"
if ($size -gt $Budget) {
    Write-Error "HomeLink.exe is $size bytes, over the budget of $Budget bytes"
}
if ($env:GITHUB_STEP_SUMMARY) {
    "HomeLink.exe: **$size** bytes (budget $Budget)" | Out-File -Append $env:GITHUB_STEP_SUMMARY
}

$manifest = Join-Path $PSScriptRoot '..\rust\app\Cargo.toml'
$version = (Select-String -Path $manifest -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
$result = Invoke-HomeLink @('--version')
"--version: exit $($result.Code), '$($result.Text)'"
if ($result.Code -ne 0 -or $result.Text -ne "HomeLink $version") {
    Write-Error "--version printed '$($result.Text)' (exit $($result.Code)), expected 'HomeLink $version'"
}

$result = Invoke-HomeLink @('--check-config', '--config', (Resolve-Path $Config).Path)
"--check-config (example): exit $($result.Code), '$($result.Text)'"
if ($result.Code -ne 0 -or -not $result.Text.EndsWith(': valid')) {
    Write-Error "the example settings did not pass --check-config: $($result.Text)"
}

$missing = Join-Path ([System.IO.Path]::GetTempPath()) 'homelink-no-such-config.toml'
$result = Invoke-HomeLink @('--check-config', '--config', $missing)
"--check-config (missing file): exit $($result.Code), '$($result.Text)'"
if ($result.Code -eq 0) {
    Write-Error 'a missing settings file passed --check-config'
}
if (Test-Path $missing) {
    Write-Error '--check-config created the missing settings file'
}
"smoke run passed"
