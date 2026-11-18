# Installs Files for the current user (no admin rights needed).
#   powershell -ExecutionPolicy Bypass -File install.ps1 [-Desktop] [-ContextMenu]
# -Desktop      also put a shortcut on the desktop
# -ContextMenu  add "Open in Files" when you right-click a folder, a drive, or empty space in a folder
# The other parameters only exist so the script can be tested without touching the real Start menu or registry.
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\Files'),
    [string]$StartMenuDir = (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'),
    [string]$DesktopDir = [Environment]::GetFolderPath('Desktop'),
    [string]$RegistryBase = 'HKCU:\Software',
    [switch]$Desktop,
    [switch]$ContextMenu
)
$ErrorActionPreference = 'Stop'

$src = Join-Path $PSScriptRoot 'fexplorer.exe'
if (-not (Test-Path $src)) { throw "fexplorer.exe was not found next to install.ps1 ($PSScriptRoot)." }
$running = Get-Process fexplorer -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $InstallDir 'fexplorer.exe') }
if ($running) { throw 'Files is running. Close it, then run the installer again.' }

New-Item -ItemType Directory -Force $InstallDir | Out-Null
Copy-Item $src, (Join-Path $PSScriptRoot 'uninstall.ps1') -Destination $InstallDir -Force
$exe = Join-Path $InstallDir 'fexplorer.exe'
$version = (Get-Item $exe).VersionInfo.ProductVersion

$shell = New-Object -ComObject WScript.Shell
function New-Shortcut([string]$dir) {
    New-Item -ItemType Directory -Force $dir | Out-Null
    $lnk = $shell.CreateShortcut((Join-Path $dir 'Files.lnk'))
    $lnk.TargetPath = $exe
    $lnk.IconLocation = "$exe,0"
    $lnk.Description = 'Files'
    $lnk.Save()
}
New-Shortcut $StartMenuDir
if ($Desktop) { New-Shortcut $DesktopDir }

# Entry in Settings > Apps > Installed apps.
$uninstallKey = Join-Path $RegistryBase 'Microsoft\Windows\CurrentVersion\Uninstall\Files'
New-Item -Force $uninstallKey | Out-Null
$sizeKb = [int]((Get-Item $exe).Length / 1KB)
$values = @{
    DisplayName     = 'Files'
    DisplayVersion  = "$version"
    DisplayIcon     = $exe
    InstallLocation = $InstallDir
    UninstallString = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$(Join-Path $InstallDir 'uninstall.ps1')`""
}
foreach ($k in $values.Keys) { Set-ItemProperty $uninstallKey -Name $k -Value $values[$k] }
Set-ItemProperty $uninstallKey -Name NoModify -Value 1 -Type DWord
Set-ItemProperty $uninstallKey -Name NoRepair -Value 1 -Type DWord
Set-ItemProperty $uninstallKey -Name EstimatedSize -Value $sizeKb -Type DWord

$menuKeys = @()
if ($ContextMenu) {
    # %1 is the clicked folder or drive; %V is the open folder when right-clicking empty space.
    foreach ($entry in @(@('Directory\shell\OpenInFiles', '%1'), @('Drive\shell\OpenInFiles', '%1'), @('Directory\Background\shell\OpenInFiles', '%V'))) {
        $key = Join-Path $RegistryBase "Classes\$($entry[0])"
        New-Item -Force $key | Out-Null
        Set-ItemProperty $key -Name '(default)' -Value 'Open in Files'
        Set-ItemProperty $key -Name Icon -Value $exe
        New-Item -Force (Join-Path $key 'command') | Out-Null
        Set-ItemProperty (Join-Path $key 'command') -Name '(default)' -Value "`"$exe`" `"$($entry[1])`""
        $menuKeys += $key
    }
}

# What uninstall.ps1 must remove, one item per line.
$info = @("startmenu=$(Join-Path $StartMenuDir 'Files.lnk')", "uninstallkey=$uninstallKey")
if ($Desktop) { $info += "desktop=$(Join-Path $DesktopDir 'Files.lnk')" }
$info += $menuKeys | ForEach-Object { "menukey=$_" }
Set-Content -Path (Join-Path $InstallDir 'install-info.txt') -Value $info -Encoding UTF8

"Installed Files $version to $InstallDir"
'Open it from the Start menu (search "Files").'
