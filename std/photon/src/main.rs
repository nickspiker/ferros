//! Photon on ferros: the app Photon's library builds, run through fluor's ferros host over the display server's app surface and the touch server. Launched as a kist (tools/photon-capsule.sh) with, in slot order, the display and touch endpoints, the app surface, the udp endpoint, its own vault domain, the ira and entropy (placements of the implicit set, KISTS.md), then be you and the person's people (asked, docs/FACETS.md); the GPU's three and sound out after them. The desktop main (photon's src/main.rs) does the winit things; this one does the ferros things.
//! Photon has no handle screen on ferros (owner, 2026-11-14: "why is that in the ferros build at all?"; photon f6d954bf): the person comes from the identity service as PhotonKeys, always into the running app (`Ferros`, `wait_for_person`), and until then Photon shows "waiting for your identity".
//! The window comes first (owner, 2026-11-16: from the tap to Photon's window took 5 to 10 s on the Pro): on b905 the first frame came 6.7 s after main began, all of it be you (the alias's bundle and possession here, the binding signed with every egg in the identity service), asked before the first frame. Now be you is asked on the waiter's thread from the start and the person handed in thru the wake path, which the late launch used already.

use std::sync::Arc;

/// When main began: the waiter says how long after it it came up (2026-11-17, the Pro: be you was asked 5.1 s after the first frame).
static MAIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

use fluor::coord::Coord;
use fluor::event::{CursorIcon, Event, ModifiersState};
use fluor::geom::Viewport;
use fluor::host::app::{Context, EventResponse, FluorApp};
use fluor::host::{WakeError, WakeSender};
use fluor::text::TextRenderer;
use photon_messenger::crypto::photon_keys::PhotonKeys;
use photon_messenger::network::fgtw::derive_device_keypair;
use photon_messenger::ui::photon_app::PhotonApp;
use photon_messenger::ui::{PhotonEvent, Px};

/// The ira's grant slot, in the spawn's order above (tools/photon-capsule.sh declares it sixth: "to tie its keys to this device"). The ira is one of the implicit set since 2026-11-12 (docs/KISTS.md), never asked; Photon's capsule still declares it, so it stays in this slot. The alias comes through the same grant (SYS_IRA x4 bit 1).
const IRA: u64 = 6;
/// Be you's grant slot (the capsule's eighth want): a send right on the identity service's `facets` endpoint, or an empty slot where the person said no or has not been asked.
const BE_YOU: u64 = 8;
/// The person's people's grant slot (the ninth want): the same endpoint, stamped by the kernel with what the person granted.
const PEOPLE: u64 = 9;

/// The kernel, as this process reaches it (ferros KERNEL.md, the process ABI): the ira and the alias, and a call on an endpoint. Inlined as Mahere's, fluor's and kete's are: ferros_sys is no_std and not a published crate.
mod sys {
    use core::arch::asm;

    const CALL: u64 = 1;
    const IRA: u64 = 23;
    /// ERR, the kernel's refusal (ferros_sys::ERR).
    const ERR: u64 = u64::MAX;

    #[inline(always)]
    fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
        let r: u64;
        unsafe { asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
        r
    }

    /// The arc base (x28, the kernel's, never written by compiled code on this target).
    #[inline(always)]
    fn arc_base() -> u64 {
        let b: u64;
        unsafe { asm!("mov {}, x28", out(reg) b, options(nomem, nostack, preserves_flags)) };
        b
    }

    #[inline(always)]
    fn off<T: ?Sized>(p: *const T) -> u64 {
        (p as *const u8 as u64).wrapping_sub(arc_base())
    }

    /// The grants page is arc offset 0: the handle table in its first 512 bytes, the u32 count at G#200, the 32-byte tokens after it (ferros SANDBOX.md, "Handles").
    pub fn cap(slot: u64) -> u64 {
        0x204 + 32 * slot
    }

