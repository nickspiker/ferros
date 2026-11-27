# husky WiFi firmware manifest

Broadcom/Google binaries for the BCM4398, **not redistributed here**: only their names, sizes and BLAKE3 hashes are pinned. Every Pixel 8 Pro carries them on its own vendor partition (`/vendor/firmware/`); `tools/husky-firmware.sh` pulls them from a booted Android on slot a and verifies every byte against this list. (Endgame: ferros reads them off its own UFS vendor partition. Fallback for a phone without Android: Google's official factory image for the build below, `vendor.img` → `/firmware/`.)

Source build: `google/husky/husky:16/CP1A.260505.005/15081906:user/release-keys`

Vendor build: `google/husky/husky:16/CP1A.260505.005/15081906:user/release-keys`

| file | bytes | BLAKE3 |
|---|---|---|
| bcmdhd.cal_4398_b0 | 17786 | fda61c7f3203e9492acfe83b09690fef2ea36ef4410f92b32910962bbffc87b5 |
| bcmdhd.cal_4398_c0 | 19329 | 54e032f3c7796e102eb0184a3b7346aa74f7264b87687c0176dc00c8dc901d63 |
| bcmdhd.cal_JP | 22701 | efceea90cdeef1454fcd6df15d971aad5cacab19a6447032dabab8a83f20fefb |
| bcmdhd.cal_MMW | 22701 | efceea90cdeef1454fcd6df15d971aad5cacab19a6447032dabab8a83f20fefb |
| bcmdhd.cal_NA | 22649 | 2fef4069e0ba87f4643d839e8775175f6262751ce0393a79530734ae9b17ca12 |
| bcmdhd.cal_ROW | 22701 | 1e13981e450207471ef6d5cb89072425d1ec9f0b0244ca5c1786ada1da059180 |
| bcmdhd.cal_USI_G5SN_9950_V10 | 17471 | c320b9a583e37873ac5b46d576430e2cf3827b3db59b6f8ff01c15992ec8849b |
| bcmdhd.cal_USI_G5SN_9951_V11 | 17636 | 0dccc2b8bb382de16c235a1304d6371bce2f2b68786fc2266582a63bd43bda04 |
| bcmdhd.cal_USI_G5SN_9953_V13 | 17686 | bf00e371b984ba7661e459715d51f4d67a9896ddfd06ec84a3fe835462a6d934 |
| bcmdhd.cal_USI_G5SN_9955_V15 | 19954 | 344fa959467364772149858109b68c5064300eeeedabfce0d43ddf6fdefbc23e |
| bcmdhd.cal_USI_G5SN_9957_V17 | 22701 | 1e13981e450207471ef6d5cb89072425d1ec9f0b0244ca5c1786ada1da059180 |
| bcmdhd_clm.blob_4398_b0 | 24132 | f027d3d484d84edbbc1670de9bf272cc30559a9d38331864672509ca897a1419 |
| bcmdhd_clm.blob_4398_c0 | 24655 | bfc9c1609e073b48c8147513685af5d8f5b0c3230b521acf1031dcc05f95c947 |
| bcmdhd_clm.blob_4398_d0 | 25536 | a6528936b7d1d32df198e28850b9f8028b9d29331c085598c69020b6cb7579dd |
| bcmdhd_clm.blob_JP | 24216 | eab0630a224b82477c7bd0b497f27ae5ec4152ac1edf10948dc501f4ac82638e |
| bcmdhd_clm.blob_MMW | 24216 | eab0630a224b82477c7bd0b497f27ae5ec4152ac1edf10948dc501f4ac82638e |
| bcmdhd_clm.blob_NA | 24156 | e0a63d4604ad45212dc3589913da826e576ca73cfe4821366b979c7a2c67b01f |
| bcmdhd_clm.blob_ROW | 25536 | a6528936b7d1d32df198e28850b9f8028b9d29331c085598c69020b6cb7579dd |
| bcmdhd_txcap.blob_4398_c0 | 2486 | f6cdc7a02528f3f9333f466c0721a6e45ed9c255ca97d237e9a3041e1a994f41 |
| bcmdhd_txcap.blob_4398_d0 | 2486 | aa704a51d173610f1550a3b22af47db6331cd1beadee7dad09b9050caf3586f3 |
| fw_bcmdhd.bin_4398_b0 | 2372543 | bd5e1601c789628f2f28042946239d81bb6842dc3f57e3ee6168624212169221 |
| fw_bcmdhd.bin_4398_c0 | 2148375 | f0b9f5e82c5efda8db89e36277d4a748c69ef0ce1a317fd511253a76d214b63d |
| fw_bcmdhd.bin_4398_d0 | 2114859 | 15db418741a74f21e4347abb26f10bd0999936839ce4301d58d759398cf95f78 |
| fw_bcmdhd.map_4398_b0 | 2083627 | ef15b0e8973f4f4042bba7483857ef5d45f56883ed8a9a7fefda816a4d849b2f |
| fw_bcmdhd.map_4398_c0 | 2225796 | c61d329097434f517dae3a08c3833a4e849bfba95c7133fe8ee949769e4bd011 |
| fw_bcmdhd.map_4398_d0 | 2253893 | 73ffb833f99161d0bf3de2911de49ff7c04968838eac6ee663321630fad21caf |
