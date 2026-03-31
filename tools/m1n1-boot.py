#!/usr/bin/env python3
"""Boot ferros on M1 via m1n1 proxy — direct EL2, minimal DTB with simplefb."""
import sys, pathlib, struct
sys.path.append(str(pathlib.Path(__file__).resolve().parents[1] / ".." / "m1n1" / "proxyclient"))

from m1n1.setup import *

kernel_path = sys.argv[1] if len(sys.argv) > 1 else str(
    pathlib.Path(__file__).resolve().parents[1] /
    "target" / "aarch64-unknown-none" / "release" / "ferros_kernel.bin"
)

print(f"Loading {kernel_path}")
kernel = open(kernel_path, "rb").read()
print(f"Kernel size: {len(kernel):#x} bytes")

# Read framebuffer info from m1n1 boot_args
ba = u.ba
fb_base = ba.video.base
fb_width = ba.video.width
fb_height = ba.video.height
fb_stride = ba.video.stride
fb_depth = ba.video.depth
print(f"Framebuffer: {fb_base:#x} {fb_width}x{fb_height} stride={fb_stride:#x} depth={fb_depth}")

# Dump USB register addresses from ADT (needed for DWC3 driver)
print("\n=== USB ADT Addresses ===")
try:
    usb_drd = u.adt["/arm-io/usb-drd0"]
    for i in range(4):
        try:
            addr, size = usb_drd.get_reg(i)
            print(f"  usb-drd0 reg[{i}]: {addr:#x} (size {size:#x})")
        except:
            pass

    atc_phy = u.adt["/arm-io/atc-phy0"]
    for i in range(4):
        try:
            addr, size = atc_phy.get_reg(i)
            print(f"  atc-phy0 reg[{i}]: {addr:#x} (size {size:#x})")
        except:
            pass

    dart_usb = u.adt["/arm-io/dart-usb0"]
    for i in range(4):
        try:
            addr, size = dart_usb.get_reg(i)
            print(f"  dart-usb0 reg[{i}]: {addr:#x} (size {size:#x})")
        except:
            pass
    print("=========================\n")
except Exception as e:
    print(f"  ADT read failed: {e}\n")

# Build a minimal FDT with a simple-framebuffer node
# Our DTB parser looks for compatible = "simple-framebuffer" and reads reg, width, height, stride
def build_simplefb_dtb(fb_base, fb_width, fb_height, fb_stride):
    """Build a minimal DTB with a simple-framebuffer node."""
    from struct import pack

    def fdt32(val):
        return pack(">I", val & 0xffffffff)

    def fdt64(val):
        return pack(">Q", val & 0xffffffffffffffff)

    # String table
    strings = b""
    def add_string(s):
        nonlocal strings
        off = strings.find(s.encode() + b"\x00")
        if off < 0:
            off = len(strings)
            strings += s.encode() + b"\x00"
        return off

    # Build structure block
    FDT_BEGIN_NODE = 1
    FDT_END_NODE = 2
    FDT_PROP = 3
    FDT_END = 9

    def align4(data):
        pad = (4 - len(data) % 4) % 4
        return data + b"\x00" * pad

    sb = b""
    # Root node
    sb += fdt32(FDT_BEGIN_NODE) + b"\x00\x00\x00\x00"  # name = ""

    # #address-cells = <2>
    name_off = add_string("#address-cells")
    sb += fdt32(FDT_PROP) + fdt32(4) + fdt32(name_off) + fdt32(2)

    # #size-cells = <2>
    name_off = add_string("#size-cells")
    sb += fdt32(FDT_PROP) + fdt32(4) + fdt32(name_off) + fdt32(2)

    # chosen node (empty, kboot may populate)
    sb += fdt32(FDT_BEGIN_NODE) + align4(b"chosen\x00")
    sb += fdt32(FDT_END_NODE)

    # framebuffer node
    sb += fdt32(FDT_BEGIN_NODE) + align4(b"framebuffer@%x\x00" % fb_base)

    # compatible = "simple-framebuffer"
    name_off = add_string("compatible")
    compat = b"simple-framebuffer\x00"
    sb += fdt32(FDT_PROP) + fdt32(len(compat)) + fdt32(name_off) + align4(compat)

    # reg = <fb_base, fb_stride * fb_height>  (2 cells each for addr and size)
    fb_size = fb_stride * fb_height
    name_off = add_string("reg")
    reg_data = fdt64(fb_base) + fdt64(fb_size)
    sb += fdt32(FDT_PROP) + fdt32(len(reg_data)) + fdt32(name_off) + reg_data

    # width = <fb_width>
    name_off = add_string("width")
    sb += fdt32(FDT_PROP) + fdt32(4) + fdt32(name_off) + fdt32(fb_width)

    # height = <fb_height>
    name_off = add_string("height")
    sb += fdt32(FDT_PROP) + fdt32(4) + fdt32(name_off) + fdt32(fb_height)

    # stride = <fb_stride>
    name_off = add_string("stride")
    sb += fdt32(FDT_PROP) + fdt32(4) + fdt32(name_off) + fdt32(fb_stride)

    # format = "a8r8g8b8" (32bpp ARGB)
    name_off = add_string("format")
    fmt = b"a8r8g8b8\x00"
    sb += fdt32(FDT_PROP) + fdt32(len(fmt)) + fdt32(name_off) + align4(fmt)

    # status = "okay"
    name_off = add_string("status")
    status = b"okay\x00"
    sb += fdt32(FDT_PROP) + fdt32(len(status)) + fdt32(name_off) + align4(status)

    sb += fdt32(FDT_END_NODE)  # end framebuffer

    sb += fdt32(FDT_END_NODE)  # end root
    sb += fdt32(FDT_END)

    # Memory reservation block (empty)
    mem_rsv = fdt64(0) + fdt64(0)

    # Header
    totalsize = 40 + len(mem_rsv) + len(sb) + len(strings)
    off_structs = 40 + len(mem_rsv)
    off_strings = 40 + len(mem_rsv) + len(sb)

    header = pack(">10I",
        0xd00dfeed,     # magic
        totalsize,      # totalsize
        off_structs,    # off_dt_struct
        off_strings,    # off_dt_strings
        40,             # off_mem_rsvmap
        17,             # version
        16,             # last_comp_version
        0,              # boot_cpuid_phys
        len(strings),   # size_dt_strings
        len(sb),        # size_dt_struct
    )

    return header + mem_rsv + sb + strings

