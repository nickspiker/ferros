//! Kūaha on the host, end to end: things kept into the hold's real server by Lumis and a list-keeping kist, found by Kūaha by kind, time, maker and words thru its UI's own taps, opened, removed with two taps, and the device's view; every frame drawn at the Pro's size and written for the owner (`CARGO_TARGET_TMPDIR`/kuaha-screens, a VSF visual and its ASCII).
//! These run on x86_64-unknown-linux-gnu: the core has no platform in it.

mod bench;
mod screen;

use bench::*;
use ferros_items::{kinds, Proposed, ORDER_LARGEST, ORDER_OLDEST, ORDER_TITLE};
use ferros_lang::Lang;
use kuaha_core::model::{Load, When};
use kuaha_core::source::{KistStore, Usage};
use kuaha_core::ui::Hit;
use kuaha_core::{HoldSource, Kuaha, Locale, Prefs, Problem, View};
use screen::*;
use vsf::VsfType;

/// The moment the screens are drawn: a little after the bench's things were kept.
fn loc() -> Locale {
    Locale::plain(T0 + 1000 * SECOND)
}

/// The hold as the owner might have it: Lumis's stills, a scan and two RAW clips (one sealed, one still recording), a grocery list and a contact from another kist, and a still from over a year ago.
fn stocked() -> (std::rc::Rc<std::cell::RefCell<Bench>>, ferros_items::kist::Clip) {
    let b = Bench::new();
    let mut l = hold(&b, &lumis());
    let mut r = hold(&b, &lists());
    // A year and more back.
    b.borrow_mut().now = T0 - 400 * 86_400 * SECOND;
    l.keep(&Proposed { kind: kinds::PHOTO, title: Some("Raglan, the bar at low tide".into()), ..Default::default() }, &bytes(3 << 20, 1)).unwrap();
    b.borrow_mut().now = T0;
    for (title, n) in [("Kids in the river", 5usize), ("Bench, lit from the left", 4), ("", 6)] {
        let p = Proposed { kind: kinds::PHOTO, title: (!title.is_empty()).then(|| title.into()), attrs: vec![("lens".into(), VsfType::u3(24))], ..Default::default() };
        l.keep(&p, &bytes(n << 20, n as u8)).unwrap();
    }
    l.keep(&Proposed { kind: kinds::SCAN, title: Some("Car registration".into()), ..Default::default() }, &bytes(1 << 20, 7)).unwrap();
    r.keep(&Proposed { kind: kinds::LIST, title: Some("Groceries: milk, kūmara, eggs".into()), ..Default::default() }, b"milk\nkumara\neggs\n").unwrap();
    r.keep(&Proposed { kind: kinds::CONTACT, title: Some("Ana".into()), ..Default::default() }, b"ana").unwrap();
    // A sealed RAW clip, a few MiB.
    let p = Proposed { kind: kinds::RAW_VIDEO, title: Some("Bench, 30 a second".into()), attrs: vec![("fps".into(), VsfType::u3(30))], ..Default::default() };
    let clip = l.make_clip(&p, 1 << 10).unwrap();
    let body = bytes(3 << 20, 9);
    for (n, chunk) in body.chunks(1 << 20).enumerate() {
        l.write_clip(&clip, (n as u64) << 8, chunk).unwrap();
    }
    l.seal_clip(clip, body.len() as u64, blake3::hash(&body).as_bytes()).unwrap();
    // One still recording: its card is placed at once, no bytes yet.
    let recording = l.make_clip(&Proposed { kind: kinds::RAW_VIDEO, title: Some("Recording now".into()), ..Default::default() }, 1 << 8).unwrap();
    (b, recording)
}

fn tap(k: &mut Kuaha, s: &mut Screen, src: &mut dyn HoldSource, which: impl Fn(&Hit) -> bool) {
    s.draw(k, &loc());
    let (x, y) = k.where_is(which).expect("not on the screen");
    k.press(x, y);
    k.release(x, y);
    pump(k, src);
    s.draw(k, &loc());
}

fn shown(k: &Kuaha) -> Vec<String> {
    match &k.model.found {
        Load::Ready(v) => v.iter().map(|(_, c)| c.title.clone().unwrap_or_default()).collect(),
        other => panic!("not found: {other:?}"),
    }
}

