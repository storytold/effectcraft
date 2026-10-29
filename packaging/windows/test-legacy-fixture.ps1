<# Exercise the legacy source transformation without building or installing. #>
param([Parameter(Mandatory = $true)] [string] $Work)
$ErrorActionPreference = 'Stop'
[void](New-Item -ItemType Directory -Force -Path $Work)
$source = Get-Content -Raw (Join-Path $PSScriptRoot 'test-msi.ps1')
$start = $source.IndexOf('$Wxs =')
$end = $source.IndexOf('$Msi = @{}', $start)
if ($start -lt 0 -or $end -le $start) { throw 'Could not locate the legacy source transformation.' }
# Run the test harness's exact source-producing block, with only scratch outputs.
$block = $source.Substring($start, $end - $start).Replace('$PSScriptRoot', ("'" + $PSScriptRoot.Replace("'", "''") + "'"))
Invoke-Expression $block
[xml]$legacy = Get-Content -Raw $LegacyWxs
$ns = New-Object Xml.XmlNamespaceManager($legacy.NameTable)
$ns.AddNamespace('w', 'http://wixtoolset.org/schemas/v4/wxs')
if ($legacy.SelectNodes('//w:RegistryValue[@Name="Path" or @Name="InstallDir" or @Name="InstallFolder"]', $ns).Count) {
  throw 'Legacy fixture must omit every modern folder record so discovery uses the executable fallback.'
}
if (-not $legacy.SelectNodes('//w:RegistryValue[contains(@Value,"EffectcraftExe")]', $ns).Count) {
  throw 'Legacy fixture must retain its executable registration.'
}
Write-Output 'PASS legacy fixture exposes only the executable-path fallback.'
