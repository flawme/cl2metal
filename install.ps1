# cl2metal installer for Windows — downloads the latest release binary.
#
# Run it straight from the web:
#
#   iex (irm https://raw.githubusercontent.com/flawme/cl2metal/master/install.ps1)
#
# Installs to %LOCALAPPDATA%\cl2metal and puts it on the user PATH.
$ErrorActionPreference = 'Stop'

$repo = 'flawme/cl2metal'
$installDir = Join-Path $env:LOCALAPPDATA 'cl2metal'

if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
    Write-Error "cl2metal: no release binary for architecture '$env:PROCESSOR_ARCHITECTURE' - build from source: git clone https://github.com/$repo; cargo build --release"
}

$asset = 'cl2metal-x86_64-pc-windows-msvc.zip'
$url = "https://github.com/$repo/releases/latest/download/$asset"
$zip = Join-Path $env:TEMP $asset

Write-Host "cl2metal: downloading $asset"
try {
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
} catch {
    Write-Error "cl2metal: download failed - is there a published release? see https://github.com/$repo/releases"
}

# Verify the published checksum when it is available.
try {
    $shaFile = "$zip.sha256"
    Invoke-WebRequest -Uri "$url.sha256" -OutFile $shaFile -UseBasicParsing
    $expected = (Get-Content $shaFile | Select-Object -First 1) -replace '\s.*', ''
    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $expected.ToLower()) {
        Remove-Item $zip
        Write-Error 'cl2metal: checksum mismatch'
    }
    Remove-Item $shaFile
} catch {
    Write-Host 'cl2metal: no published checksum - skipping verification'
}

Write-Host "cl2metal: installing to $installDir"
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Expand-Archive -Path $zip -DestinationPath $installDir -Force
Remove-Item $zip

# Put the install directory on the user PATH when it is not there yet.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$installDir*") {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$installDir", 'User')
    Write-Host "cl2metal: added $installDir to your user PATH - open a new terminal for it to take effect"
}

& (Join-Path $installDir 'cl2metal.exe') --version
Write-Host 'cl2metal: run "cl2metal --install-desktop" to add the Start Menu entry'
