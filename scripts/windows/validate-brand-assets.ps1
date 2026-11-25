param()
$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$iconPath = Join-Path $repoRoot "src-tauri/icons/icon.ico"
$configPath = Join-Path $repoRoot "src-tauri/tauri.conf.json"
if (-not (Test-Path -LiteralPath $iconPath -PathType Leaf)) { throw "WebDynamite Windows icon is missing: $iconPath" }
$bytes = [System.IO.File]::ReadAllBytes($iconPath)
if ($bytes.Length -lt 22 -or $bytes[0] -ne 0 -or $bytes[1] -ne 0 -or $bytes[2] -ne 1 -or $bytes[3] -ne 0) { throw "Windows icon has an invalid ICO header." }
$imageCount = [BitConverter]::ToUInt16($bytes, 4)
if ($imageCount -lt 2) { throw "Windows icon must include multiple resolutions." }
$config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
if ($config.bundle.icon -notcontains "icons/icon.ico") { throw "Tauri bundle configuration does not include icons/icon.ico." }
Write-Host "Brand asset validation passed: $imageCount ICO images; Tauri bundle references icons/icon.ico."