    /// A 32-byte key for `domain` rooted in the device's ira (ferros_sys `ira_key`): the same on every boot, never the ira itself; for a kist the kernel binds it to the kist's gate identity, so no other kist and no system service derives it (KISTS.md, "A kist's ira keys are its own"). None without an Ira grant (the person said no) or an ira this boot.
    pub fn ira_key(slot: u64, domain: &str) -> Option<[u8; 32]> {
        let mut out = [0u8; 32];
        let r = svc(IRA, cap(slot), off(domain.as_ptr()), domain.len() as u64, off(out.as_mut_ptr()), 0);
        (r == 32).then_some(out)
    }

    /// Photon's alias (ferros_sys `alias`; docs/FACETS.md "The alias"): the seed the kernel derives for this kist alone from the person key and Photon's gate identity, the same on every device of the person's. None before attest, before the identity service handed the kernel the person key, or on a kernel from before 2026-11-12.
    pub fn alias(slot: u64) -> Option<[u8; 32]> {
        let mut out = [0u8; 32];
        let r = svc(IRA, cap(slot), 0, 0, off(out.as_mut_ptr()), 2);
        (r == 32).then_some(out)
    }

    const STORE: u64 = 13;
    /// The kernel's "absent" (ferros_sys::NOTHING).
    const NOTHING: u64 = u64::MAX - 1;

    /// `key` read from the vault scope the Store grant in `slot` names (ferros_sys `store_get`): the value's whole length, of which at most `buf.len()` bytes were copied; None when absent or refused.
    pub fn store_get(slot: u64, key: &[u8], buf: &mut [u8]) -> Option<usize> {
        let r = svc(STORE, cap(slot), (buf.len() as u64) << 8, off(key.as_ptr()), key.len() as u64, off(buf.as_mut_ptr()));
        (r != ERR && r != NOTHING).then_some(r as usize)
    }

    /// `value` written under `key` in that scope (ferros_sys `store_set`, one transfer: at most 256 KiB); true when the vault took it.
    pub fn store_set(slot: u64, key: &[u8], value: &[u8]) -> bool {
        svc(STORE, cap(slot), 1 | (value.len() as u64) << 8, off(key.as_ptr()), key.len() as u64, off(value.as_ptr())) == 0
    }

    /// `key` deleted from that scope (ferros_sys `store_delete`).
    pub fn store_delete(slot: u64, key: &[u8]) {
        svc(STORE, cap(slot), 8, off(key.as_ptr()), key.len() as u64, 0);
    }

    /// A call on the endpoint in `slot`, its answer in `reply`: the answer's length, or None when the kernel refused (an empty slot: the person said no).
    pub fn call(slot: u64, msg: &[u8], reply: &mut [u8]) -> Option<usize> {
        let r = svc(CALL, cap(slot), off(msg.as_ptr()), msg.len() as u64, off(reply.as_mut_ptr()), reply.len() as u64);
        (r != ERR).then(|| (r as usize).min(reply.len()))
    }
}

/// Photon's own vault domain's grant slot (the capsule's fifth want, one of the implicit set): its storage on this device, under the person's key and its own (KISTS.md, VAULT-OUT.md).
const DOMAIN: u64 = 5;

/// The alias's signing bundle kept between launches (2026-11-17, the Pro: deriving it, a Falcon and a SPHINCS+ keygen, was 121 to 394 ms of every launch's be you): its secret keys (fgtw `SigningBundle::to_sealable_bytes`) sealed with XChaCha20-Poly1305 under a key from this kist's ira (bound to this device and to Photon alone), in Photon's own domain, as one VSF document with the bundle's public commitment beside the seal. Opened only when the opened bundle's commitment is the one sealed beside it and its Ed25519 key is the alias's own (`fgtw::alias::ed25519_public`, no keygen); be you then holds it to the binding the identity service returns. The secret bytes go into the seal and nowhere else: never logged, never stored outside it, wiped after use.
mod alias_cache {
    use super::{sys, DOMAIN, IRA};
    use fgtw::pq::SigningBundle;
    use vsf::VsfType;

    const NAME: &[u8] = b"photon.alias.bundle";
    const SECTION: &str = "alias_bundle";

    /// The seal's key: the ira's, for this purpose, this device and this kist.
    fn key() -> Option<[u8; 32]> {
        sys::ira_key(IRA, "photon alias bundle seal")
    }