#[test]
fn an_empty_hold_reads_well() {
    let b = Bench::new();
    let mut src = source(&b);
    let mut k = Kuaha::new(None, loc().now);
    pump(&mut k, &mut src);
    assert_eq!(k.model.found, Load::Ready(vec![]));
    assert_eq!(k.model.totals.ready().unwrap().items, 0);
    let mut s = Screen::new();
    s.draw(&mut k, &loc());
    println!("{}", s.write("00-empty"));
    // The words are drawn: something lit inside the circle beyond the tabs and the search line.
    assert!(s.ascii(112).lines().skip(20).any(|l| l.trim() != ""));
    // The device view of an empty hold: nothing by kind, the clips area all free.
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Tab(View::Device));
    assert_eq!(k.model.view, View::Device);
    let u = k.model.usage.ready().unwrap().as_ref().unwrap();
    assert!(u.kinds.is_empty());
    assert_eq!(u.clips.as_ref().map(|a| (a.taken, a.largest_free)), Some((0, AREA)));
    println!("{}", s.write("01-empty-device"));
}

#[test]
fn things_found_by_kind_time_maker_and_words() {
    let (b, _rec) = stocked();
    let mut src = source(&b);
    let mut k = Kuaha::new(None, loc().now);
    let mut s = Screen::new();
    pump(&mut k, &mut src);
    let t = k.model.totals.ready().unwrap().clone();
    assert_eq!(t.items, 9);
    assert_eq!(t.kinds.iter().map(|k| (k.kind, k.items)).collect::<Vec<_>>(), vec![(kinds::PHOTO, 4), (kinds::RAW_VIDEO, 2), (kinds::SCAN, 1), (kinds::LIST, 1), (kinds::CONTACT, 1)]);
    assert_eq!(t.makers.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["Lumis", "Rārangi"]);
    s.draw(&mut k, &loc());
    println!("{}", s.write("10-things"));
    // Newest first: the recording clip was the last made.
    assert_eq!(shown(&k)[0], "Recording now");
    // By kind: a photo's circle narrows, a second tap widens.
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Kind(Some(kinds::PHOTO)));
    assert_eq!(shown(&k).len(), 4);
    println!("{}", s.write("11-photos"));
    // Past day: the year-old still drops out.
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::When);
    assert_eq!(k.model.query.when, When::Day);
    assert!(!shown(&k).iter().any(|t| t.starts_with("Raglan")));
    assert_eq!(shown(&k).len(), 3);
    for _ in 0..4 {
        tap(&mut k, &mut s, &mut src, |h| *h == Hit::When);
    }
    assert_eq!(k.model.query.when, When::Any);
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Kind(Some(kinds::PHOTO)));
    assert_eq!(shown(&k).len(), 9);
    // By maker: the first maker in name order is Lumis, then Rārangi, then any again.
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Maker);
    assert_eq!(shown(&k).len(), 7);
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Maker);
    assert_eq!(shown(&k), vec!["Ana", "Groceries: milk, kūmara, eggs"]);
    println!("{}", s.write("12-by-rarangi"));
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Maker);
    assert_eq!(k.model.query.maker, None);
    // Orders: oldest first, by title, largest first, newest again.
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Order);
    assert_eq!(k.model.query.order, ORDER_OLDEST);
    assert!(shown(&k)[0].starts_with("Raglan"));
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Order);
    assert_eq!(k.model.query.order, ORDER_TITLE);
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Order);
    assert_eq!(k.model.query.order, ORDER_LARGEST);
    assert_eq!(k.model.card(&k.model.found.ready().unwrap()[0].0).unwrap().size, 6 << 20);
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Order);
    // By words: the search line takes the keyboard, every letter narrows.
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Search);
    assert_eq!(k.wants_keyboard(), Some(true));
    k.typed("kids");
    pump(&mut k, &mut src);
    assert_eq!(shown(&k), vec!["Kids in the river"]);
    s.draw(&mut k, &loc());
    println!("{}", s.write("13-words"));
    k.typed("x");
    pump(&mut k, &mut src);
    assert!(shown(&k).is_empty());
    s.draw(&mut k, &loc());
    println!("{}", s.write("14-nothing-matches"));
    k.backspace();
    k.enter();
    assert_eq!(k.wants_keyboard(), Some(false));
    pump(&mut k, &mut src);
    assert_eq!(shown(&k).len(), 1);
}