dtb = build_simplefb_dtb(fb_base, fb_width, fb_height, fb_stride)
print(f"DTB size: {len(dtb)} bytes")

# Upload kernel
kernel_addr = u.memalign(0x10000, len(kernel))
print(f"Kernel at: {kernel_addr:#x}")
iface.writemem(kernel_addr, kernel)
p.dc_cvau(kernel_addr, len(kernel))
p.ic_ivau(kernel_addr, len(kernel))

# Upload DTB
dtb_addr = u.memalign(0x1000, len(dtb))
print(f"DTB at: {dtb_addr:#x}")
iface.writemem(dtb_addr, dtb)

# Entry is at offset 0x1000 (_entry, past PE/COFF headers)
entry = kernel_addr + 0x1000
print(f"Entry: {entry:#x}")

# Use _m1_entry (offset 0x1010) — skips cache/MMU ops that crash on M1.
m1_entry = kernel_addr + 0x1010
print(f"M1 entry: {m1_entry:#x} (_m1_entry at offset 0x1010)")

# Pre-map kernel BSS pages in the USB DART before jumping.
# The kernel's static DMA buffers live in BSS (after the loaded image).
# We need the DART to translate their physical addresses for DWC3 DMA.
# Use identity mapping: IOVA = physical address.
print("Setting up USB DART mappings for kernel DMA buffers...")
try:
    # Get the USB DART handle from m1n1
    dart_handle = p.dart_init(0x382f80000, 0)  # dart-usb0 reg[1], sid=0
    print(f"  DART handle: {dart_handle:#x}")

    # BSS starts after the kernel image, page-aligned.
    # The linker puts BSS at offset 0x4000 from _start.
    # kernel_addr is the load address. BSS physical addr = kernel_addr + 0x4000.
    # We need to map enough pages to cover all static DMA buffers (~32KB of BSS).
    # Map 256KB to be safe (16 × 16KB DART pages).
    bss_start = (kernel_addr + len(kernel) + 0x3FFF) & ~0x3FFF  # page-align after image
    map_size = 256 * 1024  # 256KB should cover all static buffers
    print(f"  Mapping BSS region: {bss_start:#x} .. {bss_start + map_size:#x} (identity)")
    p.dart_map(dart_handle, bss_start, bss_start, map_size)
    print("  DART mappings OK")
except Exception as e:
    print(f"  DART setup failed: {e} — USB may not work")

print(f"Jumping to ferros via reload (entry={m1_entry:#x}, dtb={dtb_addr:#x})...")
try:
    p.reload(m1_entry, dtb_addr)
    print("Kernel handed off.")
except Exception as e:
    # Timeout expected — ferros doesn't reconnect to proxy
    print(f"Handoff complete (expected: {e})")
