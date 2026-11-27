# TREE LINK Windows x64 one-line installer.
# Usage in PowerShell:
#   irm https://raw.githubusercontent.com/cjypetertree-afk/tree-link/main/scripts/install-windows.ps1 | iex
$ErrorActionPreference = 'Stop'
$Repo = 'cjypetertree-afk/tree-link'
$MsiName = 'TREE-LINK-0.3.7-windows-x64.msi'
$Url = "https://github.com/$Repo/releases/latest/download/$MsiName"
$Dest = Join-Path $env:TEMP $MsiName

Write-Host "==> Downloading $MsiName from GitHub Releases..." -ForegroundColor Green
Invoke-WebRequest -Uri $Url -OutFile $Dest -UseBasicParsing

Write-Host "==> Launching TREE LINK Setup..." -ForegroundColor Green
Start-Process msiexec.exe -ArgumentList "/i `"$Dest`"" -Wait
