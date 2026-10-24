$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$root = Join-Path ([System.IO.Path]::GetTempPath()) ('effectcraft-updater-test-' + [guid]::NewGuid())
$installer = Join-Path $PSScriptRoot '../../apps/effectcraft/src/update/install.ps1'
function Assert([bool] $Ok, [string] $Message) { if (-not $Ok) { throw $Message } }
function Text([string] $Path) { return [System.IO.File]::ReadAllText($Path) }
function Package([string] $Name, [bool] $Missing = $false) {
    $source = Join-Path $root $Name
    $folder = Join-Path $source 'effectcraft-99.0.0-windows-x64-portable'
    New-Item -ItemType Directory -Force -Path $folder | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $folder 'effectcraft.exe'), 'new app')
    if (-not $Missing) { [System.IO.File]::WriteAllText((Join-Path $folder 'effectcraft-cli.exe'), 'new cli') }
    [System.IO.File]::WriteAllText((Join-Path $folder 'portable.txt'), 'portable')
    $zip = Join-Path $root ($Name + '.zip')
    [System.IO.Compression.ZipFile]::CreateFromDirectory($source, $zip)
    return $zip
}
try {
    New-Item -ItemType Directory -Path $root | Out-Null
    $installed = Join-Path $root "O'Brien portable app"
    New-Item -ItemType Directory -Path (Join-Path $installed 'EffectCraftData') -Force | Out-Null
    $exe = Join-Path $installed 'effectcraft.exe'
    $cli = Join-Path $installed 'effectcraft-cli.exe'
    $settings = Join-Path $installed 'EffectCraftData/preferences.json'
    [System.IO.File]::WriteAllText($exe, 'old app')
    [System.IO.File]::WriteAllText($cli, 'old cli')
    [System.IO.File]::WriteAllText($settings, 'my settings')
    $good = Package 'good'
    $missing = Package 'missing' $true
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $installer $missing $exe portable
    Assert ($LASTEXITCODE -ne 0) 'Incomplete update was accepted.'
    Assert ((Text $exe) -eq 'old app') 'Incomplete update changed the app.'
    $held = [System.IO.File]::Open($cli, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
    try {
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $installer $good $exe portable
        Assert ($LASTEXITCODE -ne 0) 'Locked CLI was overwritten.'
        Assert ((Text $exe) -eq 'old app') 'A failed update did not restore the app.'
    } finally { $held.Dispose() }
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $installer $good ('\\?\' + $exe) portable
    Assert ($LASTEXITCODE -eq 0) 'Portable update failed.'
    Assert ((Text $exe) -eq 'new app') 'App was not updated.'
    Assert ((Text $cli) -eq 'new cli') 'CLI was not updated.'
    Assert ((Text $settings) -eq 'my settings') 'Portable settings changed.'
    Write-Output 'Portable update, save preservation, rejected package, and rollback passed.'
} finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
