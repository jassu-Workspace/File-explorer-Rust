# Removes Files: shortcuts, the Settings > Apps entry, the "Open in Files" menu items and the program folder.
#   powershell -ExecutionPolicy Bypass -File uninstall.ps1 [-RemoveSettings]
# -RemoveSettings  also delete your tags and view settings (%APPDATA%\Files)
param([switch]$RemoveSettings)
$ErrorActionPreference = 'Stop'

$dir = $PSScriptRoot
$infoFile = Join-Path $dir 'install-info.txt'
if (-not (Test-Path $infoFile)) { throw "install-info.txt not found in $dir; is this an installed copy of Files?" }
$exe = Join-Path $dir 'fexplorer.exe'
if (Get-Process fexplorer -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }) {
    throw 'Files is running. Close it, then run the uninstaller again.'
}

foreach ($line in Get-Content $infoFile) {
    $kind, $value = $line -split '=', 2
    switch ($kind) {
        { $_ -in 'startmenu', 'desktop' } { Remove-Item -LiteralPath $value -Force -ErrorAction SilentlyContinue }
        { $_ -in 'uninstallkey', 'menukey' } { Remove-Item -LiteralPath $value -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

if ($RemoveSettings) {
    Remove-Item -LiteralPath (Join-Path $env:APPDATA 'Files') -Recurse -Force -ErrorAction SilentlyContinue
}

# Leave the folder before deleting it (this script lives there).
Set-Location $env:TEMP
Remove-Item -LiteralPath $dir -Recurse -Force
'Files has been uninstalled.'