#[test]
fn a_card_opens_and_its_remove_takes_two_taps() {
    let (b, _rec) = stocked();
    let mut src = source(&b);
    let mut k = Kuaha::new(None, loc().now);
    let mut s = Screen::new();
    pump(&mut k, &mut src);
    let list = k.model.found.ready().unwrap().iter().find(|(_, c)| c.kind == kinds::LIST).unwrap().0;
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Item(list));
    assert_eq!(k.model.open, Some(list));
    println!("{}", s.write("20-a-list-opened"));
    // Open is greyed: nothing happens, nothing is handed over.
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Open(list));
    assert_eq!(k.model.open, Some(list));
    // The first tap arms it, a tap elsewhere disarms, two in a row remove it.
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Remove(list));
    assert_eq!(k.model.armed, Some(list));
    println!("{}", s.write("21-remove-armed"));
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Open(list));
    assert_eq!(k.model.armed, None);
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Remove(list));
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Remove(list));
    assert!(k.model.card(&list).is_none());
    assert_eq!(k.model.total, 8);
    assert_eq!(k.model.totals.ready().unwrap().items, 8);
    // Gone for its maker too.
    assert_eq!(hold(&b, &lists()).find(&Default::default()).unwrap().total, 1);
    println!("{}", s.write("22-removed"));
    // A photo opened: Opsin will open it.
    let photo = k.model.found.ready().unwrap().iter().find(|(_, c)| c.kind == kinds::PHOTO).unwrap().0;
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Item(photo));
    println!("{}", s.write("23-a-photo-opened"));
}

#[test]
fn a_clip_still_recording_is_not_removed() {
    let (b, rec) = stocked();
    let mut src = source(&b);
    let mut k = Kuaha::new(None, loc().now);
    let mut s = Screen::new();
    pump(&mut k, &mut src);
    let h = k.model.found.ready().unwrap()[0].0;
    assert_eq!(k.model.card(&h).unwrap().title.as_deref(), Some("Recording now"));
    tap(&mut k, &mut s, &mut src, |x| *x == Hit::Item(h));
    tap(&mut k, &mut s, &mut src, |x| *x == Hit::Remove(h));
    tap(&mut k, &mut s, &mut src, |x| *x == Hit::Remove(h));
    assert_eq!(k.model.refused, Some((h, Problem::Refused)));
    assert!(k.model.card(&h).is_some());
    println!("{}", s.write("30-recording-not-removed"));
    // Sealed, it goes.
    let mut l = hold(&b, &lumis());
    l.seal_clip(rec, 0, blake3::hash(b"").as_bytes()).unwrap();
    tap(&mut k, &mut s, &mut src, |x| *x == Hit::Remove(h));
    tap(&mut k, &mut s, &mut src, |x| *x == Hit::Remove(h));
    assert!(k.model.card(&h).is_none());
}

#[test]
fn a_right_said_no_and_a_hold_that_does_not_answer() {
    for (right, want, name) in [(false, Problem::NoRight, "40-no-right"), (true, Problem::Silent, "41-hold-silent")] {
        let mut src = kuaha_core::Over::new(Refusing, clock, right);
        let mut k = Kuaha::new(None, loc().now);
        pump(&mut k, &mut src);
        assert_eq!(k.model.found, Load::Failed(want));
        assert_eq!(k.model.totals, Load::Failed(want));
        let mut s = Screen::new();
        s.draw(&mut k, &loc());
        println!("{}", s.write(name));
    }
}

/// The bench's hold with the kists the vault process adds on a device (the bench's server leaves them to the vault: ferros_items `usage_shows_the_whole_device`): Lumis with its clips recorded into its own domain, Mahere's downloaded areas, Photon, basecalc. Sized one a call, as the device sizes them.
struct WithKists {
    hold: kuaha_core::Over<Kist>,
    /// The kists sized so far, to show the rows fill in.
    sized: Vec<kuaha_core::source::KistId>,
}

fn kists() -> Vec<KistStore> {
    let k = |n: u8, name: &str, bytes: u64, values: u64, clip_bytes: u64| KistStore { developer: [n; 32], option: [n + 1; 32], name: name.into(), values: Some(values), bytes: Some(bytes), clip_bytes };
    vec![k(1, "Lumis", 1 << 22, 3, 5 << 29), k(10, "Mahere", 167 << 20, 1 << 12, 0), k(20, "Photon", 3 << 20, 2412, 0), k(30, "basecalc", 1 << 11, 1, 0)]
}

