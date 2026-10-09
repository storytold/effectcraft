<#
.SYNOPSIS
  Install-test the EffectCraft MSI's folder handling with stub binaries (no Rust build).

.DESCRIPTION
  Builds the MSI twice (versions 0.0.1 and 0.0.2) around two stand-in executables, validates both
  (ICE), then, silently and per-machine:
    1. installs 0.0.1 into a folder with spaces and a non-ASCII name (INSTALLFOLDER=...),
    2. upgrades to 0.0.2 without naming a folder: it must stay in that folder,
    3. repairs a deleted file there,
    4. uninstalls: files and the App Paths registration go,
    5. installs 0.0.2 with no folder: it must land in Program Files, then uninstalls.
  The wizard itself needs a person (or the snapshot steps in the PR); this covers what msiexec /qn
  and upgrades do with the folder. Needs an elevated shell and WiX v5 (`wix`) on PATH.
  Run by .github/workflows/packaging-lint.yml.

.EXAMPLE
  pwsh packaging/windows/test-msi.ps1
#>
param([string] $Work = (Join-Path ([IO.Path]::GetTempPath()) 'effectcraft-msi-test'))
$ErrorActionPreference = 'Stop'
trap { Write-Output "FAILED: $_"; exit 1 }
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$AppPaths = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\effectcraft.exe'

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}
function Invoke-Msiexec([string] $What, [string[]] $Arguments) {
  $log = Join-Path $Work "$($What -replace '\W', '-').log"
  $p = Start-Process msiexec.exe -ArgumentList ($Arguments + '/qn', '/l*v', "`"$log`"") -Wait -PassThru
  if ($p.ExitCode -ne 0) { Get-Content $log -Tail 60; throw "msiexec ($What) exited $($p.ExitCode)" }
  Write-Output "ok $What"
}
function Assert-InstalledIn([string] $Dir) {
  foreach ($exe in 'effectcraft.exe', 'effectcraft-cli.exe') {
    if (-not (Test-Path -LiteralPath (Join-Path $Dir $exe))) { throw "$exe is not in $Dir" }
  }
  $key = Get-ItemProperty -LiteralPath $AppPaths
  $want = Join-Path $Dir 'effectcraft.exe'
  if ($key.'(default)' -ne $want) { throw "App Paths points at '$($key.'(default)')', expected '$want'" }
  if ($key.Path.TrimEnd('\') -ne $Dir.TrimEnd('\')) { throw "App Paths Path is '$($key.Path)', expected '$Dir'" }
  Write-Output "ok installed in $Dir"
}

Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
$Bin = Join-Path $Work 'bin'
New-Item -ItemType Directory -Force -Path $Bin | Out-Null
# Any GUI and console executables will do: the MSI only copies them.
Copy-Item (Join-Path $env:SystemRoot 'System32\notepad.exe') (Join-Path $Bin 'effectcraft.exe')
Copy-Item (Join-Path $env:SystemRoot 'System32\whoami.exe') (Join-Path $Bin 'effectcraft-cli.exe')

$Msi = @{}
foreach ($v in '0.0.1', '0.0.2') {
  $Msi[$v] = Join-Path $Work "effectcraft-$v.msi"
  Invoke-Native "wix build $v" {
    wix build (Join-Path $PSScriptRoot 'effectcraft.wxs') (Join-Path $PSScriptRoot 'installer-ui.wxs') -arch x64 `
      -d "Version=$v" -d "BinDir=$Bin" -d "IconPath=$(Join-Path $Root 'assets\app-icon\effectcraft.ico')" -o $Msi[$v]
  }
  Invoke-Native "wix msi validate $v" { wix msi validate $Msi[$v] }
}

# Spaces and a non-ASCII letter (written as a char so the script's own encoding doesn't matter).
$Custom = Join-Path $env:SystemDrive "EffectCraft Test $([char]0x00DC)nicode\My Apps"
$Default = Join-Path $env:ProgramFiles 'EffectCraft'
if (Test-Path -LiteralPath $AppPaths) { throw 'EffectCraft is already installed here; run this on a clean machine' }

Invoke-Msiexec 'install to a custom folder' @('/i', "`"$($Msi['0.0.1'])`"", "INSTALLFOLDER=`"$Custom`"")
Assert-InstalledIn $Custom
if (Test-Path -LiteralPath $Default) { throw "a custom install also wrote $Default" }

Invoke-Msiexec 'upgrade without a folder' @('/i', "`"$($Msi['0.0.2'])`"")
Assert-InstalledIn $Custom
# Read single values: Get-ItemProperty throws on some machines' malformed Uninstall entries.
$arp = @(Get-ChildItem 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall' |
  ForEach-Object { if ($_.GetValue('DisplayName') -eq 'EffectCraft') { $_.GetValue('DisplayVersion') } })
if ($arp.Count -ne 1 -or $arp[0] -ne '0.0.2') { throw "expected one EffectCraft 0.0.2 in Apps, found: $($arp -join ', ')" }

Remove-Item -LiteralPath (Join-Path $Custom 'effectcraft-cli.exe')
Invoke-Msiexec 'repair' @('/fa', "`"$($Msi['0.0.2'])`"")
Assert-InstalledIn $Custom

Invoke-Msiexec 'uninstall the custom install' @('/x', "`"$($Msi['0.0.2'])`"")
if (Test-Path -LiteralPath (Join-Path $Custom 'effectcraft.exe')) { throw 'uninstall left effectcraft.exe behind' }
if (Test-Path -LiteralPath $AppPaths) { throw 'uninstall left the App Paths registration behind' }

Invoke-Msiexec 'install to the default folder' @('/i', "`"$($Msi['0.0.2'])`"")
Assert-InstalledIn $Default
Invoke-Msiexec 'uninstall the default install' @('/x', "`"$($Msi['0.0.2'])`"")
if (Test-Path -LiteralPath (Join-Path $Default 'effectcraft.exe')) { throw 'uninstall left effectcraft.exe behind' }
Write-Output 'MSI folder handling ok'
