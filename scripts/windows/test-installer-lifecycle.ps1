[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })]
    [string]$InstallerPath
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Assert-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "Installer lifecycle acceptance requires an elevated Windows runner."
    }
}
function Invoke-Installer([string]$Path, [string]$Arguments, [string]$Operation) {
    Write-Host "Running $Operation..."
    $process = Start-Process -FilePath $Path -ArgumentList $Arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "$Operation failed with exit code $($process.ExitCode)." }
}
function Wait-ForPath([string]$Path, [bool]$ShouldExist, [int]$TimeoutSeconds = 60) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        if ((Test-Path -LiteralPath $Path) -eq $ShouldExist) { return }
        Start-Sleep -Milliseconds 500
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($ShouldExist) { throw "Expected installer output was not created: $Path" }
    throw "Installer output still exists after uninstall: $Path"
}

Assert-Administrator
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw "This acceptance test must run on Windows."
}
$installer = (Resolve-Path -LiteralPath $InstallerPath).Path
$testId = [guid]::NewGuid().ToString("N")
$installRoot = Join-Path $env:ProgramFiles "WebDynamite-Lifecycle-$testId"
$dataRoot = Join-Path (Join-Path $env:ProgramData "WebDynamite\LifecycleAcceptance") $testId
$markerPath = Join-Path $dataRoot "user-data-preservation.marker"
$desktopPath = Join-Path $installRoot "WebDynamite.exe"
$uninstallVerified = $false

try {
    if (Test-Path -LiteralPath $installRoot) { throw "The isolated installer destination already exists: $installRoot" }
    $service = Get-CimInstance -ClassName Win32_Service -Filter "Name='WebDynamiteCore'"
    if ($service) { throw "A WebDynamiteCore service already exists on this runner; refusing to touch an existing installation." }

    New-Item -ItemType Directory -Path $dataRoot -Force | Out-Null
    Set-Content -LiteralPath $markerPath -Value "preserve-$testId" -NoNewline -Encoding ascii

    # NSIS requires /D= to be the final argument. The destination must remain under Program Files
    # because the production service installer enforces that trust boundary.
    Invoke-Installer -Path $installer -Arguments "/S /D=$installRoot" -Operation "Clean silent install"
    Wait-ForPath -Path $desktopPath -ShouldExist $true
    if (-not (Test-Path -LiteralPath (Join-Path $installRoot "resources\install-core-service.ps1") -PathType Leaf)) {
        throw "The installed service lifecycle resource is missing."
    }
    if ((Get-Content -LiteralPath $markerPath -Raw) -ne "preserve-$testId") {
        throw "The clean install modified unrelated ProgramData user data."
    }

    # Re-run the same artifact in place to exercise NSIS preinstall/postinstall repair-upgrade hooks.
    Invoke-Installer -Path $installer -Arguments "/S /D=$installRoot" -Operation "In-place repair/upgrade"
    Wait-ForPath -Path $desktopPath -ShouldExist $true
    if ((Get-Content -LiteralPath $markerPath -Raw) -ne "preserve-$testId") {
        throw "The repair/upgrade modified unrelated ProgramData user data."
    }

    $uninstaller = Join-Path $installRoot "uninstall.exe"
    if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
        $candidates = @(Get-ChildItem -LiteralPath $installRoot -Filter "uninstall*.exe" -File -Recurse)
        if ($candidates.Count -ne 1) { throw "Expected exactly one installed NSIS uninstaller; found $($candidates.Count)." }
        $uninstaller = $candidates[0].FullName
    }
    Invoke-Installer -Path $uninstaller -Arguments "/S" -Operation "Silent uninstall"
    Wait-ForPath -Path $desktopPath -ShouldExist $false
    if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
        throw "Uninstall deleted user data under ProgramData; preservation contract failed."
    }
    if ((Get-Content -LiteralPath $markerPath -Raw) -ne "preserve-$testId") {
        throw "Uninstall modified the ProgramData preservation marker."
    }
    $remainingService = Get-CimInstance -ClassName Win32_Service -Filter "Name='WebDynamiteCore'"
    if ($remainingService) { throw "The WebDynamiteCore service remained registered after uninstall." }

    $uninstallVerified = $true
    Write-Host "Windows installer lifecycle acceptance passed: clean install, in-place repair/upgrade, service cleanup, and ProgramData preservation."
}
finally {
    # Remove only this test's unique marker directory. Never delete the shared WebDynamite data root.
    if (Test-Path -LiteralPath $dataRoot) {
        Remove-Item -LiteralPath $dataRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
    if (-not $uninstallVerified -and (Test-Path -LiteralPath $installRoot)) {
        Write-Warning "Acceptance test did not complete. The isolated test installation remains at $installRoot for runner diagnostics."
    }
}
