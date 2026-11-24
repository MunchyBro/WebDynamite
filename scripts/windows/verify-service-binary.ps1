[CmdletBinding()]
param(
    [switch]$SelfTest,
    [string]$ExpectedPublisherSha256
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ServiceName = "WebDynamiteCore"

function Normalize-Sha256Pin {
    param([Parameter(Mandatory)][string]$Value)
    $normalized = ($Value -replace '[:\s]', '').ToLowerInvariant()
    if ($normalized.Length -ne 64 -or $normalized -notmatch '^[0-9a-f]{64}$') {
        throw "Expected publisher pin must be exactly 64 hexadecimal SHA-256 characters."
    }
    return $normalized
}

function Get-ServiceExecutablePath {
    param([Parameter(Mandatory)][string]$ImagePath)
    $value = $ImagePath.Trim()
    if ([string]::IsNullOrWhiteSpace($value)) { throw "The registered service image path is empty." }

    if ($value.StartsWith('"')) {
        if ($value -notmatch '^"(?<path>[^"]+\.exe)"(?:\s|$)') {
            throw "The registered service image path is not a valid quoted executable path."
        }
        $candidate = $Matches.path
    } else {
        if ($value -notmatch '^(?<path>.+?\.exe)(?:\s|$)') {
            throw "The registered service image path does not identify an executable."
        }
        $candidate = $Matches.path
    }

    if (-not [IO.Path]::IsPathRooted($candidate) -or [IO.Path]::GetExtension($candidate) -ne ".exe") {
        throw "The registered service executable path must be an absolute .exe path."
    }
    return [IO.Path]::GetFullPath($candidate)
}

function Test-NoReparsePathComponents {
    param([Parameter(Mandatory)][string]$Path)
    $full = [IO.Path]::GetFullPath($Path)
    $root = [IO.Path]::GetPathRoot($full)
    $current = $root
    foreach ($part in ($full.Substring($root.Length) -split '[\\/]' | Where-Object { $_ })) {
        $current = Join-Path $current $part
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Refusing a service executable path containing a reparse point: $current"
            }
        }
    }
}

function Get-CertificateSha256 {
    param([Parameter(Mandatory)][Security.Cryptography.X509Certificates.X509Certificate2]$Certificate)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return [Convert]::ToHexString($sha.ComputeHash($Certificate.RawData)).ToLowerInvariant() }
    finally { $sha.Dispose() }
}

function Test-InstalledServiceBinary {
    param([Parameter(Mandatory)][string]$ExpectedPin)
    $expected = Normalize-Sha256Pin -Value $ExpectedPin
    $service = Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'"
    if (-not $service) { throw "The WebDynamiteCore Windows service is not installed." }

    $binaryPath = Get-ServiceExecutablePath -ImagePath ([string]$service.PathName)
    $installRoot = [IO.Path]::GetFullPath((Join-Path $env:ProgramFiles "WebDynamite")).TrimEnd('\') + '\'
    if (-not $binaryPath.StartsWith($installRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw "The registered service executable is outside the protected WebDynamite installation directory."
    }
    if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
        throw "The registered WebDynamiteCore executable does not exist."
    }
    Test-NoReparsePathComponents -Path $binaryPath

    $signature = Get-AuthenticodeSignature -LiteralPath $binaryPath
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "The registered WebDynamiteCore executable does not have a valid trusted Authenticode signature (status: $($signature.Status))."
    }
    if ($null -eq $signature.SignerCertificate) {
        throw "The registered WebDynamiteCore executable has no Authenticode signer certificate."
    }
    $actual = Get-CertificateSha256 -Certificate $signature.SignerCertificate
    if ($actual -ne $expected) {
        throw "The WebDynamiteCore executable signer does not match the configured publisher SHA-256 pin."
    }

    return [pscustomobject]@{
        ServiceName = $ServiceName
        Executable = $binaryPath
        SignatureStatus = [string]$signature.Status
        PublisherSha256 = $actual
    }
}

if ($SelfTest) {
    $pin = "A" * 64
    if ((Normalize-Sha256Pin -Value $pin) -ne $pin.ToLowerInvariant()) { throw "Publisher pin normalization self-test failed." }
    if ((Normalize-Sha256Pin -Value ("aa:" * 31 + "aa")) -ne ("aa" * 32)) { throw "Colon-separated publisher pin self-test failed." }
    foreach ($invalid in @("", "abc", ("g" * 64), ("a" * 40))) {
        $rejected = $false
        try { Normalize-Sha256Pin -Value $invalid | Out-Null } catch { $rejected = $true }
        if (-not $rejected) { throw "Invalid publisher pin was accepted." }
    }
    $quoted = Get-ServiceExecutablePath -ImagePath '"C:\Program Files\WebDynamite\webdynamite-core.exe" --service'
    if ($quoted -ne "C:\Program Files\WebDynamite\webdynamite-core.exe") { throw "Quoted service image path parsing failed." }
    $plain = Get-ServiceExecutablePath -ImagePath 'C:\WebDynamite\webdynamite-core.exe --service'
    if ($plain -ne 'C:\WebDynamite\webdynamite-core.exe') { throw "Unquoted service image path parsing failed." }
    foreach ($invalidPath in @("webdynamite-core.exe", '"C:\bad path\missing quote.exe', "C:\folder\service.cmd")) {
        $rejected = $false
        try { Get-ServiceExecutablePath -ImagePath $invalidPath | Out-Null } catch { $rejected = $true }
        if (-not $rejected) { throw "Invalid service image path was accepted: $invalidPath" }
    }
    Write-Output "Windows service binary signature preflight self-tests passed."
    exit 0
}

if ([string]::IsNullOrWhiteSpace($ExpectedPublisherSha256)) {
    throw "Pass -ExpectedPublisherSha256 using the protected publisher pin provisioned by the installer."
}
$result = Test-InstalledServiceBinary -ExpectedPin $ExpectedPublisherSha256
Write-Output ("WebDynamiteCore binary verified: {0}; signature {1}; publisher SHA-256 {2}." -f $result.Executable,$result.SignatureStatus,$result.PublisherSha256)
