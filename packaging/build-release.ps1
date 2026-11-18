# Builds a release and packs dist\Files-<version>-x64.zip (exe, install/uninstall scripts, README).
# Run from the repository root:  powershell -ExecutionPolicy Bypass -File packaging\build-release.ps1
$ErrorActionPreference = 'Stop'

# A separate target folder, so a copy of the app you are running from target\release does not block the build.
cargo build --release --target-dir target\dist
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$exe = Resolve-Path 'target\dist\release\fexplorer.exe'
$version = (Get-Item $exe).VersionInfo.ProductVersion
$name = "Files-$version-x64"
$stage = Join-Path 'dist' $name
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe, 'packaging\install.ps1', 'packaging\uninstall.ps1', 'packaging\README.txt' -Destination $stage
$zip = Join-Path 'dist' "$name.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path "$stage\*" -DestinationPath $zip
"Built $zip"