    /// The bytes the seal authenticates beside its own: the alias's Ed25519 key and the bundle's commitment, so a seal is opened only for this alias and this bundle.
    fn aad(alias: &[u8; 32], commit: &[u8; 32]) -> [u8; 64] {
        let mut a = [0u8; 64];
        a[..32].copy_from_slice(&fgtw::alias::ed25519_public(alias));
        a[32..].copy_from_slice(commit);
        a
    }

    fn wipe(b: &mut [u8]) {
        b.fill(0);
        core::hint::black_box(b);
    }

    /// The bundle kept for `alias`, when there is one that opens and is this alias's; None otherwise (none kept, another alias's, a seal that does not open).
    pub fn open(alias: &[u8; 32]) -> Option<SigningBundle> {
        use chacha20poly1305::{aead::{Aead, Payload}, KeyInit, XChaCha20Poly1305, XNonce};
        let mut buf = vec![0u8; 1 << 14];
        let n = sys::store_get(DOMAIN, NAME, &mut buf)?;
        // WHY: the vault's word for the value's length; one longer than the buffer was cut, and is not a document to read.
        if n > buf.len() {
            return None;
        }
        let doc = &buf[..n];
        let (header, end) = vsf::verification::read_verified(doc, None).ok()?;
        let sections = header.sections(doc, end).ok()?;
        let s = sections.iter().find(|s| s.name == SECTION)?;
        let first = |name: &str| s.get_fields(name).first().and_then(|f| f.values.first()).cloned();
        let commit: [u8; 32] = match first("commit")? {
            VsfType::hb(b) => b.try_into().ok()?,
            _ => return None,
        };
        let sealed = match first("sealed")? {
            VsfType::v(b'e', b) if b.len() > 24 => b,
            _ => return None,
        };
        let mut k = key()?;
        let cipher = XChaCha20Poly1305::new_from_slice(&k).ok()?;
        wipe(&mut k);
        let nonce = XNonce::try_from(&sealed[..24]).ok()?;
        let mut plain = cipher.decrypt(&nonce, Payload { msg: &sealed[24..], aad: &aad(alias, &commit) }).ok()?;
        let bundle = SigningBundle::from_sealable_bytes(&plain);
        wipe(&mut plain);
        let bundle = bundle.ok()?;
        let public = bundle.public();
        (public.commit() == commit && public.ed25519() == fgtw::alias::ed25519_public(alias)).then_some(bundle)
    }

    /// `bundle`, the alias's, sealed and kept in Photon's domain in place of any kept before. False when it could not be (no seal key, no randomness, the vault refused): the next launch derives it again.
    pub fn seal(alias: &[u8; 32], bundle: &SigningBundle) -> bool {
        use chacha20poly1305::{aead::{Aead, Payload}, KeyInit, XChaCha20Poly1305};
        let commit = bundle.public().commit();
        let mut nonce = [0u8; 24];
        let (Some(mut k), Ok(())) = (key(), getrandom02::getrandom(&mut nonce)) else { return false };
        let Ok(cipher) = XChaCha20Poly1305::new_from_slice(&k) else { return false };
        wipe(&mut k);
        let mut plain = bundle.to_sealable_bytes();
        let sealed = cipher.encrypt(&nonce.into(), Payload { msg: &plain, aad: &aad(alias, &commit) });
        wipe(&mut plain);
        let Ok(ct) = sealed else { return false };
        let mut v = Vec::with_capacity(24 + ct.len());
        v.extend_from_slice(&nonce);
        v.extend_from_slice(&ct);
        let doc = vsf::VsfBuilder::new()
            .creation_time_oscillations(vsf::eagle_time_oscillations())
            .provenance_only()
            .add_section(SECTION, vec![("commit".to_string(), VsfType::hb(commit.to_vec())), ("sealed".to_string(), VsfType::v(b'e', v))])
            .build();
        doc.is_ok_and(|d| sys::store_set(DOMAIN, NAME, &d))
    }

    /// The kept bundle dropped (it did not match the binding the identity service returned).
    pub fn drop_kept() {
        sys::store_delete(DOMAIN, NAME);
    }
}

