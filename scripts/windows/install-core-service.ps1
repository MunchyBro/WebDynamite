[CmdletBinding()]
param(
    [ValidateSet("Install", "StopForUpgrade", "Uninstall", "SelfTest")]
    [string]$Mode = "Install",
    [string]$InstallRoot
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$LifecycleLogPath = Join-Path $env:TEMP ("WebDynamite-CoreService-" + $Mode + "-" + [guid]::NewGuid().ToString("N") + ".log")
try {
    Start-Transcript -LiteralPath $LifecycleLogPath -Force | Out-Null
} catch {
    Write-Warning "Could not start Core Service lifecycle transcript: $_"
}
Write-Output "Core Service lifecycle mode '$Mode' started. Diagnostic log: $LifecycleLogPath"

$ServiceName = "WebDynamiteCore"
$ServiceScript = Join-Path $PSScriptRoot "manage-core-service.ps1"
$LayoutScript = Join-Path $PSScriptRoot "provision-service-layout.ps1"
$TrustedConfigScript = Join-Path $PSScriptRoot "provision-trusted-service-config.ps1"
$ServiceRelativePath = "resources\webdynamite-core.exe"
$DesktopExecutableName = "webdynamite.exe"
$ApiTokenFileName = "api-token.wds"
$ApiPort = 47821

function Assert-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "The WebDynamite Core Service installer requires an elevated per-machine installer."
    }
}

function Invoke-PowerShellScript([string]$Path, [string[]]$Arguments) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Required installer script is missing: $Path"
    }
    $powerShell = Join-Path $env:WINDIR "System32\WindowsPowerShell\v1.0\powershell.exe"
    & $powerShell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $Path @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Installer prerequisite failed: $([IO.Path]::GetFileName($Path)) exited with code $LASTEXITCODE."
    }
}

function Get-CertificateSha256([Security.Cryptography.X509Certificates.X509Certificate2]$Certificate) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        return [BitConverter]::ToString($sha.ComputeHash($Certificate.RawData)).Replace("-", "").ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

function Resolve-SignatureDisposition(
    [string]$DesktopStatus,
    [string]$ServiceStatus,
    [string]$DesktopPin,
    [string]$ServicePin,
    [string]$ExpectedPin
) {
    $statuses = @($DesktopStatus, $ServiceStatus)
    if (@($statuses | Where-Object { $_ -notin @("Valid", "NotSigned") }).Count -gt 0) {
        throw "Desktop or Core Service has an invalid/untrusted Authenticode signature; refusing service activation."
    }
    if ($statuses -contains "NotSigned") {
        return "UnsignedEngineering"
    }

    $expected = ($ExpectedPin -replace '[:\s]', '').ToLowerInvariant()
    if ($expected.Length -ne 64 -or $expected -notmatch '^[0-9a-f]{64}$' -or $expected -match '^0{64}$') {
        throw "A configured, nonzero 64-character publisher SHA-256 pin is required before service activation."
    }
    if ($DesktopPin -ne $expected -or $ServicePin -ne $expected) {
        throw "Desktop or Core Service signer does not match the configured trusted publisher SHA-256 pin."
    }
    return "Signed"
}

function Get-SignatureDisposition([string]$DesktopPath, [string]$ServicePath, [string]$ExpectedPin) {
    $desktop = Get-AuthenticodeSignature -LiteralPath $DesktopPath
    $service = Get-AuthenticodeSignature -LiteralPath $ServicePath
    $desktopPin = if ($desktop.SignerCertificate) { Get-CertificateSha256 $desktop.SignerCertificate } else { "" }
    $servicePin = if ($service.SignerCertificate) { Get-CertificateSha256 $service.SignerCertificate } else { "" }
    return Resolve-SignatureDisposition -DesktopStatus ([string]$desktop.Status) -ServiceStatus ([string]$service.Status) -DesktopPin $desktopPin -ServicePin $servicePin -ExpectedPin $ExpectedPin
}

