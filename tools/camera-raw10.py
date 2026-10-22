#!/usr/bin/env python3
"""Decode a ferros main-camera frame: MIPI RAW10 packed (four pixels in five bytes: four MSB bytes, then the 2-bit LSBs of each), 4080 x 3072, stride 5120 bytes.

Writes <prefix>.npy (uint16 Bayer mosaic, sensor values 0..1023) and <prefix>-grey.png: a 2 x 2 binned grey preview for checking exposure and focus only (black-subtracted, square-root stretch). It is not a colour rendering; colour goes through the camera's IDT and VSF RGB.
Prints black level (from the darkest percentile), exposure percentiles and a sharpness figure (mean squared same-colour gradient).
Usage: tools/camera-raw10.py <frame.raw10> <prefix>
"""
import sys
import numpy as np
from PIL import Image

W, H, STRIDE = 4080, 3072, 5120
d = np.frombuffer(open(sys.argv[1], 'rb').read(), np.uint8)[:H * STRIDE].reshape(H, STRIDE)[:, :W * 5 // 4]
g = d.reshape(H, W // 4, 5).astype(np.uint16)
px = np.empty((H, W // 4, 4), np.uint16)
for k in range(4):
    px[:, :, k] = (g[:, :, k] << 2) | ((g[:, :, 4] >> (2 * k)) & 3)
raw = px.reshape(H, W)
np.save(sys.argv[2] + '.npy', raw)
p = np.percentile(raw, [0.1, 1, 50, 99, 99.9])
sat = (raw >= 1020).mean() * 100
a = raw.astype(float)
sharp = ((a[:, 2:] - a[:, :-2]) ** 2).mean() + ((a[2:] - a[:-2]) ** 2).mean()
print(f"percentiles 0.1/1/50/99/99.9: {p.round(1).tolist()}  saturated {sat:.3f}%  sharpness {sharp:.1f}")
q = (a[0::2, 0::2] + a[0::2, 1::2] + a[1::2, 0::2] + a[1::2, 1::2]) / 4
lo, hi = np.percentile(q, 0.5), np.percentile(q, 99.9)
q = np.clip((q - lo) / max(hi - lo, 1), 0, 1) ** 0.5
Image.fromarray((q * 255).astype(np.uint8)).save(sys.argv[2] + '-grey.png')
