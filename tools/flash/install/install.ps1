# ferros-flash installer for Windows (PowerShell), the same shape as Photon's.
# It downloads the installer window (ferros-flash-gui.exe) and the command line
# tool (ferros-flash.exe), checks each SHA-256 against the one publish.sh wrote
# into this script, installs them for this user only (no administrator), and
# opens the window. The tools check what they fetch later by BLAKE3 against a
# release signed with the ferros key. PowerShell has SHA-256 built in and not
# BLAKE3, so these checks, of the tools themselves, are SHA-256.
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
$expectedGuiHash = "__SHA256_GUI_WINDOWS_X86_64__"
if (($expectedHash -notmatch "^[0-9A-F]{64}$") -or
    ($expectedGuiHash -notmatch "^[0-9A-F]{64}$")) {
    Say "This copy of the installer was not published: nothing installed." Red
    exit 1
}

$installDir = "$env:LOCALAPPDATA\Programs\ferros-flash"
New-Item -ItemType Directory -Path $installDir -Force | Out-Null

# One executable: downloaded and checked against its published hash.
function Fetch([string]$name, [string]$expected) {
    $file = "$installDir\$name.exe"
    Say "Downloading $name..." Yellow
    $uri = "$base/$name-windows-x86_64.exe"
    Invoke-WebRequest -UseBasicParsing -Uri $uri -OutFile $file
    Say "Checking the download..." Yellow
    $actual = (Get-FileHash $file -Algorithm SHA256).Hash
    if ($actual -ne $expected) {
        Say "The download is not the published $name." Red
        Say "  expected $expected" Red
        Say "  got      $actual" Red
        Remove-Item $file -ErrorAction SilentlyContinue
        exit 1
    }
    Say "[OK] The download is the published $name" Green
}
Fetch "ferros-flash-gui" $expectedGuiHash
Fetch "ferros-flash" $expectedHash
$exe = "$installDir\ferros-flash-gui.exe"

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
Say "  If Windows has no driver for the device, the tool offers to install" Yellow
Say "  Google's own USB driver (one administrator prompt); no Android tools." Yellow
Say "  Every run keeps a log in $runs;" Yellow
Say "  please send it to us whatever happens (ferros-flash log send)." Yellow
Write-Host ""
Say "Opening the installer window. (Later: ferros-flash-gui from Start," Cyan
Say "or ferros-flash guide in a new terminal.)" Cyan
Start-Process $exe
