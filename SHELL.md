# SHELL — what the owner sees when ferros wakes

Decided with the owner 2026-10-16: the basics on one line, people as the home, the app list secondary. The mock is `tools/shell-mock` (fluor, headless, the panel's real geometry; `cargo run --release -- <out dir>` writes PPM frames). The display driver that will paint it is DISPLAY-DESIGN.md.

## The principle

Design the wake, not the boot. A reboot happens once a week; a wake a hundred times a day, and on ferros a wake is deliberate (power pressed, proximity far). In that moment a person wants to know the time, who wants them, or the phone's state, or wants to answer someone or capture something. So **the screen shows state, not a menu.** Boot lands wherever wake lands.

## The screens

1. **Home.** One status line (time; link, battery, slot: plain facts, never a "secure" badge). Then the people: Photon's conversations, most recent first, unread marked. Tap a person, you're in the conversation. Capture is one gesture away (a hardware chord), never through a list. "Apps" at the foot.
2. **Apps**, one step down: the vault's `sealed` list drawn as rows, name and purpose, the seal beside it. The only launcher is the OS's own record of what may run. No icons.
3. **First boot**: no people yet, so the one thing a sovereign device needs first: pair with a device you trust.
4. **Resting** (the always-on face, 10 Hz): the time, the date, how many unread and from whom. Nothing else lit.

## Who draws what

The shell is a small process that draws lists. Photon serves its conversation rows over an endpoint; the vault serves the app rows; the display server paints. Photon stays an app; the shell stays a renderer of rows. fluor's span units make the same layout right on any panel.

## Open

- Whether a lit screen gates on anything before showing names (the threat model says glance is free; acting is what gets gated, if anything).
- The clock's form (eagle time, dozenal) and what the status line says about trust.
