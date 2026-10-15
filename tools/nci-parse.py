#!/usr/bin/env python3
# Parse a BCM NCI enumeration ROM dump (pcie1 stage 04 console) the way nciutils.c nci_scan does.
import re, sys
txt = open(sys.argv[1]).read()
regions = {}
cur = None
for line in txt.splitlines():
    m = re.search(r'dump (\w+) G#([0-9A-F]{8})', line)
    if m:
        cur = m.group(1); regions[cur] = {}; continue
    m = re.search(r'G#([0-9A-F]{8}): ((?:G#[0-9A-F]{8} ?)+)', line)
    if m and cur:
        off = int(m.group(1), 16)
        for i, w in enumerate(re.findall(r'G#([0-9A-F]{8})', m.group(2))):
            regions[cur][off + 4 * i] = int(w, 16)
oobr, erom = regions['oobr'], regions['erom']
names = {0x800:'chipcommon',0x83c:'pcie2',0x847:'armca7',0x849:'sysmem',0x812:'d11',0x83e:'gci',0x829:'pmu',0x4bf:'bp?',0x135:'ns?',0x367:'oobr',0x368:'?'}
ERO2 = 0x118
p = ERO2
expect_oobr = 0x140
cores = []
while True:
    d0 = erom.get(p)
    if d0 is None or d0 == 0xFFFFFFFF:
        break
    d1 = erom[p + 4]
    coretype = (d1 >> 27) & 1
    off = d1 & 0x1FFF
    src = erom if coretype else oobr
    info, cfg = src.get(off, 0), src.get(off + 4, 0)
    cid = info & 0xFFF
    if cid < 0xFF: cid |= 0x800
    rev = (info >> 12) & 0xFF
    n_if = (cfg >> 12) & 0xF
    ifaces = []
    wordoffset = 0
    for _ in range(n_if):
        d0, d1 = erom[p], erom[p + 4]; p += 8
        master = (d1 >> 26) & 1
        nadr = ((d1 >> 24) & 3) + (0 if master else 1)
        wordoffset = d1 >> 28
        node = d0 & ~7 & 0xFFFFFFFF; ntype = d0 & 7; bpid = (d1 >> 20) & 0xF
        sps = []
        if not master:
            for _ in range(nadr):
                a = erom[p]; p += 4
                base = a & 0xFFFFFF00
                if a & 0x20:
                    p += 12; sps.append((base, '64bit'))
                elif a & 0x40:
                    hi = erom[p]; p += 4; sps.append((base, hi - base))
                else:
                    sps.append((base, 1 << ((a & 0x1F) + 8)))
        ifaces.append(('M' if master else 'S', bpid, ntype, node, sps))
    cores.append((cid, rev, 'oobr' if not coretype else 'erom1', off, ifaces))
    if wordoffset == 0:
        break
for cid, rev, where, off, ifs in cores:
    print(f"core {cid:#05x} {names.get(cid,''):10s} rev {rev:3d}  ({where}+{off:#x})")
    for kind, bpid, nt, node, sps in ifs:
        sp = ', '.join(f"{b:#010x}+{s:#x}" if isinstance(s, int) else f"{b:#010x} {s}" for b, s in sps)
        print(f"    {kind} bp{bpid} {'DMP' if nt==1 else 'booker'} node {node:#010x}  {sp}")
