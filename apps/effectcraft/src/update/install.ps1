param([string] $Package, [string] $Exe, [string] $Kind)
$ErrorActionPreference = 'Stop'
function Normal-Path([string] $Path) {
    if ($Path.StartsWith('\\?\UNC\', [StringComparison]::OrdinalIgnoreCase)) { return '\\' + $Path.Substring(8) }
    if ($Path.StartsWith('\\?\')) { return $Path.Substring(4) }
    return $Path
}
$Package = Normal-Path $Package
$Exe = Normal-Path $Exe
try {
    if ($Kind -eq 'msi') {
        $arguments = '/i "' + $Package + '" /passive /norestart REBOOT=ReallySuppress MSIRESTARTMANAGERCONTROL=Disable'
        $process = Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\msiexec.exe') -ArgumentList $arguments -Verb RunAs -Wait -PassThru
        if ($process.ExitCode -notin @(0, 3010)) { throw "Windows Installer returned $($process.ExitCode)." }
        exit 0
    }
    $dir = [System.IO.Path]::GetDirectoryName($Exe)
    $stage = Join-Path $dir ('.effectcraft-update-' + [guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $stage | Out-Null
    $backup = Join-Path $stage 'backup'
    New-Item -ItemType Directory -Path $backup | Out-Null
    $changed = @()
    try {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $zip = [System.IO.Compression.ZipFile]::OpenRead($Package)
        try {
            $files = @('effectcraft.exe', 'effectcraft-cli.exe', 'portable.txt', 'README.md', 'LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE')
            $total = [long] 0
            $seen = @{}
            foreach ($entry in $zip.Entries) {
                $parts = $entry.FullName.Replace('\', '/').Split('/')
                if ($parts.Length -eq 2 -and $parts[1] -eq '') { continue }
                if ($parts.Length -ne 2 -or $parts[0] -notmatch '^effectcraft-[0-9A-Za-z.+-]+-windows-(x64|x86|arm64)-portable$' -or $parts[1] -notin $files -or $seen.ContainsKey($parts[1])) { throw 'Unexpected or duplicate file in the portable update.' }
                $total += $entry.Length
                if ($total -gt 4294967296) { throw 'Portable update is too large.' }
                $seen[$parts[1]] = $true
                [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $stage $parts[1]))
            }
        } finally { $zip.Dispose() }
        foreach ($required in @('effectcraft.exe', 'effectcraft-cli.exe', 'portable.txt')) {
            if (-not (Test-Path -LiteralPath (Join-Path $stage $required))) { throw "The update is missing $required." }
        }
        foreach ($name in $files) {
            $new = Join-Path $stage $name
            if (-not (Test-Path -LiteralPath $new)) { continue }
            $dest = Join-Path $dir $name
            if (Test-Path -LiteralPath $dest) { Move-Item -LiteralPath $dest -Destination (Join-Path $backup $name) }
            $changed += $name
            Move-Item -LiteralPath $new -Destination $dest
        }
    } catch {
        $failure = $_
        $restoreErrors = @()
        [array]::Reverse($changed)
        foreach ($name in $changed) {
            try {
                $saved = Join-Path $backup $name
                $dest = Join-Path $dir $name
                if (Test-Path -LiteralPath $dest) { Remove-Item -LiteralPath $dest }
                if (Test-Path -LiteralPath $saved) { Move-Item -LiteralPath $saved -Destination $dest }
            } catch { $restoreErrors += $_.ToString() }
        }
        if ($restoreErrors.Count -gt 0) { throw "Update failed. Recovery files remain in $backup. $($restoreErrors -join "; ")" }
        Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
        throw $failure
    }
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
} catch {
    [Console]::Error.WriteLine($_.ToString())
    exit 1
}
