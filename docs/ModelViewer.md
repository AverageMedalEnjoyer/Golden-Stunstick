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
(0–4×), frame slider, pose-parameter sliders (blend sequences are bilinearly blended across their
blend grid), and the selected sequence's details: frame count, looping/delta, blend grid, events
(frame, event name/number, options). By default a full-body idle (`stand_*`, `*idle*`) is chosen;
delta (additive) layers are not auto-selected.

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
multi-layer gesture playback.
