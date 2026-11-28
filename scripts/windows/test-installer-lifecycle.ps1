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
function Invoke-Installer([string]$Path, [string]$Arguments, [string]$Operation, [string]$LogPath, [int]$TimeoutSeconds = 180) {
    Write-Host "Running $Operation (timeout: $TimeoutSeconds seconds)..."
    $process = Start-Process -FilePath $Path -ArgumentList $Arguments -PassThru
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        Write-Warning "$Operation exceeded its $TimeoutSeconds-second timeout. Terminating its process tree."
        & taskkill.exe /PID $process.Id /T /F 2>&1 | ForEach-Object { Write-Host $_ }
        try { $process.WaitForExit(15000) | Out-Null } catch { }
        if (Test-Path -LiteralPath $LogPath -PathType Leaf) {
            Write-Host "Last installer log lines ($LogPath):"
            Get-Content -LiteralPath $LogPath -Tail 80 | ForEach-Object { Write-Host $_ }
        } else {
            Write-Warning "NSIS did not create its requested log file: $LogPath"
        }
        $transcripts = @(Get-ChildItem -LiteralPath $env:TEMP -Filter "WebDynamite-CoreService-*.log" -File -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 5)
        foreach ($transcript in $transcripts) {
            Write-Host "Core Service lifecycle transcript ($($transcript.FullName)):"
            Get-Content -LiteralPath $transcript.FullName -Tail 100 | ForEach-Object { Write-Host $_ }
        }
        $installerProcesses = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match "WebDynamite|setup|uninst" } |
            Select-Object ProcessId, ParentProcessId, Name, CommandLine)
        if ($installerProcesses.Count -gt 0) {
            Write-Host "Remaining installer-related processes:"
            $installerProcesses | Format-List | Out-String | Write-Host
        }
        throw "$Operation timed out after $TimeoutSeconds seconds. Installer log: $LogPath"
    }
    $process.Refresh()
    if ($process.ExitCode -ne 0) {
        if (Test-Path -LiteralPath $LogPath -PathType Leaf) {
            Write-Host "Last installer log lines ($LogPath):"
            Get-Content -LiteralPath $LogPath -Tail 80 | ForEach-Object { Write-Host $_ }
        }
        throw "$Operation failed with exit code $($process.ExitCode). Installer log: $LogPath"
    }
    Write-Host "$Operation completed with exit code 0."
    if (Test-Path -LiteralPath $LogPath -PathType Leaf) {
        Get-Content -LiteralPath $LogPath -Tail 30 | ForEach-Object { Write-Host $_ }
    }
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
$logRoot = Join-Path $env:TEMP "WebDynamite-LifecycleAcceptance-$testId"
$uninstallVerified = $false

try {
    if (Test-Path -LiteralPath $installRoot) { throw "The isolated installer destination already exists: $installRoot" }
    $service = Get-CimInstance -ClassName Win32_Service -Filter "Name='WebDynamiteCore'"
    if ($service) { throw "A WebDynamiteCore service already exists on this runner; refusing to touch an existing installation." }

    New-Item -ItemType Directory -Path $dataRoot -Force | Out-Null
    New-Item -ItemType Directory -Path $logRoot -Force | Out-Null
    Set-Content -LiteralPath $markerPath -Value "preserve-$testId" -NoNewline -Encoding ascii

    # NSIS requires /D= to be the final argument. Keep it last so paths containing spaces are parsed correctly.
    $installLog = Join-Path $logRoot "clean-install.log"
    Invoke-Installer -Path $installer -Arguments "/S /LOG=`"$installLog`" /D=$installRoot" -Operation "Clean silent install" -LogPath $installLog
    Wait-ForPath -Path $desktopPath -ShouldExist $true
    if (-not (Test-Path -LiteralPath (Join-Path $installRoot "resources\install-core-service.ps1") -PathType Leaf)) {
        throw "The installed service lifecycle resource is missing."
    }
    if ((Get-Content -LiteralPath $markerPath -Raw) -ne "preserve-$testId") {
        throw "The clean install modified unrelated ProgramData user data."
    }

    # Re-run the same artifact in place to exercise NSIS preinstall/postinstall repair-upgrade hooks.
    $repairLog = Join-Path $logRoot "repair-upgrade.log"
    Invoke-Installer -Path $installer -Arguments "/S /LOG=`"$repairLog`" /D=$installRoot" -Operation "In-place repair/upgrade" -LogPath $repairLog
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
    $uninstallLog = Join-Path $logRoot "uninstall.log"
    Invoke-Installer -Path $uninstaller -Arguments "/S /LOG=`"$uninstallLog`"" -Operation "Silent uninstall" -LogPath $uninstallLog
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
        Write-Warning "Installer logs are retained at $logRoot."
    }
    elseif (Test-Path -LiteralPath $logRoot) {
        Remove-Item -LiteralPath $logRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
