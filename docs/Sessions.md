# Sessions, settings & files

## Sessions

A session ties Golden Stunstick to one game (or mod). It is just a folder:

```
goldenstunstick_files\sessions\<name>\session_settings.txt
```

`session_settings.txt` is plain text:

```
// Golden Stunstick session settings
gameinfo = C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2\tf\gameinfo.txt
studiomdl = C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2\bin\studiomdl.exe
```

`studiomdl` may be empty: it is only needed to compile (and for the decompiler's recompile
verification).

### New Session

`Files ▸ New Session` asks for:

| Field | Notes |
|---|---|
| Session name | becomes the folder name; `\ / : * ? " < > \|` are not allowed; must not already exist |
| gameinfo.txt | required; Browse… also tries to find studiomdl.exe next to the game |
| studiomdl.exe | optional |

The session opens immediately. Its name is shown in the window title and in the header above the
tabs together with the gameinfo.txt and studiomdl.exe paths.

### Open Session

Lists every folder inside `goldenstunstick_files\sessions\`. If the chosen folder has no
`session_settings.txt`, an error says it is missing and nothing else happens.

### Mounting the game

When a session opens, its gameinfo.txt is mounted in the background (a spinner shows while it
works). Every `game` search path is mounted **in the order gameinfo.txt lists them** and lookups
always use the **first** match – exactly like the engine. Macros are expanded:

| Macro | Becomes |
|---|---|
| `\|gameinfo_path\|` | the folder containing gameinfo.txt |
| `\|all_source_engine_paths\|` | the engine folder: the parent of the game folder, or for `sourcemods\<mod>` the Steam app named by `SteamAppId` (e.g. 243750 → *Source SDK Base 2013 Multiplayer*) |

`foo.vpk` entries open `foo_dir.vpk`, `custom/*` wildcards expand to every folder / VPK inside,
missing paths are skipped (and listed as skipped in the status line).

## Settings ▸ Theme

| Setting | Default |
|---|---|
| Border color | soft yellow `#E8D27A` |
| Background color | black |
| Panel color / opacity | near-black, 92 % |
| Text / accent colors | warm white / gold |
| 3D viewport background | dark grey |
| Background image | none |
| Background blur | 0 % |
| Background image opacity | 100 % |
| Border width, corner rounding, UI scale | 1, 4, 1.0 |

Changes preview live; **Apply & Save** writes them to `goldenstunstick_files\settings.txt`.
Closing the window without applying reverts the preview. To see a background image behind the
panels, lower *Panel opacity*.

## The goldenstunstick_files folder

- Created once, next to the EXE, the first time the program runs.
- If it (or `sessions\`) is deleted while the program is open it is re-created on the next
  periodic check (every 3 s) or before anything is written.
- The check only touches the disk when something is missing, so it cannot leak handles or
  memory.
