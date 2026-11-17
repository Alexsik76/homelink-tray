# Builds HomeLink.msi from a built HomeLink.exe with the WiX Toolset, then installs it quietly,
# checks the installed program and uninstalls it again. Used by CI.
#
#   pwsh tools/msi.ps1 -Exe rust/target/release/HomeLink.exe -Out rust/target/release/HomeLink.msi

param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Out
)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')

# The WiX Toolset, pinned.
dotnet tool install --global wix --version 5.0.2 | Out-Null
$env:PATH += ";$env:USERPROFILE\.dotnet\tools"

# MSI versions are numbers only: 1.0.0-rc.11 becomes 1.0.0.11, a release 1.0.0 becomes 1.0.0.0.
# Windows compares the first three numbers; the package allows the same version to replace the
# installed one, so a release candidate or the release replaces an earlier candidate.
$manifest = Join-Path $root 'rust\app\Cargo.toml'
$version = (Select-String -Path $manifest -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if ($version -notmatch '^(\d+)\.(\d+)\.(\d+)(?:-rc\.(\d+))?$') {
    Write-Error "cannot turn the version '$version' into an MSI version"
}
$candidate = if ($Matches[4]) { $Matches[4] } else { '0' }
$msiVersion = "$($Matches[1]).$($Matches[2]).$($Matches[3]).$candidate"
"HomeLink $version -> MSI version $msiVersion"

wix build (Join-Path $root 'rust\app\windows\HomeLink.wxs') -arch x64 `
    -d "Version=$msiVersion" `
    -d "Exe=$(Resolve-Path $Exe)" `
    -d "Notices=$(Join-Path $root 'THIRD-PARTY-NOTICES.txt')" `
    -d "Icon=$(Join-Path $root 'rust\app\windows\app.ico')" `
    -o $Out
if ($LASTEXITCODE -ne 0) { Write-Error "wix build failed ($LASTEXITCODE)" }
"HomeLink.msi: $((Get-Item $Out).Length) bytes"

# Install quietly, check, uninstall.
$msi = (Resolve-Path $Out).Path
$installed = Join-Path $env:ProgramFiles 'HomeLink\HomeLink.exe'
$log = Join-Path ([System.IO.Path]::GetTempPath()) 'homelink-msi.log'
$p = Start-Process msiexec -ArgumentList @('/i', "`"$msi`"", '/qn', '/l*v', "`"$log`"") -Wait -PassThru
if ($p.ExitCode -ne 0) {
    Get-Content $log -Tail 40
    Write-Error "msiexec /i ended with $($p.ExitCode)"
}
if (-not (Test-Path $installed)) { Write-Error "the installer did not put HomeLink.exe in $installed" }
if ((Get-FileHash $installed).Hash -ne (Get-FileHash $Exe).Hash) {
    Write-Error 'the installed HomeLink.exe differs from the built one'
}
"installed: $installed"
$p = Start-Process msiexec -ArgumentList @('/x', "`"$msi`"", '/qn') -Wait -PassThru
if ($p.ExitCode -ne 0) { Write-Error "msiexec /x ended with $($p.ExitCode)" }
if (Test-Path $installed) { Write-Error 'the uninstall left HomeLink.exe behind' }
"uninstalled; msi check passed"
