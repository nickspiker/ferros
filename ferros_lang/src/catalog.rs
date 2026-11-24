//! The catalog: every message the shell draws (`Msg`, words alone; `Say`, words with holes), and each language's one exhaustive match over them (catalog/en.rs is the reference; the rest are its translations).
//!
//! Where Photon says the same thing, a language here uses Photon's words for it (photon src/ui/lang/*.rs), in the shell's lower case where the shell writes lower case.

use crate::Lang;
use core::fmt::{self, Write};

mod de;
mod en;
mod es;
mod fil;
mod fr;
mod hi;
mod id;
mod it;
mod mi;
mod pl;
mod pt;
mod ru;
mod sw;
// Turkish's code is `tr`; the module is named for it like every other language here.
mod tr;
mod uk;
mod vi;

/// The words-alone messages, declared once: the enum and `Msg::ALL` (every message, for the tests that walk them) come from the same list, so neither can miss one.
macro_rules! messages {
    ($($(#[$m:meta])* $v:ident,)*) => {
        /// A message that is words alone. Grouped by where it shows; each group's comment says the screen.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Msg {
            $($(#[$m])* $v,)*
        }
        impl Msg {
            /// Every message, in declaration order.
            pub const ALL: &'static [Msg] = &[$(Msg::$v,)*];
        }
    };
}

messages! {
    // ---- the map: the settings circle and its pages' titles (payloads/map PAGES; WiFi, Bluetooth, NFC, GPS and USB are names and stay) ----
    /// The settings circle's name under it, and a link page's "settings" heading over its knobs.
    Settings,
    PageAppearance,
    /// The number base's page; also the clock page's fact naming the base in use.
    PageUnits,
    PageKeyboard,
    PageSecurity,
    /// The page listing everything on the deck as a circle, by kind (owner, 2026-11-12: "I want to make sure it says items in settings, not kists"): kists, pucks, later contacts; each row opens its card.
    PageDeck,
    /// The helm's page (owner, 2026-11-18: "a settings addition to choose what back and home do"): what Deck and Back do, tapped and swiped, in a kist and on the deck; then its rows' titles and their choices.
    PageHelm,
    HelmBackTap,
    HelmBackSwipe,
    HelmBackSwipeDeck,
    HelmDeckSwipe,
    HelmDeckSwipeDeck,
    /// Back's tap: up a level to the kist's top and no further (the default), or up and then out to the deck.
    HelmUpTop,
    HelmUpOut,
    /// Back's swipe in a kist: the kist before (the most recent, the default), or forward: the redo of the last step back, the Again key to the kist (owner: "like a redo or a forward browser navigation").
    HelmRecent,
    HelmForward,
    /// On the deck: every circle packed into the screen, as large as they all fit, touching (Back's tap scatters them back).
    HelmPack,
    /// Deck's swipe in a kist: the controls hidden or shown (the default), or out to the deck.
    HelmControls,
    HelmToDeck,
    /// Deck's swipe on the deck: every circle in view (the default).
    HelmFit,
    PagePermissions,
    PageFleet,
    PagePower,
    PageConnections,
    /// The cellular radio's page (and its row on Connections).
    PageCellular,
    PageClock,
    PageLanguage,
    PageAbout,
    /// The sound page: the speaker, microphone and buzz tests (owner, 2026-11-24).
    PageSound,
    PageDeveloper,

    // ---- a link's state (Connections, a link's page) ----
    /// Switched off by its status circle.
    SwitchedOff,
    /// Also: a value knob's "off", the light scope's "off".
    Off,
    /// Also: a link's switch, a knob's "on".
    On,
    OnNoLink,
    /// Also the attest screen's network row: the kept network is joined.
    Connected,
    /// A link's state: the name of the fact whose value is one of the above.
    State,

    // ---- a link's measures: each one's name (a link's page, the name left of its value) ----
    Technology,
    /// Also the group heading "signal".
    Signal,
    NoiseFloor,
    SignalToNoise,
    ReferencePower,
    ReferenceQuality,
    SignalToInterference,
    /// "antenna match (VSWR)": VSWR stays.
    AntennaMatch,
    TransmitPower,
    Frequency,
    ChannelWidth,
    /// Also the group heading "channel".
    Channel,
    RateDown,
    RateUp,
    RoundTrip,
    Loss,
    /// The spatial streams of a WiFi link.
    Streams,
    SatellitesSeen,
    SatellitesInFix,
    BestCarrierToNoise,
    /// "accuracy (±)".
    Accuracy,
    Registration,
    LargestPacket,
    PacketsIn,
    PacketsOut,
    BadPacketsIn,
    FailedPacketsOut,
    DroppedIn,
    RefusedOut,
    /// How often the link went to sleep between packets.
    LinkNaps,
    PowerSave,
    LinkPower,
    Altitude,
    Speed,
    Heading,
    /// A measure of a kind this page has no name for.
    Measure,
    // Registration's values.
    NotRegistered,
    /// Registered on the home network.
    RegHome,
    Searching,
    Denied,
    Roaming,
    EmergencyOnly,
    Unknown,
    // Power save's values (beside Off and On).
    Fast,
    /// WiFi power save following the screen (on while it is dark).
    WithTheScreen,
    // Link power's values.
    Working,
    LightSleep,
    Sleep,
    Suspended,
    // The measures' group headings (signal and channel are `Signal`, `Channel`).
    GroupLink,
    GroupCounters,
    GroupMotion,
    /// The heading over the satellites tracked.
    Satellites,
    /// "measures: none yet": the fact's name, then its value.
    Measures,
    NoneYet,
    NamesAndAddresses,
    // A link's names and addresses (ferros_detail): each field's name. DNS stays.
    Carrier,
    /// "data APN": APN stays.
    DataApn,
    /// "attach APN".
    AttachApn,
    Address,
    /// "address (IPv6)".
    AddressIpv6,
    /// "DNS (IPv6)".
    DnsIpv6,
    /// The network a WiFi link is on (its name).
    Network,
    AccessPoint,
    /// The device's own hardware address on that network.
    ThisDeviceOnIt,
    Router,
    /// A DHCP lease's length.
    Lease,
    Detail,
    /// A satellite system the page has no name for (GPS, GLONASS, Galileo, BeiDou, QZSS, NavIC stay).
    OtherSystem,
    // The links' knobs.
    ReRegisterNow,
    ReRegisterNote,
    Band,
    /// "any (5 GHz first)".
    BandAny,
    /// "5 GHz only".
    Band5Only,
    /// "2.4 GHz only".
    Band24Only,
    /// Two lines.
    BandNote,
    NothingToSetYet,
    // The GPS page (owner, 2026-11-10): where the device is (under `RightGps`, "where you are"), and who is using GPS now.
    /// The seven Dymaxion words (vsf WorldCoord) that name the place.
    PositionWords,
    /// "latitude, longitude", in decimal degrees.
    LatLon,
    /// "degrees, minutes, seconds".
    DegMinSec,
    NoFixYet,
    /// A GPS link's state once its driver has a fix (its state row; owner, 2026-11-10: `Searching` until then).
    HasFix,
    /// The GPS driver started for the page and not answering yet.
    StartingUp,
    /// The heading over who is using GPS now.
    UsingNow,

    // ---- the number bases (Units, the clock's "units" fact) ----
    BaseArabic,
    BaseHex,
    /// "dozenal linear (DLS)".
    BaseDls,
    /// "dozenal magnitude (DMS)".
    BaseDms,

    // ---- Appearance ----
    Brightness,
    /// The UI scale's slider.
    Scale,
    NamesUnderCircles,
    /// The themes' names, in ferros_sys::theme::THEMES order: grey, slate, ember, night.
    ThemeGrey,
    ThemeSlate,
    ThemeEmber,
    ThemeNight,

    // ---- the quick row (payloads/display, QUICK.md): each circle's word; brightness is `Brightness` ----
    /// The torch (the camera's flash held on).
    Torch,
    /// Every radio switched off at once (a tap puts back the ones that were on).
    RadiosOff,
    /// The sound mode's three: sounds play, the device buzzes instead, neither.
    SoundOn,
    Vibrate,
    Silent,
    /// The person's do not disturb (follows them across their devices).
    DoNotDisturb,
    /// A capture of the screen.
    Screencap,
    /// The app in front to the glass's edge, no circle and no bars (the quick row's middle circle).
    FullScreen,

    // ---- the torch's page (its title is `Torch`; its first slider is `Brightness`, its on and off `On`) ----
    /// The torch flashing on and off: the name of its slider, whose value is `Off` or how long each flash and its gap take together (a time).
    Strobe,
    /// How much of each flash's period the torch is lit, a share (the duty cycle).
    LitPerFlash,
    /// The torch turns itself off after this long, a time or `Never`.
    TurnsOffAfter,
    /// The timeout's "off": the torch stays lit until switched off.
    Never,

    // ---- the battery's page (the console's battery circle; its title is `Battery`) ----
    /// The state of charge.
    Charge,
    /// The cell's voltage.
    Voltage,
    /// The current in or out of the cell.
    Current,
    /// The power in or out of the cell (voltage times current).
    PowerDraw,
    /// The cell's temperature.
    Temperature,
    /// The current's direction: into the cell, or out of it.
    Charging,
    Discharging,
    /// The charge voltage cap's slider (owner, 2026-11-24; docs/CHARGING.md): its name.
    ChargeCap,
    /// Under the slider: what the cap does, and that the share beside the voltage is rough (two lines).
    ChargeCapNote,
    /// Above the default: wear is not linear, and the one sourced measure of it (Battery University BU-808, Table 4; two lines).
    ChargeCapWear,

    // ---- Keyboard ----
    /// The keyboard layout's heading (Dvorak, QWERTY stay).
    Layout,

    // ---- Security ----
    /// The checkbox: the device attests at boot without the handle typed ("attest", as the attest screen says it; owner, 2026-11-10: "always attest").
    AttestAtBoot,
    /// Two lines.
    AttestAtBootNote,

    // ---- the Android page (owner, 2026-11-21: Android starts only when the attested identity chose it; docs/BOOT.md "Android only by the identity's choice"); "ferros" and "Android" are names and stay ----
    /// The heading over the choice of what this device starts into: ferros or Android.
    StartInto,
    /// Two lines: what choosing Android there does.
    StartIntoNote,
    /// The button that restarts into Android now (its first tap; the second is `TapAgainStartAndroid`).
    StartAndroidNow,
    TapAgainStartAndroid,
    TurnAndroidOff,
    /// For a person who accepted the disclaimer before: no disclaimer again (once per person).
    TurnAndroidOn,
    AndroidOffHere,
    /// The button under the disclaimer: the device locks and the attest screen asks to approve Android on, the person's handle typed there (owner, 2026-11-24; no tap and no hold).
    AndroidAgree,
    /// The disclaimer in full (owner, 2026-11-01, android_behind_attestation; the one place "phone" is right, it names what is lost). Three paragraphs. Its number is kernel service.rs `DISCLAIMER_VERSION`: a changed text gets the next one.
    AndroidDisclaimer,
    /// Android's recovery request, asked after the attest (owner, 2026-11-22; docs/BOOT.md "Recovery requests from Android"): one question for each class, each a paragraph the page wraps. A factory reset Android asked for.
    RecoveryWipe,
    /// RescueParty: Android could not start, and its recovery would offer to erase its data.
    RecoveryRescue,
    /// A wipe asked from afar (a device admin, Find My Device): refused, only no.
    RecoveryRemote,
    /// An update, a sideload, fastbootd or the recovery menu: needs a second device of the person's, not built, so only no.
    RecoveryChanges,
    /// The yes button (its first tap; the second is `TapAgainAllowOnce`).
    AllowOnce,
    TapAgainAllowOnce,
    /// The no button: the request is cleared.
    RecoveryNo,
    /// Two lines under the buttons: what each answer does.
    RecoveryNote,

    // ---- the NFC page: attest with a tag (AUTH.md "Attest with a tag"; owner, 2026-11-11) ----
    /// The heading over the tags kept to attest with.
    TagAttest,
    /// The button that keeps the next tag held to the device.
    TagAdd,
    /// While it waits for that tag (the button then cancels: `Cancel`).
    TagHold,
    /// What the last add did: kept, kept already, not kept.
    TagKeptNow,
    TagKeptAlready,
    TagNotKept,
    /// Two lines, under the list: what a kept tag does (the warning, as attest at boot's).
    TagNote,
    /// A kept tag's button.
    TagRemove,

    // ---- Fleet: ferros's own lines (the fleet's and the identity's words are fgtw's, shared with Photon: fgtw::text) ----
    FleetServiceNotRunning,
    /// This device's state on the Fleet page while its join waits for its person's confirmation on the attest screen (the identity service's state 11).
    JoinNotConfirmed,
    DeckFetching,
    /// Also the checkbox's label.
    DeckSame,
    DeckOwn,
    /// Two lines under the Fleet page's Remove: what removing this device does (owner, 2026-11-22; docs/AUTH.md "Remove this device").
    RemoveNote,
    /// The keyboard's status once the removal started: the fleet told, the erase, the power off.
    RemovingDevice,
    /// Unbind refused: the identity's only device (an identity must live somewhere).
    RemoveLastDevice,
    /// Unbind refused: this device is in no fleet.
    RemoveNotMember,
    /// Unbind refused for any other reason; nothing erased.
    RemoveNotStarted,

    // ---- Power ----
    ProximityWake,
    /// Two lines.
    ProximityWakeNote,

    // ---- the clock page ----
    AtUtc,
    Time,
    NotReadYet,
    /// The zone picker's title and the button that opens it.
    ChooseAZone,
    Back,
    /// No zone: the clock keeps a fixed offset.
    NoneFixedOffset,
    Zone,
    NoZonesInKernel,
    /// The zone's standard offset (all year) and its legal one (with daylight time when in it).
    Standard,
    Legal,
    /// Where the zone in effect comes from: the fact's name, then one of the four below.
    From,
    FromNetwork,
    FromPosition,
    FromHand,
    FromOffset,
    /// Two lines: a zone found from a position is the nearest principal place's.
    NearestPlaceNote,
    /// The checkbox on the clock and language pages: this device follows the identity's choice.
    FollowMyIdentity,
    /// Under it, unchecked (the clock page).
    ZoneOwn,
    /// Under it, checked, before the identity has a zone.
    ZoneIdentityNone,
    /// Under it, checked, on the clock and language pages.
    SetHereAllFollow,
    Automatic,
    ByHand,
    /// Two lines, under automatic: nothing found yet.
    ZoneAutoNothingFound,
    /// Two lines, under automatic: a zone was found.
    ZoneAutoStandsIn,
    QuarterHourEast,
    QuarterHourWest,
    Sources,
    ThePmicClock,
    /// When the clock is read.
    Read,
    AtStartThenHourly,
    NetworkZone,
    NotHeardYet,
    NetworkTime,
    GpsTime,
    NotUsedYet,
    // The page's notes after a choice.
    ServiceDidNotAnswer,
    FleetNotAnsweredYet,
    NotAZone,
    KernelDidNotTake,
    NoZonesYet,

    // ---- the language page ----
    /// Under "follow my identity", unchecked.
    LangOwn,
    /// Under it, checked, before the identity has a language.
    LangIdentityNone,
    /// The units page's "follow my identity" (the number base, as the language), unchecked.
    BaseOwn,
    /// Under it, checked, before the identity has a number base.
    BaseIdentityNone,
    /// Under the list: the languages the face cannot draw yet (`Lang::renders`).
    MoreLanguagesNeedFace,
    NotALanguage,

    // ---- About ----
    /// "version Zil, development build": Zil (the dozenal version name) stays.
    VersionDev,
    /// "version Zil".
    Version,
    DeveloperAlready,
    NotRead,
    Battery,
    /// The time from the power key's press to the attest screen.
    PressToReady,
    SincePowerOn,
    Processes,
    InTheVault,
    /// The chip's temperature.
    Chip,
    /// The bootloader's lock state (ferros_sys::BootLock; FLASHER.md step 7; owner, 2026-11-13: shown, never a nag). One sentence the page wraps: unlocked, what it exposes and what it does not.
    BootloaderUnlocked,
    /// One sentence the page wraps: unlocked while attest at boot is on, so the kept seed (the device class) is exposed too.
    BootloaderUnlockedAttest,
    /// Locked to the custom key, the one this ferros is signed with (verified boot yellow).
    BootloaderLockedFerros,
    /// Locked to the maker's key (verified boot green).
    BootloaderLockedMaker,
    /// No word from the bootloader this boot (a reload).
    BootloaderNotRead,

    // ---- Kists, Permissions, a kist's first launch ----
    NoKistsStowed,
    AsksForNothing,
    NewKistsStartOn,
    /// The first-launch question's line over the wants.
    AsksForOnce,
    /// The first-launch question's button.
    Start,
    /// A live note's page (docs/NOTIFY.md "The live takeover"): take it (open the kist where the note points), or say no (the note goes; its kist tells the caller).
    Answer,
    Decline,
    /// A live note's heading when its kist gave no words: something for the person, now.
    ForYouNow,
    /// What each right is, in ferros_kist::RIGHTS order: display, touch, surface, ira, domain, entropy, gps, udp, audio.
    RightDisplay,
    RightTouch,
    RightSurface,
    RightIra,
    RightDomain,
    RightEntropy,
    RightGps,
    RightUdp,
    RightAudio,
    /// The cameras (KISTS.md, the asked rights: frames from the camera driver; built 2026-11-12 for Lumis).
    RightCamera,
    /// Be you (docs/FACETS.md "Be you"; decided as the name, D9): a binding of the kist's alias to the person's identity, so their contacts find and recognise them in it.
    RightBeYou,
    /// The person's people (FACETS.md "People"), all or the ones they choose.
    RightPeople,
    /// Bluetooth (KISTS.md, the asked rights; BT.md "The client ops", 2026-11-24): scans, connections and channels to devices near the person.
    RightBluetooth,
    // ---- People (docs/FACETS.md "People"): the system's page, its add-a-person sheet, a kist's card's chooser ----
    /// The People page's title, a settings page.
    PagePeople,
    /// The People page's button: the system's add-a-person sheet (the keyboard asks for their handle, then a name).
    AddPerson,
    /// The People page with nobody on it.
    NoPeopleYet,
    /// The keyboard's prompt for the person being added: their handle, typed once (the attest screen's sentence case).
    TypeTheirHandle,
    /// The keyboard's prompt after it: the name the person gives them (a petname, theirs alone).
    WhatDoYouCallThem,
    /// Adding: their chain asked of fgtw.org.
    LookingThemUp,
    /// Adding: done, they are in the person's people.
    PersonAdded,
    /// Adding: no chain at that handle.
    NobodyHasHandle,
    /// Adding: fgtw.org did not answer.
    CouldNotLookUp,
    /// Adding: the chain at that slot is not the one the handle derives (its genesis key is not theirs).
    RecordNotTheirs,
    /// Adding: the handle typed was the person's own.
    ThatIsYou,
    /// A kist's people: all of them.
    PeopleAll,
    /// A kist's people: only those the person picks.
    PeopleChosen,
    /// Under the add button: the handle typed is used once to find them and never kept (identity-profile.md "type it once, derive, pin, drop").
    HandleNotKept,
    /// A person the person gave no name.
    NoName,
    /// Why a kist the person tapped did not start (ferros_kist::launch::why), under `Say::CouldNotStart` on its open circle: no room in the memory pool.
    NotEnoughMemory,
    /// Not stowed on this device: no capsule here, or no seal.
    NotStowedHere,
    /// Its checks refused it: its signature, the gate, a right it does not declare.
    FailedItsChecks,
    /// Its capsule could not be read from the vault.
    CouldNotBeRead,
    /// Anything else.
    SomethingWentWrong,
    /// Why a kist that started then stopped, under `Say::Stopped`: a fault or a panic.
    ItCrashed,
    /// Ended from outside (the bridge's `diag kill`), not by the person's own relaunch.
    ClosedFromOutside,
    /// Stowing a kist (owner, 2026-11-11: "the proper term for installing a kist is stow"; docs/KISTS.md "Stowing"): the Kists page's heading over a kist offered over USB, checked and waiting on the person.
    OfferedOverUsb,
    /// Its button that puts it in the person's list (a verb: put away, store aboard).
    Stow,
    /// Its end: in the person's list.
    Stowed,
    /// Its end refused (the reason after it).
    NotStowed,
    /// An offer of native, unchecked machine code, which only a development build stows (SANDBOX.md), said plainly on the page.
    UncheckedDevOnly,
    /// Over an offer's wants: each is asked once, at its first start.
    AsksAtFirstStart,
    /// The stower's stages on the Deck page (payloads/kists `STAGE_*`; until 2026-11-12 English alone): a job queued behind another.
    WaitingItsTurn,
    /// Its signature, every egg against its developer's bundle.
    CheckingSignature,
    /// The kernel reading it back and running the checker over its code.
    CheckingCode,
    /// Its seal, keyed to this device.
    SealingToDevice,
    /// Into the person's list.
    StoringIt,
    /// An unstow's end (a stow's is `Stowed`).
    Unstowed,
    /// An unstow refused (the reason after it).
    NotUnstowed,
    /// A restore's end: a capsule the person's list names, brought back to this device.
    Restored,
    /// A restore refused (the reason after it).
    NotRestored,

    // ---- Deck: what is on deck, and each thing's card (owner, 2026-11-12) ----
    /// A row's kind: a kist (opened, an app; a ferros word, kept unless a language has a reason).
    KindKist,
    /// A row's kind: a puck (a live circle never opened, docs/PUCKS.md; a ferros word, never "widget").
    KindPuck,
    /// The heading over the pandok's kists on the Deck (the pandok: the place with the things, the kist catalog; a ferros name, kept).
    Pandok,
    /// The Deck with nothing on deck but settings.
    NothingStowed,
    /// A card's fact: who made it (the developer's name, else their key's first bytes).
    MadeBy,
    /// A card's fact: its version (`Version` is About's line).
    FactVersion,
    /// A card's fact: its capsule's size.
    Size,
    /// A card's fact: its identity, the developer's key and its option, shortened.
    Identity,
    /// A kist's state on its row and card: a process of it alive.
    Running,
    /// Its last process ended by a fault, a panic or a kill from outside, or its last launch did not start (the reason under it).
    Stopped,
    /// Stowed and neither: not launched since boot, or ended by its own exit or the person's stop.
    NotRunning,
    /// A card's fact: where it keeps its data.
    Keeps,
    /// Its value: the kist's own vault domain.
    OwnDomain,
    /// A card's button: a running kist ended, one shot dead.
    Stop,
    /// A card's and the pandok's button: a newer version from the pandok.
    Update,
    /// A kist's card: out of the person's list (two taps), the opposite of stow.
    Unstow,
    /// Under a puck's armed remove: its answers stay in the person's list.
    AnswersKept,
    // The unstow sheet, in a kist's card's place (docs/KISTS.md "The unstow sheet"): a tick keeps a kind of thing, unticked it is erased at once.
    /// Its heading over the ticks.
    UnstowWhatToKeep,
    /// A tick: the kist's own vault domain (caches, downloaded areas, its settings).
    KeptForItself,
    /// Under it while its size is not known.
    UselessWithoutIt,
    /// In its size's place when it kept nothing.
    NothingYet,
    /// A tick: the kist's items in the person's hold (docs/STORE.md).
    ThingsMadeForYou,
    /// Under it.
    ThingsMadeYours,
    /// A tick: the person's answers to its rights.
    YourAnswers,
    /// Under it, ticked.
    AnswersAskNothing,
    /// Under it, unticked.
    AnswersAskAgain,
    /// Under it, fixed, when it goes from this device only.
    AnswersOtherDevices,
    /// Its notes, always cleared.
    NotesGoWithIt,
    /// The heading over where it goes from.
    UnstowFrom,
    AllYourDevices,
    ThisDeviceOnly,
    /// Over the sheet's Unstow: nothing unticked comes back.
    GoneMeansGone,
    /// An unstow's stage while its own data is erased.
    ClearingWhatItKept,
    /// A puck's card: off the deck (two taps; an unstow underneath).
    RemoveFromDeck,
    TapAgainRemove,
    /// A puck's card: the fact naming what its circle shows.
    Shows,
    /// The clock puck's: the time.
    TheTime,

    // ---- Developer ----
    TemporalDither,
    LightScope,
    /// The light scope's channels (IR stays; "off" is `Off`); red, green and blue name the panel's channels too.
    ScopeRed,
    ScopeVisible,
    ScopeBlue,
    ScopeGreen,
    ScopeClear,
    /// The display's hit map shown in every fourth frame (development builds).
    HitMap,
    /// The heading over the panel's red, green and blue checkboxes (development builds).
    PanelChannels,
    /// The display's release look (no orange) for the rest of this boot (development builds).
    ReleaseLook,
    /// Under the panel's channels and the release look: nothing there is kept past a reboot.
    UntilReboot,
    /// The display's frame counter (frames a second and a frame's time, in the console's USB circle), a checkbox on the Developer page.
    FrameCounter,
    // The Developer page's tests (payloads/map devtest.rs): the speaker test's heading, its warning, its buttons' note and labels, what the audio service answered; the microphone test's heading, its listen checkbox, its record button and outcomes; the buzz test's heading, its button, its failure.
    SpeakerTest,
    NoSpeakerProtection,
    HoldToPlay,
    Left,
    Right,
    Both,
    Playing,
    ToneStopped,
    AudioRefused,
    AudioNotAnswering,
    AmpFault,
    NoAudioRight,
    MicTest,
    Listen,
    RecordPlayBack,
    Recording,
    MicGaveSilence,
    MicsWouldNotStart,
    SpeakerWouldNotStart,
    BuzzTest,
    Buzz,
    MotorDidNotPlay,

    // ---- the attest screen (payloads/ahau): its button ----
    /// The button that attests the typed handle (owner, 2026-11-10: "needs to say attest, not unlock. always attest").
    Attest,
    Mint,
    StartOver,
    Cancel,
    // Attest to approve (owner, 2026-11-24; docs/AUTH.md "Approve on the attest screen"): the button that attests and applies (the only one: no Decline), and the button that closes the screen after a removal it could not start.
    Approve,
    Done,
    /// The attest screen's state line above the handle box (owner, 2026-11-24: "1 device, 1 identity"), never whose: the device not bound yet, and bound. An approve keeps it and puts what is approved under it.
    AttestUnbound,
    AttestBound,
    /// Any handle that does not open a bound device, the device's own mistyped or another identity's alike (the kernel cannot tell them apart, and says nothing of whether one exists).
    DeniedNotThisIdentity,
    /// The Fleet page's button that unbinds this device (owner, 2026-11-24: "I need to unbind first, then she needs to bind it to her fleet"; docs/AUTH.md "Remove this device"), an approve on the attest screen.
    UnbindPill,
    /// What is approved, one message a request (ahau_core::approve), never words of the requester's. Each starts "Approve:"; two lines where the second says what it means.
    ApproveAttestAtBootOn,
    ApproveAttestAtBootOff,
    ApproveAndroidOn,
    ApproveRemoveDevice,
    ApproveGuardOffChargeCap,
    ApproveGuardOffStartInto,
    ApproveGuardOffDeveloper,
    /// One line.
    ApproveStartIntoFerros,
    ApproveStartIntoAndroid,
    ApproveDeveloper,
    /// A guardable setting's checkbox beside it (owner, 2026-11-24: "a toggle that requires attest"): on, changing the setting is an approve.
    RequireAttest,
    /// Two lines under the guards: what the checkbox does, and that turning it off is an approve too.
    RequireAttestNote,
    /// The Fleet page's button that opens the add flow (owner, 2026-11-24: "the fleet page does nothing unique until you press 'bind new device' or similar, then it does the scan/tap enable, all that"): an approve on the attest screen first.
    BindDevice,
    /// What that approve approves: two lines.
    ApproveFleetAdd,
    /// The button while a network is being joined.
    JoiningButton,
    Join,
    // Its prompts (owner, 2026-10-31: simple and direct; sentences, capitalised).
    WaitingConnectivity,
    FindingIdentity,
    /// Three lines.
    MintPrompt,
    NoAnswerYet,
    /// Two lines.
    RecordMismatch,
    Minting,
    MintingFailed,
    // The join (owner, 2026-11-22: nothing reaches the person's devices until they confirm on the new one; then NFC first, the touch to add second, the words only on asking).
    /// Two lines: the identity has a fleet; nothing is sent before the confirmation.
    JoinConfirmPrompt,
    /// The confirmation's button, in the owner's words: a sentence the attest screen wraps.
    JoinConfirmYes,
    /// Two lines: how to finish the join, the two devices held together (NFC) with "bind device" pressed and approved on the other's Fleet page (owner, 2026-11-24: its page is passive until then), or this device touched on that page and its green button (BLE).
    JoinTogether,
    /// The button behind which the pairing words wait (never shown by default: shoulder surfing), a sentence the attest screen wraps.
    JoinShowWords,
    /// A tag held to the attest screen that does not open the device: one refusal, no detail (AUTH.md "Attest with a tag").
    TagDenied,
    /// A handle that is not the one it must be (the removal's, after its approve): nothing changed.
    TheyDontMatch,
    ComputingKeys,
    IdentityServiceNotAnswering,
    // Its network page.
    NotSeenLastScan,
    Wpa2Personal,
    OpenNotJoined,
    Wpa3NotJoined,
    KeptOnDevice,
    /// Beside the network the device keeps, when it is not joined (short: it sits at a row's right).
    KeptTag,
    WifiStarting,
    LookingForNetworks,
    NoNetworksFound,
    NoNetworkMatches,
    TypeToNarrow,
    WifiNotAnswering,
    AlreadyOnNetwork,
    CouldNotKeepNetwork,
    // ---- the WiFi page's picker (payloads/map wifi.rs, 2026-11-18): its headings, its buttons, a network's facts and what the page says ----
    NetworksInRange,
    NetworksRemembered,
    /// The button that asks the keyboard for a network's passphrase.
    TypePassphrase,
    /// The same for a network remembered: its passphrase typed again.
    NewPassphrase,
    /// A remembered network's button, and its first tap (two taps forget it).
    Forget,
    TapAgainForget,
    /// A network's facts: how many access points answered under its name, its security, when this device last joined it.
    AccessPoints,
    Security,
    LastJoined,
    NotJoinedYet,
    /// Whose list keeps a remembered network: the person's (joined while they were attested), the device's (joined while nobody was).
    KeptForYou,
    KeptForDevice,
    /// A network's security, short (WPA2, WPA3, WEP stay).
    SecurityOpen,
    SecurityOther,
    JoinsWpa2Only,
    /// Under the remembered networks: kept on this device only (whether they should travel with the person is the owner's question).
    NetworksKeptNote,
    /// A WiFi driver (or a grant) from before the picker: no list.
    NoNetworkList,
    NotRemembered,
    CouldNotForget,

    // ---- the honeycomb keyboard's bar (attest screen, hive): short, they sit on keys ----
    KeyMap,
    KeyShift,
    KeySpace,
    KeyDel,
    KeyEnter,
    /// The key that turns to the next page of symbols.
    KeyPage,
    /// The attest screen's left key: clears what is typed.
    KeyClear,
    /// The system keyboard's left key (over an app): puts the keyboard away.
    KeyHide,
    /// The hive's own line over its keyboard (payloads/hive).
    HiveHint,

    // ---- Kūaha (its own repository, ../kuaha; docs/KISTS.md "Kūaha"): the kist that shows what is on this device; its two views, the find's conditions, an item's card, the device's totals ----
    /// Its first view: the person's items in the hold (STORE.md's "your things").
    KuahaThings,
    /// Its second view: what this device holds, the hold's totals, the RAW clips area, each kist's own storage.
    KuahaDevice,
    /// The search line's words before anything is typed.
    KuahaSearch,
    /// The kinds' first circle: no kind chosen.
    KuahaEverything,
    /// The time condition's choices: none, then the last day, week, month and year back from now.
    KuahaAnyTime,
    KuahaPastDay,
    KuahaPastWeek,
    KuahaPastMonth,
    KuahaPastYear,
    /// The orders (ferros_items ORDER_*).
    KuahaNewest,
    KuahaOldest,
    KuahaByTitle,
    KuahaLargest,
    /// The maker condition's first choice: any kist.
    KuahaAnyMaker,
    /// The hold with nothing in it. Three lines.
    KuahaEmpty,
    /// A find with no answers.
    KuahaNothingMatches,
    /// The row under a page of answers that asks for the next.
    KuahaMore,
    /// The `items` right answered no (an empty slot). Two lines.
    KuahaNoRight,
    /// Every hold call refused with the right held: a kernel or vault from before the hold. Two lines.
    KuahaHoldSilent,
    /// An item's card: when it was made; its kind.
    KuahaMade,
    KuahaKind,
    /// An item's remove, its first tap (the second is `TapAgainRemove`).
    KuahaRemove,
    /// A remove the hold refused.
    KuahaNotRemoved,
    /// Under an item's open, greyed: the hand-off to the kist that opens it is not built (STORE.md step 1).
    KuahaHandOffLater,
    /// An item of a kind no kist is known to open.
    KuahaNoOpener,
    /// The device's view: its headings and facts.
    KuahaInTheHold,
    KuahaClipsArea,
    KuahaUsed,
    KuahaFree,
    KuahaLargestStretch,
    KuahaKistsKeep,
    /// A total the device does not count yet (the usage query not on this ferros).
    KuahaNotCounted,
    /// A clip still being recorded.
    KuahaRecording,
    KuahaTotal,
    /// The project's kinds of items (ferros_items `kinds`), each its idea's phrasing until the ideas' sheet carries them (IDEAS.md); RAW, GPS and Sega Genesis are names.
    ItemPhoto,
    ItemRawVideo,
    ItemVideo,
    ItemDocument,
    ItemScan,
    ItemList,
    ItemContact,
    ItemSong,
    ItemGpsTrack,
    ItemFind,
    ItemCollection,
    ItemGenesisSave,
    ItemGenesisRam,
    /// A kind this build has no words for.
    ItemOther,
}

/// A message with holes: names (a kist's, a device's, a network's: never translated), words typed (kept as typed), and numbers (in the person's base: a count is written by `num`, a value the caller formats arrives as a `&str` already written in it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Say<'a> {
    // ---- a link's page ----
    /// Where a satellite stands and how strong it is: its elevation and azimuth (degrees) and carrier to noise (dB-Hz), each written already in the person's base; `used` whether the fix uses it.
    SatPlace { up: &'a str, round: &'a str, cn0: &'a str, used: bool },
    /// The GPS page before a fix (2026-11-11): what the chip has sent so far, its frames and how many kinds of them (payloads/gps op 4), and that satellites come with its first fix.
    ChipHeard { messages: u64, kinds: u64 },
    // ---- the clock page ----
    /// The legal offset (written already: "UTC + 1 h") while daylight time is in it.
    DaylightTime(&'a str),
    /// The network's mobile country code heard (written already in the person's base).
    HeardMcc(&'a str),
    /// The same, with the country it names (ISO 3166 alpha-2, stays).
    HeardMccIn { mcc: &'a str, country: &'a str },
    // ---- About ----
    /// Taps left on the version line until Developer is a page.
    TapsToDeveloper(u64),
    /// The battery's level (written already) while it charges.
    Charging(&'a str),
    /// The battery's level (written already) while the device runs on it.
    OnBattery(&'a str),
    /// The charge cap's value: the voltage (written already, with its unit) and the rough share of a full charge it holds (written already).
    ChargeCapAt { mv: &'a str, share: &'a str },
    /// Why the default is the default: the default voltage (written already, with its unit).
    ChargeCapDefault(&'a str),
    /// The attest screen's approve of a guarded charge cap: the cap, written already in the person's base (mV).
    ApproveChargeCap(&'a str),
    /// Above the default: the share of the default's charge cycles (written already) and the default voltage (written already, with its unit).
    ChargeCapCycles { share: &'a str, mv: &'a str },
    /// The chip's temperature (written already, with its unit) while its clock is held down for heat.
    Throttled(&'a str),
    // ---- Kists, Permissions ----
    /// A right (or a kist's name) whose question was never asked: "the screen (not asked yet)".
    NotAskedYet(&'a str),
    /// A kist using a right now (it holds a claim on it, ferros_sys::claim): "Mahere (using now)".
    UsingNow(&'a str),
    /// A kist the person tapped that did not start (its name): "Mahere could not start", the reason under it (`Msg::NotEnoughMemory` and its neighbours).
    CouldNotStart(&'a str),
    /// A kist that started and then stopped abnormally (its name): "Mahere stopped", the reason under it (`Msg::ItCrashed`, `Msg::ClosedFromOutside`).
    Stopped(&'a str),
    /// Who made a kist offered to stow (its developer's name as the capsule gives it): "by Nick".
    ByDeveloper(&'a str),
    /// The unstow sheet: what a kist kept for itself, its size written already in the person's base: "12 MiB, of no use without it".
    KeptForItself(&'a str),
    /// The stower fetching a kist (its name), the bytes so far written already in the person's base: "fetching Mahere: 3 MiB".
    Fetching { name: &'a str, got: &'a str },
    /// The same with the whole size known: "fetching Mahere: 3 MiB of 12 MiB".
    FetchingOf { name: &'a str, got: &'a str, total: &'a str },
    // ---- the attest screen ----
    /// A network's signal, its dBm written already in the person's base.
    Signal(&'a str),
    JoiningNet(&'a str),
    PassphraseFor(&'a str),
    OutOfReach(&'a str),
    SecurityUnsupported(&'a str),
    DidNotAnswer(&'a str),
    RefusedPassphrase(&'a str),
    // ---- the WiFi page's picker ----
    /// The keyboard asked for this network's passphrase.
    TypeOnKeyboard(&'a str),
    JoinedNet(&'a str),
    Forgotten(&'a str),
    /// A passphrase's length, in characters.
    PassphraseLength { min: u64, max: u64 },
    // ---- Kūaha ----
    /// An item's open, naming the kist that opens its kind (a name, never translated): "open in Opsin".
    OpenIn(&'a str),
    // ---- the Developer page's tests ----
    /// A level as decibels under full scale (a count, in the person's base).
    UnderFullScale(u64),
    /// An amplifier's state (its side's word, `Msg::Left` or `Msg::Right`): PUP_DONE and PLL lock as 0 or 1, the error bits written already (developer G#): "left: PUP_DONE 1, PLL lock 1, errors G#0". PUP_DONE and PLL are register names and stay.
    AmpState { side: &'a str, pup: u64, pll: u64, errors: &'a str },
    /// A microphone's level now (its id, the decibels under full scale): "microphone 0: 42 dB under full scale".
    MicLevel { mic: u64, under: u64 },
    /// A microphone whose samples were all zero.
    MicQuiet(u64),
    /// A microphone by its id (husky's places for them are unknown): "microphone 0".
    MicName(u64),
    /// A recording made and being played back, its peak as decibels under full scale.
    PlayingBack(u64),
    /// The vibration motor's ROM waveform by number.
    RomWaveform(u64),
}

/// A message in language `l`.
pub fn text(l: Lang, m: Msg) -> &'static str {
    match l {
        Lang::En => en::text(m),
        Lang::Es => es::text(m),
        Lang::Mi => mi::text(m),
        Lang::Pt => pt::text(m),
        Lang::Fr => fr::text(m),
        Lang::Id => id::text(m),
        Lang::De => de::text(m),
        Lang::Sw => sw::text(m),
        Lang::Vi => vi::text(m),
        Lang::Tr => tr::text(m),
        Lang::Ru => ru::text(m),
        Lang::Hi => hi::text(m),
        Lang::Uk => uk::text(m),
        Lang::It => it::text(m),
        Lang::Pl => pl::text(m),
        Lang::Fil => fil::text(m),
    }
}

/// A message with holes in language `l`, written into `o`, its counts in `f` (the person's base: ferros_sys::numbers).
pub fn say(l: Lang, m: Say, f: tatau::Fmt, o: &mut dyn Write) -> fmt::Result {
    match l {
        Lang::En => en::say(m, f, o),
        Lang::Es => es::say(m, f, o),
        Lang::Mi => mi::say(m, f, o),
        Lang::Pt => pt::say(m, f, o),
        Lang::Fr => fr::say(m, f, o),
        Lang::Id => id::say(m, f, o),
        Lang::De => de::say(m, f, o),
        Lang::Sw => sw::say(m, f, o),
        Lang::Vi => vi::say(m, f, o),
        Lang::Tr => tr::say(m, f, o),
        Lang::Ru => ru::say(m, f, o),
        Lang::Hi => hi::say(m, f, o),
        Lang::Uk => uk::say(m, f, o),
        Lang::It => it::say(m, f, o),
        Lang::Pl => pl::say(m, f, o),
        Lang::Fil => fil::say(m, f, o),
    }
}

/// A count in the person's base, for a language's `say` (zero, which DMS has no doublings for, as 0).
fn num(o: &mut dyn Write, f: tatau::Fmt, n: u64) -> fmt::Result {
    let mut o = o;
    if let tatau::Out::Nothing = tatau::count(&mut o, f, n, 1)? {
        o.write_str("0")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
