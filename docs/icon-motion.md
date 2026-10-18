# Icon motion — the spec

How a dock icon moves on Golem, distilled from the icons Max directed by hand
(2026-10-15 → 10-18: the dock's own trio, then WhatsApp, YouTube, YouTube Music,
Discord, Spotify, Google, Chrome, Golem Files, Terminal). This is the brief for
animating the rest of the catalogue (~100 icons): read it, propose, build in the
lab, land on the dock. The lab is https://claude.ai/artifact/BP4E7xwMBtEWNgwqrN11Ta.

## 1. The icon itself

- **The real mark, in its real colours.** The brand's own glyph (simple-icons'
  path, or the official geometry when simple-icons is monochrome: Google's G,
  Chrome's wheel, YouTube's red tile). Never an approximation drawn by hand —
  every redrawn part was rejected ("the eyes you made are bigger and whiter").
  Papirus is the fallback only where no brand exists (the terminal became `</>`).
- **No palette tint on brand marks.** Brand colours stay as published. (Open:
  whether the Colours engine should ever touch them — Max has not said.)
- **One axis, one size.** Centre by the *measured* drawn bounds, not the file
  box; fit to 56 of 64 (80 of 100 on the dock's figure grid), the size every
  theme icon has. YouTube keeps its wide shape, centred vertically.
- **Rest is the frozen last frame.** Every move ends exactly on the resting
  icon; a resting icon may itself be redesigned as the end of a motion (the
  terminal's rest is the typed `</>` with its cursor).
- **Readable at 48 px.** Nothing thinner than a pixel at dock size; a closed eye
  keeps ≥ 20 % of its height (10 % read as "the eyes vanished"); a hairline at
  a mask's edge is a bug (overhang the mask where it does not nick the mark).

## 2. Parts and pivots

Every motion starts by taking the mark apart into its natural pieces and giving
each its own pivot.

| Part | What it is | Pivot |
|---|---|---|
| **plate** | the mark's own background shape: tile, disc, bubble, card | its centre |
| **mark(s)** | 1–3 glyph pieces: phone, play, ring, eyes, lines, bar, blades, folders, typed line | each its own centroid — or an edge (a lid, a hinge) — or a point OUTSIDE the plate (Spotify's lines orbit a point 42 units below the disc centre) |

- A glyph that is a **hole** in a solid (Discord's eyes, Chrome's lens) cannot
  move: redraw the solid whole and lay the glyph on it. (An overlay on a hole
  shows nothing — it cost a round.)
- A part that leaves the plate is **clipped at the plate's edge** (a real SVG
  `<clipPath>`, not CSS `clip-path:circle()` — that one measures in CSS px).
  Nothing floats over the dock behind the mark.
- Pieces of one glyph that are "the same" (three lines, two eyes, three
  folders) are **separate parts** so they can be desynchronised.
- The dock's tile behind the icon never moves (the Files/apps `BLANK` layer
  trick).

## 3. The two gestures

### Hover — a reaction

Two kinds, usually combined:

- **Held**: a state the icon takes while the pointer stays and leaves when it
  goes. Exponential approach, τ 80–110 ms. Used for: the plate's turn
  (15°, 20°, 45°), a lens opening (×1.6), folders sliding out (80 ms, "soft").
- **One-shot**: a single gesture of the mark that plays once per entry
  (0.7–1.5 s) and settles: shake, seek, swallow, orbit, wink, sweep, type.

Rules: it plays once (move off and back on to replay); it must not pause
between phases ("more continuous"); the plate's hover turn is *held*, the
mark's gesture is *one-shot*; a hover never grows the plate (a grow on hover
was swapped for a turn — grows are for clicks).

### Click — the event

The decisive version of the same metaphor: the shot, the send, the pause, the
blink. 0.7–1.1 s, never over 1.5. It:

