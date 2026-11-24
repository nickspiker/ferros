# ferros-flash installer for Windows (PowerShell), the same shape as Photon's install-release.ps1.
# It downloads the tool, checks the download's SHA-256 against the one written into this script at publish time, and installs it for this user only (no administrator).
# The tool then checks everything else it fetches (the boot images, the board manifests) by BLAKE3 against a release signed with the ferros key, before it writes anything to the phone.
# PowerShell has SHA-256 built in and not BLAKE3, so this one check, of the tool itself, is SHA-256; nothing after it is.
# Run: powershell -ExecutionPolicy Bypass -c "irm __BASE__/install.ps1 | iex"

$ErrorActionPreference = "Stop"
trap {
    Write-Host ""
    Write-Host "ERROR: $_" -ForegroundColor Red
    Write-Host "Press Enter to exit..." -ForegroundColor Yellow
    Read-Host
    exit 1
}

Write-Host "ferros-flash installer" -ForegroundColor Cyan
Write-Host ""

$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne "AMD64") {
    Write-Host "This first build of ferros-flash is for 64-bit Intel and AMD Windows only (this computer is $arch)." -ForegroundColor Red
    exit 1
}

# Written by tools/flash/publish.sh: the host and the executable's SHA-256.
$base = "__BASE__"
$expectedHash = "__SHA256_WINDOWS_X86_64__"
if ($expectedHash -notmatch "^[0-9A-F]{64}$") {
    Write-Host "This copy of the installer carries no hash: it was not published. Nothing was installed." -ForegroundColor Red
    exit 1
}

$installDir = "$env:LOCALAPPDATA\Programs\ferros-flash"
New-Item -ItemType Directory -Path $installDir -Force | Out-Null
$exe = "$installDir\ferros-flash.exe"

Write-Host "Downloading ferros-flash..." -ForegroundColor Yellow
Invoke-WebRequest -Uri "$base/ferros-flash-windows-x86_64.exe" -OutFile $exe -ErrorAction Stop

Write-Host "Checking the download..." -ForegroundColor Yellow
$actualHash = (Get-FileHash $exe -Algorithm SHA256).Hash
if ($actualHash -ne $expectedHash) {
    Write-Host "The download is not the published ferros-flash." -ForegroundColor Red
    Write-Host "  expected $expectedHash" -ForegroundColor Red
    Write-Host "  got      $actualHash" -ForegroundColor Red
    Remove-Item $exe -ErrorAction SilentlyContinue
    exit 1
}
Write-Host "[OK] The download is the published ferros-flash" -ForegroundColor Green

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$installDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$installDir", "User")
    $env:Path += ";$installDir"
}

Write-Host ""
Write-Host "Installed to $installDir" -ForegroundColor Green
Write-Host ""
Write-Host "Notes for this first test:" -ForegroundColor Yellow
Write-Host "  ferros-flash is not yet signed for Windows (Authenticode), so Windows may warn that it is from an unknown publisher." -ForegroundColor Yellow
Write-Host "  Windows needs the WinUSB driver for the phone's bootloader; if the guide says the phone cannot be opened, see its note." -ForegroundColor Yellow
Write-Host "  Every run keeps a log in $env:LOCALAPPDATA\ferros-flash\runs; please send it to us whatever happens." -ForegroundColor Yellow
Write-Host ""
Write-Host "Starting the guide. (Later: open a new terminal and run ferros-flash.)" -ForegroundColor Cyan
& $exe guide
