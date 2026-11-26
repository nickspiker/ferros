# The ferros flasher

Owner, 2026-11-10: "we do need to make a flash tool so users can download one signed executable, click thru some GUI options, then click download and flash, then it'll walk user thru unlocking bootloader if needed, etc." And: "it does need done for both targets."

Owner, 2026-11-24: "the 8a and windows will be our first real world test and we'll need to make sure we store pre-built signed OS images on fgtw.org and/or ferros.org".
Then: "I'd say we mirror the windows install for Photon and also focus on making sure the install process, OS and all of that is logged in proper VSF so that at any point the user can submit a log. I suspect this to be a bumpy road."

Owner, 2026-11-24, on what is installed: "Default pure ferros, we do not support Android dual boot. Period. This will be the first legit production build and deploy."
And: "if they want to enable the development gate that lets them run Android and build it themselves, they can but we do not host built OS images that run Android in any way."
So the flasher offers no keep-Android option, and every hosted image is a production build with every Android path compiled out (below, "Production images").

Owner, 2026-11-24, on the boot: "no 1s boot gate or green arrow, just straight to ferros since the google nag is already obnoxious."

Owner, 2026-11-25: the installer is a GUI, in fluor, "supa easy", and its one word is consent; the brand it speaks is docs/BRAND.md.

The owner's answers, 2026-11-24 (relayed by the coordinator):
- Hosting: "R2", ferros.org on R2 as Photon's is served; deploys to ferros.org need no further go. Built: the bucket `ferros` at https://get.ferros.org.
- The image: "Bare ferros, no hypervisor".
- userdata: "Sure!", a 1 GiB floor holding the device-only secret, so a bootloader unlock destroys the device-class keys with no writes of ours.
- USB doors: "Yes": the production install door (open only while no ferros partition exists), the signed-only update door, and the signed-only boot-slot write for the A/B updater.
- Logs: "big yes": fgtw.org/report, consent per report, the serial blinded by default; fgtw.org deploys need no go, handles, keys and location kept out by construction. Built (below, "Submitting").

The targets are the Pixel 8a (akita, the first real-world test, from a Windows PC) and the Pixel 8 Pro (husky).

## What a person does

0. Paste one line into PowerShell (or a terminal on Linux and macOS). It downloads the tool, checks it, installs it for that user and starts the guide.
1. Put the phone in its bootloader and plug it in. The tool finds it by its product, names it, and never writes to a product it has no board for.
2. If the bootloader is locked, the tool walks them through unlocking: Developer options, OEM unlocking, then `flashing unlock`, answered on the phone's own screen. It says plainly that unlocking wipes the phone.
3. The tool says what it will do: ferros replaces Android entirely; the phone's own calibration is copied to the computer first; userdata shrinks to its floor and `ferros` takes the rest; the signed production image goes on both boot slots.
4. The calibration backup, never optional, kept on the computer with its BLAKE3.
5. Download and flash: the signed release fetched, every file checked against its BLAKE3 before anything is written, the carve, both slots flashed and read back.
6. First boot: the tool waits for ferros on USB and checks it answers, then hands the phone to the attest screen.
7. **Re-lock, the default** (owner, 2026-11-13: "3 is required"): the ferros key set as the bootloader's custom key, and `flashing lock` on the phone's own screen, the person's own act (the tool never sends it: "Claude memories/feedback_lock_prep_not_lock.md"). Leaving it unlocked is an explicit choice, and the tool says what it exposes: the device's ID (the ira) has no papers, so while unlocked anyone holding the phone can boot their own image, derive it, and read the device's settings, stowed kists and cached wallpaper; the person's identity, their people and anything of the fleet stay closed without the identity. ferros shows the lock state afterwards, never a nag: a line on the settings' About page, a console line at boot, and SYS_FACTS fact 87 (`Facts::boot_lock`), from ABL's bootconfig (kernel bootlock.rs).

Every step says what it will do before it does it, every write is read back, and every step is in the run's log.

## Status (2026-11-24)

