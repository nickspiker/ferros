# Sensors on husky

What the Pixel 8 Pro has, read from Android's sensor service on the phone (`adb shell dumpsys sensorservice`, 2026-10-23). Every hardware sensor carries the handle prefix G#0101: all of them are served by one sensor HAL, the AoC's (Google's always-on sensor hub). None of them appears in the application processor's device tree. Android reaches them only through the AoC's firmware and its mailbox, so for ferros they are one project: the AoC sensor hub.

| Sensor | Part | Vendor | What it gives |
|---|---|---|---|
| Accelerometer, gyroscope, IMU temperature, motion/stationary detect | ICM45631 | InvenSense | the gravity vector for gravity-up UI (the clock's zero and digits), rotation, steps |
| Ambient light, proximity | TMD3719 | AMS | the proximity-gated wake (SHELL.md), auto brightness |
| Non-contact infrared thermometer, normal and extended range | MLX90632 | Melexis | an object's emissive temperature (the "Thermometer" sensor by the cameras) |
| Rear light sensor (spectral) | VD6282 | STMicro | the light the cameras see (white balance) |
| Magnetometers ×2 | MMC56X3X | MEMSIC | the compass |
| Pressure, its temperature | ICP20100 | InvenSense | altitude, weather |
| Camera vsync ×4 | — | Google | frame timing for sensor fusion |

The rest of the list (gravity, rotation vector, step counter, lift to wake, double twist, …) is Google's fusion running on the AoC from these parts.

Outside the AoC, ferros already reads: the battery fuel gauge (MAX77759, I2C: charge, temperature, voltage, current), the RTC (s2mpg14 via ACPM), touch (FTM5), the keys. The SoC's own thermal zones (TMU) are behind ACPM, which ferros speaks.

## The way in

An hv oracle capture first: Android under the hv, recording how it boots the AoC and asks it for streams. Then one of two routes:
- Run Google's AoC firmware and speak its sensor protocol.
- Keep the AoC off and drive the sensors' buses directly from ferros, if the AP can reach them.

The capture decides which.