impl HoldSource for WithKists {
    fn find(&mut self, f: &ferros_items::Find) -> Result<ferros_items::Found, Problem> {
        self.hold.find(f)
    }
    fn card(&mut self, h: &[u8; 32]) -> Result<ferros_items::Card, Problem> {
        self.hold.card(h)
    }
    fn read(&mut self, h: &[u8; 32], offset: u64, buf: &mut [u8]) -> Result<(usize, u64), Problem> {
        self.hold.read(h, offset, buf)
    }
    fn remove(&mut self, h: &[u8; 32]) -> Result<(), Problem> {
        self.hold.remove(h)
    }
    fn keep(&mut self, p: &Proposed, body: &[u8]) -> Result<([u8; 32], ferros_items::Card), Problem> {
        self.hold.keep(p, body)
    }
    fn usage(&mut self) -> Result<Option<Usage>, Problem> {
        let mut u = self.hold.usage()?.unwrap();
        u.kists = kists().into_iter().map(|k| KistStore { values: None, bytes: None, ..k }).collect();
        Ok(Some(u))
    }
    fn size_kists(&mut self, ids: &[kuaha_core::source::KistId]) -> Result<Vec<KistStore>, Problem> {
        self.sized.extend_from_slice(ids);
        Ok(kists().into_iter().filter(|k| ids.contains(&(k.developer, k.option))).collect())
    }
    fn links(&self) -> Vec<kuaha_core::Link> {
        self.hold.links()
    }
}

#[test]
fn the_device_view_from_the_usage_call() {
    let (b, _rec) = stocked();
    let mut src = source(&b);
    let mut k = Kuaha::new(Some(Prefs { view: View::Device, query: Default::default() }), loc().now);
    let mut s = Screen::new();
    pump(&mut k, &mut src);
    // The bench's own usage: every item by kind, the clips area with its two clips' blocks taken; no kists (the vault adds them on a device).
    let u = k.model.usage.ready().unwrap().as_ref().unwrap().clone();
    assert_eq!(u.kinds.iter().map(|k| (k.kind, k.items)).collect::<Vec<_>>().len(), 5);
    let a = u.clips.unwrap();
    assert_eq!((a.blocks, a.taken), (AREA, (3 << 8) + (1 << 8)));
    s.draw(&mut k, &loc());
    println!("{}", s.write("50-device-hold-only"));
    // With the kists: the rows come first, their sizes as each is sized, the quick asks ahead of them.
    let mut src = WithKists { hold: src, sized: Vec::new() };
    k.focus(true);
    let asks = k.take_asks();
    assert!(asks.iter().all(|a| !a.slow()));
    for a in asks {
        let answer = kuaha_core::serve(&mut src, a);
        k.answer(answer);
    }
    let unsized_: Vec<_> = k.model.usage.ready().unwrap().as_ref().unwrap().kists.iter().map(|k| k.bytes).collect();
    assert_eq!(unsized_, vec![None; 4]);
    s.draw(&mut k, &loc());
    println!("{}", s.write("51-device-kists-unsized"));
    let asks = k.take_asks();
    assert_eq!(asks.len(), 4);
    assert!(asks.iter().all(|a| a.slow()));
    for a in asks {
        let answer = kuaha_core::serve(&mut src, a);
        k.answer(answer);
    }
    assert_eq!(src.sized.len(), 4);
    assert_eq!(k.model.usage.ready().unwrap().as_ref().unwrap().kists, kists());
    s.draw(&mut k, &loc());
    println!("{}", s.write("52-device-counted"));
    // In arabic too (sizes in bytes with binary prefixes there; bits elsewhere).
    let mut a = loc();
    a.base = tatau::Base::Arabic;
    s.draw(&mut k, &a);
    println!("{}", s.write("53-device-arabic"));
}

#[test]
fn kuahas_choices_are_kept_and_read_back() {
    let (b, _rec) = stocked();
    let mut src = source(&b);
    let mut k = Kuaha::new(None, loc().now);
    let mut s = Screen::new();
    pump(&mut k, &mut src);
    assert_eq!(k.take_prefs(), None);
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Kind(Some(kinds::RAW_VIDEO)));
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Order);
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Maker);
    tap(&mut k, &mut s, &mut src, |h| *h == Hit::Search);
    k.typed("bench");
    let p = k.take_prefs().expect("changed");
    assert_eq!(k.take_prefs(), None);
    let doc = p.build(T0).unwrap();
    assert_eq!(Prefs::parse(&doc).unwrap(), p);
    assert_eq!(p.query.kind, Some(kinds::RAW_VIDEO));
    assert_eq!(p.query.words, "bench");
    // Kūaha started again from them finds the same.
    let mut k2 = Kuaha::new(Some(Prefs::parse(&doc).unwrap()), loc().now);
    pump(&mut k2, &mut src);
    assert_eq!(shown(&k2), vec!["Bench, 30 a second"]);
}

