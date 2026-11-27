[CmdletBinding()]
param(
    [string]$ApiUrl = $env:RESONANCE_DEFAULT_API_URL,
    [switch]$SkipTests
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)) {
    throw 'The Windows installer must be built on Windows with Inno Setup installed.'
}
$parsedApiUrl = $null
if (
    -not [Uri]::TryCreate($ApiUrl, [UriKind]::Absolute, [ref]$parsedApiUrl) -or
    $parsedApiUrl.Scheme -ne [Uri]::UriSchemeHttps -or
    [string]::IsNullOrWhiteSpace($parsedApiUrl.Host) -or
    $parsedApiUrl.Query -or
    $parsedApiUrl.Fragment
) {
    throw 'Set RESONANCE_DEFAULT_API_URL or pass -ApiUrl with the HTTPS PHP API file URL (without a query or fragment).'
}

$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargo) {
    throw 'Rust/Cargo was not found. Install Rust stable and add Cargo to PATH.'
}

$versionLine = Select-String -Path (Join-Path $repositoryRoot 'Cargo.toml') `
    -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $versionLine) {
    throw 'Could not read the package version from Cargo.toml.'
}
$version = $versionLine.Matches[0].Groups[1].Value

$compiler = $null
$compilerCommand = Get-Command ISCC.exe -ErrorAction SilentlyContinue
if ($compilerCommand) {
    $compiler = $compilerCommand.Source
}
else {
    $programDirectories = @(
        [Environment]::GetEnvironmentVariable('ProgramFiles(x86)'),
        $env:ProgramFiles
    ) | Where-Object { $_ }
    foreach ($directory in $programDirectories) {
        $candidate = Join-Path $directory 'Inno Setup 6\ISCC.exe'
        if (Test-Path $candidate) {
            $compiler = $candidate
            break
        }
    }
}
if (-not $compiler) {
    throw 'Inno Setup 6 was not found. Install it, then rerun this script.'
}

$previousApiUrl = $env:RESONANCE_DEFAULT_API_URL
Push-Location $repositoryRoot
try {
    $env:RESONANCE_DEFAULT_API_URL = $ApiUrl

    if (-not $SkipTests) {
        & $cargo.Source test
        if ($LASTEXITCODE -ne 0) {
            throw "cargo test failed with exit code $LASTEXITCODE."
        }
    }

    & $cargo.Source build --release
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release failed with exit code $LASTEXITCODE."
    }

    & $compiler 'installer\windows\Resonance.iss'
    if ($LASTEXITCODE -ne 0) {
        throw "Inno Setup failed with exit code $LASTEXITCODE."
    }
    $artifact = Join-Path (Join-Path $repositoryRoot 'dist') "Resonance-Setup-$version-x64.exe"

    if (-not (Test-Path $artifact -PathType Leaf)) {
        throw "The packaging command completed but the expected installer was not created: $artifact"
    }
    Write-Host "Created installer: $artifact"
}
finally {
    $env:RESONANCE_DEFAULT_API_URL = $previousApiUrl
    Pop-Location
}
