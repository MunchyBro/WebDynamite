[CmdletBinding()]
param(
    [ValidateSet("Install", "Verify", "Stop", "Remove", "SelfTest", "IntegrationSelfTest")]
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
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw "Run this service lifecycle operation from an elevated installer process." }
}
function Get-ServiceSidType {
    $output = & sc.exe qsidtype $ServiceName 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Could not query service SID type: $output" }
    if (($output | Out-String) -notmatch '(?im)SERVICE_SID_TYPE:\s*UNRESTRICTED') { throw "$ServiceName must have an unrestricted service SID." }
    return "UNRESTRICTED"
}
function Get-ServiceRecoveryPolicy {
    return [pscustomobject]@{ ResetPeriodSeconds = 86400; RestartDelaysMilliseconds = @(5000, 15000, 60000) }
}
function Get-ServiceStartupScArguments { return @("start=", "auto") }
function Get-ServiceCreateScArguments([string]$BinaryPath) {
    return @("binPath=", $BinaryPath, "start=", "auto", "obj=", $ServiceAccount)
}
function Get-ServiceRecoveryScArguments {
    $policy = Get-ServiceRecoveryPolicy
    $actions = @($policy.RestartDelaysMilliseconds | ForEach-Object { "restart/$_" }) -join "/"
    return @("reset=", [string]$policy.ResetPeriodSeconds, "actions=", $actions)
}
function Test-ServiceRecoveryConfiguration([string]$Output) {
    if ([string]::IsNullOrWhiteSpace($Output)) { return $false }
    $policy = Get-ServiceRecoveryPolicy
    $reset = [regex]::Match($Output, '(?im)RESET_PERIOD\s*\(in seconds\)\s*:\s*(\d+)')
    if (-not $reset.Success -or [int]$reset.Groups[1].Value -ne $policy.ResetPeriodSeconds) { return $false }
    $delays = @([regex]::Matches($Output, '(?i)RESTART\s*--\s*Delay\s*=\s*(\d+)\s*milliseconds') | ForEach-Object { [int]$_.Groups[1].Value })
    if ($delays.Count -ne $policy.RestartDelaysMilliseconds.Count) { return $false }
    for ($i = 0; $i -lt $delays.Count; $i++) { if ($delays[$i] -ne $policy.RestartDelaysMilliseconds[$i]) { return $false } }
    return $true
}
function Set-ServiceRecoveryPolicy {
    $startupArguments = Get-ServiceStartupScArguments
    $start = & sc.exe config $ServiceName @startupArguments 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Could not configure automatic startup for ${ServiceName}: $start" }
    $arguments = Get-ServiceRecoveryScArguments
    $failure = & sc.exe failure $ServiceName @arguments 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Could not configure bounded SCM recovery for ${ServiceName}: $failure" }
    $flag = & sc.exe failureflag $ServiceName 1 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Could not enable recovery actions for non-zero service exits: $flag" }
}
function Assert-ServiceRecoveryPolicy {
    $service = Get-RegisteredService
    if (-not $service -or -not [string]::Equals([string]$service.StartMode, "Auto", [StringComparison]::OrdinalIgnoreCase)) { throw "$ServiceName must be configured for automatic machine startup." }
    $output = & sc.exe qfailure $ServiceName 2>&1
    if ($LASTEXITCODE -ne 0 -or -not (Test-ServiceRecoveryConfiguration ($output | Out-String))) { throw "$ServiceName does not have the required bounded restart policy (5s, 15s, 60s; reset after 24 hours)." }
    $flagOutput = & sc.exe qfailureflag $ServiceName 2>&1
    if ($LASTEXITCODE -ne 0 -or ($flagOutput | Out-String) -notmatch '(?im)NONCRASH_FAILURES\s*:\s*(?:0x)?0*1\b') { throw "$ServiceName must restart after non-zero service process exits. SCM query output: $($flagOutput | Out-String)" }
    return $service
}
function Get-RegisteredService { return Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'" }
function Get-NormalizedBinaryPath([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { throw "A service binary path is required." }
    $full = [IO.Path]::GetFullPath($Path)
    if (-not [IO.Path]::IsPathRooted($Path)) { throw "The service binary path must be absolute." }
    if ([IO.Path]::GetExtension($full) -ine ".exe") { throw "The service binary must be an .exe file." }
    if ($full.Contains('"')) { throw "The service binary path contains an invalid quote." }
    return $full
}
function Assert-RegisteredService {
    $service = Get-RegisteredService
    if (-not $service) { throw "$ServiceName is not registered." }
    if (-not [string]::Equals([string]$service.StartName, $ServiceAccount, [StringComparison]::OrdinalIgnoreCase)) { throw "$ServiceName must run as $ServiceAccount; refusing another service identity." }
    $null = Get-ServiceSidType
    return $service
}
function Stop-RegisteredService {
    $service = Get-RegisteredService
    if (-not $service) { return $false }
    $null = Assert-RegisteredService
    if ($service.State -eq "Stopped") { return $true }
    $stop = & sc.exe stop $ServiceName 2>&1
    if ($LASTEXITCODE -ne 0 -and ($stop | Out-String) -notmatch "1062") { throw "Could not stop ${ServiceName}: $stop" }
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    do {
        Start-Sleep -Milliseconds 250
        $service = Get-RegisteredService
        if (-not $service -or $service.State -eq "Stopped") { return $true }
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "$ServiceName did not stop within 45 seconds; refusing further lifecycle changes."
}
function Invoke-ScmIntegrationSelfTest {
    Assert-Administrator
    $originalName = $script:ServiceName
    $originalAccount = $script:ServiceAccount
    $testName = "WDSvcTest" + ([guid]::NewGuid().ToString("N").Substring(0, 12))
    $testAccount = "NT SERVICE\$testName"
    $created = $false
    try {
        $testBinary = Join-Path $env:WINDIR "System32\cmd.exe"
        $script:ServiceName = $testName
        $script:ServiceAccount = $testAccount
        $createArguments = Get-ServiceCreateScArguments $testBinary
        $create = & sc.exe create $testName @createArguments 2>&1
        if ($LASTEXITCODE -ne 0) { throw "Could not create disposable SCM integration-test service: $create" }
        $created = $true
        $sid = & sc.exe sidtype $testName unrestricted 2>&1
        if ($LASTEXITCODE -ne 0) { throw "Could not set disposable service SID type: $sid" }
        Set-ServiceRecoveryPolicy
        $null = Assert-RegisteredService
        $null = Assert-ServiceRecoveryPolicy
        Write-Output "Disposable SCM integration test passed: automatic startup, service SID, bounded recovery, and non-crash failure flag verified."
    } finally {
        if ($created) {
            $delete = & sc.exe delete $testName 2>&1
            if ($LASTEXITCODE -ne 0) { Write-Warning "Could not delete disposable SCM integration-test service ${testName}: $delete" }
        }
        $script:ServiceName = $originalName
        $script:ServiceAccount = $originalAccount
    }
}
if ($Mode -eq "SelfTest") {
    $valid = Get-NormalizedBinaryPath (Join-Path $env:TEMP "webdynamite-core.exe")
    if ([IO.Path]::GetFileName($valid) -ine "webdynamite-core.exe") { throw "Binary path normalization failed." }
    foreach ($invalid in @("", "relative.exe", "bad.txt")) {
        $rejected = $false
        try { $null = Get-NormalizedBinaryPath $invalid } catch { $rejected = $true }
        if (-not $rejected) { throw "Invalid service binary path was accepted: $invalid" }
    }
    $expectedArguments = Get-ServiceRecoveryScArguments
    if ($expectedArguments.Count -ne 4 -or $expectedArguments[0] -ne "reset=" -or $expectedArguments[1] -ne "86400" -or $expectedArguments[2] -ne "actions=" -or $expectedArguments[3] -ne "restart/5000/restart/15000/restart/60000") { throw "SCM recovery action arguments must be separate sc.exe tokens." }
    $startupArguments = Get-ServiceStartupScArguments
    if ($startupArguments.Count -ne 2 -or $startupArguments[0] -ne "start=" -or $startupArguments[1] -ne "auto") { throw "SCM automatic-start arguments must be separate sc.exe tokens." }
    $createArguments = Get-ServiceCreateScArguments "C:\Program Files\WebDynamite\resources\webdynamite-core.exe"
    if ($createArguments.Count -ne 6 -or $createArguments[0] -ne "binPath=" -or $createArguments[1] -notmatch "Program Files" -or $createArguments[2] -ne "start=" -or $createArguments[3] -ne "auto" -or $createArguments[4] -ne "obj=" -or $createArguments[5] -ne $ServiceAccount) { throw "SCM service creation arguments are not correctly tokenized." }
$validRecovery = @"
[SC] QueryServiceConfig2 SUCCESS

SERVICE_NAME: WebDynamiteCore
        RESET_PERIOD (in seconds)    : 86400
        REBOOT_MESSAGE               :
        COMMAND_LINE                 :
        FAILURE_ACTIONS              : RESTART -- Delay = 5000 milliseconds
                                       RESTART -- Delay = 15000 milliseconds
                                       RESTART -- Delay = 60000 milliseconds
"@
    if (-not (Test-ServiceRecoveryConfiguration $validRecovery)) { throw "Valid SCM recovery output was rejected." }
    $invalidRecovery = $validRecovery.Replace("Delay = 15000", "Delay = 90000")
    if (Test-ServiceRecoveryConfiguration $invalidRecovery) { throw "Invalid SCM recovery delay was accepted." }
    Write-Output "Windows Core Service registration, automatic startup, and bounded recovery self-tests passed."
    exit 0
}
if ($Mode -eq "IntegrationSelfTest") { Invoke-ScmIntegrationSelfTest; exit 0 }
Assert-Administrator
switch ($Mode) {
    "Install" {
        $binary = Get-NormalizedBinaryPath $ServiceBinaryPath
        if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Core Service executable does not exist: $binary" }
        $existing = Get-RegisteredService
        if ($existing) {
            $verified = Assert-RegisteredService
            $expectedPath = '"' + $binary + '"'
            if (-not [string]::Equals([string]$verified.PathName, $expectedPath, [StringComparison]::OrdinalIgnoreCase)) { throw "$ServiceName is already registered with a different executable path; refusing to overwrite it." }
            Set-ServiceRecoveryPolicy
            $null = Assert-ServiceRecoveryPolicy
            Write-Output "$ServiceName is registered with the expected identity, executable, automatic startup, and bounded recovery policy."
            exit 0
        }
        $createArguments = Get-ServiceCreateScArguments $binary
        $created = & sc.exe create $ServiceName @createArguments 2>&1
        if ($LASTEXITCODE -ne 0) { throw "Could not register ${ServiceName}: $created" }
        try {
            $sidResult = & sc.exe sidtype $ServiceName unrestricted 2>&1
            if ($LASTEXITCODE -ne 0) { throw "Could not enable the WebDynamite service SID: $sidResult" }
            $description = & sc.exe description $ServiceName "WebDynamite Core Service (managed by the WebDynamite installer)." 2>&1
            if ($LASTEXITCODE -ne 0) { throw "Could not set the service description: $description" }
            Set-ServiceRecoveryPolicy
            $null = Assert-RegisteredService
            $null = Assert-ServiceRecoveryPolicy
        } catch {
            $failure = $_
            & sc.exe delete $ServiceName | Out-Null
            throw $failure
        }
        Write-Output "$ServiceName registered as $ServiceAccount with an unrestricted service SID, automatic machine startup, and bounded SCM recovery."
        Write-Output "The service was not started; installer provisioning must finish first."
    }
    "Verify" {
        $service = Assert-RegisteredService
        $service = Assert-ServiceRecoveryPolicy
        Write-Output ("{0}: account={1}; state={2}; startMode={3}; recovery=5s/15s/60s; reset=24h; SID=UNRESTRICTED" -f $service.Name,$service.StartName,$service.State,$service.StartMode)
    }
    "Stop" {
        if (Stop-RegisteredService) { Write-Output "$ServiceName is stopped and safe for installer file replacement." }
        else { Write-Output "$ServiceName is not installed; nothing to stop." }
    }
    "Remove" {
        $service = Get-RegisteredService
        if (-not $service) { Write-Output "$ServiceName is not installed."; exit 0 }
        $null = Stop-RegisteredService
        $deleted = & sc.exe delete $ServiceName 2>&1
        if ($LASTEXITCODE -ne 0) { throw "Could not unregister ${ServiceName}: $deleted" }
        Write-Output "$ServiceName unregistered. ProgramData data and secrets were not deleted."
    }
}