#[test]
fn a_fling_settles_inside_the_column() {
    let b = Bench::new();
    let mut r = hold(&b, &lists());
    for i in 0..40 {
        r.keep(&Proposed { kind: kinds::LIST, title: Some(format!("List {i}")), ..Default::default() }, b"x").unwrap();
    }
    let mut src = source(&b);
    let mut k = Kuaha::new(None, loc().now);
    let mut s = Screen::new();
    pump(&mut k, &mut src);
    s.draw(&mut k, &loc());
    let (_, most) = k.scrolled();
    assert!(most > 0., "forty cards are taller than the circle");
    // A drag up, a frame at a time, then the lift: the column flies on and settles, never past its end for good.
    let (x, mut y) = (W as f32 * 0.5, H as f32 * 0.75);
    k.press(x, y);
    for _ in 0..8 {
        y -= 120.;
        k.move_to(x, y);
        k.tick();
    }
    k.release(x, y);
    let mut frames = 0;
    let mut last = k.scrolled().0;
    while k.tick() {
        frames += 1;
        assert!(frames < 1 << 12, "never settled");
        let now = k.scrolled().0;
        assert!(now >= last - 1e-3 || last > most, "moved back before the end");
        last = now;
        s.draw(&mut k, &loc());
    }
    let (at, most) = k.scrolled();
    assert!((0. ..=most).contains(&at), "settled at {at} outside 0..{most}");
    println!("{}", s.write("60-scrolled"));
    // Pulled past the top and let go: back to 0 by halving.
    k.press(x, H as f32 * 0.3);
    k.move_to(x, H as f32 * 0.3 + 30.);
    k.move_to(x, H as f32 * 0.3 + 30. + at + 500.);
    k.tick();
    k.tick();
    k.tick();
    k.release(x, H as f32 * 0.3 + 30. + at + 500.);
    while k.tick() {}
    assert_eq!(k.scrolled().0, 0.);
}

#[test]
fn every_language_and_base_draws() {
    let (b, _rec) = stocked();
    let mut src = source(&b);
    let mut k = Kuaha::new(None, loc().now);
    let mut s = Screen::new();
    pump(&mut k, &mut src);
    let h = k.model.found.ready().unwrap()[1].0;
    s.draw(&mut k, &loc());
    tap(&mut k, &mut s, &mut src, |x| *x == Hit::Item(h));
    for i in 0..ferros_lang::COUNT {
        let lang = Lang::from_index(i);
        if !ferros_lang::renders(lang) {
            continue;
        }
        for base in [tatau::Base::Arabic, tatau::Base::Dms, tatau::Base::Hex, tatau::Base::Dls] {
            let mut l = loc();
            l.lang = lang;
            l.base = base;
            s.draw(&mut k, &l);
        }
    }
    let mut mi = loc();
    mi.lang = Lang::Mi;
    s.draw(&mut k, &mi);
    println!("{}", s.write("70-maori"));
    for theme in 1..4 {
        let mut t = loc();
        t.theme = theme;
        s.draw(&mut k, &t);
        println!("{}", s.write(&format!("71-theme-{theme}")));
    }
}

/// Back goes up a level at a time: the keyboard, an armed remove, an open card, the device's view; at the trunk it says so, and the deck takes it.
#[test]
fn back_goes_up_a_level_at_a_time() {
    let (b, _rec) = stocked();
    let mut src = source(&b);
    let mut k = Kuaha::new(None, loc().now);
    let mut s = Screen::new();
    pump(&mut k, &mut src);
    let h = k.model.found.ready().unwrap()[2].0;
    tap(&mut k, &mut s, &mut src, |x| *x == Hit::Item(h));
    tap(&mut k, &mut s, &mut src, |x| *x == Hit::Remove(h));
    assert!(k.back());
    assert_eq!(k.model.armed, None);
    assert!(k.back());
    assert_eq!(k.model.open, None);
    // Any tap but the search line's ends typing, so the keyboard is its own level.
    tap(&mut k, &mut s, &mut src, |x| *x == Hit::Search);
    assert_eq!(k.wants_keyboard(), Some(true));
    assert!(k.back());
    assert_eq!(k.wants_keyboard(), Some(false));
    assert!(!k.back());
    tap(&mut k, &mut s, &mut src, |x| *x == Hit::Tab(View::Device));
    assert!(k.back());
    assert_eq!(k.model.view, View::Things);
    assert!(!k.back());
}
