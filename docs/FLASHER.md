# The ferros flasher

Owner, 2026-11-10: "we do need to make a flash tool so users can download one signed executable, click thru some GUI options, then click download and flash, then it'll walk user thru unlocking bootloader if needed, etc." And: "it does need done for both targets."

This is the plan, shelved on 2026-11-10 to be picked up whole. The targets are the Pixel 8 Pro (husky) and the Pixel 8a (akita); nothing here is written for one of them only.

## What a person does

0. Download one signed executable for their computer (Linux, macOS, Windows) and run it.
1. Plug the phone in. The tool finds it by its product (husky, akita), says which phone it is, and never writes an image to a product it is not for.
2. Choose: keep Android beside ferros (userdata shrunk, Android the escape) or pure ferros; the size of the ferros partition.
3. If the bootloader is locked, the tool walks them through it: Developer options, OEM unlocking, `flashing unlock` on the phone's own screen. It says plainly that unlocking wipes the phone.
4. The tool backs up the phone's own calibration before it writes anything, and keeps the backup on the computer. This step is not optional.
5. Download and flash: the tool fetches the signed ferros images, checks each against its BLAKE3, carves the partition table, flashes, provisions, and turns pKVM off (again after any wipe).
6. First boot: the tool waits for ferros on USB and checks it answers, then hands the phone over to the attest screen.

## What exists today (2026-11-10)

| Piece | Where | Both targets? |
|---|---|---|
| GPT carve: read both ends of the disk, plan the shrink of userdata and the new `ferros` entry (BLAKE3("ferros") type GUID), write and verify primary and backup, the original kept | `tools/flash` (ferros-flash lib + CLI), driven by `tools/pixel-carve.sh` | built for akita first |
| Signed boot images, the hv and ferros in one, both slots, A/B fallback | `tools/akita-hv.sh`, `tools/husky-hv.sh`, `sign_only.sh`, the owner's AVB key | yes, as two scripts |
| Calibration backup | by hand (dd through Android with root), hash-checked, kept on Harbor | yes, by hand |
| Shell processes and firmware into the vault | `ferros-bridge store put` over PT; `tools/husky-aoc-fw.sh`, `tools/husky-aoc-reg.sh`, `tools/akita-aoc.sh`, `tools/f2fs-cat` | yes, per-board scripts |
| Device guard | `tools/lib/devices.sh`, the product checks in each script | yes |

What a full reset loses today: the vault holds the shell's processes, the GPU's and the AoC's firmware, the sensor registry and the WiFi credential, so a wiped ferros partition boots to the bare usb process until all of it is stored again by hand. The identity side survives: the device key is derived from the ira, so the fleet chain on fgtw.org still lists the phone, the owner's handle binds it without words, and fgtw.org's device owner index still refuses any other handle.

## What it needs

### 0. Firmware from the phone's own vendor partition

Google's files cannot be shipped by us: `aoc.bin`, the Mali firmware, WiFi and Bluetooth firmware and their CLM and calibration, the GNSS firmware (akita's `kepler.bin`), the sensor registry scripts. Every phone carries them in its own vendor partition, and ferros already reads that partition on the device (partfs: the modem loads its carrier config from `vendor_b` this way). So the kernel and drivers read their firmware from the phone's own vendor partition rather than from copies in the vault, and the phone's own calibration from its persist and efs (persist is f2fs, read by ferros_f2fs). Provisioning shrinks to flashing ferros, nothing Google's passes through the tool, and a reset loses no firmware. Each driver keeps a fallback to the vault copy for development.

### 1. Board manifests and `ferros-flash provision`

One manifest a board (husky, akita), in VSF: the product name, the slot ferros boots from, the partition layout and sizes, the boot image layout (the hv's link base, the board's DRAM window), where each firmware lives on the phone, which calibration partitions to back up, and the per-board traps (husky: the 64 MiB fastboot send fails, so prefix flashes with the footer unchanged; akita: the hv on slot b through the avb footer). `ferros-flash provision <board>` replaces the per-board scripts: the shell's processes stored and read back, then a reboot and a check that each came up. The same library answers a full reset on either phone.

### 2. Native fastboot

The single executable cannot depend on Android's platform-tools. A fastboot client in Rust over USB bulk (the transport ferros-bridge already speaks for PT): `getvar`, `download` in pieces small enough for the Pro, `flash`, `set_active`, `oem pkvm disable` and status, `flashing unlock` and its state, `reboot bootloader`. The bridge's USB port kick (tools/usb-kick.sh) becomes a retry in the tool.

### 3. The flow, as a CLI wizard

Detect, name the phone, the bootloader's lock state and the unlock walkthrough, the choices, the backup, the carve, the flash, provisioning, pKVM, first boot. Every step says what it will do before it does it, and every write is verified by reading it back.

### 4. The GUI and the executable

A fluor front end over the same library: the same steps as screens, nothing the CLI cannot do. One executable a platform. Windows (Authenticode) and macOS (Developer ID, notarization) signing need certificates: outside decisions and costs for the owner. Linux: a signed static binary.

### 5. Hosting

The signed ferros images (boot, the hv, the shell's capsules) published where the tool fetches them (ferros.org or fgtw.org: the owner's call), each named by its BLAKE3 in a signed manifest the tool checks before writing anything.

## Order

0 (firmware from the phone) and 1 (manifests and provision) first: they are also what a full reset needs on both phones today. Then 2 (fastboot), 3 (the wizard), 4 (the GUI and signing), with 5 decided before the first release.

## Open for the owner

- Where the images are hosted, and the signing certificates for Windows and macOS.
- Whether the default is keep-Android or pure ferros, and the default ferros size.
- How the tool treats a phone whose Android build differs from the one its firmware paths were read from (the vendor files are read from the phone itself, so this matters only where a path or format moved).