/// The alias's signing bundle: the one kept sealed when it opens and is this alias's (no keygen), else derived and sealed for the next launch. With whether it was the kept one.
fn alias_signer(alias: &[u8; 32]) -> (fgtw::pq::SigningBundle, bool) {
    if let Some(b) = alias_cache::open(alias) {
        return (b, true);
    }
    let b = fgtw::alias::bundle(alias);
    if !alias_cache::seal(alias, &b) {
        println!("photon: the alias's bundle could not be kept sealed; the next launch derives it again");
    }
    (b, false)
}

/// The person as Photon's messenger knows them on ferros (docs/PHOTON.md "Photon's own key: the migration"): its alias from the kernel, and from the identity service the handle proof, the pinned identity key and the binding of the alias to the person (be you, op 0 on `facets`). None without the alias or without be you: Photon then waits for them (`wait_for_person`), on its "waiting for your identity" line, never a handle screen. Never the identity seed: ferros hands no kist a root, so `legacy_seed` is None.
fn photon_keys() -> Option<PhotonKeys> {
    let Some(alias) = sys::alias(IRA) else {
        println!("photon: no alias from the kernel (before attest, or the person key not handed in yet)");
        return None;
    };
    // Each step timed (the owner, 2026-11-16: the launch's wait): the alias's bundle (kept sealed from a launch before, or its keys made from the seed), the possession eggs, and the call (the identity service's loop and its signing, which it says itself).
    // A kept bundle whose binding comes back for another bundle is dropped and the bundle made again, once.
    for again in [false, true] {
        let t0 = std::time::Instant::now();
        let (signer, kept) = if again { (fgtw::alias::bundle(&alias), false) } else { alias_signer(&alias) };
        let public = signer.public();
        let bundle = public.to_bytes();
        let t1 = std::time::Instant::now();
        let eggs = fgtw::binding::possession(&signer, &public);
        let t2 = std::time::Instant::now();
        let mut req = Vec::with_capacity(3 + bundle.len() + eggs.len());
        req.push(0);
        req.extend_from_slice(&(bundle.len() as u16).to_le_bytes());
        req.extend_from_slice(&bundle);
        req.extend_from_slice(&eggs);
        let mut reply = vec![0u8; 1 << 16];
        let answered = sys::call(BE_YOU, &req, &mut reply);
        println!(
            "photon: be you took {} ms: the alias's bundle {} ms ({}), its possession {} ms, the identity service's answer {} ms",
            t0.elapsed().as_millis(),
            (t1 - t0).as_millis(),
            if kept { "kept sealed, opened" } else { "made" },
            (t2 - t1).as_millis(),
            t2.elapsed().as_millis()
        );
        let Some(n) = answered else {
            println!("photon: be you not granted (or no identity service behind the slot): Photon waits for your identity");
            return None;
        };
        if n < 65 || reply[0] != 0 {
            println!("photon: the identity service refused be you ({})", reply.first().copied().unwrap_or(0xFF));
            return None;
        }
        let mut hp = [0u8; 32];
        let mut identity = [0u8; 32];
        hp.copy_from_slice(&reply[1..33]);
        identity.copy_from_slice(&reply[33..65]);
        let binding = reply[65..n].to_vec();
        // The binding names this alias, this handle proof and this bundle, or it is not taken: a check of the service's answer against what this process holds, never the chain's (that is a contact's, against the chain it fetches).
        match fgtw::binding::Binding::from_vsf_bytes(&binding) {
            Ok(b) if b.alias_key() == public.ed25519() && b.handle_proof == hp && b.alias.commit() == public.commit() => {
                println!("photon: be you: the alias and its binding in hand ({} bytes)", binding.len());
                return Some(PhotonKeys::new(alias, hp, identity, binding, None));
            }
            Ok(b) if kept && b.alias_key() == public.ed25519() && b.handle_proof == hp => {
                println!("photon: the binding is for another bundle of this alias than the one kept: the kept one dropped, the bundle made again");
                alias_cache::drop_kept();
            }
            _ => {
                println!("photon: the identity service's binding does not name this alias");
                return None;
            }
        }
    }
    None
}

