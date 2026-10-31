param(
    [Parameter(Mandatory = $true)][string]$Target,
    [Parameter(Mandatory = $true)][string]$Version
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $root

$outputDir = 'target/module-e2e'

& (Join-Path $PSScriptRoot 'build-module.ps1') -Crate tinychannels-module -Version $Version -Target $Target -HostKey ci -OutputDir $outputDir
if ($LASTEXITCODE -ne 0) { throw 'tinychannels module build failed' }
& (Join-Path $PSScriptRoot 'verify-module.ps1') -Archive "$outputDir/tinychannels-module-$Version-ci.zip"
if ($LASTEXITCODE -ne 0) { throw 'tinychannels module end-to-end verification failed' }
& (Join-Path $PSScriptRoot 'test-client-e2e.ps1')
if ($LASTEXITCODE -ne 0) { throw 'tinychannels client native E2E failed' }