- **starts from the hovered state** (Chrome's click starts on the open iris),
- carries the plate: a **beat** (squeeze to 0.8 in the first 8 %, back to 1 by
  60 %, timed so the plate is at rest exactly when the mark's event ends) or
  **spins** (1–4 full turns in 0.7–0.9 s whatever the count),
- goes the **opposite way** to the hover (hover turns anticlockwise → click
  spins clockwise; the plate one way → the mark the other, same duration, both
  landing together),
- ends at rest and **spends the hover**: with the pointer still on it the
  held state does *not* come back until the pointer has left (Chrome's
  `spent`).

### What Max rejected (do not propose)

Generic nudges, pops, bounces and beats with no metaphor ("your animations are
shit"); whole-part rotation where a hinge is meant (iris blades' outer ends
stay on the rim; only the inner ends move); a poker-card swap; folding flips,
even gentle and lit; a grow on hover; a pause between two phases; a blink too
quick to see; eyes "shining" (a lid over a hole); approximated marks; changed
colours; an endless loop of any kind.

## 4. Feel

- **Snappy, then settle.** Fast start, long tail: spins `cubic-bezier(.45,0,.2,1)`
  (or `.3,0,.15,1` for the fastest), moves `.4,0,.3,1`, slides `.22,.8,.28,1`
  ("soft"). A snap up `.15,.8,.3,1`, a heavy drop `.55,0,.85,.4`, a spring back
  with overshoot `.2,.7,.3,1.15`.
- **Overshoot and come back.** Past the rest point and return: 8° past on an
  orbit, a bounce of −14 % below rest after a jump, a play springing back at
  1.12 before 1.
- **Desynchronise siblings.** Same parts never move together: 65–220 ms apart
  (Spotify's lines 0/80/220 ms; the folder wave 65 ms, *back one first*), and
  one of them may go the other way (the middle line).
- **Counter-motion.** Parts move against each other: plate against phone, tile
  against play, compass plate against its needle (35 %, 80 ms behind).
- **Real rhythm, not a metronome.** Key presses 80–120 ms apart and uneven; a
  pause before noticing a typo; "faster", "more aggressive", "less rhythm,
  more real" were the recurring notes. When in doubt, shorter.
- **Tie things together.** A secondary motion follows the primary one directly
  rather than running on its own clock: the wink is a function of the tilt
  (shut at the low point, open as it comes up); the plate's turn is a function
  of the lens's opening; the disc's beat ends when the pause is gone.
- **Light** is for the dock's cast figures (gears, compass, bin: resvg
  filters, one fixed light from above, pre-drawn per look). Brand marks stay
  flat.

### Numbers that worked

| Thing | Value |
|---|---|
| plate held turn | 15° (WhatsApp) · 20° tilt-and-back (YouTube, Discord) · 45° (Chrome) |
| held approach τ | 80 ms (turn, folders) · 110 ms (lens) |
| shake | ±24° decaying over 7 swings (−24, 22, −18, 13, −8, 3), 700 ms |
| spins | 1 turn 900 ms · 2 turns 700–800 ms · 4 turns 800 ms |
| beat | 1 → 0.8 by 6–8 % → 1 by 34–60 % of the click |
| swell | ×1.1 (disc) · ×2.4–2.5 fading (a pause "swelling away") |
| lens | ×1.6 open · 0.22 shut in 180 ms |
| drop / lift | 5 / 10 of 64 |
| eyes | grow ×1.2 · shut to 0.2 (close 70 ms, hold 90, open 120) |
| stagger | 65 ms (wave) · 80 ms · 220 ms |
| typing | keys 85–120 ms apart; cursor solid while typing, blinks 320 ms cycles when waiting |
| one-shot hover | 0.9–1.5 s · click 0.7–1.1 s |

## 5. The vocabulary of moves

Named primitives, each proven on the dock. Compose two or three per icon.

| Move | What happens | Seen on |
|---|---|---|
| **TURN-HELD** | the plate turns θ while hovered, back on leave | WhatsApp 15°, Chrome −45° |
| **TILT-AND-BACK** | one pass: out to θ by 25 %, back by 82 % | YouTube −20°, Discord 20° |
| **SPIN(n, dir)** | n full turns, fast start, long settle; click goes against the hover | Google −4, Chrome ±2, WhatsApp 1 |
| **COUNTER-SPIN** | two parts, opposite ways, same duration, land together | WhatsApp plate/phone |
| **BEAT** | the plate squeezes to 0.8 and grows back in step with the mark's event | YouTube, YT Music |
| **SHAKE** | a decaying wobble about the part's own centre | WhatsApp's phone |
| **SEEK** | a bar fills along the plate's foot while the mark turns once | YouTube |
| **SWALLOW / SPIT** | one part grows and eats another, which returns from outside; the reverse pushes it off and spits it back | YT Music |
| **SWEEP** | a hand goes round like a clock, erasing the mark behind it and growing it back in ONE 720° motion | Google |
| **ORBIT** | parts swing about an axis outside the plate, out one side (clipped), in from the other, 8° past, settle | Spotify |
| **PRESS-JUMP** | parts drop and bunch, then jump up and spread while the plate swells | Spotify click |
| **WAVE** | staggered jumps through a stack, back first: snap up, drop, small bounce | Files click |
| **STACK-OUT** | parts slide out from behind the front one, each a step darker, front shrinks to fit | Files hover |
| **IRIS** | blades whose long sides pivot on a fixed outer end and stay tangent to a lens that opens (hover) or snaps shut (the shot) | Chrome |
| **WINK / BLINK** | an eye squashes to 0.2 of its height about its centre, tied to a tilt or after a landing | Discord |
| **GROW-DROP** | the face drops while its eyes grow, lands with normal eyes, then blinks | Discord click |
| **TYPE** | a cursor types the mark key by key, makes a typo, fixes it; the final line is the rest | Terminal |
| **SWING + COUNTER-PLATE** | a needle swings and settles over 2 s, the plate turning 35 % the other way 80 ms behind | Seam's compass |

## 6. Choosing a metaphor

The motion is what the app *is*, done to its own mark:

- a messenger rings → the handset shakes;
- a player plays → the play leans, a seek bar runs, a pause takes over;
- a record spins; sound waves orbit; a search is a clock hand loading;
- a browser is a camera lens: focus on hover, the shutter on click;
- a chat face winks at you and blinks when pressed;
- folders are paper: they fan out and ripple;
- a terminal types — and mistypes.

Ask: what would this thing do if you touched it? Hover = it notices you.
Click = it does its one thing. If no metaphor comes, the plate turn + a
counter-spin of the mark is the honest default — never a generic bounce.

## 7. Catalogue of the hand-made ones

| Icon (dock id) | Parts | Hover | Click |
|---|---|---|---|
| WhatsApp `webapp-whatsapp` | plate (bubble + ring), phone | plate +15° held; phone SHAKE | plate −360°, phone +360°, 900 ms |
| YouTube `webapp-youtube` | tile, play, pause, bar | SEEK 900 ms + play 360° + tile −20° TILT-AND-BACK | BEAT; play → pause swells ×2.4 away; play springs back; 1.1 s |
| YouTube Music `webapp-youtube-music` | disc, ring, play, pause | play SWALLOWs the ring, spins 360° at its biggest, gulps; ring back from outside; 1.2 s | BEAT; pause swells pushing the ring off; play returns and SPITs the ring; 1.4 s |
| Discord `webapp-discord` | tile, face (solid), two eyes | TILT 20° with the right eye WINKing (shut at the low point); 0.9 s | GROW-DROP (drop 5, eyes ×1.2), land, BLINK both; 1.1 s |
| Spotify `webapp-spotify` | disc, three lines | ORBIT ±55° about (32,74); top/bottom left, middle right; starts 0/80/220; 1.3 s each | PRESS-JUMP: drops 14/10/6, jumps −7/−4/−1.5, disc ×1.1; 0.9 s |
| Google `webapp-google` | disc, G (masked), blue bar | SWEEP: erase 0–360, regrow 360–720, 1.4 s | SPIN −4, 800 ms |
| Chrome `google-chrome` | lens (white + blue), three blades | IRIS ×1.6 held, plate −45° with it, SPIN −2 in 700 ms from entry | shut to 0.22 in 180 ms, SPIN +2 (800 ms) while it springs to rest; spent |
| Golem Files `golem-files` | three folders | STACK-OUT 80 ms (steps 5×9, 20 % darker each) | WAVE back first, lift 10, 240 ms each, 65 ms apart |
| Terminal (`foot`, **lab only**) | black card, typed `</>`, cursor | TYPE: `<` `.` `>` · erase `>` · `/` · erase `/` `.` · `/` `>`; 1.35 s; rest = the typed line | *(not yet directed)* |
| Seam `seam` | plate (rose + ticks), needle | SWING 30° + counter-plate | one turn 900 ms, then settle |
| control panel | three gears | turn 60°/s while hovered, coast onto the rest pose | — |
| apps button | disc + nine squares | pages left ×2, right ×1 | squares drop while the grid is open |
| bin | can, lid | lid lifts, can shakes | (drag states: orange / red / swallow) |

## 8. Building one

1. **Get the mark.** simple-icons path (24-unit square; split its subpaths at
   each `m`/`M` to find the parts — `lab/bbox.py`-style measuring) or the
   official geometry. Fit 56/64 by measured bounds.
2. **Write the motion card** (one per icon, before any code):
   `parts + pivots · metaphor · hover (held + one-shot) · click · numbers`.
3. **Lab first.** Add it to the App Motion Lab page: a row of three plates —
   REST, HOVER (point at it), CLICK (click it; it hovers too, so the click
   starts from the hovered state) — and its copy in the dock strip at the top.
   Verify with headless Seam screenshots at the key moments (`firefox
   --headless --screenshot`, a throwaway profile; the page carries
   `<meta name="darkreader-lock">` so Seam's Dark websites does not recolour
   it). Judge colours on Max's real screen with `grim`, not in a render.
4. **Max directs, one icon at a time**, usually 3–8 rounds of "faster /
   stronger / the other way / less". Change exactly what he names.
5. **On "put it on the dock"**: a `<name>_icon.rs` beside the others —
   `Pose`, `figure(&Pose) -> svg`, `frame()` 128² via resvg (`FRAME`),
   `Motion { step(hover, dt) -> bool, click(), pose() }` — wired in four
   places: `main.rs` (mod, the `App` field, the click in `Hit::DockIcon`),
   `frame.rs` (step each frame, `update_icon_layer` for every layer of that
   id), `apps.rs` (`dock_figure` returns the rest; `<name>_at(pose)` the
   frame). Plain shapes are drawn live each frame (`youtube_icon` is the
   template; `ytmusic_icon::track` plays keyframe tracks; `youtube_icon::bezier`
   is the easing); lit or filtered figures are pre-drawn per look on a
   low-priority thread (`compass_icon`, `whatsapp_icon`). Tests: every pose
   parses and draws; both moves end exactly at `REST`; the two or three claims
   that define the icon ("the middle line goes the other way", "the hover does
   not come back after a click"). Clippy clean, full suite green, release
   build, dock restarted via `hyprctl dispatch 'hl.dsp.exec_cmd("~/launcher/waverunner-dev")'`.
6. **Ids.** A webapp is `webapp-<slug>` (the slug of its name in
   `~/.config/webapps.list` — the dock reads *that* copy, not Golem's); a native
   app its `.desktop` id (`google-chrome`); Golem's own by their constants
   (`filer::APP_ID`, `compass_icon::SEAM_ID`).

## 9. Generating the rest

For a batch ("auto-animate these 20"):

- Produce the **motion cards** first, in one table: icon · parts · metaphor ·
  hover · click · numbers — each hover and click built from §5's vocabulary,
  within §4's numbers, obeying §3's rules. Max strikes or edits cards before
  anything is drawn.
- Build the approved cards **into the lab in one go**, three plates each, with
  the brand marks split into their parts; take one screenshot strip of the
  resting icons (they must all sit on the same axis) and one of a mid-moment
  per icon.
- Max reviews the lab; per-icon rounds as in §8.4; "dock" per icon or per
  batch.
- For the dock side at this scale, stop writing a module per icon: build one
  **data-driven brand-icon engine** (a `BrandIcon { parts: [{ path, fill,
  pivot, clip }], hover: { held: [...], once: [...] }, click: [...] }` with
  tracks of `(t, value, easing)` keyframes — the shape `ytmusic_icon::track`
  already has) and ship each icon as data (`assets/motion/<id>.json` or a
  const table), with the same `Motion` interface and the same four wiring
  points done once for all. The nine hand-made modules stay as they are.

## 10. Status (2026-10-18)

On the dock (launcher main, unpushed): WhatsApp fcd2731, YouTube 9145b01,
YouTube Music c52db81, Discord d6e07cf, Spotify aa69ef9, Google 18de07a,
Chrome 47825c1 + ef56503 + 0a6d74f, Files click b7a8b22. In the lab only:
Terminal (hover done, click not directed, not on the dock). The webapp catalog
the dock reads (`~/.config/webapps.list`) was edited by hand on the dev box for
WhatsApp's icon; Golem's `system/home/webapps.list` is not what it installs.