| Step | State | Where |
|---|---|---|
| 0 Firmware from the phone's own partitions | surveyed, per-firmware proposal below; kernel and driver work, staged, needs device slots | this file |
| 1 Board manifests in VSF | built and host-tested (akita, husky) | tools/flash/src/board.rs |
| `ferros-flash provision` | not built: needs the production install door (below) | |
| 2 Native fastboot | built, host-tested against a scripted bootloader; not yet run on a phone | tools/flash/src/fastboot.rs, usb.rs |
| 3 The guide | built to the lock step (find, name, lock state, unlock walkthrough, the plan); stops before the backup | tools/flash/src/main.rs `guide` |
| 4 Distribution | built and live: the signed release (VSF, BLAKE3 names), the PowerShell and sh installers, on https://get.ferros.org (R2 bucket `ferros`) | tools/flash/src/release.rs, publish.sh, install/ |
| 5 Logging | built and live: the run log, `log send` to fgtw.org/report on a typed yes, the endpoint, `reports.sh` to read what came | tools/flash/src/runlog.rs, reports.sh; fgtw-bootstrap `handle_report` |
| Production images | Android paths compiled out of the kernel and the hv; image composition proposed (bare ferros) | below |
| Updating while locked | designed, for the owner | below |
| The fluor GUI | built to screen 1 (below, "The GUI"): the welcome with the live device silhouette, the steps screen, the log offer; three targets built | tools/flash-gui |
| Code signing | later; the owner's cost decision | |

## 0. Firmware from the phone's own partitions

Google's files cannot be shipped by us, and every phone carries them in its own partitions, so the kernel and drivers read them there.
A reset then loses no firmware, and provisioning shrinks to ferros's own files.

The survey of 2026-11-24 (every firmware and calibration file the kernel and drivers load):

| Read from the phone today (partfs, nothing to provision) | Stored in the vault by a host script today | Not on the phone at all |
|---|---|---|
| modem.bin and uecap (modem_X, ext4); NV images (efs, f2fs); replay_region.bin (modem_userdata); cpsha (persist); carrier configs (vendor); BT patch BCM.hcd (vendor) | WiFi firmware, NVRAM, CLM, txcap (`wifi.*`); GPU CSF firmware (`gpu.fw`); AoC firmware (`aoc.fw.*`, the kernel's vault); the sensor registry (`aoc.reg.*`, vendor) and its factory calibration (persist); the 8a's GNSS `gps.kepler` (vendor) | camera inits, CPU programs and PDMA jobs (oracle captures of Android's register writes); camera IDTs (the owner's own measurements); husky's GPS replay; the 8a's `gps.kepler.cfg` and session; the display's cold-power and cold-pipe snapshots (compiled in) |

The proposal, one firmware at a time, each its own stage with its own device slot:

0. **gpu.fw**: the gpu process gains a `vendor` partition grant and reads `/firmware/mali_csffw-<its interface>.bin`. Blocker on the 8a: its vendor carries r54p1 to r54p3 and r56p0, and the process speaks r52p0, so the 8a runs husky's file today; the process must learn r54 first, or the 8a keeps a vault copy.
1. **wifi.\***: pcie1 reads the board's files from vendor by the names the board manifest lists (the 8a's `bcmdhd.cal_*` name is still to be read from a phone).
2. **aoc.fw**: the kernel's AoC loader reads `/firmware/aoc.bin` thru partfs in place of the kernel vault (it already reads in runs).
3. **aoc.reg.\***: the sensors process reads `/etc/sensors/registry` from vendor and the factory calibration from persist (f2fs), in the order tools/akita-aoc.sh stores them today.
4. **gps.kepler**: the gnss_kepler process reads `/firmware/kepler.bin` from vendor.
5. Each driver keeps the vault copy as its development fallback, and IDENTITY-SPACE.md's "Device firmware" plan (refill from vendor on a miss) becomes the first path rather than the fallback.

Two things this rests on:
- **Which slot's vendor.** The kernel grants `vendor_b`/`modem_b` on akita and `_a` on husky, fixed (service.rs). With virtual A/B, only the slot Android last ran from has its logical partitions in super; on the 8a since 2026-11-18 that is slot a, and `vendor_b` may be gone. The kernel must find the slot whose `vendor` exists in super's metadata and grant that one. Pure ferros freezes it there: Android never updates again, so the firmware is the build the phone carried when ferros went on.
- **What is not on the phone ships with ferros.** The camera's captured programs and the GNSS session are ferros's own data, in the release beside the capsules. Whether captured register sequences may be hosted publicly is the owner's call.

## 1. Board manifests