/// The person's people Photon may know (people, op 1 on `facets`): all of them or the ones the person chose, without their fleets. Empty without the grant: no lying, nobody is nobody.
fn people() -> Vec<fgtw::people::Person> {
    let mut reply = vec![0u8; 1 << 18];
    match sys::call(PEOPLE, &[1], &mut reply) {
        Some(n) if n >= 1 && reply[0] == 0 => fgtw::people::from_doc(&reply[1..n]).unwrap_or_default(),
        Some(_) => {
            println!("photon: the identity service refused the people ({})", reply[0]);
            Vec::new()
        }
        None => Vec::new(),
    }
}

/// What the running app hears besides Photon's own wakes: the person, handed in once be you answers (`wait_for_person`).
enum Ev {
    Photon(PhotonEvent),
    Person(PhotonKeys, Vec<fgtw::people::Person>),
}

/// Photon's wakes, carried in `Ev` on the host's one wake path, so its background threads wake the frame loop as before.
struct Lift(Arc<dyn WakeSender<Ev>>);

impl WakeSender<PhotonEvent> for Lift {
    fn send(&self, event: PhotonEvent) -> Result<(), WakeError> {
        self.0.send(Ev::Photon(event))
    }
}

/// PhotonApp under fluor's ferros host, with one addition: the person handed into the running app.
/// fluor's ferros host gives a background thread one way into the app's thread, the wake sender (`set_event_proxy`): a send queues the event, rings the window's bell (the frame loop wakes even while behind) and is delivered thru `on_user_event` at the top of the next pass. Photon's own event type has no variant for the person, so this wrapper's event carries Photon's or the person, and every other call goes straight to PhotonApp.
/// Every method of fluor's FluorApp is passed on here, defaults included, so PhotonApp answers each as it does under any host; the GPU ones (`cfg(fluor_gpu)`) are not compiled for ferros. A method fluor adds later takes its default here until it is passed on too.
struct Ferros {
    app: PhotonApp,
    /// The waiter is started once the host hands over the wake sender: every launch since 2026-11-16, as the person is never asked before the first frame.
    waiting: bool,
    /// The device's base and language as last handed to Photon (`numerals`).
    numerals: (u8, Option<[u8; 3]>),
}

/// The device's number base (preference 0, tatau's code) and language in effect (ISO 639-3; None, English), read from this process's own grants page: no call, so every frame looks, as ferros_fluor reads the app scale.
fn numerals_now() -> (u8, Option<[u8; 3]>) {
    (ferros_sys::device_base(), ferros_sys::lang::effective())
}

/// The device's base and language handed to Photon, where they win over Photon's own fleet settings (photon c7935249 `set_host_numerals`; owner, 2026-11-22: "That's an identity preference, not Photon. Same with language."): the person's identity preference reaches the device, and Photon follows the device. Photon has one dozenal form, so DMS (12) and DLS (G#8C) are both its dozenal; a language Photon's list lacks, or none, is English, as the shell's.
fn numerals_to_photon((base, lang): (u8, Option<[u8; 3]>)) {
    let b = match base {
        10 => photon_messenger::NumBase::Arabic,
        16 => photon_messenger::NumBase::Hex,
        _ => photon_messenger::NumBase::Dozenal,
    };
    let l = lang.and_then(fgtw::text::Lang::from_iso).unwrap_or(fgtw::text::Lang::En);
    photon_messenger::set_host_numerals(Some(b), Some(l));
}

impl FluorApp<Px> for Ferros {
    type UserEvent = Ev;

