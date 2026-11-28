[CmdletBinding()]
param(
    [ValidateSet("Install", "StopForUpgrade", "Uninstall", "SelfTest")]
    [string]$Mode = "Install",
    [string]$InstallRoot
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ServiceName = "WebDynamiteCore"
$ServiceScript = Join-Path $PSScriptRoot "manage-core-service.ps1"
$LayoutScript = Join-Path $PSScriptRoot "provision-service-layout.ps1"
$TrustedConfigScript = Join-Path $PSScriptRoot "provision-trusted-service-config.ps1"
$ServiceRelativePath = "resources\webdynamite-core.exe"
$DesktopExecutableName = "WebDynamite.exe"
$ApiTokenFileName = "api-token.wds"
$ApiPort = 47821

function Assert-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw "The WebDynamite Core Service installer requires an elevated per-machine installer." }
}
function Invoke-PowerShellScript([string]$Path, [string[]]$Arguments) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Required installer script is missing: $Path" }
    $powerShell = Join-Path $env:WINDIR "System32\WindowsPowerShell\v1.0\powershell.exe"
    & $powerShell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $Path @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Installer prerequisite failed: $([IO.Path]::GetFileName($Path)) exited with code $LASTEXITCODE." }
}
function Get-SignatureDisposition([string]$DesktopPath, [string]$ServicePath) {
    $desktop = Get-AuthenticodeSignature -LiteralPath $DesktopPath
    $service = Get-AuthenticodeSignature -LiteralPath $ServicePath
    $statuses = @([string]$desktop.Status, [string]$service.Status)
    if ($statuses -contains "NotSigned") { return "UnsignedEngineering" }
    if ($desktop.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or $service.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Desktop and Core Service must both have valid trusted Authenticode signatures; refusing service activation."
    }
    if (-not $desktop.SignerCertificate -or -not $service.SignerCertificate) { throw "Signed desktop and Core Service binaries must both expose signer certificates." }
    return "Signed"
}
function Resolve-InstallPaths([string]$Root) {
    if ([string]::IsNullOrWhiteSpace($Root) -or -not [IO.Path]::IsPathRooted($Root)) { throw "A fully qualified per-machine installation root is required." }
    $fullRoot = [IO.Path]::GetFullPath($Root).TrimEnd('\') + '\'
    $programFiles = [IO.Path]::GetFullPath($env:ProgramFiles).TrimEnd('\') + '\'
    if (-not $fullRoot.StartsWith($programFiles, [StringComparison]::OrdinalIgnoreCase)) { throw "The Core Service must be installed beneath Program Files." }
    return [pscustomobject]@{ Root=$fullRoot.TrimEnd('\'); Desktop=(Join-Path $fullRoot $DesktopExecutableName); Service=(Join-Path $fullRoot $ServiceRelativePath) }
}
function Start-And-VerifyService {
    $start = & sc.exe start $ServiceName 2>&1
    if ($LASTEXITCODE -ne 0 -and ($start | Out-String) -notmatch "1056") { throw "Could not start $ServiceName after trusted provisioning: $start" }
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    do {
        Start-Sleep -Milliseconds 500
        $service = Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'"
        if ($service -and $service.State -eq "Running") {
            $client = New-Object System.Net.Sockets.TcpClient
            try {
                $connect = $client.BeginConnect("127.0.0.1", $ApiPort, $null, $null)
                if ($connect.AsyncWaitHandle.WaitOne(500) -and $client.Connected) { $client.EndConnect($connect); return }
            } finally { $client.Dispose() }
        }
    } while ([DateTime]::UtcNow -lt $deadline)
    $service = Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'"
    $state = if ($service) { $service.State } else { "missing" }
    throw "$ServiceName did not reach a listening API state within 45 seconds (SCM state: $state)."
}
function Invoke-Install {
    Assert-Administrator
    $paths = Resolve-InstallPaths $InstallRoot
    foreach ($path in @($paths.Desktop, $paths.Service)) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Required installed executable is missing: $path" }
    }
    $signature = Get-SignatureDisposition $paths.Desktop $paths.Service
    if ($signature -eq "UnsignedEngineering") {
        Write-Warning "Unsigned engineering build: installing the desktop application only. The privileged Core Service remains unregistered and inactive until both binaries are signed by the trusted production publisher."
        return
    }

    # Register first so Windows can resolve the exact virtual service SID used by protected ProgramData ACLs.
    Invoke-PowerShellScript $ServiceScript @("-Mode", "Install", "-ServiceBinaryPath", $paths.Service)
    Invoke-PowerShellScript $LayoutScript @("-Mode", "Prepare")

    # Preserve the existing machine-DPAPI token on repair/upgrade. Provision only when absent.
    $tokenPath = Join-Path (Join-Path $env:ProgramData "WebDynamite\Secrets") $ApiTokenFileName
    if (-not (Test-Path -LiteralPath $tokenPath -PathType Leaf)) {
        & $paths.Service --provision-api-token
        if ($LASTEXITCODE -ne 0) { throw "Core Service API-token provisioning failed with exit code $LASTEXITCODE." }
    }

    Invoke-PowerShellScript $TrustedConfigScript @("-Mode", "Prepare", "-DesktopExecutable", $paths.Desktop, "-ServiceBinary", $paths.Service)
    Invoke-PowerShellScript $TrustedConfigScript @("-Mode", "Verify")
    Invoke-PowerShellScript $LayoutScript @("-Mode", "Verify")
    Start-And-VerifyService
    Write-Output "WebDynamite Core Service was securely provisioned, started, and its local API listener was verified."
}
function Invoke-Uninstall {
    Assert-Administrator
    Invoke-PowerShellScript $ServiceScript @("-Mode", "Remove")
    Write-Output "WebDynamite Core Service removed. User data, database, configuration, and machine secrets under ProgramData were preserved."
}

if ($Mode -eq "SelfTest") {
    function Resolve-SignatureStatus([string]$DesktopStatus, [string]$ServiceStatus) {
        if ($DesktopStatus -eq "NotSigned" -or $ServiceStatus -eq "NotSigned") { return "UnsignedEngineering" }
        if ($DesktopStatus -ne "Valid" -or $ServiceStatus -ne "Valid") { throw "Invalid signature state." }
        return "Signed"
    }
    if ((Resolve-SignatureStatus "NotSigned" "NotSigned") -ne "UnsignedEngineering") { throw "Unsigned engineering policy failed." }
    if ((Resolve-SignatureStatus "Valid" "Valid") -ne "Signed") { throw "Signed activation policy failed." }
    foreach ($pair in @(@("HashMismatch", "Valid"), @("NotTrusted", "Valid"), @("Valid", "UnknownError"))) {
        $rejected = $false
        try { $null = Resolve-SignatureStatus $pair[0] $pair[1] } catch { $rejected = $true }
        if (-not $rejected) { throw "Invalid Authenticode status was accepted: $($pair -join ', ')" }
    }
    Write-Output "Windows Core Service installer lifecycle self-tests passed."
    exit 0
}
switch ($Mode) {
    "Install" { Invoke-Install }
    "StopForUpgrade" {
        Assert-Administrator
        Invoke-PowerShellScript $ServiceScript @("-Mode", "Stop")
        Write-Output "Existing Core Service stopped before installer file replacement."
    }
    "Uninstall" { Invoke-Uninstall }
}