One VSF document a board (tools/flash/src/board.rs), built into the tool and published in the signed release, so a board fix ships without a new executable.
Section `board`: the product, its name, the fastboot and ferros USB ids, the block size and LU, whether `set_active` may be sent (husky: never), the boot partition's size, the download piece, userdata's floor, the smallest ferros partition today's kernel opens, and, on husky, the exact LBAs today's kernel demands.
Repeated sections: `backup` (every per-unit partition copied before any write: persist, efs, efs_backup, modem_userdata, devinfo, pinfo, blenv, mfg_data, fips, trusty_persist, misc, frp), `firmware` (where each file lives on the phone and where the driver takes it from today), `trap` (what is known to go wrong, shown before the step).

**The layout, and its gap to the storage founding rule.** ONE-VAULT.md is a plan; nothing of it is built. So the manifest describes what today's kernel expects:
- one `ferros` partition, found by name on akita, at least 20 GiB for the vault process's window, 72 GiB for RAW clips (process.rs `WINDOWS`);
- husky's kernel opens its vault only at FERROS_BASE_LBA..=FERROS_LAST_LBA (ufs_device.rs `locate`), so a person's husky must be carved to exactly those LBAs until the kernel finds the partition by name as on akita.

The fixed windows inside the partition (flash log, kvault, the vault window, clips, rawbench) are the gap to the founding rule, flagged to the coordinator; the carve itself already follows it (one partition, everything past userdata's floor).

## 2. Native fastboot

tools/flash/src/fastboot.rs speaks the protocol over any bulk pipe; usb.rs is that pipe over nusb.
- Commands: `getvar` (one or `all`), `download` (refused before sending when larger than the bootloader's `max-download-size`; sent in 1 MiB writes so a failing cable fails on a piece the log names), `flash`, `set_active`, `oem pkvm status|disable`, `flashing get_unlock_ability`, `flashing unlock`, `reboot`, `reboot-bootloader`.
- Never sent, whoever asks: `flashing lock`, `flashing lock_critical`, `oem lock`; `set_active` on husky; a flash of anything but `boot_a`, `boot_b` and `avb_custom_key`.
- pKVM: with no Android and no hv in a production image, pKVM no longer matters to ferros; the guide reads its status into the log only.

**nusb, and why.** Plain Rust on Linux (usbfs), macOS (IOKit) and Windows (WinUSB), no libusb to ship or build, and the crate ferros-bridge and ferros-link already use. rusb would bring libusb's C and the same WinUSB need on Windows.

**Windows and the driver.** nusb on Windows opens only a device bound to WinUSB, and the Pixel's bootloader (G#18D1:G#4EE0) does not ask Windows for it; a bare Windows shows an unknown device. The first real user (2026-11-24) was told to install adb to get the driver, the experience the coordinator asked removed. Built the same day (tools/flash/src/driver.rs, usb.rs `scan`):
- **Detection.** Windows lists no interfaces for a one-interface device (nusb reads them only from a composite device's children), so the fastboot triple was never seen there; `scan` now opens every Google device listed without interfaces and reads its configuration, and for a phone in Android claims its ADB interface (G#FF, G#42, G#01). A refused open is a missing driver (`Seen::NoDriver`, nusb's own words logged).
- **The install, on a typed yes, said in full first:** Google's own package fetched from Google (`https://dl.google.com/android/repository/usb_driver_r13-windows.zip`, the URL Google's SDK manifest addon2-3.xml names for `extras;google;usb_driver` revision 13, A#8682039 bytes, its SHA-1 08a48c39... as the manifest gives), checked against its SHA-256 pinned in driver.rs (360b01d3...), unpacked into %TEMP%\ferros-usb-driver with Expand-Archive, and installed with `pnputil /add-driver android_winusb.inf /install`, run elevated (one UAC prompt, `Start-Process -Verb RunAs`) from a script file whose output is read back. Its INF covers the bootloader (PID G#4EE0) and the ADB interfaces (G#4EE2, G#4EE7). Never rehosted. Every step and pnputil's words go into the run's log; then the person replugs the phone and the search runs again.
- **publish.sh** fetches Google's file at every publish and refuses to publish when it no longer has the pinned SHA-256.
- **When it cannot:** the guide says so and names the hand route: Windows Update's optional driver updates ("Google, Inc. - Android Bootloader Interface"), or Device Manager's Update driver, Search automatically. No Zadig, and no step ever sends the person to adb or platform-tools.
- Not yet run on Windows; Jeff's next run is the check.
- ferros's own USB device (G#1209:G#4665) had no MS OS descriptors, so on Windows the tool could not reach ferros after the first boot. **Built 2026-11-24, off by default** (feature `msos`: ferros_hal, the kernel, payloads/usb): bcdUSB 2.01, a BOS with the USB 2.0 extension (no LPM claimed) and the Microsoft OS 2.0 platform capability, and the descriptor set (ferros_hal msos.rs: compatible ID WINUSB, DeviceInterfaceGUIDs {94952487-3D5A-4981-B569-5AA375AE5768}) on vendor request G#5E, wIndex 7. Host-tested byte for byte; not yet enumerated by any host. The check, on a device slot: a kernel and usb capsule built `--features msos`, then on Linux `lsusb -v` shows the BOS with the platform capability and the bridge works as before (bcdUSB 2.01 makes every host read the BOS, and it now claims no LPM); then on Windows the device appears as a WinUSB device with no driver installed. On by default after that.

## 3. The guide

`ferros-flash guide` (the default with no arguments): find the phone (one in fastboot, else the serial named), name it from the board manifests, the bootloader's lock state and unlock ability, the unlock walkthrough (the request sent only after a typed yes; the phone asks again on its own screen), then the plan in words.
It stops there and says so: the backup, the carve and the flash run thru ferros on the phone (below), which is not built yet.

**The carve without Android.** A person's phone has no root, so the GPT edit cannot go thru Android as tools/pixel-carve.sh does. The proposed order:
0. Unlock (the phone wipes itself).
1. Flash the signed production image to both boot slots (no `set_active`, so husky's trap never arises).
2. Reboot: ferros starts with no `ferros` partition and opens an **install door** on USB, in production builds too, only while no `ferros` partition exists (an installed phone never has it open).
3. Thru that door the tool reads the calibration partitions (the backup), reads both boot slots back (the flash's verification), reads the GPT, plans the carve on the computer (lib.rs `carve`, the board's floor), and sends the changed blocks; the kernel checks the new table itself (every entry kept but userdata's end and the new `ferros`) before writing, and reads it back.
4. Reboot: the vault is created on the new partition, the release's capsules are stored thru the same door and read back, and the tool checks each process came up.

The install door is kernel work, approved by the owner (2026-11-24: "Yes"); it needs a device slot from the coordinator.

## 4. Distribution

The same shape as Photon's install (photon README "Installation", installers/install-release.ps1 and .sh).
- **The one line.** Windows: two lines in PowerShell, `irm <base>/install.ps1 -OutFile $env:TEMP\ferros-install.ps1` then `powershell -ExecutionPolicy Bypass -File $env:TEMP\ferros-install.ps1` (run as a file, never thru `iex`: "The first Windows test"). Linux and macOS: `curl -sSfL <base>/install.sh | sh`. The installer downloads `ferros-flash-<platform>`, checks its SHA-256 against the one tools/flash/publish.sh wrote into it, installs it for the user only (no administrator), and starts the guide.
- **SHA-256 at the bootstrap only.** PowerShell has SHA-256 built in and no BLAKE3, and few systems have b3sum, so the installer's one check of the tool itself is SHA-256, as Photon's. Everything the tool fetches after that is BLAKE3 in VSF.
- **The release** (release.rs): one VSF document Ed25519-signed with the ferros key (keys/ferros.key.vsf; the tool pins keys/ferros.pub), section `release` (channel, the commit) and one `artefact` section a file (role, name, board, platform, size, BLAKE3 `hb`). Every file is served under its BLAKE3 in hex, so a host can swap nothing.
- **publish.sh** builds the Windows (x86_64-pc-windows-gnu), Linux and macOS (aarch64-apple-darwin, cross-built with osxcross at /mnt/Harbor/Code/osxcross as Photon's deploy.sh builds it) executables, the CLI and the GUI each, the board manifests, the mark (logo.vsf, role `logo`), any boot images given, signs the release, writes the installers and stages it all; it uploads only with `--upload`.
- **The installers** fetch both executables (`ferros-flash-gui-<platform>` and `ferros-flash-<platform>`), check each against its SHA-256 written in at publish time, install them for the user, and open the window. On macOS the executables are unsigned and unnotarized, so Gatekeeper warns; install.sh clears the quarantine flag, and the person allows the rest in Privacy & Security.
- **The host** (owner, 2026-11-24: "R2"): the R2 bucket `ferros`, its custom domain https://get.ferros.org (made 2026-11-24). `tools/flash/publish.sh --upload` puts the staged release there under the R2 token in keys/: files named by their BLAKE3 cached for good, the installers, the fixed-name executables and release.vsf with `no-cache`, so an edge never pairs an old executable with a new installer. The one lines: the two PowerShell lines above and `curl -sSfL https://get.ferros.org/install.sh | sh`.
- **Code signing** (Authenticode, and Developer ID for macOS) is the owner's cost decision. Until then the executable is unsigned, the installer says Windows may warn of an unknown publisher, and the signed release is what vouches for the images.

## 5. Logging

**On the computer** (runlog.rs): one file a run, `<data>/ferros-flash/runs/<start in Eagle time, hex>.log.vsf` (Windows: `%LOCALAPPDATA%`; macOS: Application Support; Linux: XDG data), opened at the run's first step.
- Each record is a complete VSF document in the stack's log shape (vsf::logging: section `log` with `lvl`, a `msg` template, typed `val`s), so `vsflog` reads it as it reads Photon's; each also carries its `step` and typed fields of its own (a partition, a size, a BLAKE3, a bootloader variable, a time taken).
- Every record is on the disk (`sync_data`) before the step it describes goes on; a crash, a pulled cable or a power cut leaves every record before it, and a torn last record is skipped by the reader. A panic is logged as a record before the tool ends.
- It holds the tool's version and platform, the phone's product, serial, bootloader and baseband versions, slot and lock state, every step with what it read and wrote (names, sizes, BLAKE3), every bootloader INFO line, every error with its context, all in Eagle time. Never a handle, a key, a location, or the contents of a calibration partition.
- `ferros-flash log show <file>` renders it; `ferros-flash log report <file> <out>` makes the one document a person submits: section `report` (the tool, the record count, whether the tail was torn, whether the serial was kept) and each record whole as a `record` field, the serial replaced by its keyed BLAKE3 unless the person keeps it in.

**On the phone.** The flash log is the ledger's ring (ferros_ledger): every entry a complete VSF document with its provenance hash, chained by BLAKE3, its words sealed under the ira-derived system key (personal entries under the person's key). The host pulls it today with `ferros-bridge diag log [--raw]` (the kernel opens the entries for the answer), but that is a development door, closed in production. Proposed: the install door and a production `log` door hand the tool the system entries sealed as stored plus the kernel's opened copy only while the device is unlocked or the person approves on the phone, so a submitted report can carry the phone's side of the run.

**Submitting (built 2026-11-24; the owner: "big yes").**
- **The person's side:** `ferros-flash log send [run] [--keep-serial]` (the newest run when none is named), and the guide offers it at its end; every way out of the tool names the run's log and this command. Before anything is sent it says where it goes, how big it is, how many records, that it holds no handle, key, location or file, whether the serial is blinded, how long it is kept, and how to read every line of it (`log show`); it sends only on a typed yes. The consent and fgtw.org's answer are added to that run's own log.
- **The endpoint:** `POST https://fgtw.org/report` (fgtw-bootstrap `handle_report`). It takes a report only whole and clean: the report and every record checked by provenance hash; one `report` section of known fields; each `record` one `log` section; refused if anything carries a key, a signature, a MAC, handle material (`hP`, `hI`, `hV`), a place (`w`, `wm`, `wa`) or a network address (`n*`), or a `serial` the report says was left out; at most 16 MiB. The answer is `report_taken` with the report's BLAKE3, which the person quotes to us; a refusal is an `error` section with its reason.
- **The privacy rules:**
  - Nothing is sent without the person's typed yes, per report; there is no standing setting and nothing is sent on its own, a crash included.
  - A report never holds a handle, a key, a location or the contents of the phone's partitions: the tool never writes them into a run, and the endpoint refuses any report whose values are of those kinds.
  - The phone's serial is replaced by its keyed BLAKE3 unless the person keeps it in (`--keep-serial`).
  - No device key or identity is asked for; the sender's address is neither stored nor logged with it.
  - Kept 2^5 days under `ferros-reports/<Eagle time>-<BLAKE3>.vsf` in the bucket fgtw-data, then deleted by the hourly sweep.
- **Reading them:** `tools/flash/reports.sh [hash]` lists what came, fetches each once into target/flash-reports, unpacks it back into a run log (`log unpack`) and renders it.

## Production images

Hosted images are production builds (owner, 2026-11-24), and production has no way into Android:
- **Built (2026-11-24):** the hv's gate honours no ANDROID_ONCE or RECOVERY_ONCE in a production build (ferros, or fastboot when no ferros is packed); the kernel's ANDROID_ONCE writer, the boot-time Android branch, the Start into launch, the Android launch and the recovery "yes" are compiled out (`cfg(ferros_production)`), so a production build that still referred to them would not compile. The gate screen, the 1 s wait and the arrow were already compiled out of production in both the kernel (ready.rs) and the hv (gate.rs).
- **Found 2026-11-24, before the image can be built: a production kernel starts no native code** (process.rs `spawn_capsule_as`: "production refuses unmasked native code"), and a masked capsule runs only once its seal is in the vault's `sealed` list. A fresh phone has no vault, so the usb process (and the vault process) the kernel carries would be refused, and with no usb there is no install door. A design for the owner: either the capsules carried inside the kernel image are trusted as the kernel is (VAULT-OUT.md decision 0: "whatever verifies the kernel verifies it"; AVB verifies the image on a locked phone), or the kernel seals what it carries when it creates the vault, as an install's first act. Until one is chosen, (e) builds a production kernel that boots to no USB.
- **Left for the map's owner:** the Settings page "Android" still shows in a production build (payloads/map PAGES); its controls set requests the production kernel no longer acts on. It should be absent there.
- **The hosted image is bare ferros** (owner, 2026-11-24: "Bare ferros, no hypervisor"), `ferros-mkimg boot` with its AVB footer, no hv and no Linux Image. The gate format needs a Linux Image packed in front of ferros, and that Image is Google's Android kernel for one Android build: hosting it is hosting an image that runs Android, and it would be one image per Android build. Without Android the hv gives ferros nothing it needs: the UFS firewall, the chords into Android and the key routing exist for Android; the killswitch is the kernel's. A bare boot loses the jump-style restart (ferros reboots thru the bootloader instead) and the hv's early fastboot escape (ABL's own power + volume down remains). akita marked its slot only thru the hv; a production build now marks it bare too (main.rs), which the A/B updater needs.

## Updating while locked (owner, 2026-11-24: to build before the first locked deploy)

### (a) The A/B updater in ferros

- **What it writes:** the release's boot image, whole partition, already signed with the ferros AVB key (the updater never signs), to the slot not running, by GPT name; read back and checked against the release's BLAKE3.
- **The opening:** nothing in the kernel writes a boot partition today (processes reach only the ferros partition's windows; partfs only reads). The hv's UFS firewall guards against Android at EL1 only, and a bare production image has no hv. So the opening is a new kernel call, signed-only: it takes the release document and the image bytes, checks the release's signature against the ferros key compiled into the kernel and the image's BLAKE3 against it, and writes only `boot_a` or `boot_b`, the one not running. Nothing else can be written thru it.
- **Then:** `slot::set_active` on the new slot (devinfo flags and `bBootLunEn`, as the Android path does today), reboot; the locked bootloader verifies the image against the custom key. A good boot marks itself successful (`slot::mark_booted_slot`); a bad one burns its retries and the bootloader falls back to the old slot (proven 2026-10-28), where ferros logs the failed update to the ledger. The old slot stays as the fallback until the next update.
- **Who may ask:** the person, behind an approve on the phone; the release fetched over the network or handed over USB by the tool (a production `update` door taking only signed bytes), so a locked phone is updated without unlocking.
- **Rollback index:** the image's AVB rollback index must not be below what the bootloader stored; raised only for security fixes, never past what the owner sets.

### (b) "Allow unlocking" in Security

- **What it is:** Android's "OEM unlocking" switch, which ferros must set itself as there is no Android. ON by default for the first deploys, behind an approve, the page saying plainly: with it on, anyone holding the phone can unlock it; unlocking wipes userdata, and ferros's device-class data (settings, stowed kists, cached wallpaper) becomes readable to whoever unlocks it, while the person's identity and fleet stay closed.
- **Where the flag lives, found so far:** Android writes the last byte of `frp` (LU0 LBA 37126..=37253 on both phones, 512 KiB; husky's read 1 on 2026-11-01), with a SHA-256 over the partition after its first 32 bytes in those 32 bytes (AOSP's PersistentDataBlockService, from memory, unverified); a later Android boot wipes frp if the digest is wrong. But on Pixels with a Titan M2 the bootloader most likely asks Titan's own device lock (the AVB app's SetDeviceLock, which Android's Titan-backed OemLock service also sets), reached on zuma thru GSA (`GSA_MB_CMD_GSC_NOS_CALL`). Not proven from the trees: no oemlock HAL, no `get_unlock_ability` handler, no ABL log of it.
- **The measurement first, on the 8a** (unlocked today; one boot of ferros): `fastboot flashing get_unlock_ability`, then ferros sets frp's last byte to 0 with the digest recomputed, then `get_unlock_ability` again. If it follows frp, (b) is an frp write behind the approve. If not, (b) is a Titan call thru GSA, and Titan may refuse it from anything but Android; that would make (b) much harder and is the owner's to weigh.
- **Decided (owner, 2026-11-24: "Sure!"):** keep ferros's device-only secret (the salt of the device-class keys) in the small userdata partition the carve leaves. The bootloader's unlock erases userdata, so an unlock becomes the key shred by the bootloader's own hand ("Claude memories/feedback_no_needless_writes.md": destroying data is destroying its keys), and the device-class data a thief would read after unlocking is noise.

### (c) Kernel updates thru the vault's kernel ring

With ONE-VAULT.md the boot slots hold the seed, not the kernel, and a kernel update is a vault write; the bootloader and AVB are no longer involved. It needs:
- ONE-VAULT decision 0 (the kernel's runs in the spine entry beside `kernel_hash` and `kernel_sig`) and the migrating spine with its root ring;
- the seed booting from the boot partition (bare, as above), reading the root ring and the spine, checking the kernel against the ferros key, jumping;
- kernel A/B by `active_slot` in the spine entry, with a fallback when the new kernel does not mark itself good;
- a kernel rollback floor kept in the spine (the seed refuses a kernel below it), since AVB's index no longer covers the kernel;
- (a) still for the seed itself, which changes rarely.

## Re-lock: what it needs

`fastboot flash avb_custom_key <ferros AVB key blob>` (hosted as a release file), then the person's `flashing lock`.
A locked bootloader verifies the top-level `vbmeta` against that key, and Google's top `vbmeta` chains `boot` to Google's key, so the phone also needs a top `vbmeta` signed by the ferros key. That `vbmeta` carries Google's hash descriptors for `dtbo`, `vendor_boot`, `vendor_kernel_boot`, `init_boot` and `pvmfw` (the bootloader still loads them), which differ per Android build: one signed `vbmeta` a (board, Android build) pair, hosted, the tool matching the phone's build (read by ferros from `vbmeta_a`/`_b`, `diag part avb` today) and refusing the lock on a build it has none for. The boot image's rollback index must not be below the one the phone stored under Google's build.
This is not built and needs measuring on the 8a before any lock (none is ever sent by the tool).

## The GUI (built 2026-11-25, to screen 1)

The owner, 2026-11-25: the installer must be a GUI, in fluor, "supa easy", anyone can follow it; its one word is **consent** (docs/BRAND.md has the thesis, the voice, the look and the lines).
tools/flash-gui is that window: a crate of its own beside tools/flash (as tools/attest-mark and tools/tune are, so fluor's host build stays out of the workspace lock and the CLI stays small), on the same library: the same detection (usb.rs), the same signed release (release.rs), the same run log (runlog.rs). The CLI stays as it is.

- **Screen 0, the welcome.** The mark (logo.vsf from the release, drawn at the window's size by vsf's analytic rasterizer, composed under by fluor), "Consent." large with one warm glow breathing behind it, the three dim lines, the device silhouette, the bottom line "This replaces Android and erases the device. You're choosing that." Nothing else but a small "send the log" in a corner.
- **The silhouette is the live detector**, on every screen: `ferros_flash::usb::watch` holds nusb's hotplug stream on a thread (udev's netlink, IOKit, Windows device notifications), scans on each arrival or departure, and the window gets each change as a user event; nothing polls. Grey `?` with nothing seen; amber `no access` ("driver needed" on Windows, where the one-prompt Google driver install from driver.rs is the next step; "no permission to open it" on Linux without the udev rule); warm with the model name and "in Android" or "in its bootloader" (a bootloader is opened just long enough to ask its product); orange `ferros` for a device already running ferros. While a device is seen, one button wakes under it, **Install ferros**; unplugged, it goes grey and the button goes.
- **Screen 1, after Install ferros.** "Welcome!" and the owner's words (BRAND.md), one short paragraph on the signed release, the steps ahead one line each (plug in, turn on unlocking, unlock on the device (this erases it), install, lock it back, done), the silhouette still live, and the honest line that this build goes no further yet. The irreversible step's button, when the flow reaches it, reads **Erase and install**.
- **send the log**, on every screen: the consent words (where it goes, what it never holds, the serial blinded, 32 days), "Send it" or "Not now", the answer's name shown; the same report and endpoint as `ferros-flash log send`, the consent recorded in the run's own log.
- **No timers.** The glow's phase is the count of shown frames; the detector is event-driven; nothing waits or delays.
- **Looking at it:** `ferros-flash-gui --release <staged dir> --snap out.vsf` writes the first settled frame as a VSF visual (tools/vsf2web turns it into AV1 for a browser), `--steps` opens on screen 1, `--pretend bootloader` shows a device state in place of the detector, without a device on the cable. The first captures of 2026-11-25 were taken this way on Linux with the Pro on the bench (orange, `ferros`).
- **Three targets built** on this host: Linux x86_64 (run here), Windows x86_64 (x86_64-pc-windows-gnu, a GUI-subsystem executable, no console), macOS arm64 (osxcross, unsigned, unnotarized).
- **Not yet:** the steps after screen 1 (they wait on the install door and the production image, above), the Windows driver step from the window itself (the CLI has it; the window names it), and a run on Windows or macOS.

## The first Windows test (what it looks like today)

On the owner's Windows PC, with the 8a in its bootloader on the cable:
0. In PowerShell, two lines: `irm https://get.ferros.org/install.ps1 -OutFile $env:TEMP\ferros-install.ps1`, then `powershell -ExecutionPolicy Bypass -File $env:TEMP\ferros-install.ps1`. The installer runs as a file, so nothing reformats it on the way.
   Two one-liners failed on 2026-11-24 (the owner, Windows PowerShell as administrator), both before the tool ran:
   - `powershell -ExecutionPolicy Bypass -c "irm https://get.ferros.org/install.ps1 | iex"`: Invoke-Expression got the script a line at a time ("Cannot bind argument to parameter 'Command' because it is an empty string", then "trap {" alone, "Missing closing '}'"). The one served difference from Photon's working installer was ours carrying `charset=utf-8`; both installers are now served as `text/plain`, as Photon's are. Whether that was the whole cause is not proven.
   - `powershell -ExecutionPolicy Bypass -c "iex (irm ... | Out-String)"`: "The string is missing the terminator". Out-String formats to the console's width, so the script's long lines were wrapped and a string split (the coordinator's diagnosis).
   So no Invoke-Expression at all, and every line of install.ps1 stays under 100 characters, so no width can split one.
   A single line that pastes into both cmd and PowerShell needs no `$` in it (PowerShell expands `$env:TEMP` in the outer double quotes first, and a temp path with a space then splits into two arguments for the inner one): `powershell -ExecutionPolicy Bypass -c "irm https://get.ferros.org/install.ps1 -OutFile ([IO.Path]::GetTempPath()+'ferros-install.ps1'); & ([IO.Path]::GetTempPath()+'ferros-install.ps1')"`. Not yet tried on Windows, so not published.
1. The guide finds the phone and names it, or says the phone cannot be opened (the WinUSB question, with what to install); the log says which.
2. It reads the lock state (the 8a is unlocked), shows the plan, and stops before the backup.
3. It offers to send the run's log to fgtw.org/report; he types yes, and quotes the hash it answers (or runs `ferros-flash log send` later, from any terminal).
That run tests the install, the executable, USB on Windows and the log, and writes nothing to the phone. The flash itself follows once the install door, the production image and the measurements above are in.

## Open for the owner

- Code signing for Windows and macOS (the macOS executables warn in Gatekeeper until then).
- (b)'s path once measured, and whether Titan may be asked.
- Whether captured register sequences (camera, GNSS) may be hosted.
