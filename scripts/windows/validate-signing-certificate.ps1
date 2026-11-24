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

function Assert-PfxBundleSize {
    param([Parameter(Mandatory)][byte[]]$Bytes)
    if ($Bytes.Length -eq 0 -or $Bytes.Length -gt 10485760) {
        throw "The signing certificate bundle must be between 1 byte and 10 MiB."
    }
}

function ConvertFrom-Base64PfxBundle {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Value)
    try { $bytes = [Convert]::FromBase64String($Value) }
    catch { throw "The signing certificate secret is not valid base64." }
    try {
        Assert-PfxBundleSize -Bytes $bytes
        return ,$bytes
    } catch {
        [Array]::Clear($bytes, 0, $bytes.Length)
        throw
    }
}

function Test-CertificatePrivateKey {
    param([Parameter(Mandatory)][System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate)
    if (-not $Certificate.HasPrivateKey) {
        throw "The signing certificate does not contain its private key."
    }
}

function Test-CertificateValidity {
    param(
        [Parameter(Mandatory)][System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate,
        [DateTime]$UtcNow = [DateTime]::UtcNow
    )
    $now = $UtcNow.ToUniversalTime()
    if ($now -lt $Certificate.NotBefore.ToUniversalTime() -or $now -gt $Certificate.NotAfter.ToUniversalTime()) {
        throw "The signing certificate is not currently valid."
    }
}

function Test-CodeSigningEku {
    param([Parameter(Mandatory)][System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate)
    $extension = $Certificate.Extensions | Where-Object { $_.Oid.Value -eq "2.5.29.37" } | Select-Object -First 1
    if ($null -eq $extension) { throw "The certificate has no Extended Key Usage; Code Signing must be explicitly permitted." }
    $eku = [System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($extension, $extension.Critical)
    if (-not ($eku.EnhancedKeyUsages | Where-Object { $_.Value -eq "1.3.6.1.5.5.7.3.3" })) {
        throw "The certificate EKU does not permit Code Signing."
    }
}

function Test-CodeSigningKeyUsage {
    param([Parameter(Mandatory)][System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate)
    $extension = $Certificate.Extensions | Where-Object { $_.Oid.Value -eq "2.5.29.15" } | Select-Object -First 1
    if ($null -eq $extension) { throw "The certificate has no Key Usage extension; Digital Signature must be explicitly permitted." }
    $usage = [System.Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new($extension, $extension.Critical)
    $required = [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature
    if (($usage.KeyUsages -band $required) -ne $required) { throw "The certificate Key Usage does not permit Digital Signature." }
}

function Get-CertificateSha256Pin {
    param([Parameter(Mandatory)][System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try { return [Convert]::ToHexString($sha.ComputeHash($Certificate.RawData)).ToLowerInvariant() }
    finally { $sha.Dispose() }
}

function Test-CertificatePublisherPin {
    param(
        [Parameter(Mandatory)][System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate,
        [Parameter(Mandatory)][string]$ExpectedSha256
    )
    $expected = Normalize-Sha256Thumbprint -Value $ExpectedSha256
    $actual = Get-CertificateSha256Pin -Certificate $Certificate
    if ($actual -ne $expected) { throw "The certificate SHA-256 publisher pin does not match the configured expected pin." }
    return $actual
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
    $pfxBytes = ConvertFrom-Base64PfxBundle -Value $PfxBase64

    $certificate = $null
    try {
        $flags = [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet
        $certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($pfxBytes, $PfxPassword, $flags)
        Test-CertificatePrivateKey -Certificate $certificate
        Test-CertificateValidity -Certificate $certificate
        Test-CodeSigningEku -Certificate $certificate
        Test-CodeSigningKeyUsage -Certificate $certificate
        Test-TrustedCodeSigningChain -Certificate $certificate

        return Test-CertificatePublisherPin -Certificate $certificate -ExpectedSha256 $expected
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
    $rejected = $false
    try { ConvertFrom-Base64PfxBundle -Value "not-base64!" | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw "Malformed base64 PFX input was accepted." }

    $rejected = $false
    try { ConvertFrom-Base64PfxBundle -Value "" | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw "An empty PFX bundle was accepted." }

    $oversizedPfxBytes = [byte[]]::new(10485761)
    try {
        $rejected = $false
        try { Assert-PfxBundleSize -Bytes $oversizedPfxBytes } catch { $rejected = $true }
        if (-not $rejected) { throw "A PFX bundle larger than 10 MiB was accepted." }
    } finally { [Array]::Clear($oversizedPfxBytes, 0, $oversizedPfxBytes.Length) }

    function New-KeyUsageTestCertificate {
        param(
            [switch]$IncludeKeyUsage,
            [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]$KeyUsage = [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::None
        )
        $key = [System.Security.Cryptography.RSA]::Create(2048)
        try {
            $request = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(
                "CN=WebDynamite Signing Preflight Self-Test",
                $key,
                [System.Security.Cryptography.HashAlgorithmName]::SHA256,
                [System.Security.Cryptography.RSASignaturePadding]::Pkcs1
            )
            if ($IncludeKeyUsage) {
                $request.CertificateExtensions.Add(
                    [System.Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new($KeyUsage, $true)
                )
            }
            return $request.CreateSelfSigned([DateTimeOffset]::UtcNow.AddDays(-1), [DateTimeOffset]::UtcNow.AddDays(1))
        } finally {
            $key.Dispose()
        }
    }

    $digitalSignature = [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature
    $usageCertificate = New-KeyUsageTestCertificate -IncludeKeyUsage -KeyUsage $digitalSignature
    try { Test-CodeSigningKeyUsage -Certificate $usageCertificate }
    finally { $usageCertificate.Dispose() }

    $encipherment = [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::KeyEncipherment
    $usageCertificate = New-KeyUsageTestCertificate -IncludeKeyUsage -KeyUsage $encipherment
    try {
        $rejected = $false
        try { Test-CodeSigningKeyUsage -Certificate $usageCertificate } catch { $rejected = $true }
        if (-not $rejected) { throw "Key-encipherment-only certificate was accepted." }
    } finally { $usageCertificate.Dispose() }

    $usageCertificate = New-KeyUsageTestCertificate
    try {
        $rejected = $false
        try { Test-CodeSigningKeyUsage -Certificate $usageCertificate } catch { $rejected = $true }
        if (-not $rejected) { throw "Certificate without a Key Usage extension was accepted." }
    } finally { $usageCertificate.Dispose() }
    function New-ValidityTestCertificate {
        param([DateTimeOffset]$NotBefore, [DateTimeOffset]$NotAfter)
        $key = [System.Security.Cryptography.RSA]::Create(2048)
        try {
            $request = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(
                "CN=WebDynamite Validity Preflight Self-Test",
                $key,
                [System.Security.Cryptography.HashAlgorithmName]::SHA256,
                [System.Security.Cryptography.RSASignaturePadding]::Pkcs1
            )
            return $request.CreateSelfSigned($NotBefore, $NotAfter)
        } finally { $key.Dispose() }
    }

    $privateKeyCertificate = New-ValidityTestCertificate -NotBefore ([DateTimeOffset]::UtcNow.AddDays(-1)) -NotAfter ([DateTimeOffset]::UtcNow.AddDays(1))
    try {
        Test-CertificatePrivateKey -Certificate $privateKeyCertificate
        $publicOnlyCertificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($privateKeyCertificate.RawData)
        try {
            $rejected = $false
            try { Test-CertificatePrivateKey -Certificate $publicOnlyCertificate } catch { $rejected = $true }
            if (-not $rejected) { throw "A certificate without a private key was accepted." }
        } finally { $publicOnlyCertificate.Dispose() }
    } finally { $privateKeyCertificate.Dispose() }

    $pinTestCertificate = New-ValidityTestCertificate -NotBefore ([DateTimeOffset]::UtcNow.AddDays(-1)) -NotAfter ([DateTimeOffset]::UtcNow.AddDays(1))
    try {
        $expectedPin = Get-CertificateSha256Pin -Certificate $pinTestCertificate
        $actualPin = Test-CertificatePublisherPin -Certificate $pinTestCertificate -ExpectedSha256 $expectedPin
        if ($actualPin -ne $expectedPin) { throw "Matching publisher pin was not returned." }
        $formattedPin = ($expectedPin.ToUpperInvariant().ToCharArray() | ForEach-Object -Begin { $index = 0 } -Process {
            if ($index -gt 0 -and ($index % 2) -eq 0) { ":" }
            $index++
            [string]$_
        }) -join ""
        $formattedResult = Test-CertificatePublisherPin -Certificate $pinTestCertificate -ExpectedSha256 (" " + $formattedPin + "`t")
        if ($formattedResult -ne $expectedPin) { throw "Uppercase, colon-separated publisher pin was not normalized." }
        $wrongPin = if ($expectedPin[0] -eq "0") { "1" + $expectedPin.Substring(1) } else { "0" + $expectedPin.Substring(1) }
        $rejected = $false
        try { Test-CertificatePublisherPin -Certificate $pinTestCertificate -ExpectedSha256 $wrongPin | Out-Null } catch { $rejected = $true }
        if (-not $rejected) { throw "A mismatched publisher pin was accepted." }
    } finally { $pinTestCertificate.Dispose() }

    $testNow = [DateTime]::UtcNow
    $validityCertificate = New-ValidityTestCertificate -NotBefore ([DateTimeOffset]::UtcNow.AddDays(-1)) -NotAfter ([DateTimeOffset]::UtcNow.AddDays(1))
    try { Test-CertificateValidity -Certificate $validityCertificate -UtcNow $testNow }
    finally { $validityCertificate.Dispose() }

    $validityCertificate = New-ValidityTestCertificate -NotBefore ([DateTimeOffset]::UtcNow.AddDays(-3)) -NotAfter ([DateTimeOffset]::UtcNow.AddDays(-2))
    try {
        $rejected = $false
        try { Test-CertificateValidity -Certificate $validityCertificate -UtcNow $testNow } catch { $rejected = $true }
        if (-not $rejected) { throw "An expired signing certificate was accepted." }
    } finally { $validityCertificate.Dispose() }

    $validityCertificate = New-ValidityTestCertificate -NotBefore ([DateTimeOffset]::UtcNow.AddDays(2)) -NotAfter ([DateTimeOffset]::UtcNow.AddDays(3))
    try {
        $rejected = $false
        try { Test-CertificateValidity -Certificate $validityCertificate -UtcNow $testNow } catch { $rejected = $true }
        if (-not $rejected) { throw "A not-yet-valid signing certificate was accepted." }
    } finally { $validityCertificate.Dispose() }

    function New-EkuTestCertificate {
        param([switch]$IncludeEku, [string]$EkuOid = "1.3.6.1.5.5.7.3.3")
        $key = [System.Security.Cryptography.RSA]::Create(2048)
        try {
            $request = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(
                "CN=WebDynamite EKU Preflight Self-Test",
                $key,
                [System.Security.Cryptography.HashAlgorithmName]::SHA256,
                [System.Security.Cryptography.RSASignaturePadding]::Pkcs1
            )
            if ($IncludeEku) {
                $oids = [System.Security.Cryptography.OidCollection]::new()
                [void]$oids.Add([System.Security.Cryptography.Oid]::new($EkuOid))
                $request.CertificateExtensions.Add(
                    [System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($oids, $true)
                )
            }
            return $request.CreateSelfSigned([DateTimeOffset]::UtcNow.AddDays(-1), [DateTimeOffset]::UtcNow.AddDays(1))
        } finally { $key.Dispose() }
    }

    $ekuCertificate = New-EkuTestCertificate -IncludeEku
    try { Test-CodeSigningEku -Certificate $ekuCertificate }
    finally { $ekuCertificate.Dispose() }

    $ekuCertificate = New-EkuTestCertificate -IncludeEku -EkuOid "1.3.6.1.5.5.7.3.2"
    try {
        $rejected = $false
        try { Test-CodeSigningEku -Certificate $ekuCertificate } catch { $rejected = $true }
        if (-not $rejected) { throw "A non-Code-Signing EKU certificate was accepted." }
    } finally { $ekuCertificate.Dispose() }

    $ekuCertificate = New-EkuTestCertificate
    try {
        $rejected = $false
        try { Test-CodeSigningEku -Certificate $ekuCertificate } catch { $rejected = $true }
        if (-not $rejected) { throw "Certificate without an EKU extension was accepted." }
    } finally { $ekuCertificate.Dispose() }

    Write-Output "Publisher identity validation self-tests passed (PFX base64 and size validation, pin normalization, matching and mismatch, private-key presence, validity dates, Code Signing EKU, Digital Signature, and missing-extension cases)."
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
