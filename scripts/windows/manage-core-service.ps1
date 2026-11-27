[CmdletBinding()]
param(
    [ValidateSet("Install", "Verify", "Remove", "SelfTest")]
    [string]$Mode = "Verify",
    [string]$ServiceBinaryPath
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ServiceName = "WebDynamiteCore"
$ServiceAccount = "NT SERVICE\$ServiceName"

function Assert-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "Run this service lifecycle operation from an elevated installer process."
    }
}
function Get-ServiceSidType {
    $output = & sc.exe qsidtype $ServiceName 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Could not query service SID type: $output" }
    if (($output | Out-String) -notmatch '(?im)SERVICE_SID_TYPE:\s*UNRESTRICTED') {
        throw "$ServiceName must have an unrestricted service SID."
    }
    return "UNRESTRICTED"
}
function Get-RegisteredService {
    return Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'"
}
function Get-NormalizedBinaryPath([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { throw "A service binary path is required." }
    $full = [IO.Path]::GetFullPath($Path)
    if ([IO.Path]::GetExtension($full) -ine ".exe") { throw "The service binary must be an .exe file." }
    if ($full.Contains('"')) { throw "The service binary path contains an invalid quote." }
    return $full
}
function Assert-RegisteredService {
    $service = Get-RegisteredService
    if (-not $service) { throw "$ServiceName is not registered." }
    if (-not [string]::Equals([string]$service.StartName, $ServiceAccount, [StringComparison]::OrdinalIgnoreCase)) {
        throw "$ServiceName must run as $ServiceAccount; refusing another service identity."
    }
    $null = Get-ServiceSidType
    return $service
}

if ($Mode -eq "SelfTest") {
    $valid = Get-NormalizedBinaryPath (Join-Path $env:TEMP "webdynamite-core.exe")
    if ([IO.Path]::GetFileName($valid) -ine "webdynamite-core.exe") { throw "Binary path normalization failed." }
    foreach ($invalid in @("", "relative.exe", "bad.txt")) {
        $rejected = $false
        try { $null = Get-NormalizedBinaryPath $invalid } catch { $rejected = $true }
        if (-not $rejected) { throw "Invalid service binary path was accepted: $invalid" }
    }
    Write-Output "Windows Core Service registration self-test passed."
    exit 0
}
Assert-Administrator
switch ($Mode) {
    "Install" {
        $binary = Get-NormalizedBinaryPath $ServiceBinaryPath
        if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Core Service executable does not exist: $binary" }
        $existing = Get-RegisteredService
        if ($existing) {
            $verified = Assert-RegisteredService
            $expectedPath = '"' + $binary + '"'
            if (-not [string]::Equals([string]$verified.PathName, $expectedPath, [StringComparison]::OrdinalIgnoreCase)) {
                throw "$ServiceName is already registered with a different executable path; refusing to overwrite it."
            }
            Write-Output "$ServiceName is already registered with the expected service identity and executable."
            exit 0
        }
        $binaryPath = '"' + $binary + '"'
        $created = & sc.exe create $ServiceName "binPath= $binaryPath" "start= demand" "obj= $ServiceAccount" 'password= ""' 2>&1
        if ($LASTEXITCODE -ne 0) { throw "Could not register $ServiceName: $created" }
        try {
            $sidResult = & sc.exe sidtype $ServiceName unrestricted 2>&1
            if ($LASTEXITCODE -ne 0) { throw "Could not enable the WebDynamite service SID: $sidResult" }
            $description = & sc.exe description $ServiceName "WebDynamite Core Service (managed by the WebDynamite installer)." 2>&1
            if ($LASTEXITCODE -ne 0) { throw "Could not set the service description: $description" }
            $null = Assert-RegisteredService
        } catch {
            $failure = $_
            & sc.exe delete $ServiceName | Out-Null
            throw $failure
        }
        Write-Output "$ServiceName registered as $ServiceAccount with an unrestricted service SID and demand-start mode."
        Write-Output "The service was not started. Trusted config and the DPAPI API token must be provisioned before startup."
    }
    "Verify" {
        $service = Assert-RegisteredService
        Write-Output ("{0}: account={1}; state={2}; startMode={3}; SID=UNRESTRICTED" -f $service.Name,$service.StartName,$service.State,$service.StartMode)
    }
    "Remove" {
        $service = Get-RegisteredService
        if (-not $service) { Write-Output "$ServiceName is not installed."; exit 0 }
        $null = Assert-RegisteredService
        if ($service.State -ne "Stopped") {
            $stop = & sc.exe stop $ServiceName 2>&1
            if ($LASTEXITCODE -ne 0 -and ($stop | Out-String) -notmatch "1062") {
                throw "Could not stop $ServiceName: $stop"
            }
            $deadline = [DateTime]::UtcNow.AddSeconds(30)
            do {
                Start-Sleep -Milliseconds 250
                $service = Get-RegisteredService
                if (-not $service -or $service.State -eq "Stopped") { break }
            } while ([DateTime]::UtcNow -lt $deadline)
            if ($service -and $service.State -ne "Stopped") { throw "$ServiceName did not stop within 30 seconds; refusing deletion." }
        }
        $deleted = & sc.exe delete $ServiceName 2>&1
        if ($LASTEXITCODE -ne 0) { throw "Could not unregister $ServiceName: $deleted" }
        Write-Output "$ServiceName unregistered. ProgramData data and secrets were not deleted."
    }
}
