# The ferros brand

The owner, 2026-11-25, designing the installer's welcome: "We gotta figure out branding and likeness anyway. we're advertising to ourselves, nobody is buying the data, there is nobody to call but yourself if shit goes awry, it's your utility (wateo) and your operating system."
This file is that branding and likeness: the thesis, the voice, the look, and the lines every first-contact surface says in the same words.
It binds the installer (tools/flash-gui, docs/FLASHER.md), ferros.org (web/), wateo.net and Photon's first run.

## The thesis: money is control, this is consent

The owner, on money: "Control people. that's it. that's the entire purpose. all money is human control over time. I wish it was trust."
The brand is the absence of a seller.
Nobody is advertising to anybody: the person who installs ferros is the person it is for, and there is no third party in the room.
Nobody buys the data.
When something breaks there is nobody to call but yourself, and that is said plainly, not hidden under a support promise.

Distilled to one word (the owner, 2026-11-25): **consent.**
"We probably will offer data reselling services at some point but if I could distill it to one word it's consent."
Nothing happens on the device unless the person says so, and that includes any future sale of the person's own data: theirs to decide, never decided for them.
The first consent is the install itself: the welcome's last line says what it does to the device and that the person is choosing it.

## The voice

- **The owner is the subject.** The words are one person's, said to one person. Never a company "we", never "our team", never "we're committed to".
- **No features list.** The welcome does not say what ferros can do. ferros.org's "What runs today" is a plain inventory, after the thesis, not a pitch.
- **No support promise.** "The only person to call is you" is the truth and the brand at once. The one offer is honest: send the log, and here is exactly what it holds and where it goes.
- **Honest.** The wipe line is on the first screen. A build that stops short says so ("This build goes no further yet: nothing is written to the device."). Nothing is softened into "set up" or "migrate".
- **A little quirky.** "oh and a fast cable to transfer the goods." The tone the owner asked for: "Exciting, quirky, unique, this is not a company, this is you owning your device and we removed as much google as we humanly could giving you the most you that we can give you."
- **"Device", never "phone"** ("Claude memories/feedback_device_not_phone.md"): ferros places no calls.
- Short spellings where the house uses them (thru, altho); no em dashes in wire text or UI words.

## The likeness

- **The mark** is logo.vsf: the round iron phase map, analytic ring segments and pie sectors (tools/logo-vsf reads them back from the Illustrator curves; nothing is a transcribed Bézier). It is drawn from the geometry at whatever size is needed, never scaled from pixels, and the installer takes it from the signed release, not from a copy baked into its executable.
- **Colour is VSF RGB** (monochromatic primaries, Illuminant E, gamma 2 encoded, g2t for headroom), never sRGB, OKLab or Rec.709 ("Claude memories/feedback_vsf_rgb_only.md"). Every surface is authored in it; a foreign space appears only where a platform forces it, at the output edge (ferros.org's AV1 pictures are made by tools/vsf2web from VSF captures).
- **Darkness space, front to back.** Pixels are α + darkness, composed with fluor's `under` alone: the frontmost thing first, the canvas last, never Porter-Duff over ("Claude memories/feedback_darkness_space.md"). A glow is a layer of light at a falling α, never colourless opacity.
- **Near black, one warm glow.** The canvas is near black (g2t codes 9, 9, 10). One warm light (codes 232, 150, 64) breathes low behind the headline, a C∞ falloff (1 / (1 + d²)) whose phase follows the shown frames, never a clock. Nothing else glows except a button that is ready.
- **Squircles.** The window, the buttons and the device silhouette take fluor's squircle: the window's exponent for the silhouette, the house pill for buttons.
- **Continuity.** Every curve and every animation is at least C¹ in the parameters that drive it (AGENT.md "GUI continuity"): no clamps, no steps, no piecewise layouts. Sizes come from the window's span and width, never from a pixel count.
- **Numbers** in the house format: G# hex, A# decimal, never 0x; counting from zero.

## The shared lines

These are used verbatim wherever they appear: the installer's screens, ferros.org, wateo.net, Photon's first run.

The welcome (screen 0; the owner, 2026-11-25):

    Consent.

    Nothing happens on this device unless you say so.
    Nobody's buying your data. If you ever choose to sell it, that's yours to decide too.
    And when something breaks, the only person to call is you.

Under the device silhouette, the first consent:

    This replaces Android and erases the device. You're choosing that.

The button that follows a seen device: **Install ferros.** The later irreversible step's button: **Erase and install.**

Screen 1 (the owner's words, light punctuation only):

    Welcome!
    This completely removes Android and as much google as we can and replaces it with an operating system that respects you. You'll need a Pixel 8a or a Pixel 8 Pro as that's all there is right now, oh and a fast cable to transfer the goods.

The steps, one line each: plug in, turn on unlocking, unlock on the device (this erases it), install, lock it back, done.

The silhouette's words, by state: `?` (nothing seen), the model name with "in Android" or "in its bootloader" under it, `no access` with "driver needed" (Windows) or "no permission to open it" (Linux and macOS), `ferros` with "already running ferros".

The log offer, on every screen: a small "send the log" in a corner; what it sends and where, said in full, and nothing sent without a yes.

## Where it lives

- The installer: tools/flash-gui (the screens), tools/flash (the engine), docs/FLASHER.md (the lifecycle).
- ferros.org: web/index.html; the Install section carries the same thesis lines.
- Photon's first run and wateo.net reuse the lines above as they are written here, and take their look from the same rules.
- The owner's words behind this file: "Claude memories/brand_thesis.md".