    fn title(&self) -> &str {
        self.app.title()
    }
    fn covers_menu_bar(&self) -> bool {
        self.app.covers_menu_bar()
    }
    fn menu(&self) -> Vec<fluor::host::menu::MenuItem> {
        self.app.menu()
    }
    fn dock_menu(&self) -> Vec<fluor::host::menu::MenuItem> {
        self.app.dock_menu()
    }
    fn window_icon(&self) -> Option<&fluor::host::icon::Icon> {
        self.app.window_icon()
    }
    fn set_event_proxy(&mut self, proxy: Arc<dyn WakeSender<Ev>>) {
        self.app.set_event_proxy(Arc::new(Lift(proxy.clone())));
        if self.waiting {
            // A thread of its own, as the call it makes is held by the identity service until something changes; named, so a fault's console line says whose it was.
            let spawned = std::thread::Builder::new().name("person-wait".into()).spawn(move || wait_for_person(proxy));
            if spawned.is_err() {
                println!("photon: no thread to wait for your identity on: Photon waits until it is launched again");
            }
        }
    }
    fn init(&mut self, ctx: &mut Context) {
        self.app.init(ctx)
    }
    fn on_resize(&mut self, width: u32, height: u32, ctx: &mut Context) {
        self.app.on_resize(width, height, ctx)
    }
    fn on_zoom(&mut self, factor: f32, anchor_x: Coord, anchor_y: Coord, ctx: &mut Context) {
        self.app.on_zoom(factor, anchor_x, anchor_y, ctx)
    }
    fn on_zoom_end(&mut self, ctx: &mut Context) {
        self.app.on_zoom_end(ctx)
    }
    fn owns_zoom_gesture(&self) -> bool {
        self.app.owns_zoom_gesture()
    }
    fn on_focus_changed(&mut self, focused: bool) {
        self.app.on_focus_changed(focused)
    }
    // The display's safe area (EV_SAFE: the great circle), handed on so Photon lays itself out inside it.
    fn on_safe_area(&mut self, area: fluor::host::app::SafeArea) {
        self.app.on_safe_area(area)
    }
    fn on_window_rect_changed(&mut self, x: i32, y: i32, w: u32, h: u32) {
        self.app.on_window_rect_changed(x, y, w, h)
    }
    fn on_event(&mut self, event: &Event, ctx: &mut Context) -> EventResponse {
        self.app.on_event(event, ctx)
    }
    fn on_activate(&mut self, hit_id: fluor::paint::HitId, x: Coord, y: Coord, mods: ModifiersState, ctx: &mut Context) -> EventResponse {
        self.app.on_activate(hit_id, x, y, mods, ctx)
    }
    fn damage_rect(&mut self, viewport: Viewport) -> Option<fluor::canvas::PixelRect> {
        self.app.damage_rect(viewport)
    }
    // The damage as Photon's rects and its word that the scratch outside them is never read (fluor 6fc6ea5, photon f209cf09): without these two the host took the defaults, one bounding box and a moved scratch (2026-11-17, the Pro: 5 ms of each scroll frame).
    fn damage_rects(&mut self, viewport: Viewport, out: &mut [fluor::canvas::PixelRect; fluor::canvas::DAMAGE_RECTS]) -> usize {
        self.app.damage_rects(viewport, out)
    }
    fn scratch_unread_outside_damage(&self) -> bool {
        self.app.scratch_unread_outside_damage()
    }
    fn scroll_hint(&mut self, viewport: Viewport) -> Option<(fluor::canvas::PixelRect, i32)> {
        self.app.scroll_hint(viewport)
    }
    fn render(&mut self, target: &mut [Px], ctx: &mut Context) {
        self.app.render(target, ctx)
    }
    fn render_own(&mut self, ctx: &mut Context) -> bool {
        self.app.render_own(ctx)
    }
    fn own_frame(&self) -> Option<&[Px]> {
        self.app.own_frame()
    }
    fn own_surface(&self) -> Option<u64> {
        self.app.own_surface()
    }
    fn own_baked(&self) -> bool {
        self.app.own_baked()
    }
    fn opaque(&self) -> bool {
        self.app.opaque()
    }
    fn overlay_deltas(&mut self) -> Vec<u32> {
        self.app.overlay_deltas()
    }
    fn overlay_bboxes(&mut self, viewport_w: usize, viewport_h: usize) -> Vec<Option<fluor::canvas::PixelRect>> {
        self.app.overlay_bboxes(viewport_w, viewport_h)
    }
    fn hit_test_map(&mut self) -> Option<(&mut [fluor::paint::HitId], usize, usize)> {
        self.app.hit_test_map()
    }
    fn builtin_zoom_chords(&self) -> bool {
        self.app.builtin_zoom_chords()
    }
    fn cursor_for(&self, x: Coord, y: Coord, ctx: &Context) -> CursorIcon {
        self.app.cursor_for(x, y, ctx)
    }
    fn wake_at(&self) -> Option<std::time::Instant> {
        self.app.wake_at()
    }
    fn take_zoom_request(&mut self) -> Option<f32> {
        self.app.take_zoom_request()
    }
    fn take_window_geometry_request(&mut self) -> Option<(i32, i32, u32, u32)> {
        self.app.take_window_geometry_request()
    }
    fn tick(&mut self, ctx: &mut Context) -> bool {
        // The device's base or language changed (the settings page, or the person's identity preference arriving): Photon follows, and the frame is drawn again.
        let now = numerals_now();
        let moved = now != self.numerals;
        if moved {
            self.numerals = now;
            numerals_to_photon(now);
        }
        self.app.tick(ctx) || moved
    }
    fn on_user_event(&mut self, event: Ev, ctx: &mut Context) -> EventResponse {
        match event {
            Ev::Photon(e) => self.app.on_user_event(e, ctx),
            // The people first, so they are in hand when the vault opens and its contacts load (set_photon_keys opens it at once on a Photon waiting for its identity).
            Ev::Person(keys, people) => {
                println!("photon: your identity arrived: be you and {} of your people handed in", people.len());
                self.app.set_people(people);
                self.app.set_photon_keys(keys);
                ctx.window.request_redraw();
                EventResponse::Handled
            }
        }
    }
    fn start_hidden(&self) -> bool {
        self.app.start_hidden()
    }
    fn on_close_requested(&mut self) -> bool {
        self.app.on_close_requested()
    }
    fn on_window_shown(&mut self) {
        self.app.on_window_shown()
    }
    fn initial_size(&self, monitor: (u32, u32)) -> (u32, u32) {
        self.app.initial_size(monitor)
    }
    fn wants_keyboard(&mut self) -> Option<bool> {
        self.app.wants_keyboard()
    }
    fn on_keyboard_inset(&mut self, rows: u32) {
        self.app.on_keyboard_inset(rows)
    }
    fn ime_editor_state(&mut self) -> Option<(String, usize)> {
        self.app.ime_editor_state()
    }
    fn ime_replace_chars(&mut self, start: usize, end: usize, s: &str, text: &mut TextRenderer) {
        self.app.ime_replace_chars(start, end, s, text)
    }
    fn wants_input_reset(&mut self) -> bool {
        self.app.wants_input_reset()
    }
}

