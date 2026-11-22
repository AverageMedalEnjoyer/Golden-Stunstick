# Compiler & Decompiler

## Compile

Runs the session's `studiomdl.exe` with `-game <folder> -nop4` on the input, which is a QC file or
every QC in a folder (optionally including sub-folders); **Browse…** picks either. studiomdl's
output appears in the log as it happens.

**Output** – where the compiled `.mdl`, `.vvd`, `.vtx`, `.phy` and `.ani` files go:

| Choice | Files end up in |
|---|---|
| Game's models folder (default) | `<game>\models\<$modelname>`, as studiomdl normally does |
| Folder next to each QC | `<QC folder>\<sub-folder>` (default `compiled`) |
| Custom folder | the folder you choose |

studiomdl can only write into a game's `models` folder. For the last two choices it compiles
into a private staging game in `%TEMP%`, which searches the session's game content, and the
finished files are then moved to your folder, so the real game folder is not touched. The
staging folder is deleted when the run ends. *Keep the $modelname sub-folders* recreates
`player\john\…` under the output folder. **Open output folder** opens where the last model went.
*Also copy output to* still makes an extra copy (with sub-folders) wherever the files went.

Options: *Check QC before compiling*, `-verbose`, `-nox360`, `-fastbuild`, `-definebones`,
`-printbones`, `-dumpmaterials`, and free-form extra arguments.

### QC pre-check

Before studiomdl starts, the QC and every `$include` are checked for:

- missing closing quotes and unclosed `/*` comments (with the line)
- unbalanced `{` `}` (the line of the unmatched brace)
- missing `$modelname`, `$modelname` without `.mdl`, missing `$cdmaterials`
- mesh / animation / collision files that do not exist (relative to the QC or the including QCI)
- duplicate `$sequence` names
- unknown `$commands`, commands without required values, `$bodygroup` without a `{ }` block

If the pre-check finds errors, studiomdl is not started (untick the option to compile anyway).

### Errors

studiomdl's output is turned into short issues (file, line, what went wrong) shown on the right.
Known messages are recognised (too many bones/verts/materials, unknown bone, unknown command,
missing files, missing braces, duplicate names, bad weights, frame out of range, include models,
collision models, `-game` problems…). **Suggested Fixes** (right-hand button) opens every issue
with the offending source line and a concrete fix.

When a compile fails, an error report is appended to

```
goldenstunstick_files\sessions\<session>\logs\compileerror_MMDDYYYY.txt
```

using the computer's current date (e.g. `compileerror_11222026.txt` for 22 Nov 2026). Each report
has a header with the time, QC, studiomdl and game paths, the short summary with fixes, and the
full studiomdl output. Several failures on the same day are appended to the same file.

## Decompile

Sources: an MDL file, a folder of MDLs (each model goes into its own sub-folder), or a model
picked from the mounted game. Output (per model):

| File | Contents |
|---|---|
| `<model>.qc` | the QC (optionally split into `_model`, `_textures`, `_bones`, `_animations`, `_collision` QCIs) |
| `<body>.smd` | reference mesh of every body / bodygroup choice |
| `<body>_lodN.smd` | LOD meshes (`$lod` / `$shadowlod` with switch points) |
| `<model>_physics.smd` | collision model (`$collisionmodel` or `$collisionjoints`) |
| `anims/<anim>.smd` | every animation |
| `<body>.vta` | flex shapes |
| `<model>.vrd` | quaternion-interpolation procedural bones |

The QC reproduces: `$modelname`, `$includemodel`, `$body` / `$model` / `$bodygroup`, eyeballs,
flex files, `flexcontroller` and flex rules, `$lod` / `$shadowlod`, `$surfaceprop`, `$contents`,
model flags (`$staticprop`, `$opaque`, `$mostlyopaque`, `$ambientboost`, `$donotcastshadows`,
`$noforcedfade`, `$constantdirectionallight`, …), `$eyeposition`, `$illumposition`,
`$maxeyedeflection`, `$illumpositionattachment`, `$cdmaterials`, `$texturegroup`, `$keyvalues`,
`$definebone` (when needed), `$controller`, `$attachment`, `$bbox`, `$cbox`, `$bonemerge`,
`$hboxset` / `$hbox`, `$ikchain`, `$proceduralbones`, `$jigglebone`, `$poseparameter`,
`$sectionframes`, `$animblocksize`, `$weightlist`, `$animation` (fps, loop, snap, post, delta
via `subtract`, root motion via `walkframe`, `ikrule`, `noautoik`), `$sequence` (blends with
`blendwidth`/`blend`, activities and modifiers, fades, flags, cycle pose, nodes and transitions,
events, `addlayer`/`blendlayer`, weightlists, IK locks, keyvalues), `$declaresequence`, and the
collision block (`$mass`, `$rootbone`, `$concave`, `$inertia`/`$damping`/`$rotdamping` and per-joint
overrides, `$jointmassbias`, `$jointconstrain`, `$jointmerge`, `$noselfcollisions`,
`$jointcollide`, `$jointsurfaceprop`, `$collisiontext`).

