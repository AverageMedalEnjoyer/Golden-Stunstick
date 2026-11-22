# Animator

The Animator animates the skeleton of a model described by a `.qc` file – typically a model you
decompiled with the Compiler & Decompiler tab, or your own source files.

## Opening a QC

**Open QC…** reads the QC (and every `$include`d QCI) and loads:

- **Meshes** – the first choice of every `$body`, `$model` and `$bodygroup`, from SMD or DMX
  (binary encodings 1–5 and KeyValues2 text; skeleton, skinning weights, UVs and face-set
  materials are read, flex shapes are not).
- **Skeleton** – the union of the mesh SMDs' skeletons; its bind pose is frame 0 of the first
  mesh. QCs without meshes (animation-only) take the skeleton from their first
  `$sequence`/`$animation` SMD.
- **Textures** – SMD material names are looked up through the QC's `$cdmaterials` in the
  session's mounted game (first search path wins), via VMT `$basetexture` (including `patch`
  materials) to the VTF.

*Stand the model up* (on by default) is a display-only rotation for rigs authored lying down
(TF2 characters, for example). It never changes saved or exported data.

## Viewport controls

| Input | Action |
|---|---|
| Left drag on empty space | orbit |
| Right drag / wheel | zoom |
| Middle drag / Shift + left drag | pan |
| Click near a joint | select that bone |
| **Left drag starting on a joint** | select it and rotate it (horizontal = around the screen's up axis, vertical = around its right axis) |
| **Left drag starting on an IK hand/foot** (green, easy rig on) | move the effector; the whole limb follows |
| Ctrl + left drag anywhere | the same, for the already-selected bone (handy when joints overlap) |

Joints are grabbed within 14 pixels of the cursor; dragging pauses playback. Bones are
rotated, not stretched – only IK hands/feet "move", by bending their limb.

The selected bone also has numeric **Position** / **Rotation °** fields in the left panel
(local to its parent, rotation in Source's X/Y/Z Euler order), plus *Reset bone* and
*Reset whole pose*.

## Easy rig

**Create easy rig** finds limbs from bone names:

- arms: `hand`/`wrist` → forearm → upper arm → clavicle/collar/shoulder
- legs: `foot`/`ankle` → calf/knee → thigh/hip

Valve biped `Wrist` helper bones are skipped when a real `Hand` bone shares the forearm.
Each chain can be toggled or removed, and *Add IK chain ending at '<bone>'* makes a custom chain
from the selected bone and its two parents.

Dragging an effector works like Source Filmmaker's easy rig: an exact two-bone solve on the
upper arm/forearm (thigh/calf), with the clavicle and pelvis left alone.

- The hand/foot goes exactly where you drag it, in any direction (straight up included), and
  keeps its own rotation. Past the limb's reach it stretches toward the cursor.
- The elbow/knee always bends the natural way. Its direction comes from the model's reference
  pose and is kept relative to the body: knees forward, elbows back. If the reference limb is
  perfectly straight, knees follow the toes and elbows point the opposite way.
- The upper and lower bones get a fixed, twist-free orientation, so dragging back and forth
  never corkscrews the arm. Dragging back to a spot gives exactly the same pose.

## Keys, frames and transitions

- Editing a bone on a frame without a key **creates a key** from the current interpolated pose
  (auto-key). **Set key (K)** does the same without editing.
- **Delete key** (Del), **Copy pose** / **Paste pose**, **< key** / **key >** jump between keys.
- **Frames** and **FPS** set the animation length; **Loop** makes the last key blend back into the
  first over the remaining frames.
- Each key has a **transition** to the next key:

| Transition | Curve |
|---|---|
| Linear | constant speed |
| Smooth | ease in + ease out (smoothstep) |
| Ease in / Ease out | slow start / slow end |
| Spline | Catmull-Rom through the neighbouring keys (no stops at keys) |
| Hold | step: the pose holds until the next key |

- **Smoothing** (0–1) blends *every* transition toward the spline through the keys, to round off
  a pose-to-pose animation.
- **Fade frame A into frame B** replaces the keys between A and B with a single transition from
  A's pose to B's pose using the chosen curve – an easy way to make two frames fade into each
  other.
- Undo / redo: **Ctrl+Z** / **Ctrl+Y**. Play / pause: **Space**.

## Saving

Animations are saved as text in the session:

```
goldenstunstick_files\sessions\<session>\animations\<name>.txt
```

- **Save** (Ctrl+S) – uses the animation's name; an animation that has never been named gets
  `<qc name>_animation_N` with the next free number (e.g. `tfheavymodel_animation_12`).
- **Save as…** – choose the name.
- **New animation** – a fresh animation for the current QC (bind pose at frame 0); it is not
  written to disk until you save it.
- Switching QC / opening another animation with unsaved changes asks to save, discard or cancel.

## Opening a saved animation

**Open animation…** lists the session's `animations` folder (or *Browse…*). Then:

1. The QC stored in the file is loaded. If it no longer exists you are told so and asked where
   the QC is now.
2. The animation's bones (names and parents) are compared with the QC's skeleton. If they differ,
   a popup lists every difference and lets you **Continue** (bones are matched by name; bones the
   animation does not have use the model's bind pose) or **Cancel**.

## Exporting

**Export SMD…** / **Export DMX…** open a save dialog in the QC's folder with the animation's
name pre-filled (rename freely). Every frame is sampled, so easing and smoothing are baked in.

- **SMD** – standard `nodes` / `skeleton` animation SMD.
- **DMX** – KeyValues2 DMX, `format model 1` (the version Source SDK 2013 / TF2 studiomdl reads
  natively; newer format versions need `dmxconvert`), with integer time ticks.

Both formats are covered by `tests/animator_export.rs`, which exports an IK-posed animation,
compiles it with TF2's studiomdl and checks the edited rotation survives (0.600 rad in, 0.600 rad
out). Use the exported file in a QC like any other animation:

```
$sequence "my_taunt" "my_taunt.smd" fps 30 loop
```

## The animation .txt format

```
// Golden Stunstick animation
version 1
name heavy_animation_1
qc C:\models\heavy\heavy.qc
fps 30
frames 60
loop 0
smoothing 0
bones 79
bone "bip_pelvis" ""
bone "bip_spine_0" "bip_pelvis"
...
key 0 smooth
  0 px py pz qx qy qz qw     <- bone index, local position, local rotation quaternion
  ...
endkey
key 30 linear
  ...
endkey
```