function Resolve-InstallPaths([string]$Root) {
    if ([string]::IsNullOrWhiteSpace($Root) -or -not [IO.Path]::IsPathRooted($Root)) {
        throw "A fully qualified per-machine installation root is required."
    }
    $trimChars = [char[]]@([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
    $fullRoot = [IO.Path]::GetFullPath($Root).TrimEnd($trimChars)
    $programFiles = [IO.Path]::GetFullPath($env:ProgramFiles).TrimEnd($trimChars)
    $requiredPrefix = $programFiles + [IO.Path]::DirectorySeparatorChar
    if (-not $fullRoot.StartsWith($requiredPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "The Core Service must be installed beneath Program Files."
    }
    return [pscustomobject]@{
        Root = $fullRoot
        Desktop = Join-Path $fullRoot $DesktopExecutableName
        Service = Join-Path $fullRoot $ServiceRelativePath
    }
}
function Start-And-VerifyService {
    $start = & sc.exe start $ServiceName 2>&1
    if ($LASTEXITCODE -ne 0 -and ($start | Out-String) -notmatch "1056") {
        throw "Could not start $ServiceName after trusted provisioning: $start"
    }

    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    do {
        Start-Sleep -Milliseconds 500
        $service = Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'"
        if ($service -and $service.State -eq "Running") {
            $client = New-Object System.Net.Sockets.TcpClient
            try {
                $connect = $client.BeginConnect("127.0.0.1", $ApiPort, $null, $null)
                if ($connect.AsyncWaitHandle.WaitOne(500) -and $client.Connected) {
                    $client.EndConnect($connect)
                    return
                }
            } finally {
                $client.Dispose()
            }
        }
    } while ([DateTime]::UtcNow -lt $deadline)

    $service = Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'"
    $state = if ($service) { $service.State } else { "missing" }
    throw "$ServiceName did not reach a listening API state within 45 seconds (SCM state: $state)."
}

function Invoke-Install {
    Write-Output "Install: checking elevation and install paths."
    Assert-Administrator
    $paths = Resolve-InstallPaths $InstallRoot
    foreach ($path in @($paths.Desktop, $paths.Service)) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Required installed executable is missing: $path"
        }
    }

    $publisherPinPath = Join-Path (Join-Path $paths.Root "resources") "webdynamite-publisher.sha256"
    $publisherPin = if (Test-Path -LiteralPath $publisherPinPath -PathType Leaf) {
        (Get-Content -LiteralPath $publisherPinPath -Raw).Trim()
    } else {
        ""
    }
    Write-Output "Install: checking Authenticode status for desktop and service binaries."
    $signature = Get-SignatureDisposition $paths.Desktop $paths.Service $publisherPin
    Write-Output "Install: signature disposition is $signature."
    if ($signature -eq "UnsignedEngineering") {
        Write-Warning "Unsigned engineering build: installing the desktop application only. The privileged Core Service remains unregistered and inactive until both binaries are signed by the trusted production publisher."
        return
    }

    Write-Output "Install: preparing signed production Core Service."
    # Register first so Windows can resolve the exact virtual service SID used by protected ProgramData ACLs.
    Invoke-PowerShellScript $ServiceScript @("-Mode", "Install", "-ServiceBinaryPath", $paths.Service)
    Invoke-PowerShellScript $LayoutScript @("-Mode", "Prepare")

    # Preserve the existing machine-DPAPI token on repair/upgrade. Provision only when absent.
    $tokenPath = Join-Path (Join-Path $env:ProgramData "WebDynamite\Secrets") $ApiTokenFileName
    if (-not (Test-Path -LiteralPath $tokenPath -PathType Leaf)) {
        & $paths.Service --provision-api-token
        if ($LASTEXITCODE -ne 0) {
            throw "Core Service API-token provisioning failed with exit code $LASTEXITCODE."
        }
    }

    Invoke-PowerShellScript $TrustedConfigScript @("-Mode", "Prepare", "-DesktopExecutable", $paths.Desktop, "-ServiceBinary", $paths.Service)
    Invoke-PowerShellScript $TrustedConfigScript @("-Mode", "Verify")
    Invoke-PowerShellScript $LayoutScript @("-Mode", "Verify")

    try {
        Start-And-VerifyService
    } catch {
        try { Invoke-PowerShellScript $ServiceScript @("-Mode", "Stop") } catch { }
        throw
    }
    Write-Output "WebDynamite Core Service was securely provisioned, started, and its local API listener was verified."
}

function Invoke-Uninstall {
    Write-Output "Uninstall: checking elevation."
    Assert-Administrator
    Invoke-PowerShellScript $ServiceScript @("-Mode", "Remove")
    Write-Output "WebDynamite Core Service removed. User data, database, configuration, and machine secrets under ProgramData were preserved."
}

if ($Mode -eq "SelfTest") {
    $pin = "ab" * 32
    if ((Resolve-SignatureDisposition "NotSigned" "NotSigned" "" "" "") -ne "UnsignedEngineering") {
        throw "Unsigned engineering policy failed."
    }
    if ((Resolve-SignatureDisposition "Valid" "Valid" $pin $pin $pin) -ne "Signed") {
        throw "Pinned signed activation policy failed."
    }

    $cases = @(
        [pscustomobject]@{ DesktopStatus="HashMismatch"; ServiceStatus="Valid"; DesktopPin=$pin; ServicePin=$pin; ExpectedPin=$pin },
        [pscustomobject]@{ DesktopStatus="NotTrusted"; ServiceStatus="Valid"; DesktopPin=$pin; ServicePin=$pin; ExpectedPin=$pin },
        [pscustomobject]@{ DesktopStatus="Valid"; ServiceStatus="UnknownError"; DesktopPin=$pin; ServicePin=$pin; ExpectedPin=$pin },
        [pscustomobject]@{ DesktopStatus="NotSigned"; ServiceStatus="HashMismatch"; DesktopPin=""; ServicePin=""; ExpectedPin=$pin },
        [pscustomobject]@{ DesktopStatus="Valid"; ServiceStatus="Valid"; DesktopPin=$pin; ServicePin=$pin; ExpectedPin="" },
        [pscustomobject]@{ DesktopStatus="Valid"; ServiceStatus="Valid"; DesktopPin=$pin; ServicePin=$pin; ExpectedPin=("00" * 32) },
        [pscustomobject]@{ DesktopStatus="Valid"; ServiceStatus="Valid"; DesktopPin=$pin; ServicePin=("cd" * 32); ExpectedPin=$pin }
    )
    foreach ($case in $cases) {
        $rejected = $false
        try {
            $null = Resolve-SignatureDisposition $case.DesktopStatus $case.ServiceStatus $case.DesktopPin $case.ServicePin $case.ExpectedPin
        } catch {
            $rejected = $true
        }
        if (-not $rejected) {
            throw "Invalid signature/publisher pin combination was accepted."
        }
    }

    Write-Output "Windows Core Service installer lifecycle self-tests passed."
    exit 0
}

switch ($Mode) {
    "Install" {
        Invoke-Install
    }
    "StopForUpgrade" {
        Write-Output "StopForUpgrade: checking elevation and existing service."
        Assert-Administrator
        $paths = Resolve-InstallPaths $InstallRoot
        $existing = Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'"
        if ($existing) {
            $expectedPath = '"' + $paths.Service + '"'
            if (-not [string]::Equals([string]$existing.PathName, $expectedPath, [StringComparison]::OrdinalIgnoreCase)) {
                throw "The installed service points to a different installation directory; refusing to stop or replace it."
            }
        }
        if (-not $existing) {
            Write-Output "No existing WebDynamiteCore service is registered; no stop action is required."
        } else {
            Invoke-PowerShellScript $ServiceScript @("-Mode", "Stop")
            Write-Output "Existing Core Service stopped before installer file replacement."
        }
    }
    "Uninstall" {
        Invoke-Uninstall
    }
}

try { Stop-Transcript | Out-Null } catch { }
