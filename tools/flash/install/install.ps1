# ferros-flash installer for Windows (PowerShell), the same shape as Photon's.
# It downloads the tool, checks its SHA-256 against the one publish.sh wrote
# into this script, installs it for this user only (no administrator), and
# starts the guide. The tool checks what it fetches later by BLAKE3 against a
# release signed with the ferros key. PowerShell has SHA-256 built in and not
# BLAKE3, so this one check, of the tool itself, is SHA-256.
# Run it as a file, never thru Invoke-Expression (docs/FLASHER.md "The first
# Windows test"): in PowerShell,
#   irm __BASE__/install.ps1 -OutFile $env:TEMP\ferros-install.ps1
#   powershell -ExecutionPolicy Bypass -File $env:TEMP\ferros-install.ps1
# Every line here stays under 100 characters.

$ErrorActionPreference = "Stop"
# One line of words in one colour.
function Say([string]$text, [string]$colour = "White") {
    Write-Host $text -ForegroundColor $colour
}
trap {
    Write-Host ""
    Say "ERROR: $_" Red
    Say "Press Enter to exit..." Yellow
    Read-Host
    exit 1
}

Say "ferros-flash installer" Cyan
Write-Host ""

$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne "AMD64") {
    Say "This first build is for 64-bit Intel and AMD Windows only." Red
    Say "This computer is $arch." Red
    exit 1
}

# Written by tools/flash/publish.sh: the host and the executable's SHA-256.
$base = "__BASE__"
$expectedHash = "__SHA256_WINDOWS_X86_64__"
if ($expectedHash -notmatch "^[0-9A-F]{64}$") {
    Say "This copy of the installer was not published: nothing installed." Red
    exit 1
}

$installDir = "$env:LOCALAPPDATA\Programs\ferros-flash"
New-Item -ItemType Directory -Path $installDir -Force | Out-Null
$exe = "$installDir\ferros-flash.exe"

Say "Downloading ferros-flash..." Yellow
Invoke-WebRequest -UseBasicParsing -Uri "$base/ferros-flash-windows-x86_64.exe" -OutFile $exe

Say "Checking the download..." Yellow
$actualHash = (Get-FileHash $exe -Algorithm SHA256).Hash
if ($actualHash -ne $expectedHash) {
    Say "The download is not the published ferros-flash." Red
    Say "  expected $expectedHash" Red
    Say "  got      $actualHash" Red
    Remove-Item $exe -ErrorAction SilentlyContinue
    exit 1
}
Say "[OK] The download is the published ferros-flash" Green

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$installDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$installDir", "User")
    $env:Path += ";$installDir"
}

$runs = "$env:LOCALAPPDATA\ferros-flash\runs"
Write-Host ""
Say "Installed to $installDir" Green
Write-Host ""
Say "Notes for this first test:" Yellow
Say "  ferros-flash is not yet code-signed for Windows (Authenticode)," Yellow
Say "  so Windows may warn that it is from an unknown publisher." Yellow
Say "  If Windows has no driver for the phone, the guide offers to install" Yellow
Say "  Google's own USB driver (one administrator prompt); no Android tools." Yellow
Say "  Every run keeps a log in $runs;" Yellow
Say "  please send it to us whatever happens (ferros-flash log send)." Yellow
Write-Host ""
Say "Starting the guide. (Later: open a new terminal, run ferros-flash.)" Cyan
& $exe guide
