<#
.SYNOPSIS
  Install-test the EffectCraft MSI's folder handling with stub binaries (no Rust build).

.DESCRIPTION
  Builds the MSI around two stand-in executables as 0.0.1 the way releases before this installer
  change are (no App Paths "Path" value), then as 0.0.2 and 0.0.3, validates them (ICE), and, silently and per-machine:
    1. installs 0.0.1 into a folder with spaces and a non-ASCII name (INSTALLFOLDER=...),
    2. upgrades to 0.0.2 without naming a folder: it must stay in that folder (found from the
       installed effectcraft.exe, as there is no Path value yet),
    3. upgrades to 0.0.3 the same way (found from the Path value 0.0.2 wrote),
    4. repairs a damaged (unversioned) CLI there with the wizard's Repair mode,
    5. uninstalls: files and the App Paths registration go,
    6. installs 0.0.3 with no folder: it must land in Program Files, then uninstalls.
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
function Assert-InstalledIn([string] $Dir, [switch] $NoPathValue) {
  foreach ($exe in 'effectcraft.exe', 'effectcraft-cli.exe') {
    if (-not (Test-Path -LiteralPath (Join-Path $Dir $exe))) { throw "$exe is not in $Dir" }
  }
  $key = Get-ItemProperty -LiteralPath $AppPaths
  $want = Join-Path $Dir 'effectcraft.exe'
  if ($key.'(default)' -ne $want) { throw "App Paths points at '$($key.'(default)')', expected '$want'" }
  if ($NoPathValue) {
    if ($null -ne $key.Path) { throw "the legacy package wrote an App Paths Path value" }
  } elseif ($key.Path.TrimEnd('\') -ne $Dir.TrimEnd('\')) {
    throw "App Paths Path is '$($key.Path)', expected '$Dir'"
  }
  Write-Output "ok installed in $Dir"
}

Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
$Bin = Join-Path $Work 'bin'
New-Item -ItemType Directory -Force -Path $Bin | Out-Null
# The MSI only copies these. The app is versioned (it embeds VERSIONINFO); the real CLI has no
# version resource, so its stand-in is an unversioned file too: Windows Installer keeps a changed
# unversioned file on repair unless the repair mode forces it.
Copy-Item (Join-Path $env:SystemRoot 'System32\notepad.exe') (Join-Path $Bin 'effectcraft.exe')
[IO.File]::WriteAllText((Join-Path $Bin 'effectcraft-cli.exe'), 'effectcraft-cli stand-in')

# 0.0.1 stands in for releases before this installer change: the same package without the App
# Paths "Path" value.
$Wxs = Join-Path $PSScriptRoot 'effectcraft.wxs'
$LegacyWxs = Join-Path $Work 'effectcraft-legacy.wxs'
$pathValue = '(?m)^.*<RegistryValue [^>]*Name="Path"[^>]*/>\r?\n'
$text = Get-Content -Raw $Wxs
if ($text -notmatch $pathValue) { throw "no App Paths Path value in $Wxs to leave out" }
[IO.File]::WriteAllText($LegacyWxs, ($text -replace $pathValue, ''))

$Msi = @{}
foreach ($v in '0.0.1', '0.0.2', '0.0.3') {
  $Msi[$v] = Join-Path $Work "effectcraft-$v.msi"
  $src = if ($v -eq '0.0.1') { $LegacyWxs } else { $Wxs }
  Invoke-Native "wix build $v" {
    wix build $src (Join-Path $PSScriptRoot 'installer-ui.wxs') -arch x64 `
      -d "Version=$v" -d "BinDir=$Bin" -d "IconPath=$(Join-Path $Root 'assets\app-icon\effectcraft.ico')" -o $Msi[$v]
  }
  Invoke-Native "wix msi validate $v" { wix msi validate $Msi[$v] }
}

# Spaces and a non-ASCII letter (written as a char so the script's own encoding doesn't matter).
$Custom = Join-Path $env:SystemDrive "EffectCraft Test $([char]0x00DC)nicode\My Apps"
$Default = Join-Path $env:ProgramFiles 'EffectCraft'
if (Test-Path -LiteralPath $AppPaths) { throw 'EffectCraft is already installed here; run this on a clean machine' }

Invoke-Msiexec 'install the legacy package to a custom folder' @('/i', "`"$($Msi['0.0.1'])`"", "INSTALLFOLDER=`"$Custom`"")
Assert-InstalledIn $Custom -NoPathValue
if (Test-Path -LiteralPath $Default) { throw "a custom install also wrote $Default" }

Invoke-Msiexec 'upgrade the legacy install without a folder' @('/i', "`"$($Msi['0.0.2'])`"")
Assert-InstalledIn $Custom
if (Test-Path -LiteralPath $Default) { throw "the upgrade moved the install to $Default" }

Invoke-Msiexec 'upgrade again without a folder' @('/i', "`"$($Msi['0.0.3'])`"")
Assert-InstalledIn $Custom
# Read single values: Get-ItemProperty throws on some machines' malformed Uninstall entries.
$arp = @(Get-ChildItem 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall' |
  ForEach-Object { if ($_.GetValue('DisplayName') -eq 'EffectCraft') { $_.GetValue('DisplayVersion') } })
if ($arp.Count -ne 1 -or $arp[0] -ne '0.0.3') { throw "expected one EffectCraft 0.0.3 in Apps, found: $($arp -join ', ')" }

# Repair the way the wizard's Repair button does: damage the CLI in place (a later write time, as a
# user edit or corruption would have), repair with the wizard's REINSTALLMODE, compare contents.
$mode = [regex]::Match((Get-Content -Raw (Join-Path $PSScriptRoot 'installer-ui.wxs')), 'Event="ReinstallMode" Value="([^"]+)"').Groups[1].Value
if (-not $mode) { throw 'no ReinstallMode in installer-ui.wxs' }
$cli = Join-Path $Custom 'effectcraft-cli.exe'
Start-Sleep -Seconds 2
[IO.File]::WriteAllText($cli, 'damaged')
Invoke-Msiexec "repair ($mode)" @('/i', "`"$($Msi['0.0.3'])`"", 'REINSTALL=ALL', "REINSTALLMODE=$mode")
Assert-InstalledIn $Custom
if ((Get-FileHash -LiteralPath $cli).Hash -ne (Get-FileHash -LiteralPath (Join-Path $Bin 'effectcraft-cli.exe')).Hash) {
  throw "repair ($mode) left the damaged effectcraft-cli.exe in place"
}
Write-Output "ok repair ($mode) restored effectcraft-cli.exe"

Invoke-Msiexec 'uninstall the custom install' @('/x', "`"$($Msi['0.0.3'])`"")
if (Test-Path -LiteralPath (Join-Path $Custom 'effectcraft.exe')) { throw 'uninstall left effectcraft.exe behind' }
if (Test-Path -LiteralPath $AppPaths) { throw 'uninstall left the App Paths registration behind' }

Invoke-Msiexec 'install to the default folder' @('/i', "`"$($Msi['0.0.3'])`"")
Assert-InstalledIn $Default
Invoke-Msiexec 'uninstall the default install' @('/x', "`"$($Msi['0.0.3'])`"")
if (Test-Path -LiteralPath (Join-Path $Default 'effectcraft.exe')) { throw 'uninstall left effectcraft.exe behind' }
Write-Output 'MSI folder handling ok'
