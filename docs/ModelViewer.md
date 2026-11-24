# Model Viewer

An HLMV++-style viewer for the models of the session's game.

## Opening models

**Open Model…** lists every `.mdl` in every mounted search path. Filters:

| Filter | Matches |
|---|---|
| Name | file name contains the text |
| Folder | path starts with the text, e.g. `models/player` |
| Source | only models whose winning copy comes from that VPK / folder |
| Only models that override another copy | models present in more than one search path |

Each row shows the VPK/folder the model comes from and how many other copies it overrides. The
copy in the **first** (top-most) search path always wins – for the model and for every VMT and
VTF it uses – exactly like the engine.

**Open from disk…** opens any `.mdl` file (its `.vvd`, `.dx90.vtx`, `.phy`, `.ani` are read from
the same folder; materials still come from the mounted game).

Models load on a background thread. `$includemodel` animation models (e.g. TF2's
`player/heavy_animations.mdl`) are loaded too, so their sequences are playable; the list marks
them with `*`.

## Viewport

| Input | Action |
|---|---|
| Left drag | rotate |
| Right drag / wheel | zoom |
| Middle drag / Shift + left drag | pan |
| F | frame the model |
| Click near a joint (Bones shown) | select the bone |

The camera starts facing the model's front (+X) and framed on its movement hull.

## Panels

**Render** – wireframe, flat, smooth or textured; wireframe overlay; backface culling; ground,
bones and bone names, attachments and names, hitboxes, normals, physics model, bounding boxes
(movement hull in green, view box in red); light yaw/pitch and ambient; field of view; custom
background colour. **Screenshot** saves the viewport as PNG into the session's `screenshots`
folder.

**Sequence** – searchable list (name and activity), play/pause, frame stepping, loop, speed
(0–4×), frame slider, **pose parameters**, and the selected sequence's details: frame count,
looping/delta, blend grid, events (frame, event name/number, options, and the sound it plays
with a **Play** button). By default a full-body idle (`stand_*`, `*idle*`) is chosen; delta
(additive) layers are not auto-selected.

**Event sounds** – while a sequence plays, its animation events play their sounds the way the game
does (turn off with *Play event sounds*; *Volume* sets the level). Sounds are looked up in the
game's soundscripts (`scripts/game_sounds_manifest.txt` and every file it lists, loaded in the
background), with the random wave, volume and pitch of each entry; `$gender` in wave paths becomes
`male` or `female` from the model's name. Supported events:

| Event | Sound |
|---|---|
| `AE_CL_PLAYSOUND`, `AE_SV_PLAYSOUND`, `AE_CL_EXCLUDE_PLAYER_SOUND` and other `*SOUND` events, `5004` (CL_EVENT_SOUND), `1004`, `1008` | the gamesound (or wave) named in the options |
| `6004` / `6005` (NPC footsteps) | `<options>.FootstepLeft/Right` (`RunFootstep…` in run sequences); default `NPC_CombineS` |
| `6006`–`6009` (material footsteps), TF2 `7001` (`left` / `right`) | `Concrete.StepLeft/Right` (the viewer has no ground surface) |
| HL2 zombie / antlion step and attack events | the NPC's own footstep / attack gamesounds |

WAV (PCM 8/16/24/32-bit, float, MS-ADPCM, MP3-in-WAV) and MP3 files are decoded. Models whose
footsteps come from game code rather than animation events (HL2MP players such as
`john_freeman`, which has no events at all) have no footsteps to play.

Pose parameters (`move_x`, `move_y`, `body_yaw`, `body_pitch`, …) are collected from the model
*and* its `$includemodel`s, matched by name as the engine does. Each one has a slider and a
number box; *Reset* puts them all back to 0 (or mid-range). Names in bold are used by the current
sequence or its layers. They drive:

- the sequence's blend grid (e.g. `run_*` blends direction by `move_x`/`move_y`), blended
  bilinearly;
- the sequence's auto-layers (`addlayer` / `blendlayer`), evaluated like the engine:
  start/peak/tail/end ramps, `spline`, `xfade`, `noblend`, pose-parameter-driven layers,
  weight lists, and `delta` / `predelta` addition. HL2MP-style aim layers, for example, turn
  the upper body with `body_yaw` and `body_pitch`.

**Body / Skin / Paint** – every bodygroup with more than one choice, skin family, LOD, and the TF2
paints (plus a custom colour). Paint applies to materials with the `ItemTintColor` proxy or
`$blendtintbybasealpha`.

**Flexes** – a slider per flex controller; flex rules are evaluated (constant, controller,
flex, + − × ÷, negate, min/max, 2-way, combo, dominate) and vertex animations (including stereo
left/right flexes) are applied before skinning.

**Bones** – list with parent, bind position/rotation, flags, procedural type, surface prop;
the selected bone is highlighted with axes and its name.

**Attachments** / **Hitboxes** – lists with highlighting; hitbox sets selectable; boxes coloured
by hit group.

**Physics** – solids from the `.phy` (bone, mass, surface prop, convex pieces), total mass, root
and concave flag; *Show physics model* draws the collision hulls on the animated model.

**Bonemerge** – add extra models (weapons, cosmetics) from the game or disk; bones are matched by
name and follow the main model, unmatched bones keep their bind pose. Each merged model has its
own skin and can be hidden or removed.

**Materials** – every material with the VMT found (and which VPK/folder it came from), its
`$basetexture` (and source), shader, translucent/alphatest/paintable flags, or the error if
something is missing.

**Model Info** – name, version, checksum, counts, pose parameters, IK chains, include models,
surface prop, mass, flags, eye position, hull, vertices, LODs and the model's keyvalues.

## Rendering notes

Meshes are skinned on the CPU (up to 3 bone weights per vertex, as in Source) and drawn with
OpenGL inside the egui window: half-lambert lighting, alpha test, translucent and additive
blending, `$nocull`, `$selfillum`, `$color2`. Not reproduced: phong, rim lighting, normal/detail
maps, env maps, eye refraction and glints, shadows, physics simulation, facial phonemes and
manually stacked gesture layers (HLMV's extra sequence slots). Sequences' own auto-layers do play.
