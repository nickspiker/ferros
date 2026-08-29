# CallRec — automatic dual-channel VoIP call recorder (husky)

Records every VoIP call automatically, both directions on **separate channels**, to the shared **Recordings** folder as stereo Opus. No tapping record, survives reboots, works for any VoIP call app (Google Voice, Signal, WhatsApp…). Pairs with [`../transcribe/`](../transcribe/) for a fully-local, speaker-labeled transcript.

This is the husky/Android stopgap; the proper version is native ferros audio policy later.

## How it works

A persistent priv-app polls the audio mode; when a call goes active (`MODE_IN_COMMUNICATION`) it opens two `AudioRecord`s and writes interleaved stereo:

- **LEFT = `VOICE_UPLINK`** (source 2) — your side. A stream tap of the call uplink, *not* a mic open, so it doesn't fight the call app for the mic.
- **RIGHT = `VOICE_DOWNLINK`** (source 3) — the far end.

Both are captured at the AudioFlinger VoIP endpoints (husky routes VoIP through the AoC `audio_voip_rx`/`audio_voip_tx` PCMs), which is **upstream of physical output routing** — so it's route-independent: earpiece, speaker, wired, USB, or Bluetooth all capture identically. Channel separation is physical ground truth (no diarization needed downstream).

Audio is encoded live with MediaCodec Opus (`c2.android.opus.encoder`) → MediaMuxer `.ogg`, 48 kHz stereo ~64 kbps VBR.

### Notes / gotchas baked into the code
- **Flat top-level classes only** — the bundled `d8` NPEs on inner classes.
- **No uplink gain.** A fixed boost clips normal-volume speech; channels are separated so balance in post (lossless) instead. `GAIN` in `Recorder.java` is the knob.
- **Filenames** are `call <yyyy-MM-dd HH.mm.ss>.ogg`. Colons are illegal on Android's emulated storage (it enforces FAT-compat filename rules regardless of the real fs), so the time uses dots.
- **`VOICE_COMMUNICATION` does NOT work for the uplink** — it's a mic source and gets silenced because the call app already holds the mic. Use `VOICE_UPLINK`.

## Permissions / grants

- `CAPTURE_AUDIO_OUTPUT`, `MODIFY_AUDIO_ROUTING` — privileged, granted via the priv-app allowlist (`module/system/etc/permissions/privapp-permissions-callrec.xml`).
- `RECORD_AUDIO` — runtime; `MANAGE_EXTERNAL_STORAGE` — appop (needed to write the shared Recordings folder). Both asserted every boot by `module/service.sh`, which also restarts the app once so the all-files storage view takes effect (the view only applies to a process that started *with* the grant).

## Build & install

```bash
adb pull /system/framework/framework-res.apk tools/callrec/framework-res.apk   # once
tools/callrec/build.sh                      # needs a REAL android.jar (API 34+), see build.sh header
# flash build/callrec-module.zip in Magisk, or hot-swap the APK + reboot
# (persistent apps are not updatable via `pm install`, so reboot to load a new APK):
adb push build/CallRec.apk /data/local/tmp/CallRec.apk
adb shell 'su -c "cp /data/local/tmp/CallRec.apk /data/adb/modules/callrec/system/priv-app/CallRec/CallRec.apk && chmod 0644 $_"'
adb reboot
```

Recordings land in `/sdcard/Recordings/call <timestamp>.ogg`, you-left / them-right.
