//! The menu kernel (src/lib.rs `menu_kernel`) in valhall's emulator against lumis_core's menu drawn as ink and composed on the CPU (`ui::draw::compose_cpu`): a small picker with a camera's row pressed, every channel within 2 codes, opaque, the surface outside the frame untouched, and the invocations past the frame drawing its last pixel again.

use lumis_core::sensor_interface::CameraInfo;
use lumis_core::ui::draw;
use lumis_core::ui_main_menu::MainMenu;
use lumis_gpu::{menu, menu_kernel};
use valhall::sim::{run, Memory};

fn camera() -> CameraInfo {
    CameraInfo { id: "main".into(), index: 0, facing: 1, width: 4080, height: 3072, white_level: 1023, black_level: 64, bayer_pattern: 0, supports_raw: true, min_iso: 100, max_iso: 16000, min_exposure: 100_000, max_exposure: 68_800_000, sensor_width: 9.792, sensor_height: 7.3728, focal_lengths: vec![6.9], apertures: vec![1.68], min_focus_distance: 8., has_ois: true, hardware_level: 3, sensor_orientation: 90, pixel_array_width: 8160, mode_count: 1, group_id: 0, max_res: false, is_cropped: false, has_cal: false, fov_diagonal: 82. }
}

#[test]
fn the_menu_as_the_cpu_composes_it() {
    // An eighth of the Pro's panel, a stride past the frame's width.
    let (w, h, stride) = (168usize, 374usize, 176usize);
    draw::set_ink(true);
    let mut m = MainMenu::new(w as u32, h as u32, 489. / 8.);
    draw::set_ink(false);
    m.set_camera_list(vec![camera()]);
    // The camera's row (the one above Exit, the last), pressed: its middle, found from the bottom margin up as the menu lays its rows.
    let row_h = (h as f32 - 2. * (h as f32 / 12.)) / 7.;
    m.handle_touch(0, w as f32 / 2., h as f32 - h as f32 / 12. - 1.5 * row_h);
    let (mut light, mut cover, mut rows, mut slots) = (vec![0u8; w * h * 3], vec![0u8; w * h], vec![0u32; h], Vec::new());
    m.ink_frame(&mut light, &mut cover, &mut rows, &mut slots);
    assert!(slots.len() >= 4 && cover.iter().any(|&c| c == 255), "the camera's and Exit's slots, each drawn and pressed, and words");
    let mut want = vec![0u8; w * h * 3];
    draw::compose_cpu(m.background(), &light, &cover, &rows, &slots, w, &mut want);
    // The slots show: without them a good part of the frame differs.
    let mut bare = vec![0u8; w * h * 3];
    draw::compose_cpu(m.background(), &light, &cover, &vec![0; h], &slots, w, &mut bare);
    let slotted = want.iter().zip(&bare).filter(|(a, b)| a != b).count();
    assert!(slotted > w * h / 8, "{slotted} channels show a slot");

    // The block: the parts, the empty record first, an identity code table (each channel's G2t16 its code).
    let base = 0x8000_0000u64;
    let mut at = 0usize;
    let mut place = |n: usize| {
        let a = at;
        at += (n + 255) & !255;
        a
    };
    let (bg_at, ink_at, cover_at, rows_at, slots_at, lut_at, dst_at) = (place(w * h * 3), place(w * h * 3), place(w * h), place(h * 4), place((slots.len() + 1) * menu::RECORD_WORDS * 4), place(1024), place(stride * h * 8));
    let mut mem = Memory { base, bytes: vec![0u8; at] };
    mem.bytes[bg_at..bg_at + w * h * 3].copy_from_slice(m.background());
    mem.bytes[ink_at..ink_at + w * h * 3].copy_from_slice(&light);
    mem.bytes[cover_at..cover_at + w * h].copy_from_slice(&cover);
    for (y, r) in rows.iter().enumerate() {
        mem.bytes[rows_at + 4 * y..rows_at + 4 * y + 4].copy_from_slice(&r.to_le_bytes());
    }
    for (k, s) in slots.iter().enumerate() {
        let ch = s.ch.map(|c| (c.k, c.f, c.colour));
        let r = menu::record(s.x0 as u32, s.y0 as u32, s.x1 as u32, s.y1 as u32, s.big, s.glow, ch, s.fa);
        for (i, v) in r.iter().enumerate() {
            let o = slots_at + ((k + 1) * menu::RECORD_WORDS + i) * 4;
            mem.bytes[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    for v in 0..256u32 {
        mem.bytes[lut_at + 4 * v as usize..lut_at + 4 * v as usize + 4].copy_from_slice(&v.to_le_bytes());
    }
    let k = menu_kernel();
    let wu = |a: u64, b: u64| a | b << 32;
    let mut uniforms = vec![0u64; menu::SLOT_COUNT as usize];
    uniforms[menu::DST as usize] = base + dst_at as u64;
    uniforms[menu::BG as usize] = base + bg_at as u64;
    uniforms[menu::INK as usize] = base + ink_at as u64;
    uniforms[menu::COVER as usize] = base + cover_at as u64;
    uniforms[menu::ROWS as usize] = base + rows_at as u64;
    uniforms[menu::SLOTS as usize] = base + slots_at as u64;
    uniforms[menu::LUT as usize] = base + lut_at as u64;
    uniforms[menu::SIZE as usize] = wu(w as u64, (w * h) as u64);
    uniforms[menu::STRIDE as usize] = wu(stride as u64, 0);
    uniforms.extend_from_slice(&k.consts);
    // Every pixel, and a group's worth past the frame.
    for id in 0..(w * h + 15) as u32 {
        run(&k.words, &uniforms, [id, 0], &mut mem);
    }
    let mut most = 0i32;
    for y in 0..h {
        for x in 0..stride {
            let got = u64::from_le_bytes(mem.bytes[dst_at + 8 * (y * stride + x)..dst_at + 8 * (y * stride + x) + 8].try_into().unwrap());
            if x >= w {
                assert_eq!(got, 0, "({x}, {y}) outside the frame");
                continue;
            }
            assert_eq!(got >> 48, 0xFFFF, "({x}, {y}) opaque");
            let p = (y * w + x) * 3;
            for (c, shift) in [32, 16, 0].iter().enumerate() {
                let d = ((got >> shift) & 0xFFFF) as i32 - want[p + c] as i32;
                assert!(d.abs() <= 2, "({x}, {y}) channel {c}: {} on the GPU, {} on the CPU", (got >> shift) & 0xFFFF, want[p + c]);
                most = most.max(d.abs());
            }
        }
    }
    eprintln!("the menu kernel: every channel within {most} codes of the CPU's");
}