/// Be you asked at once (ready, it answers in one pass of the identity service), else wait for the person (facets op 2, ferros_sys::facets::WAIT), then hand them into the running app: the identity service holds the call until be you's readiness changes (attest, the fleet joined, the person key handed to the kernel, or a lock), never asked on a clock (owner, 2026-11-10: "No polling!"). Ready, be you is asked; refused all the same (a lock between the two, a binding not for this alias), the next wait holds until the next change.
/// Ends on a refusal of the wait itself: be you not granted (the grant is the launch's, so it never comes while this process runs), or an identity service from before op 2. Photon then stays on its waiting line.
fn wait_for_person(wake: Arc<dyn WakeSender<Ev>>) {
    let since = |t: std::time::Instant| t.elapsed().as_millis();
    let started = std::time::Instant::now();
    println!("photon: the waiter is up {} ms after main", MAIN.get().map_or(0, |&t| since(t)));
    // Be you asked first, straight away: when the identity is ready (a relaunch, or any launch after attest) it answers in one pass of the identity service, where the wait below (op 2) took a pass to say "ready" and only then was be you asked (2026-11-17, the Pro: the first frame 5.1 s before be you's call). Refused (the person not in yet), the wait below holds until the identity service's readiness changes.
    if let Some(keys) = photon_keys() {
        println!("photon: be you answered at the first ask, {} ms after the waiter started", since(started));
        let people = people();
        if wake.send(Ev::Person(keys, people)).is_err() {
            println!("photon: the app's loop is gone; your identity not handed in");
        }
        return;
    }
    // A count the service never answered (it starts at 0 and moves by one), so the first wait is answered at once with the state now.
    let mut seen = u32::MAX;
    loop {
        let asked = std::time::Instant::now();
        let mut req = [2u8, 0, 0, 0, 0];
        req[1..5].copy_from_slice(&seen.to_le_bytes());
        let mut r = [0u8; 8];
        let Some(n) = sys::call(BE_YOU, &req, &mut r) else {
            println!("photon: be you not granted: Photon waits for your identity until it is granted and Photon launched again");
            return;
        };
        if n < 6 || r[0] != 0 {
            println!("photon: the identity service refused the wait for be you ({}): Photon waits until it is launched again", r[0]);
            return;
        }
        seen = u32::from_le_bytes([r[1], r[2], r[3], r[4]]);
        println!("photon: the identity service answered the wait for be you in {} ms ({}, its count {seen})", since(asked), if r[5] == 1 { "ready" } else { "not ready" });
        if r[5] != 1 {
            println!("photon: waiting for your identity (the identity service's count {seen})");
            continue;
        }
        if let Some(keys) = photon_keys() {
            let people = people();
            if wake.send(Ev::Person(keys, people)).is_err() {
                println!("photon: the app's loop is gone; your identity not handed in");
            }
            return;
        }
    }
}

