$ErrorActionPreference = "Stop"

$Repo = "jss826/den"
$InstallDir = if ($env:DEN_INSTALL_DIR) { $env:DEN_INSTALL_DIR } else { "$env:LOCALAPPDATA\den" }

# Detect architecture
$Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
switch ($Arch) {
    "X64"   { $Target = "x86_64-pc-windows-msvc" }
    "Arm64" { $Target = "aarch64-pc-windows-msvc" }
    default { Write-Error "Unsupported architecture: $Arch"; exit 1 }
}

# Fetch latest release tag
Write-Host "Fetching latest release..."
$Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
$Tag = $Release.tag_name

if (-not $Tag) {
    Write-Error "Failed to fetch latest release."
    exit 1
}

$Asset = "den-$Target.zip"
$Url = "https://github.com/$Repo/releases/download/$Tag/$Asset"

Write-Host "Installing den $Tag ($Target)..."

# Download and extract
$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $TmpDir | Out-Null

try {
    $ZipPath = Join-Path $TmpDir $Asset
    Invoke-WebRequest -Uri $Url -OutFile $ZipPath -UseBasicParsing
    Expand-Archive -Path $ZipPath -DestinationPath $TmpDir -Force

    # Install
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Move-Item -Path (Join-Path $TmpDir "den.exe") -Destination (Join-Path $InstallDir "den.exe") -Force

    Write-Host "Installed den to $InstallDir\den.exe"
} finally {
    Remove-Item -Path $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
}

# Add to user PATH if not already present
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    $Answer = Read-Host "Add $InstallDir to your PATH? (Y/n)"
    if ($Answer -ne "n") {
        [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
        $env:Path = "$env:Path;$InstallDir"
        Write-Host "Added to PATH. Restart your terminal for it to take effect."
    } else {
        Write-Host "Skipped. Add it manually:`n  `$env:Path += `";$InstallDir`""
    }
}
