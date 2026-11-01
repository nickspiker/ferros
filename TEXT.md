# Text entry and selection: one model for ferros and Photon

Decided with the owner, 2026-11-01. The ocean is open on ferros, so this is designed once for every place Photon runs: Photon on ferros has to select text the same way it does wherever else it runs on glass, and nothing here may break how Photon selects today on desktop and Android.

## Layers

0. **Contacts.** The touch driver hands out raw contacts: an id, x, y and time, as down, move and up. No gesture is decided at this level. This is what the attest screen polls today (the touch endpoint, op 1) and what winit hands Photon on Android.
1. **One text model: fluor's `Textbox`.** Characters, their widths, the caret, the horizontal scroll, the selection (`selection_anchor`), the blinkey. Photon already uses it on desktop and Android. On ferros an app turns contacts into the same calls Photon makes from winit (`pointer_press`, the drag pan, `select_word_at`, `delete_selection` …), so selection, scrolling and the caret chase live in one place and cannot fork.
2. **Screens use `Textbox`, not their own.** The attest screen's entry line becomes a `Textbox` with selection off. Until then its bespoke line follows this document (see "The attest line" below).

## No timing

ferros has no time-windowed gestures (owner, 2026-10-22). Photon's double-tap (a word) and triple-tap (everything) are time-windowed, so on glass under ferros every gesture here is decided by how many fingers, where they land and how far they move, never by how fast or how soon.

## Gestures on glass

- **One finger, tap** (lifts within the slop, a 56th of the screen's width): the caret to the nearest character boundary, the blinkey solid again.
- **One finger, drag starting on the text:** pans the text, the grabbed character staying under the finger. It stops at the text's ends while the finger moves; nothing moves after the finger lifts (no spring: ferros has no animations).
- **One finger, drag starting on the blinkey:** selects, from where the caret stood to the boundary under the finger. The one-handed way to start a selection.
- **Two fingers landing on the text:** a selection between the boundaries under them, one finger per end. Then each end moves by how far its finger travels (relative), not to where the finger is, so the ends can come closer than two fingertips can (about a centimetre, four characters of the attest line's text).
- **After either start, handles stay.** The selection stands after the fingers lift, and one finger landing near either end drags that end (relative again). So starting a two-finger selection needs two hands, but refining it needs only a thumb.
- **While fingers cover the ends,** a static echo of the selected span shows above the line: it is under the fingertips otherwise. Not animated.
- **A tap anywhere else on the text** drops the selection and places the caret. Delete removes the selection; typing replaces it.
- **Two fingers on the keyboard still scroll its extent.** The split is by where the fingers land (on the text: select; on the keys: scroll), and every screen keeps it the same.

On Android, whether Photon keeps double and triple tap or moves to these gestures is the owner's call. With a mouse (desktop) the pointer model stays: drag selects, double-click selects a word; a mouse has no fingertip covering what it selects.

## The attest line

- No selection: a handle is typed fresh at each boot and never copied out (the attest screen has no clipboard, and a handle is secret); the `clear` key empties the line.
- **Window:** the underline's span, the middle three quarters of the screen, not the screen's edges.
- **Edges fade, never cut:** a fixed C∞ ramp over the last half em at each end of the window (a smooth step built from exp(−1/t), exactly 0 and 1 at its ends), in linear light, applied to the text whether or not it overflows, so nothing jumps when the handle grows one character past the edge. Text that fits is measured inside the fade, so it is never dimmed; the fade only shows where text continues past the window.
- **Scroll:** centred while it fits; past that, the caret chase (fluor `update_scroll`): the caret is kept inside the window, landing exactly on the edge it crossed, so typing at the end keeps the newest characters in view.
- **Pan:** a one-finger drag starting on the line, the grabbed character under the finger, stopping at the text's ends while the finger moves (a bound on the scroll, like fluor's `clamp_scroll_to_bounds`: the owner accepts or replaces it before it ships).
- **Built** in the back surface as the line is now: everything outside the window zeroed and the fade multiplied into the edge columns before the blinkey is drawn and the line copied in. No new display op.

## Open

0. **Where text is measured and drawn on ferros.** fluor's `Textbox` measures with its own `TextRenderer`; ferros apps have no typeface of their own, and the display server draws (op 26), measures (op 28) and centres (op 29) text in the one typeface (SHELL.md, WINDOWS.md). Either `Textbox` takes its widths and glyphs from the display's ops on ferros, or apps carry the typeface. One typeface, one set of widths: a caret placed by one engine and drawn by another drifts (the attest blinkey did, 2026-11-01).
1. **Contacts to pointer calls:** a small adapter on ferros (contacts in, `Textbox` calls out) shared by Photon on ferros and the attest screen, so the finger rules above are written once.
2. **The pan's stop at the ends:** a hard stop is a bound with a velocity step. A C∞ alternative (the scroll eased into its bound) cannot also land the caret exactly on the window's edge at the text's end; the owner picks.

## Order of work

0. This document.
1. The attest line's window, fade, caret chase and pan, on its current bespoke line (the drag needs a `Drag` contact event from honeycomb's `Touches`, which today reports a moving off-key finger as nothing).
2. The contacts-to-pointer adapter, and `Textbox` on ferros with the measuring question (Open 0) settled.
3. The glass selection gestures in `Textbox` as a mode, Photon on ferros first; the attest line moves onto `Textbox` with selection off.
