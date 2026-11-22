//! Golden Stunstick core library. The GUI binary lives in `main.rs`; all
//! file-format and tool logic lives here so it can be tested headlessly.

pub mod anim;
pub mod compile;
pub mod decompile;
pub mod formats;
pub mod gamefs;
pub mod render;
pub mod scene;
pub mod verify;
