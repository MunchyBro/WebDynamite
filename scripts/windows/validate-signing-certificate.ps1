[CmdletBinding()]
param([switch]$SelfTest)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Normalize-Sha256Thumbprint {
    param([Parameter(Mandatory)][string]$Value)
    $normalized = ($Value -replace '[:\s]', '').ToLowerInvariant()
    if ($normalized.Length -ne 64 -or $normalized -notmatch '^[0-9a-f]{64}$') {
        throw "Publisher pin must be exactly 64 hexadecimal SHA-256 characters."
    }
    return $normalized
}

function Test-CodeSigningKeyUsage {
    param([Parameter(Mandatory)][System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate)
    $extension = $Certificate.Extensions | Where-Object { $_.Oid.Value -eq "2.5.29.15" } | Select-Object -First 1
    if ($null -eq $extension) { throw "The certificate has no Key Usage extension; Digital Signature must be explicitly permitted." }
    $usage = [System.Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new($extension, $extension.Critical)
    $required = [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature
    if (($usage.KeyUsages -band $required) -ne $required) { throw "The certificate Key Usage does not permit Digital Signature." }
}

function Test-TrustedCodeSigningChain {
    param([Parameter(Mandatory)][System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate)
    $chain = [System.Security.Cryptography.X509Certificates.X509Chain]::new()
    try {
        $chain.ChainPolicy.ApplicationPolicy.Add([System.Security.Cryptography.Oid]::new("1.3.6.1.5.5.7.3.3"))
        $chain.ChainPolicy.RevocationMode = [System.Security.Cryptography.X509Certificates.X509RevocationMode]::Online
        $chain.ChainPolicy.RevocationFlag = [System.Security.Cryptography.X509Certificates.X509RevocationFlag]::ExcludeRoot
        $chain.ChainPolicy.VerificationFlags = [System.Security.Cryptography.X509Certificates.X509VerificationFlags]::NoFlag
        $chain.ChainPolicy.UrlRetrievalTimeout = [TimeSpan]::FromSeconds(15)
        if (-not $chain.Build($Certificate)) {
            $problems = @($chain.ChainStatus | ForEach-Object { "$($_.Status): $($_.StatusInformation.Trim())" }) -join "; "
            if ([string]::IsNullOrWhiteSpace($problems)) { $problems = "Windows could not build a trusted Code Signing certificate chain." }
            throw "The signing certificate chain is not trusted or its revocation status could not be verified: $problems"
        }
    } finally { $chain.Dispose() }
}

function Test-SigningCertificate {
    param(
        [Parameter(Mandatory)][string]$PfxBase64,
        [Parameter(Mandatory)][string]$PfxPassword,
        [Parameter(Mandatory)][string]$ExpectedSha256
    )
    $expected = Normalize-Sha256Thumbprint -Value $ExpectedSha256
    try { $pfxBytes = [Convert]::FromBase64String($PfxBase64) }
    catch { throw "The signing certificate secret is not valid base64." }
    if ($pfxBytes.Length -eq 0 -or $pfxBytes.Length -gt 10485760) {
        throw "The signing certificate bundle must be between 1 byte and 10 MiB."
    }

    $certificate = $null
    try {
        $flags = [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet
        $certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($pfxBytes, $PfxPassword, $flags)
        if (-not $certificate.HasPrivateKey) { throw "The signing certificate does not contain its private key." }
        $now = [DateTime]::UtcNow
        if ($now -lt $certificate.NotBefore.ToUniversalTime() -or $now -gt $certificate.NotAfter.ToUniversalTime()) {
            throw "The signing certificate is not currently valid."
        }

        $ekuExtension = $certificate.Extensions | Where-Object { $_.Oid.Value -eq "2.5.29.37" } | Select-Object -First 1
        if ($null -eq $ekuExtension) {
            throw "The certificate has no Extended Key Usage; Code Signing must be explicitly permitted."
        }
        $eku = [System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($ekuExtension, $ekuExtension.Critical)
        if (-not ($eku.EnhancedKeyUsages | Where-Object { $_.Value -eq "1.3.6.1.5.5.7.3.3" })) {
            throw "The certificate EKU does not permit Code Signing."
        }

        Test-CodeSigningKeyUsage -Certificate $certificate
        Test-TrustedCodeSigningChain -Certificate $certificate

        $sha = [System.Security.Cryptography.SHA256]::Create()
        try { $actual = [Convert]::ToHexString($sha.ComputeHash($certificate.RawData)).ToLowerInvariant() }
        finally { $sha.Dispose() }
        if ($actual -ne $expected) { throw "The certificate SHA-256 publisher pin does not match the configured expected pin." }
        return $actual
    } catch {
        if ($_.Exception.Message -match "signing certificate|Code Signing|publisher pin|private key|currently valid|Extended Key Usage|Key Usage|certificate chain") { throw }
        throw "Could not load or validate the signing certificate. Check the PFX and password secrets."
    } finally {
        if ($null -ne $certificate) { $certificate.Dispose() }
        [Array]::Clear($pfxBytes, 0, $pfxBytes.Length)
    }
}

if ($SelfTest) {
    $valid = "A" * 64
    if ((Normalize-Sha256Thumbprint -Value $valid) -ne $valid.ToLowerInvariant()) { throw "Thumbprint normalization self-test failed." }
    foreach ($invalid in @("", "abc", ("g" * 64), ("a" * 40))) {
        $rejected = $false
        try { Normalize-Sha256Thumbprint -Value $invalid | Out-Null } catch { $rejected = $true }
        if (-not $rejected) { throw "Invalid thumbprint self-test failed to reject input." }
    }
    Write-Output "Publisher identity validation self-tests passed."
    exit 0
}

$pfx = [Environment]::GetEnvironmentVariable("WEBDYNAMITE_SIGNING_PFX_BASE64")
$password = [Environment]::GetEnvironmentVariable("WEBDYNAMITE_SIGNING_PFX_PASSWORD")
$pin = [Environment]::GetEnvironmentVariable("WEBDYNAMITE_PUBLISHER_SHA256")
if ([string]::IsNullOrWhiteSpace($pfx) -or [string]::IsNullOrWhiteSpace($password) -or [string]::IsNullOrWhiteSpace($pin)) {
    throw "Set WEBDYNAMITE_SIGNING_PFX_BASE64, WEBDYNAMITE_SIGNING_PFX_PASSWORD, and WEBDYNAMITE_PUBLISHER_SHA256."
}
$validatedPin = Test-SigningCertificate -PfxBase64 $pfx -PfxPassword $password -ExpectedSha256 $pin
Write-Output "Signing certificate structure, validity, Code Signing EKU, Digital Signature key usage, trusted chain/revocation, private key, and SHA-256 pin validated."
Write-Output "Publisher SHA-256: $validatedPin"