fn main() {
    photon_messenger::init_logging();
    photon_messenger::log_version();
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        println!("PANIC at {location}: {info}");
    }));
    // The device key from this kist's ira (docs/PHOTON.md, "Photon's device key on ferros", 2026-11-12). Photon's own fallback reads the machine fingerprint, which on ferros (no files) was tohu's last resort, the bytes "unknown": every ferros device's Photon had one device key, derivable by anyone.
    // No ira, no Photon (the ira is handed to every kist since 2026-11-12, KISTS.md "The implicit set", so this is a kernel from before, where a person could say no to it, or no ira this boot): without it Photon has no key of its own to be a device with, and the shared one is the bug above. It ends at once, as it did before with every want off (KISTS.md, "Measured on the Pro").
    let started = std::time::Instant::now();
    let _ = MAIN.set(started);
    let Some(root) = sys::ira_key(IRA, "photon device") else {
        println!("photon: no ira key at slot {IRA} (the ira right was not granted); Photon has no device key of its own and does not start");
        return;
    };
    // The person, as facets (docs/FACETS.md; PHOTON.md "ferros's part"): Photon's alias and its binding, and the person's people, are asked on the waiter's thread once the host hands over the wake sender (`set_event_proxy`), never before the first frame. Photon starts on "waiting for your identity" (photon f6d954bf) and `wait_for_person` hands the person in: at once when the identity service is ready (op 2 answers a first wait straight away), else when it becomes so.
    let mut app = PhotonApp::new();
    let made = std::time::Instant::now();
    app.set_device_keypair(derive_device_keypair(&root));
    // No post-quantum device bundle (a Falcon and a SPHINCS+ keygen, about 370 ms before the first frame on the Pro): Photon on ferros acts as its alias alone and never signs a fleet write with the device key, so init installs the Ed25519 device signer (photon-9c, 2026-11-17).
    println!(
        "photon: {} ms from main to the host: the app made in {} ms, the device's keys from the ira in {} ms",
        started.elapsed().as_millis(),
        (made - started).as_millis(),
        made.elapsed().as_millis()
    );
    let waiting = true;
    // The device's base and language before the first frame, then followed every frame (`tick`).
    let numerals = numerals_now();
    numerals_to_photon(numerals);
    // No window controls strip (minimise, maximise, close: a desktop window manager's, nothing on ferros), as Photon's Android build has none: since 2026-11-12 the frame reaches the glass's edge under the console (WINDOWS.md "Drawn under the controls"), and the strip, cut away with the corner until then, would show in the top right under the radios' circles, which take every touch there.
    fluor::paint::DEBUG_SKIP_CONTROLS.store(true, std::sync::atomic::Ordering::Relaxed);
    let caps = fluor::host::ferros::Caps { display: 1, touch: 2, surface: 3 };
    // The person's app scale and the device's theme, followed live, and a pinch's end written back as the app scale with the display's grant (ferros_fluor; owner 2026-11-20: every kist follows the global scale and theme).
    let scale_token = sys::cap(caps.display);
    match ferros_fluor::run_app(Ferros { app, waiting, numerals }, caps, None, scale_token) {
        Ok(()) => println!("photon: closed"),
        Err(e) => println!("photon: {e}"),
    }
}
