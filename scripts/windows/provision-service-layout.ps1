[CmdletBinding()]
param([ValidateSet("Prepare", "Verify", "VerifyService", "SelfTest")][string]$Mode = "Verify")
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ServiceName = "WebDynamiteCore"
$DataRoot = Join-Path $env:ProgramData "WebDynamite"
$ManagedDirectories = @($DataRoot,(Join-Path $DataRoot "Config"),(Join-Path $DataRoot "Data"),(Join-Path $DataRoot "Secrets"))
function Test-ServiceSidShape([string]$Sid) { return $Sid -match '^S-1-5-80-(\d+-){4}\d+$' }
function Get-WebDynamiteServiceSid {
    $account = New-Object System.Security.Principal.NTAccount("NT SERVICE\$ServiceName")
    $sid = $account.Translate([System.Security.Principal.SecurityIdentifier]).Value
    if (-not (Test-ServiceSidShape $sid)) { throw "Windows did not resolve the expected virtual service SID." }
    return $sid
}

function Test-InstalledServiceIdentity {
    $service = Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'"
    if (-not $service) { throw "The WebDynamiteCore Windows service is not installed." }
    $expectedAccount = "NT SERVICE\$ServiceName"
    if (-not [string]::Equals([string]$service.StartName, $expectedAccount, [StringComparison]::OrdinalIgnoreCase)) {
        throw "WebDynamiteCore must run as $expectedAccount; refusing LocalSystem or another account."
    }
    $sidOutput = & sc.exe qsidtype $ServiceName 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Could not query the WebDynamiteCore service SID type: $sidOutput" }
    $sidText = ($sidOutput | Out-String)
    if ($sidText -notmatch '(?im)SERVICE_SID_TYPE:\s*UNRESTRICTED') {
        throw "WebDynamiteCore must have an unrestricted service SID enabled. sc.exe output: $sidText"
    }
    $resolvedSid = Get-WebDynamiteServiceSid
    if (-not (Test-ServiceSidShape $resolvedSid)) { throw "The installed service account did not resolve to a service SID." }
    return [pscustomobject]@{ Name=$ServiceName; StartName=$service.StartName; ServiceSid=$resolvedSid; SidType="UNRESTRICTED"; State=$service.State }
}

function Get-ExpectedSddl([string]$ServiceSid) {
    if (-not (Test-ServiceSidShape $ServiceSid)) { throw "A valid WebDynamite service SID is required." }
    return "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;$ServiceSid)"
}
function Test-PathHasNoReparseComponents([string]$Path) {
    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)
    $current = $root
    foreach ($part in ($full.Substring($root.Length) -split '[\\/]' | Where-Object { $_ })) {
        $current = Join-Path $current $part
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Refusing a reparse-point path component: $current" }
        }
    }
}
function Assert-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw "Run this provisioning step from an elevated installer process." }
}
function Set-ProtectedDirectoryAcl([string]$Path, [string]$Sddl) {
    $security = New-Object System.Security.AccessControl.DirectorySecurity
    $security.SetSecurityDescriptorSddlForm($Sddl,[System.Security.AccessControl.AccessControlSections]::Access)
    Set-Acl -LiteralPath $Path -AclObject $security
}
function Test-ProtectedDirectoryAcl([string]$Path, [string]$Sddl) {
    $actual = Get-Acl -LiteralPath $Path
    $expected = New-Object System.Security.AccessControl.DirectorySecurity
    $expected.SetSecurityDescriptorSddlForm($Sddl,[System.Security.AccessControl.AccessControlSections]::Access)
    $expectedRules = @($expected.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]))
    $actualRules = @($actual.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]))
    if (-not $actual.AreAccessRulesProtected -or $actualRules.Count -ne 3) { return $false }
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
if ($Mode -eq "VerifyService") {
    Assert-Administrator
    $identity = Test-InstalledServiceIdentity
    Write-Output ("WebDynamiteCore identity verified: {0}; SID {1}; SID type {2}; state {3}." -f $identity.StartName,$identity.ServiceSid,$identity.SidType,$identity.State)
    exit 0
}
if ($Mode -eq "SelfTest") {
    if (-not (Test-ServiceSidShape "S-1-5-80-123-456-789-101112-131415")) { throw "Valid SID rejected." }
    if (Test-ServiceSidShape "S-1-5-32-544") { throw "Administrators SID incorrectly accepted." }
    $sddl = Get-ExpectedSddl "S-1-5-80-123-456-789-101112-131415"
    if ($sddl -ne "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;S-1-5-80-123-456-789-101112-131415)") { throw "Protected ACL contract mismatch." }
    Write-Output "Windows service provisioning self-test passed."
    exit 0
}
Assert-Administrator
if (-not $env:ProgramData) { throw "ProgramData is unavailable." }
$expectedRoot = [System.IO.Path]::GetFullPath((Join-Path $env:ProgramData "WebDynamite"))
if ([System.IO.Path]::GetFullPath($DataRoot) -ne $expectedRoot) { throw "Unexpected data root." }
foreach ($directory in $ManagedDirectories) { Test-PathHasNoReparseComponents $directory }
$serviceSid = Get-WebDynamiteServiceSid
$sddl = Get-ExpectedSddl $serviceSid
if ($Mode -eq "Prepare") {
    foreach ($directory in $ManagedDirectories) {
        if (-not (Test-Path -LiteralPath $directory)) { New-Item -ItemType Directory -Path $directory -Force | Out-Null }
        Set-ProtectedDirectoryAcl $directory $sddl
    }
}
$failures = @()
foreach ($directory in $ManagedDirectories) {
    if (-not (Test-Path -LiteralPath $directory -PathType Container)) { $failures += "Missing directory: $directory"; continue }
    if (-not (Test-ProtectedDirectoryAcl $directory $sddl)) { $failures += "ACL does not match the protected three-principal contract: $directory" }
}
if ($failures.Count -gt 0) { throw ($failures -join [Environment]::NewLine) }
Write-Output "WebDynamite service data directories satisfy the protected ACL contract."
