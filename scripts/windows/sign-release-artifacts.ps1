[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet("Sign", "Verify", "SelfTest")]
    [string]$Mode,
    [string[]]$Paths = @()
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Normalize-PublisherPin {
    param([Parameter(Mandatory)][string]$Value)
    $pin = ($Value -replace '[:\s]', '').ToLowerInvariant()
    if ($pin -notmatch '^[0-9a-f]{64}$' -or $pin -eq ('0' * 64)) {
        throw "Publisher pin must be a nonzero 64-character SHA-256 hex value."
    }
    return $pin
}

function Get-SignToolPath {
    $command = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $programFilesX86 = [Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFilesX86)
    $kits = Join-Path $programFilesX86 "Windows Kits\10\bin"
    if (Test-Path -LiteralPath $kits) {
        $candidate = Get-ChildItem -Path $kits -Filter signtool.exe -File -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.Directory.Name -eq "x64" } |
            Sort-Object FullName -Descending |
            Select-Object -First 1
        if ($candidate) { return $candidate.FullName }
    }
    throw "Windows SDK signtool.exe was not found on this runner."
}

function Get-ExpectedPin {
    $value = [Environment]::GetEnvironmentVariable("WEBDYNAMITE_PUBLISHER_SHA256")
    if ([string]::IsNullOrWhiteSpace($value)) { throw "WEBDYNAMITE_PUBLISHER_SHA256 is required." }
    return Normalize-PublisherPin -Value $value
}

function Assert-AuthenticodePublisher {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$ExpectedPin)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Release executable does not exist: $Path" }
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Authenticode signature is not valid for $Path (status: $($signature.Status))."
    }
    if ($null -eq $signature.SignerCertificate) { throw "No signer certificate was found for $Path." }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try { $actual = [Convert]::ToHexString($sha.ComputeHash($signature.SignerCertificate.RawData)).ToLowerInvariant() }
    finally { $sha.Dispose() }
    if ($actual -ne $ExpectedPin) { throw "Signer certificate pin mismatch for $Path." }
    return $actual
}

function Invoke-SignArtifacts {
    param([Parameter(Mandatory)][string[]]$ArtifactPaths)
    if ($ArtifactPaths.Count -lt 1) { throw "At least one executable path is required." }
    $expectedPin = Get-ExpectedPin
    $pfxBase64 = [Environment]::GetEnvironmentVariable("WEBDYNAMITE_SIGNING_PFX_BASE64")
    $password = [Environment]::GetEnvironmentVariable("WEBDYNAMITE_SIGNING_PFX_PASSWORD")
    if ([string]::IsNullOrWhiteSpace($pfxBase64) -or [string]::IsNullOrWhiteSpace($password)) {
        throw "Protected PFX base64 and password secrets are required for signing."
    }

    $bytes = [Convert]::FromBase64String($pfxBase64)
    if ($bytes.Length -eq 0 -or $bytes.Length -gt 10485760) {
        [Array]::Clear($bytes, 0, $bytes.Length)
        throw "Signing PFX must be between 1 byte and 10 MiB."
    }

    $pfxPath = Join-Path ([IO.Path]::GetTempPath()) ("webdynamite-signing-" + [guid]::NewGuid().ToString("N") + ".pfx")
    try {
        [IO.File]::WriteAllBytes($pfxPath, $bytes)
        $identity = [Security.Principal.WindowsIdentity]::GetCurrent().Name
        $grant = $identity + ":(R,W)"
        & icacls.exe $pfxPath /inheritance:r /grant:r $grant | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Could not restrict access to the temporary signing PFX." }

        $tool = Get-SignToolPath
        foreach ($path in $ArtifactPaths) {
            $fullPath = [IO.Path]::GetFullPath($path)
            if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) { throw "Release executable does not exist: $fullPath" }
            & $tool sign /q /fd SHA256 /td SHA256 /tr "http://timestamp.digicert.com" /f $pfxPath /p $password $fullPath
            if ($LASTEXITCODE -ne 0) { throw "Authenticode signing failed for $fullPath." }
            $null = Assert-AuthenticodePublisher -Path $fullPath -ExpectedPin $expectedPin
            Write-Output "Signed and verified publisher identity: $([IO.Path]::GetFileName($fullPath))"
        }
    } finally {
        [Array]::Clear($bytes, 0, $bytes.Length)
        if (Test-Path -LiteralPath $pfxPath) { Remove-Item -LiteralPath $pfxPath -Force }
    }
}

if ($Mode -eq "SelfTest") {
    $valid = "ab" * 32
    if ((Normalize-PublisherPin -Value $valid.ToUpperInvariant()) -ne $valid) { throw "Publisher pin normalization self-test failed." }
    if ((Normalize-PublisherPin -Value (("ab:" * 31) + "ab")) -ne $valid) { throw "Colon-separated publisher pin self-test failed." }
    foreach ($invalid in @("", "abc", ("g" * 64), ("0" * 64), ("a" * 40))) {
        $rejected = $false
        try { Normalize-PublisherPin -Value $invalid | Out-Null } catch { $rejected = $true }
        if (-not $rejected) { throw "Invalid publisher pin was accepted." }
    }
    Write-Output "Signed release artifact self-tests passed."
    exit 0
}

if ($Mode -eq "Sign") {
    Invoke-SignArtifacts -ArtifactPaths $Paths
    exit 0
}

if ($Paths.Count -lt 1) { throw "At least one executable path is required for verification." }
$pin = Get-ExpectedPin
$tool = Get-SignToolPath
foreach ($path in $Paths) {
    $fullPath = [IO.Path]::GetFullPath($path)
    if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) { throw "Release executable does not exist: $fullPath" }
    & $tool verify /pa /all /q $fullPath
    if ($LASTEXITCODE -ne 0) { throw "Windows Authenticode trust verification failed for $fullPath." }
    $null = Assert-AuthenticodePublisher -Path $fullPath -ExpectedPin $pin
    Write-Output "Verified signature and pinned publisher: $([IO.Path]::GetFileName($fullPath))"
}
