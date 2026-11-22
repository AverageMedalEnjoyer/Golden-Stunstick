# File formats & internals

All parsers are bounds-checked (`formats/bin.rs`): a truncated or corrupt file produces an error
message, never a crash.

| Format | Module | Notes |
|---|---|---|
| VPK v1/v2 | `formats/vpk.rs` | directory parsed up front, data read on demand from `_NNN.vpk` or the dir file; preload bytes supported |
| KeyValues | `formats/keyvalues.rs` | gameinfo.txt, VMT, MDL/PHY keyvalue text; `//` and `/* */` comments, `[$COND]` ignored |
| VTF 7.0–7.5 | `formats/vtf.rs` | DXT1/3/5 and all common uncompressed formats, RGBA16F tone-mapped; resources (7.3+); picks the largest mip ≤ the requested size |
| MDL v44–49 | `formats/mdl.rs` | layouts from `public/studio.h` (SDK 2013): bones, controllers, hitboxes, animations (sections, `.ani` blocks, movements, IK rules), sequences (events, blends, layers, weightlists, IK locks, keyvalues, activity modifiers), textures, skins, body parts, meshes, flexes (float16 or fixed-point vertex animation), eyeballs, attachments, nodes, flex controllers/rules, IK chains, pose parameters, include models, studiohdr2 |
| Animation data | `formats/mdl.rs` | RLE `mstudioanimvalue_t`, Quaternion48/64, Vector48, delta animations, `STUDIO_ALLZEROS` |
| VVD | `formats/mdl.rs` | LOD fixups applied |
| VTX (`.dx90.vtx`) | `formats/mdl.rs` | strip groups (SDK 2013 and newer layouts), triangle lists and strips; indices address the strip group's vertices (multi-strip meshes) |
| PHY | `formats/phy.rs` | IVP compact surfaces/ledges, IVP→Source axes and units, trailing text section |
| SMD / VTA | `formats/smd.rs` | read and write |
| DMX | `formats/dmx.rs` | binary encodings 1–5 and KeyValues2 text; DmeModel → skeleton + meshes for the Animator |
| QC | `formats/qc.rs` | tokenizer with line numbers, `$include`, lint |

## Coordinate systems

Source is Z-up. Rotations use Source's `RadianEuler` (x = roll, y = pitch, z = yaw) with
`AngleMatrix` order R = Rz · Ry · Rx (`euler_to_quat` / `quat_to_euler`, the latter computed in
f64 for accuracy near ±90° pitch). See the table in [CompilerDecompiler.md](CompilerDecompiler.md)
for how studiomdl transforms each kind of data.

## Rendering

`render.rs` draws inside egui with a glow paint callback: one dynamic vertex/index buffer per
frame (meshes are skinned on the CPU), one textured mesh shader (half-lambert, alpha test,
translucent/additive passes, flat shading via screen-space derivatives) and one line shader for
grids, bones, hitboxes, physics and normals. Textures are uploaded once and cached by VTF path.
The renderer is created lazily on the first paint and destroyed on exit.

## Threads

Mounting a game, loading a model, compiling and decompiling run on background threads and report
through channels, so the window stays responsive. studiomdl runs without a console window.