### Why the output is accurate

Crowbar-style decompilers commonly get root bones, animation orientation, collision placement and
sequence options subtly wrong. Golden Stunstick's conventions were **measured** against TF2's
studiomdl with probe models (`tests/probe.rs`) instead of assumed:

| Data | What studiomdl does | What the decompiler writes |
|---|---|---|
| Reference skeleton and mesh | stored as in the SMD | unchanged |
| Animation root bones | rotated +90° about Z | rotated −90° |
| `$eyeposition`, `$illumposition` | rotated +90° about Z | rotated −90° |
| `$staticprop` mesh and collision | rotated +90° about Z | rotated −90° |
| `$collisionmodel` | bone space, posed by frame 0 of the first sequence | posed back to the bind pose |
| Root motion (`LX`, `LY`…) | removed from the root bone | added back; `walkframe` pins each piece |
| `loop` on an `$animation` | overwrites the last frame with the first | put on the sequence when the frames differ |
| `$ikchain` present | invents IK rules | the exact rules + `noautoik` |
| `$weightlist` | ignores bones it hasn't loaded yet; sequences take weights from animations | declared after `$definebone`, attached to the animations |
| Bones used only by attachments/hitboxes | removed | kept with `$definebone` |
| Per-bone ragdoll masses edited after compiling | n/a | `$jointmassbias` solved to reproduce every mass exactly |
| Mesh triangle winding | — | chosen so face normals agree with vertex normals |

### Accuracy report

Every decompile runs two checks and shows them under **Accuracy report**:

1. **Self-check** – every SMD written is read back and compared with the data decoded straight
   from the MDL (every frame of every animation, all triangles, vertex weights).
2. **Recompile comparison** (needs studiomdl; on by default) – the QC is compiled into a
   throw-away game folder whose gameinfo.txt mounts the same content, and the new model is
   compared with the original: skeleton (names, parents, bind pose), attachments, hitboxes,
   bodygroups, triangles (positions and UVs), materials and skins, every sequence (activity,
   flags, blends, fades, events, layers, weightlists), animation data frame by frame (worst
   position and rotation error with the animation, frame and bone), root motion, model properties
   (eye/illum positions, boxes, surface prop, contents, flags, pose parameters, IK chains,
   includes, flexes, mass) and physics (solids, convex pieces, bounds, per-bone masses).

Results from the test suite on TF2 (all checks pass):

| Model | Notes |
|---|---|
| `props_2fort/oildrum.mdl` | static prop, 6 skins, LOD, collision |
| `weapons/w_models/w_shotgun.mdl` | original compiler did not weld vertices (reported as a note) |
| `weapons/c_models/c_shotgun/c_shotgun.mdl` | 10 attachments, bone kept by `$definebone` |
| `player/heavy.mdl` | 79 bones, 259 sequences, 46 flexes, eyeballs, 16-solid ragdoll, 8 skins, LODs |
| `player/heavy_animations.mdl` | 819 animations, 317 sequences, IK rules, blends, weightlists; 14 350 frames compared, worst error 0.06 units / 0.08° (studiomdl re-compression) |

Differences that studiomdl itself introduces are reported as notes, not hidden: vertex welding,
unused materials in the original, header mass vs. edited ragdoll masses, and looping animations
whose last frame differs from the first.

### Known limitations

- Stereo flex `flexpair` split widths are approximated (the MDL keeps only per-vertex side weights).
- Eyelid (DME) flex rules have no QC syntax; studiomdl regenerates them from eyelid data and they
  are written as comments.
- CS:GO frame-encoded animations (`STUDIO_FRAMEANIM`) and procedural bone types other than
  quaternion-interpolation and jiggle bones are not exported.
- IK rules stored in external `.ani` animation blocks are not read (noted in the QC).
