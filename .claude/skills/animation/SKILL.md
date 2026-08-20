---
name: animation
description: Design, tune, or review a waverunner/OPTIONS animation — dock/box open-close, pill reveals, the metadata detail, magnification, launch bounce, any motion. Encodes the design language (dt-based, the 3 base flows, springs vs eases, Shinings feel) and gives a way to actually SEE the motion as a frame strip, not just a still. Use whenever a change touches how something moves.
---

# animation

Animation is the soul of OPTIONS. Getting it *right* means matching feel, not
just compiling. This skill carries the rules and a way to inspect motion.

## Non-negotiable rules

- **dt-based, frame-rate independent.** Never advance by a fixed per-frame
  step. Animators take a wall-clock `dt`; the 60/144 Hz invariance tests in
  `animation.rs` must still pass (`nix develop -c cargo test -p waverunner-daemon`).
- **Two primitives** (`animation.rs`): `ease_toward(cur, target, dt, rate, snap)`
  = exponential glide to rest (page slides, reflows, box heights); `Follower`
  = damped spring carrying momentum (the AGUA water bodies — card, dock icons,
  box content — each with slightly different k/c so swells overlap, never lockstep).
- **Idle at rest.** The frame loop must go fully idle (0% CPU) once settled —
  every animator reports "still moving?" and stops scheduling when false.
- **Never resize the surface** — animate content within the fixed layer surface.

## The design language (the OPTIONS body)

Three base flows — pick the one that fits, don't invent motion:
- **from-above** — a surface descends/wipes in from an edge (e.g. the clipboard
  detail top strip wipes down from the box top).
- **from-parent** — an element grows out from behind the pill/row that spawned
  it (control pills emerging behind the window pill; the detail card growing
  from the clicked row).
- **become-more** — an element intensifies in place (hover wash, the clip beat).

Match the Shinings palette, the micro-physics curves, and flow-protection (never
yank focus/motion while the user is mid-action). The authoritative reference is
the `options-ux-guidelines` memory and the HTML mockups in `~/*-mockup`, which
are the acceptance target for each OPTION's motion.

## Seeing the motion (not just a still)

A screenshot shows one frame; feel lives in the sequence. Use:

```
.claude/skills/animation/capture-seq.sh <clip-open|clip-detail|notif-open|dock|launcher> [geom] [frames]
```

It restarts to a clean closed state, fires the animation, and bursts ~8 `grim`
frames across it into `/tmp/waverunner-verify/seq/`. Read the **first, a middle,
and the last** frame to judge the progression (does the strip wipe from above?
do the pills seat *with* it or pop late? any jump at the start/end?). Then
compare against the matching mockup. ~8 frames covers a ~0.34s open; for a
slower motion pass more `frames`.

If frame-strip resolution proves too coarse for a fast animation, the next step
is a daemon debug time-scale (multiply `dt` at the tick sites behind an atomic +
a `debug-anim-scale` ctl verb) to run motion in slow-mo — ask before adding it,
it touches several `dt` sites.

## Workflow for an animation change

1. Read the current animator (find it via the `CLAUDE.md` "where things live" map).
2. Make the change; keep it dt-based.
3. `cargo test -p waverunner-daemon` — the Hz-invariance tests must stay green.
4. `capture-seq.sh` the affected motion; compare frames to the mockup.
5. Only claim it works once you've looked at the strip.
