# Interactive offline signing. Passwords go only to the verified official minisign.
# Never invoke this script from a captured agent terminal.
[CmdletBinding()]
param(
    [ValidatePattern('^v[0-9]+\.[0-9]+\.[0-9]+$')]
    [string]$Tag = 'v0.2.0',
    [Parameter(Mandatory)]
    [ValidatePattern('^[a-fA-F0-9]{64}$')]
    [string]$ManifestSha256,
    [Parameter(Mandatory)]
    [ValidatePattern('^[a-fA-F0-9]{64}$')]
    [string]$DrillSha256
)
$ErrorActionPreference = 'Stop'
try {
    $repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
    $releaseDir = Join-Path $repo "release-work\$Tag"
    $drillDir = Join-Path $repo "release-work\key-drill-$Tag"
    $keyRoot = Join-Path $env:USERPROFILE 'MKLM-signing'
    $signer = Join-Path $keyRoot 'tools\minisign.exe'
    $arch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    $expectedSigner = switch ($arch) {
        'X64' { '5535BE9E4E123831EBE6EF324AAFE9DDE507015C176191F9E20C3AD60567F9E1' }
        'Arm64' { 'F39E065E649D5ED7075675ACCFE0ADA234175D63479DF650654EC4365D7C4513' }
        default { throw "Unsupported signing architecture: $arch" }
    }
    if ((Get-FileHash -LiteralPath $signer -Algorithm SHA256).Hash -ne $expectedSigner) { throw 'Official minisign hash mismatch.' }
    $manifestPath = Join-Path $releaseDir 'latest.json'
    $noncePath = Join-Path $drillDir 'nonce.bin'
    if ((Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash -ne $ManifestSha256) { throw 'Prepared manifest hash mismatch.' }
    if ((Get-FileHash -LiteralPath $noncePath -Algorithm SHA256).Hash -ne $DrillSha256) { throw 'Backup drill hash mismatch.' }
    $manifest = Get-Content -Encoding UTF8 -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    $version = $Tag.Substring(1)
    if ($manifest.version -cne $version) { throw 'Manifest version mismatch.' }
    $issuedAt = [string]$manifest.issued_at
    if ($issuedAt -notmatch '^[0-9]+$') { throw 'Invalid issued_at.' }
    $comment = "mklm-latest-json v1 version=$version issued_at=$issuedAt"
    $preparedComment = (Get-Content -Encoding UTF8 -Raw -LiteralPath (Join-Path $releaseDir 'trusted-comment.txt')).TrimEnd("`r", "`n")
    if ($preparedComment -cne $comment) { throw 'Prepared trusted comment mismatch.' }
    $signaturePath = Join-Path $releaseDir 'latest.json.minisig'
    $drillSignature = Join-Path $drillDir 'nonce.bin.minisig'
    if ((Test-Path -LiteralPath $signaturePath) -or (Test-Path -LiteralPath $drillSignature)) { throw 'Signatures already exist. Preserve them and ask for verification/resume; do not overwrite.' }
    Write-Host "Release: $Tag"
    Write-Host "Manifest SHA-256: $ManifestSha256"
    Write-Host "Trusted comment: $comment"
    Write-Host 'Compare these values with the prepared-release summary.'
    Write-Host 'Disconnect Wi-Fi / Ethernet; close MKLM, editor, browser and coding-agent app.'
    Write-Host 'Keep only this console open. Passwords are entered directly into minisign.'
    if ((Read-Host 'After completing those steps, type OFFLINE') -cne 'OFFLINE') { throw 'Cancelled; no signatures written.' }
    if (Get-Process mklm,mklm-cli,mklm-helper,cargo,rustc -ErrorAction SilentlyContinue) { throw 'Close MKLM and Cargo processes before signing.' }
    # Check again immediately before signing public data. Never read/copy .key files here.
    if ((Get-FileHash -LiteralPath $signer -Algorithm SHA256).Hash -ne $expectedSigner) { throw 'minisign changed.' }
    if ((Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash -ne $ManifestSha256) { throw 'Manifest changed.' }
    if ((Get-FileHash -LiteralPath $noncePath -Algorithm SHA256).Hash -ne $DrillSha256) { throw 'Drill changed.' }
    Write-Host 'Backup-key drill: enter the BACKUP password.'
    & $signer -S -s (Join-Path $keyRoot 'backup\mklm-backup.key') -m $noncePath -x $drillSignature -t 'mklm-key-drill v1'
    if ($LASTEXITCODE -ne 0) { throw 'Backup drill signing failed.' }
    Write-Host 'Release signing: enter the PRIMARY password.'
    & $signer -S -s (Join-Path $keyRoot 'primary\mklm-primary.key') -m $manifestPath -x $signaturePath -t $comment
    if ($LASTEXITCODE -ne 0) { throw 'Release signing failed. Preserve any signatures already written.' }
    Write-Host 'Signing complete. Reconnect the network, reopen the coding-agent app and report completion.'
} catch {
    Write-Host "Stopped: $($_.Exception.Message)" -ForegroundColor Red
}
[void](Read-Host 'Press Enter to close this console')
