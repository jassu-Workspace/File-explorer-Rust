Files - a fast, light file explorer for Windows
================================================

Install
-------
1. Unzip this folder anywhere.
2. Right-click install.ps1 and choose "Run with PowerShell".
   Or open PowerShell in this folder and run:

     powershell -ExecutionPolicy Bypass -File install.ps1

   Add -Desktop for a desktop shortcut.
   Add -ContextMenu for "Open in Files" when you right-click a folder.

No admin rights are needed. Files is installed for your account only, in
%LOCALAPPDATA%\Programs\Files, with a Start menu shortcut.

The first time you run it, Windows may say "Windows protected your PC".
The app is not code-signed. Click "More info", then "Run anyway".

Without installing
------------------
fexplorer.exe runs on its own. Copy it anywhere and double-click it.
You can also pass a folder: fexplorer.exe "C:\Projects"

Uninstall
---------
Settings > Apps > Installed apps > Files > Uninstall.
To also delete your tags and view settings, run uninstall.ps1 with
-RemoveSettings from %LOCALAPPDATA%\Programs\Files.

Your data
---------
Tags and per-folder view settings live in %APPDATA%\Files.
Files never connects to the internet.
