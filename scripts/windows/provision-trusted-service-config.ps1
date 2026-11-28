[CmdletBinding()]
param(
    [ValidateSet("Prepare", "Verify", "SelfTest")]
    [string]$Mode = "Verify",
    [string]$DesktopExecutable,
    [string]$ServiceBinary
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ServiceName = "WebDynamiteCore"
$ServiceAccount = "NT SERVICE\$ServiceName"
$TrustedRoot = Join-Path $env:ProgramData "WebDynamite"
$ConfigDirectory = Join-Path $TrustedRoot "Config"
$ConfigPath = Join-Path $ConfigDirectory "service.json"
$SecretStoreRoot = Join-Path $TrustedRoot "Secrets"
$DataRoot = Join-Path $TrustedRoot "Data"
$SystemSid = "S-1-5-18"
$AdministratorsSid = "S-1-5-32-544"

function Test-ServiceSidShape([string]$Sid) {
    if ([string]::IsNullOrWhiteSpace($Sid)) { return $false }
    $parts = $Sid.Split('-')
    if ($parts.Count -ne 9 -or $parts[0] -ne "S" -or $parts[1] -ne "1" -or $parts[2] -ne "5" -or $parts[3] -ne "80") { return $false }
    foreach ($part in $parts[4..8]) {
        $number = 0L
        if ([string]::IsNullOrEmpty($part) -or -not [long]::TryParse($part, [ref]$number) -or $number -lt 0) { return $false }
    }
    return $true
}
function Test-UserSid([string]$Sid) {
    if ([string]::IsNullOrWhiteSpace($Sid)) { return $false }
    $parts = $Sid.Split('-')
    if ($parts.Count -ne 8 -or $parts[0] -ne "S" -or $parts[1] -ne "1" -or $parts[2] -ne "5" -or $parts[3] -ne "21") { return $false }
    foreach ($part in $parts[4..7]) {
        $number = 0L
        if ([string]::IsNullOrEmpty($part) -or -not [long]::TryParse($part, [ref]$number) -or $number -lt 0) { return $false }
    }
    return $true
}
function Normalize-Thumbprint([string]$Value) {
    $normalized = ($Value -replace '[:\s]', '').ToLowerInvariant()
    if ($normalized.Length -ne 64 -or $normalized -notmatch '^[0-9a-f]{64}$' -or $normalized -match '^0{64}$') {
        throw "A real SHA-256 publisher certificate thumbprint is required."
    }
    return $normalized
}
function Get-WebDynamiteServiceSid {
    $account = New-Object System.Security.Principal.NTAccount($ServiceAccount)
    $sid = $account.Translate([System.Security.Principal.SecurityIdentifier]).Value
    if (-not (Test-ServiceSidShape $sid)) { throw "Windows did not resolve the registered WebDynamiteCore service SID." }
    return $sid
}
function Get-NormalizedExecutablePath([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path) -or -not [IO.Path]::IsPathRooted($Path)) { throw "Desktop and Core Service executable paths must be absolute." }
    $full = [IO.Path]::GetFullPath($Path)
    if ([IO.Path]::GetExtension($full) -ine ".exe" -or $full.Contains('"')) { throw "A normalized absolute .exe path is required." }
    if ($full -match '(^|[\\/])\.\.?([\\/]|$)') { throw "Executable paths must not contain dot segments." }
    return $full
}
function Assert-NoReparsePathComponents([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    $root = [IO.Path]::GetPathRoot($full)
    $current = $root
    foreach ($part in ($full.Substring($root.Length) -split '[\\/]' | Where-Object { $_ })) {
        $current = Join-Path $current $part
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Refusing a reparse-point path component: $current" }
        }
    }
}
function Get-PublisherSha256([string]$Executable) {
    $full = Get-NormalizedExecutablePath $Executable
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "Required signed executable is missing: $full" }
    Assert-NoReparsePathComponents $full
    $signature = Get-AuthenticodeSignature -LiteralPath $full
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or -not $signature.SignerCertificate) {
        throw "Executable does not have a valid Authenticode signature: $full"
    }
    return Normalize-Thumbprint ($signature.SignerCertificate.GetCertHashString([System.Security.Cryptography.HashAlgorithmName]::SHA256))
}
function Assert-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw "Run trusted service configuration provisioning from an elevated installer process." }
}
function Get-ExpectedFileSddl([string]$ServiceSid) {
    if (-not (Test-ServiceSidShape $ServiceSid)) { throw "A valid WebDynamite service SID is required." }
    return "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;$ServiceSid)"
}
function Set-ProtectedConfigAcl([string]$Path, [string]$ServiceSid) {
    $security = New-Object System.Security.AccessControl.FileSecurity
    $security.SetSecurityDescriptorSddlForm((Get-ExpectedFileSddl $ServiceSid), [System.Security.AccessControl.AccessControlSections]::Access)
    $security.SetOwner((New-Object System.Security.Principal.SecurityIdentifier($AdministratorsSid)))
    Set-Acl -LiteralPath $Path -AclObject $security
}
function Test-ProtectedConfigAcl([string]$Path, [string]$ServiceSid) {
    $actual = Get-Acl -LiteralPath $Path
    if (-not $actual.AreAccessRulesProtected) { return $false }
    $owner = $actual.GetOwner([System.Security.Principal.SecurityIdentifier]).Value
    if ($owner -ne $SystemSid -and $owner -ne $AdministratorsSid) { return $false }
    $expected = New-Object System.Security.AccessControl.FileSecurity
    $expected.SetSecurityDescriptorSddlForm((Get-ExpectedFileSddl $ServiceSid), [System.Security.AccessControl.AccessControlSections]::Access)
    $expectedRules = @($expected.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
    $actualRules = @($actual.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
    if ($actualRules.Count -ne 3) { return $false }
    foreach ($rule in $expectedRules) {
        $matches = @($actualRules | Where-Object {
            $_.IdentityReference.Value -eq $rule.IdentityReference.Value -and
            $_.AccessControlType -eq $rule.AccessControlType -and
            $_.FileSystemRights -eq $rule.FileSystemRights -and
            $_.InheritanceFlags -eq $rule.InheritanceFlags -and
            $_.PropagationFlags -eq $rule.PropagationFlags
        })
        if ($matches.Count -ne 1) { return $false }
    }
    return $true
}
function Test-PathsOverlap([string]$Left, [string]$Right) {
    $leftValue = [IO.Path]::GetFullPath($Left).Replace('/', '\').ToLowerInvariant()
    $rightValue = [IO.Path]::GetFullPath($Right).Replace('/', '\').ToLowerInvariant()
    $leftRoot = [IO.Path]::GetPathRoot($leftValue)
    $rightRoot = [IO.Path]::GetPathRoot($rightValue)
    if ($leftValue.Length -gt $leftRoot.Length) { $leftValue = $leftValue.TrimEnd('\') }
    if ($rightValue.Length -gt $rightRoot.Length) { $rightValue = $rightValue.TrimEnd('\') }
    if ($leftValue -eq $rightValue) { return $true }
    return $leftValue.StartsWith($rightValue.TrimEnd('\') + '\') -or $rightValue.StartsWith($leftValue.TrimEnd('\') + '\')
}
function Assert-ConfigDocument($Config, [string]$ExpectedServiceSid) {
    if ($Config.version -ne 1) { throw "Unsupported trusted service configuration version." }
    if (-not (Test-ServiceSidShape ([string]$Config.service_sid)) -or -not ([string]$Config.service_sid).Equals($ExpectedServiceSid, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Trusted service configuration does not identify the installed WebDynamiteCore service."
    }
    if (-not (Test-UserSid ([string]$Config.interactive_user_sid))) { throw "Trusted service configuration contains an invalid interactive user SID." }
    if ([uint32]$Config.interactive_session_id -eq 0) { throw "An interactive Windows session ID is required." }
    $desktop = Get-NormalizedExecutablePath ([string]$Config.desktop_executable)
    $pin = Normalize-Thumbprint ([string]$Config.publisher_sha256)
    if ($pin -ne ([string]$Config.publisher_sha256).ToLowerInvariant()) { throw "Trusted service configuration publisher thumbprint is not normalized." }
    $secret = [string]$Config.secret_store_root
    $data = [string]$Config.data_root
    if (-not [IO.Path]::IsPathRooted($secret) -or -not [IO.Path]::IsPathRooted($data) -or (Test-PathsOverlap $secret $data)) {
        throw "Trusted service secret and data roots must be absolute and non-overlapping."
    }
    if ($desktop -ne [string]$Config.desktop_executable) { throw "Trusted desktop executable path is not normalized." }
}
function Get-CurrentInteractiveIdentity {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $identity.User) { throw "Could not resolve the installing user's Windows SID." }
    $sid = $identity.User.Value
    $session = [Diagnostics.Process]::GetCurrentProcess().SessionId
    if (-not (Test-UserSid $sid) -or $session -eq 0) { throw "Trusted service provisioning must run in the installing interactive user's Windows session." }
    return [pscustomobject]@{ UserSid = $sid; SessionId = [uint32]$session }
}
function Read-TrustedConfig {
    if (-not (Test-Path -LiteralPath $ConfigPath -PathType Leaf)) { throw "Trusted service configuration is missing: $ConfigPath" }
    Assert-NoReparsePathComponents $ConfigPath
    $sid = Get-WebDynamiteServiceSid
    if (-not (Test-ProtectedConfigAcl $ConfigPath $sid)) { throw "Trusted service configuration file owner or DACL is not the exact protected installer contract." }
    $bytes = [IO.File]::ReadAllBytes($ConfigPath)
    if ($bytes.Length -eq 0 -or $bytes.Length -gt 16KB) { throw "Trusted service configuration must be between 1 byte and 16 KiB." }
    $config = [Text.Encoding]::UTF8.GetString($bytes) | ConvertFrom-Json
    Assert-ConfigDocument $config $sid
    return $config
}
function Invoke-Prepare {
    Assert-Administrator
    if (-not $env:ProgramData -or -not (Test-Path -LiteralPath $ConfigDirectory -PathType Container)) {
        throw "The protected ProgramData layout must be prepared before trusted config provisioning."
    }
    & (Join-Path $PSScriptRoot "provision-service-layout.ps1") -Mode Verify | Out-Null
    $serviceSid = Get-WebDynamiteServiceSid
    $desktopPath = Get-NormalizedExecutablePath $DesktopExecutable
    $servicePath = Get-NormalizedExecutablePath $ServiceBinary
    if (-not (Test-Path -LiteralPath $desktopPath -PathType Leaf) -or -not (Test-Path -LiteralPath $servicePath -PathType Leaf)) {
        throw "Both installed desktop and Core Service executables must exist."
    }
    $desktopPublisher = Get-PublisherSha256 $desktopPath
    $servicePublisher = Get-PublisherSha256 $servicePath
    if ($desktopPublisher -ne $servicePublisher) { throw "Desktop and Core Service executables must be signed by the same pinned publisher." }
    $interactive = Get-CurrentInteractiveIdentity
    $config = [ordered]@{
        version = 1
        service_sid = $serviceSid
        interactive_user_sid = $interactive.UserSid
        interactive_session_id = $interactive.SessionId
        desktop_executable = $desktopPath
        publisher_sha256 = $desktopPublisher
        secret_store_root = [IO.Path]::GetFullPath($SecretStoreRoot)
        data_root = [IO.Path]::GetFullPath($DataRoot)
    }
    Assert-ConfigDocument ([pscustomobject]$config) $serviceSid
    $json = $config | ConvertTo-Json -Depth 4
    $bytes = (New-Object Text.UTF8Encoding($false)).GetBytes($json)
    if ($bytes.Length -eq 0 -or $bytes.Length -gt 16KB) { throw "Generated trusted service configuration exceeds 16 KiB." }
    $temporary = Join-Path $ConfigDirectory ("service.json." + [Guid]::NewGuid().ToString("N") + ".tmp")
    $backup = Join-Path $ConfigDirectory ("service.json." + [Guid]::NewGuid().ToString("N") + ".bak")
    $replaced = $false
    try {
        $created = New-Object IO.FileStream($temporary, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        $created.Dispose()
        Set-ProtectedConfigAcl $temporary $serviceSid
        $stream = [IO.File]::Open($temporary, [IO.FileMode]::Open, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
        if (-not (Test-ProtectedConfigAcl $temporary $serviceSid)) { throw "Temporary trusted configuration did not receive the exact protected DACL." }
        if (Test-Path -LiteralPath $ConfigPath) {
            Assert-NoReparsePathComponents $ConfigPath
            if (-not (Test-ProtectedConfigAcl $ConfigPath $serviceSid)) { throw "Existing trusted configuration has an unexpected owner or DACL; refusing to replace it." }
            [IO.File]::Replace($temporary, $ConfigPath, $backup, $true)
            $replaced = $true
        } else {
            [IO.File]::Move($temporary, $ConfigPath)
            $replaced = $true
        }
        $null = Read-TrustedConfig
        if (Test-Path -LiteralPath $backup) { Remove-Item -LiteralPath $backup -Force }
    } catch {
        if ($replaced -and (Test-Path -LiteralPath $backup)) {
            try { [IO.File]::Replace($backup, $ConfigPath, $null, $true) }
            catch { throw "Trusted config verification failed and automatic rollback also failed. Preserve $backup and investigate. Original error: $($_.Exception.Message)" }
        }
        throw
    } finally {
        if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Force }
    }
    Write-Output "Trusted WebDynamite Core Service configuration was atomically provisioned and verified. The service was not started."
}
if ($Mode -eq "SelfTest") {
    if (-not (Test-ServiceSidShape "S-1-5-80-1-2-3-4-5")) { throw "Valid service SID rejected." }
    if (Test-ServiceSidShape "S-1-5-21-1-2-3-1001") { throw "User SID accepted as a service SID." }
    if (-not (Test-UserSid "S-1-5-21-1-2-3-1001")) { throw "Valid user SID rejected." }
    if (Test-UserSid "S-1-5-80-1-2-3-4-5") { throw "Service SID accepted as a user SID." }
    if ((Normalize-Thumbprint ("01" * 32)).Length -ne 64) { throw "Valid SHA-256 thumbprint rejected." }
    foreach ($bad in @("", ("00" * 32), "not-a-thumbprint")) {
        $rejected = $false
        try { $null = Normalize-Thumbprint $bad } catch { $rejected = $true }
        if (-not $rejected) { throw "Invalid publisher thumbprint accepted." }
    }
    if (Test-PathsOverlap "C:\ProgramData\WebDynamite\Secrets" "C:\ProgramData\WebDynamite\Data") { throw "Sibling data roots incorrectly overlap." }
    if (-not (Test-PathsOverlap "C:\ProgramData\WebDynamite" "C:\ProgramData\WebDynamite\Data")) { throw "Nested data roots were not detected." }
    if (-not (Test-PathsOverlap "C:\" "C:\ProgramData\WebDynamite\Data")) { throw "Drive-root overlap was not detected." }
    $sample = [pscustomobject]@{
        version=1; service_sid="S-1-5-80-1-2-3-4-5"; interactive_user_sid="S-1-5-21-1-2-3-1001"
        interactive_session_id=1; desktop_executable="C:\Program Files\WebDynamite\WebDynamite.exe"
        publisher_sha256=("01" * 32); secret_store_root="C:\ProgramData\WebDynamite\Secrets"
        data_root="C:\ProgramData\WebDynamite\Data"
    }
    Assert-ConfigDocument $sample "S-1-5-80-1-2-3-4-5"
    $bad = $sample | Select-Object *
    $bad.service_sid = "S-1-5-80-9-8-7-6-5"
    $rejected = $false
    try { Assert-ConfigDocument $bad "S-1-5-80-1-2-3-4-5" } catch { $rejected = $true }
    if (-not $rejected) { throw "Mismatched service SID was accepted." }
    Write-Output "Trusted Windows service configuration provisioning self-test passed."
    exit 0
}
if ($Mode -eq "Prepare") { Invoke-Prepare; exit 0 }
if ($Mode -eq "Verify") {
    Assert-Administrator
    $null = Read-TrustedConfig
    Write-Output "Trusted WebDynamite Core Service configuration, publisher pin, service identity, owner, and DACL verified."
    exit 0
}
