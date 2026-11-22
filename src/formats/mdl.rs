//! Source engine studio model (.mdl v44-v49) parser, plus VVD/VTX geometry
//! and the RLE animation decoder.
//!
//! Struct layouts follow `public/studio.h` from the Source SDK 2013.

use super::bin::{Bin, FResult, add_off, err, f16_to_f32};
use glam::{Quat, Vec3};

pub const STUDIO_ANIM_RAWPOS: u8 = 0x01;
pub const STUDIO_ANIM_RAWROT: u8 = 0x02;
pub const STUDIO_ANIM_ANIMPOS: u8 = 0x04;
pub const STUDIO_ANIM_ANIMROT: u8 = 0x08;
pub const STUDIO_ANIM_DELTA: u8 = 0x10;
pub const STUDIO_ANIM_RAWROT2: u8 = 0x20;

pub const STUDIO_LOOPING: i32 = 0x0001;
pub const STUDIO_SNAP: i32 = 0x0002;
pub const STUDIO_DELTA: i32 = 0x0004;
pub const STUDIO_AUTOPLAY: i32 = 0x0008;
pub const STUDIO_POST: i32 = 0x0010;
pub const STUDIO_ALLZEROS: i32 = 0x0020;
pub const STUDIO_FRAMEANIM: i32 = 0x0040;
pub const STUDIO_CYCLEPOSE: i32 = 0x0080;
pub const STUDIO_REALTIME: i32 = 0x0100;
pub const STUDIO_LOCAL: i32 = 0x0200;
pub const STUDIO_HIDDEN: i32 = 0x0400;
pub const STUDIO_OVERRIDE: i32 = 0x0800;
pub const STUDIO_WORLD: i32 = 0x4000;
pub const STUDIO_NOFORCELOOP: i32 = 0x8000;

#[derive(Clone, Debug, Default)]
pub struct Bone {
    pub name: String,
    pub parent: i32,
    pub bonecontroller: [i32; 6],
    pub pos: Vec3,
    pub quat: Quat,
    pub rot: Vec3,
    pub posscale: Vec3,
    pub rotscale: Vec3,
    pub pose_to_bone: [[f32; 4]; 3],
    pub q_alignment: Quat,
    pub flags: i32,
    pub proctype: i32,
    pub procindex: i32,
    pub physicsbone: i32,
    pub surfaceprop: String,
    pub contents: i32,
    /// Absolute offset of this bone struct (needed for procedural bone data).
    pub offset: usize,
}

#[derive(Clone, Debug, Default)]
pub struct BoneController {
    pub bone: i32,
    pub kind: i32,
    pub start: f32,
    pub end: f32,
    pub rest: i32,
    pub inputfield: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Hitbox {
    pub bone: i32,
    pub group: i32,
    pub min: Vec3,
    pub max: Vec3,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct HitboxSet {
    pub name: String,
    pub hitboxes: Vec<Hitbox>,
}

#[derive(Clone, Debug, Default)]
pub struct Attachment {
    pub name: String,
    pub flags: u32,
    pub bone: i32,
    pub local: [[f32; 4]; 3],
}

#[derive(Clone, Debug, Default)]
pub struct Texture {
    pub name: String,
    pub flags: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Flex {
    pub flexdesc: i32,
    pub target: [f32; 4],
    pub flexpair: i32,
    pub vertanimtype: u8,
    pub verts: Vec<VertAnim>,
}

#[derive(Clone, Debug, Default)]
pub struct VertAnim {
    pub index: u16,
    pub speed: u8,
    pub side: u8,
    pub delta: Vec3,
    pub ndelta: Vec3,
    pub wrinkle: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub material: i32,
    pub num_vertices: i32,
    pub vertex_offset: i32,
    pub material_type: i32,
    pub material_param: i32,
    pub mesh_id: i32,
    pub center: Vec3,
    pub flexes: Vec<Flex>,
}

#[derive(Clone, Debug, Default)]
pub struct Eyeball {
    pub name: String,
    pub bone: i32,
    pub org: Vec3,
    pub zoffset: f32,
    pub radius: f32,
    pub up: Vec3,
    pub forward: Vec3,
    pub texture: i32,
    pub iris_scale: f32,
    pub upperflexdesc: [i32; 3],
    pub lowerflexdesc: [i32; 3],
    pub uppertarget: [f32; 3],
    pub lowertarget: [f32; 3],
    pub upperlidflexdesc: i32,
    pub lowerlidflexdesc: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Model {
    pub name: String,
    pub kind: i32,
    pub radius: f32,
    pub meshes: Vec<Mesh>,
    pub num_vertices: i32,
    /// index of the model's first vertex in the VVD LOD0 vertex array
    pub first_vertex: i32,
    pub eyeballs: Vec<Eyeball>,
}

#[derive(Clone, Debug, Default)]
pub struct BodyPart {
    pub name: String,
    pub base: i32,
    pub models: Vec<Model>,
}

#[derive(Clone, Debug, Default)]
pub struct Movement {
    pub endframe: i32,
    pub motionflags: i32,
    pub v0: f32,
    pub v1: f32,
    pub angle: f32,
    pub vector: Vec3,
    pub position: Vec3,
}

#[derive(Clone, Debug, Default)]
pub struct IkRule {
    pub index: i32,
    pub kind: i32,
    pub chain: i32,
    pub bone: i32,
    pub slot: i32,
    pub height: f32,
    pub radius: f32,
    pub floor: f32,
    pub pos: Vec3,
    pub q: Quat,
    pub start: f32,
    pub peak: f32,
    pub tail: f32,
    pub end: f32,
    pub contact: f32,
    pub drop: f32,
    pub top: f32,
    pub attachment: String,
}

#[derive(Clone, Debug, Default)]
pub struct AnimDesc {
    pub name: String,
    pub fps: f32,
    pub flags: i32,
    pub numframes: i32,
    pub movements: Vec<Movement>,
    pub animblock: i32,
    pub animindex: i32,
    pub sectionindex: i32,
    pub sectionframes: i32,
    pub ikrules: Vec<IkRule>,
    pub numikrules: i32,
    pub offset: usize,
    pub zeroframeindex: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Event {
    pub cycle: f32,
    pub event: i32,
    pub kind: i32,
    pub options: String,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct AutoLayer {
    pub sequence: i16,
    pub pose: i16,
    pub flags: i32,
    pub start: f32,
    pub peak: f32,
    pub tail: f32,
    pub end: f32,
}

#[derive(Clone, Debug, Default)]
pub struct IkLock {
    pub chain: i32,
    pub pos_weight: f32,
    pub local_q_weight: f32,
    pub flags: i32,
}

#[derive(Clone, Debug, Default)]
pub struct SeqDesc {
    pub name: String,
    pub activity_name: String,
    pub flags: i32,
    pub activity: i32,
    pub actweight: i32,
    pub events: Vec<Event>,
    pub bbmin: Vec3,
    pub bbmax: Vec3,
    pub anim_indices: Vec<i16>,
    pub groupsize: [i32; 2],
    pub paramindex: [i32; 2],
    pub paramstart: [f32; 2],
    pub paramend: [f32; 2],
    pub paramparent: i32,
    pub fadeintime: f32,
    pub fadeouttime: f32,
    pub entrynode: i32,
    pub exitnode: i32,
    pub nodeflags: i32,
    pub entryphase: f32,
    pub exitphase: f32,
    pub lastframe: f32,
    pub nextseq: i32,
    pub pose: i32,
    pub numikrules: i32,
    pub autolayers: Vec<AutoLayer>,
    pub weightlist: Vec<f32>,
    pub iklocks: Vec<IkLock>,
    pub keyvalues: String,
    pub cycleposeindex: i32,
    pub activity_modifiers: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PoseParam {
    pub name: String,
    pub flags: i32,
    pub start: f32,
    pub end: f32,
    pub loop_range: f32,
}

#[derive(Clone, Debug, Default)]
pub struct IkChain {
    pub name: String,
    pub linktype: i32,
    pub links: Vec<(i32, Vec3)>,
}

#[derive(Clone, Debug, Default)]
pub struct FlexController {
    pub kind: String,
    pub name: String,
    pub local_to_global: i32,
    pub min: f32,
    pub max: f32,
}

#[derive(Clone, Debug, Default)]
pub struct FlexRule {
    pub flex: i32,
    pub ops: Vec<(i32, i32, f32)>, // (op, index, value)
}

#[derive(Clone, Debug, Default)]
pub struct IncludeModel {
    pub label: String,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct Mdl {
    pub data: Vec<u8>,
    pub version: i32,
    pub checksum: i32,
    pub name: String,
    pub eyeposition: Vec3,
    pub illumposition: Vec3,
    pub hull_min: Vec3,
    pub hull_max: Vec3,
    pub view_bbmin: Vec3,
    pub view_bbmax: Vec3,
    pub flags: i32,
    pub bones: Vec<Bone>,
    pub bone_controllers: Vec<BoneController>,
    pub hitbox_sets: Vec<HitboxSet>,
    pub anims: Vec<AnimDesc>,
    pub seqs: Vec<SeqDesc>,
    pub activity_list_version: i32,
    pub textures: Vec<Texture>,
    pub cdmaterials: Vec<String>,
    pub skin_ref: i32,
    pub skins: Vec<Vec<i16>>,
    pub bodyparts: Vec<BodyPart>,
    pub attachments: Vec<Attachment>,
    pub node_names: Vec<String>,
    pub flex_descs: Vec<String>,
    pub flex_controllers: Vec<FlexController>,
    pub flex_rules: Vec<FlexRule>,
    pub ik_chains: Vec<IkChain>,
    pub pose_params: Vec<PoseParam>,
    pub surfaceprop: String,
    pub keyvalues: String,
    pub mass: f32,
    pub contents: i32,
    pub include_models: Vec<IncludeModel>,
    pub anim_block_name: String,
    pub anim_blocks: Vec<(i32, i32)>,
    pub constdirectionallightdot: u8,
    pub root_lod: u8,
    pub num_allowed_root_lods: u8,
    pub vert_anim_fixed_point_scale: f32,
    pub illum_position_attachment: i32,
    pub max_eye_deflection: f32,
}

fn v3(a: [f32; 3]) -> Vec3 {
    Vec3::from_array(a)
}

impl Mdl {
    pub fn parse(data: Vec<u8>) -> FResult<Mdl> {
        let mut m = Mdl { data, ..Default::default() };
        m.parse_inner()?;
        Ok(m)
    }

    fn parse_inner(&mut self) -> FResult<()> {
        let data = std::mem::take(&mut self.data);
        let r = self.parse_with(&data);
        self.data = data;
        r
    }

    fn parse_with(&mut self, data: &[u8]) -> FResult<()> {
        let b = Bin::new(data);
        if b.bytes(0, 4)? != b"IDST" {
            return err("not a studio model (missing IDST)");
        }
        self.version = b.i32(4)?;
        if !(44..=49).contains(&self.version) {
            return err(format!("unsupported MDL version {} (supported: 44-49)", self.version));
        }
        self.checksum = b.i32(8)?;
        self.name = b.fixed_str(12, 64)?;
        self.eyeposition = v3(b.vec3(80)?);
        self.illumposition = v3(b.vec3(92)?);
        self.hull_min = v3(b.vec3(104)?);
        self.hull_max = v3(b.vec3(116)?);
        self.view_bbmin = v3(b.vec3(128)?);
        self.view_bbmax = v3(b.vec3(140)?);
        self.flags = b.i32(152)?;

        let cnt = |off: usize| -> FResult<(usize, usize)> {
            let n = b.i32(off)?;
            let i = b.i32(off + 4)?;
            if n < 0 || n > 1_000_000 || i < 0 {
                return err(format!("corrupt count/index at header offset {off}"));
            }
            Ok((n as usize, i as usize))
        };

        // bones
        let (nb, bi) = cnt(156)?;
        for i in 0..nb {
            let o = bi + i * 216;
            let mut bc = [0i32; 6];
            for (k, v) in bc.iter_mut().enumerate() {
                *v = b.i32(o + 8 + k * 4)?;
            }
            let q = b.vec4(o + 44)?;
            let qa = b.vec4(o + 144)?;
            self.bones.push(Bone {
                name: b.rel_str(o, 0)?,
                parent: b.i32(o + 4)?,
                bonecontroller: bc,
                pos: v3(b.vec3(o + 32)?),
                quat: Quat::from_xyzw(q[0], q[1], q[2], q[3]),
                rot: v3(b.vec3(o + 60)?),
                posscale: v3(b.vec3(o + 72)?),
                rotscale: v3(b.vec3(o + 84)?),
                pose_to_bone: b.mat3x4(o + 96)?,
                q_alignment: Quat::from_xyzw(qa[0], qa[1], qa[2], qa[3]),
                flags: b.i32(o + 160)?,
                proctype: b.i32(o + 164)?,
                procindex: b.i32(o + 168)?,
                physicsbone: b.i32(o + 172)?,
                surfaceprop: b.rel_str(o, 176)?,
                contents: b.i32(o + 180)?,
                offset: o,
            });
        }

        // bone controllers
        let (n, idx) = cnt(164)?;
        for i in 0..n {
            let o = idx + i * 56;
            self.bone_controllers.push(BoneController {
                bone: b.i32(o)?,
                kind: b.i32(o + 4)?,
                start: b.f32(o + 8)?,
                end: b.f32(o + 12)?,
                rest: b.i32(o + 16)?,
                inputfield: b.i32(o + 20)?,
            });
        }

        // hitbox sets
        let (n, idx) = cnt(172)?;
        for i in 0..n {
            let o = idx + i * 12;
            let nh = b.i32(o + 4)?.max(0) as usize;
            let hi = add_off(o, b.i32(o + 8)?)?;
            let mut set = HitboxSet { name: b.rel_str(o, 0)?, hitboxes: Vec::new() };
            for h in 0..nh {
                let ho = hi + h * 68;
                set.hitboxes.push(Hitbox {
                    bone: b.i32(ho)?,
                    group: b.i32(ho + 4)?,
                    min: v3(b.vec3(ho + 8)?),
                    max: v3(b.vec3(ho + 20)?),
                    name: b.rel_str(ho, 32).unwrap_or_default(),
                });
            }
            self.hitbox_sets.push(set);
        }

        // animations
        let (n, idx) = cnt(180)?;
        for i in 0..n {
            let o = idx + i * 100;
            let nm = b.i32(o + 20)?.max(0) as usize;
            let mi = b.i32(o + 24)?;
            let mut movements = Vec::new();
            if nm > 0 && nm < 1000 {
                let mo = add_off(o, mi)?;
                for k in 0..nm {
                    let p = mo + k * 44;
                    movements.push(Movement {
                        endframe: b.i32(p)?,
                        motionflags: b.i32(p + 4)?,
                        v0: b.f32(p + 8)?,
                        v1: b.f32(p + 12)?,
                        angle: b.f32(p + 16)?,
                        vector: v3(b.vec3(p + 20)?),
                        position: v3(b.vec3(p + 32)?),
                    });
                }
            }
            let numikrules = b.i32(o + 60)?;
            let ikruleindex = b.i32(o + 64)?;
            let animblock = b.i32(o + 52)?;
            let mut ikrules = Vec::new();
            if numikrules > 0 && numikrules < 64 && animblock == 0 && ikruleindex != 0 {
                let base = add_off(o, ikruleindex)?;
                for k in 0..numikrules as usize {
                    if let Ok(r) = read_ikrule(&b, base + k * 152) {
                        ikrules.push(r);
                    }
                }
            }
            self.anims.push(AnimDesc {
                name: b.rel_str(o, 4)?,
                fps: b.f32(o + 8)?,
                flags: b.i32(o + 12)?,
                numframes: b.i32(o + 16)?,
                movements,
                animblock,
                animindex: b.i32(o + 56)?,
                numikrules,
                ikrules,
                sectionindex: b.i32(o + 80)?,
                sectionframes: b.i32(o + 84)?,
                offset: o,
                zeroframeindex: b.i32(o + 92)?,
            });
        }

        // sequences
        let (n, idx) = cnt(188)?;
        for i in 0..n {
            let o = idx + i * 212;
            let mut s = SeqDesc {
                name: b.rel_str(o, 4)?,
                activity_name: b.rel_str(o, 8)?,
                flags: b.i32(o + 12)?,
                activity: b.i32(o + 16)?,
                actweight: b.i32(o + 20)?,
                bbmin: v3(b.vec3(o + 32)?),
                bbmax: v3(b.vec3(o + 44)?),
                groupsize: [b.i32(o + 68)?, b.i32(o + 72)?],
                paramindex: [b.i32(o + 76)?, b.i32(o + 80)?],
                paramstart: [b.f32(o + 84)?, b.f32(o + 88)?],
                paramend: [b.f32(o + 92)?, b.f32(o + 96)?],
                paramparent: b.i32(o + 100)?,
                fadeintime: b.f32(o + 104)?,
                fadeouttime: b.f32(o + 108)?,
                entrynode: b.i32(o + 112)?,
                exitnode: b.i32(o + 116)?,
                nodeflags: b.i32(o + 120)?,
                entryphase: b.f32(o + 124)?,
                exitphase: b.f32(o + 128)?,
                lastframe: b.f32(o + 132)?,
                nextseq: b.i32(o + 136)?,
                pose: b.i32(o + 140)?,
                numikrules: b.i32(o + 144)?,
                cycleposeindex: b.i32(o + 180)?,
                ..Default::default()
            };
            // events
            let ne = b.i32(o + 24)?.max(0) as usize;
            let ei = b.i32(o + 28)?;
            if ne > 0 && ne < 4096 {
                let eo = add_off(o, ei)?;
                for k in 0..ne {
                    let p = eo + k * 80;
                    s.events.push(Event {
                        cycle: b.f32(p)?,
                        event: b.i32(p + 4)?,
                        kind: b.i32(p + 8)?,
                        options: b.fixed_str(p + 12, 64)?,
                        name: b.rel_str(p, 76).unwrap_or_default(),
                    });
                }
            }
            // blends
            let count = (s.groupsize[0].max(1) * s.groupsize[1].max(1)) as usize;
            let ai = add_off(o, b.i32(o + 60)?)?;
            for k in 0..count.min(4096) {
                s.anim_indices.push(b.i16(ai + k * 2)?);
            }
            // autolayers
            let nal = b.i32(o + 148)?.max(0) as usize;
            if nal > 0 && nal < 256 {
                let ao = add_off(o, b.i32(o + 152)?)?;
                for k in 0..nal {
                    let p = ao + k * 24;
                    s.autolayers.push(AutoLayer {
                        sequence: b.i16(p)?,
                        pose: b.i16(p + 2)?,
                        flags: b.i32(p + 4)?,
                        start: b.f32(p + 8)?,
                        peak: b.f32(p + 12)?,
                        tail: b.f32(p + 16)?,
                        end: b.f32(p + 20)?,
                    });
                }
            }
            // weightlist
            let wi = b.i32(o + 156)?;
            if wi != 0 {
                let wo = add_off(o, wi)?;
                for k in 0..self.bones.len() {
                    s.weightlist.push(b.f32(wo + k * 4).unwrap_or(1.0));
                }
            }
            // iklocks
            let nl = b.i32(o + 164)?.max(0) as usize;
            if nl > 0 && nl < 64 {
                let lo = add_off(o, b.i32(o + 168)?)?;
                for k in 0..nl {
                    let p = lo + k * 32;
                    s.iklocks.push(IkLock { chain: b.i32(p)?, pos_weight: b.f32(p + 4)?, local_q_weight: b.f32(p + 8)?, flags: b.i32(p + 12)? });
                }
            }
            // keyvalues
            let kvi = b.i32(o + 172)?;
            let kvs = b.i32(o + 176)?;
            if kvi != 0 && kvs > 0 {
                let ko = add_off(o, kvi)?;
                if let Ok(bytes) = b.bytes(ko, kvs as usize) {
                    s.keyvalues = String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string();
                }
            }
            // activity modifiers (SDK 2013, v49)
            let ami = b.i32(o + 184)?;
            let amn = b.i32(o + 188)?;
            if self.version >= 49 && ami > 0 && amn > 0 && amn < 64 {
                let mo = add_off(o, ami)?;
                for k in 0..amn as usize {
                    let p = mo + k * 4;
                    let nm = b.rel_str(p, 0).unwrap_or_default();
                    if !nm.is_empty() {
                        s.activity_modifiers.push(nm);
                    }
                }
            }
            self.seqs.push(s);
        }
        self.activity_list_version = b.i32(196)?;

        // textures
        let (n, idx) = cnt(204)?;
        for i in 0..n {
            let o = idx + i * 64;
            self.textures.push(Texture { name: b.rel_str(o, 0)?, flags: b.i32(o + 4)? });
        }
        let (n, idx) = cnt(212)?;
        for i in 0..n {
            let off = b.i32(idx + i * 4)?;
            self.cdmaterials.push(b.cstr(off.max(0) as usize));
        }
        // skins
        self.skin_ref = b.i32(220)?;
        let nfam = b.i32(224)?.max(0) as usize;
        let si = b.i32(228)? as usize;
        for f in 0..nfam.min(1024) {
            let mut fam = Vec::new();
            for r in 0..self.skin_ref.max(0) as usize {
                fam.push(b.i16(si + (f * self.skin_ref as usize + r) * 2)?);
            }
            self.skins.push(fam);
        }

        // body parts
        let (n, idx) = cnt(232)?;
        for i in 0..n {
            let o = idx + i * 16;
            let nmodels = b.i32(o + 4)?.max(0) as usize;
            let mi = add_off(o, b.i32(o + 12)?)?;
            let mut bp = BodyPart { name: b.rel_str(o, 0)?, base: b.i32(o + 8)?, models: Vec::new() };
            for k in 0..nmodels.min(1024) {
                let mo = mi + k * 148;
                let nmesh = b.i32(mo + 72)?.max(0) as usize;
                let meshi = add_off(mo, b.i32(mo + 76)?)?;
                let vertexindex = b.i32(mo + 84)?;
                let mut model = Model {
                    name: b.fixed_str(mo, 64)?,
                    kind: b.i32(mo + 64)?,
                    radius: b.f32(mo + 68)?,
                    num_vertices: b.i32(mo + 80)?,
                    first_vertex: vertexindex / 48,
                    meshes: Vec::new(),
                    eyeballs: Vec::new(),
                };
                for me in 0..nmesh.min(4096) {
                    let p = meshi + me * 116;
                    let nflex = b.i32(p + 16)?.max(0) as usize;
                    let mut mesh = Mesh {
                        material: b.i32(p)?,
                        num_vertices: b.i32(p + 8)?,
                        vertex_offset: b.i32(p + 12)?,
                        material_type: b.i32(p + 24)?,
                        material_param: b.i32(p + 28)?,
                        mesh_id: b.i32(p + 32)?,
                        center: v3(b.vec3(p + 36)?),
                        flexes: Vec::new(),
                    };
                    if nflex > 0 && nflex < 4096 {
                        let fo = add_off(p, b.i32(p + 20)?)?;
                        for fx in 0..nflex {
                            let q = fo + fx * 60;
                            let nverts = b.i32(q + 20)?.max(0) as usize;
                            let vi = add_off(q, b.i32(q + 24)?)?;
                            let vtype = b.u8(q + 32)?;
                            let stride = if vtype == 1 { 18 } else { 16 };
                            let fixed = self.flags & (1 << 21) != 0;
                            let scale = b.f32(392).unwrap_or(0.0);
                            let mut verts = Vec::with_capacity(nverts);
                            for vv in 0..nverts.min(200_000) {
                                let a = vi + vv * stride;
                                let rd = |k: usize| -> FResult<f32> {
                                    let raw = b.u16(a + 4 + k * 2)?;
                                    Ok(if fixed { raw as i16 as f32 * scale } else { f16_to_f32(raw) })
                                };
                                verts.push(VertAnim {
                                    index: b.u16(a)?,
                                    speed: b.u8(a + 2)?,
                                    side: b.u8(a + 3)?,
                                    delta: Vec3::new(rd(0)?, rd(1)?, rd(2)?),
                                    ndelta: Vec3::new(rd(3)?, rd(4)?, rd(5)?),
                                    wrinkle: if vtype == 1 { b.i16(a + 16)? as f32 / 32767.0 } else { 0.0 },
                                });
                            }
                            mesh.flexes.push(Flex {
                                flexdesc: b.i32(q)?,
                                target: [b.f32(q + 4)?, b.f32(q + 8)?, b.f32(q + 12)?, b.f32(q + 16)?],
                                flexpair: b.i32(q + 28)?,
                                vertanimtype: vtype,
                                verts,
                            });
                        }
                    }
                    model.meshes.push(mesh);
                }
                // eyeballs
                let ne = b.i32(mo + 100)?.max(0) as usize;
                if ne > 0 && ne < 16 {
                    let eo = add_off(mo, b.i32(mo + 104)?)?;
                    for e in 0..ne {
                        let p = eo + e * 172;
                        let i3 = |off: usize| -> FResult<[i32; 3]> { Ok([b.i32(p + off)?, b.i32(p + off + 4)?, b.i32(p + off + 8)?]) };
                        let f3 = |off: usize| -> FResult<[f32; 3]> { b.vec3(p + off) };
                        model.eyeballs.push(Eyeball {
                            name: b.rel_str(p, 0)?,
                            bone: b.i32(p + 4)?,
                            org: v3(b.vec3(p + 8)?),
                            zoffset: b.f32(p + 20)?,
                            radius: b.f32(p + 24)?,
                            up: v3(b.vec3(p + 28)?),
                            forward: v3(b.vec3(p + 40)?),
                            texture: b.i32(p + 52)?,
                            iris_scale: b.f32(p + 60)?,
                            upperflexdesc: i3(68)?,
                            lowerflexdesc: i3(80)?,
                            uppertarget: f3(92)?,
                            lowertarget: f3(104)?,
                            upperlidflexdesc: b.i32(p + 116)?,
                            lowerlidflexdesc: b.i32(p + 120)?,
                        });
                    }
                }
                bp.models.push(model);
            }
            self.bodyparts.push(bp);
        }

        // attachments
        let (n, idx) = cnt(240)?;
        for i in 0..n {
            let o = idx + i * 92;
            self.attachments.push(Attachment { name: b.rel_str(o, 0)?, flags: b.u32(o + 4)?, bone: b.i32(o + 8)?, local: b.mat3x4(o + 12)? });
        }

        // transition nodes
        let nn = b.i32(248)?.max(0) as usize;
        let nni = b.i32(256)?;
        if nni > 0 {
            for i in 0..nn.min(1024) {
                let off = b.i32(nni as usize + i * 4)?;
                self.node_names.push(b.cstr(off.max(0) as usize));
            }
        }

        // flexes
        let (n, idx) = cnt(260)?;
        for i in 0..n {
            self.flex_descs.push(b.rel_str(idx + i * 4, 0)?);
        }
        let (n, idx) = cnt(268)?;
        for i in 0..n {
            let o = idx + i * 20;
            self.flex_controllers.push(FlexController {
                kind: b.rel_str(o, 0)?,
                name: b.rel_str(o, 4)?,
                local_to_global: b.i32(o + 8)?,
                min: b.f32(o + 12)?,
                max: b.f32(o + 16)?,
            });
        }
        let (n, idx) = cnt(276)?;
        for i in 0..n {
            let o = idx + i * 12;
            let nops = b.i32(o + 4)?.max(0) as usize;
            let oi = add_off(o, b.i32(o + 8)?)?;
            let mut ops = Vec::new();
            for k in 0..nops.min(4096) {
                let p = oi + k * 8;
                ops.push((b.i32(p)?, b.i32(p + 4)?, b.f32(p + 4)?));
            }
            self.flex_rules.push(FlexRule { flex: b.i32(o)?, ops });
        }

        // ik chains
        let (n, idx) = cnt(284)?;
        for i in 0..n {
            let o = idx + i * 16;
            let nl = b.i32(o + 8)?.max(0) as usize;
            let li = add_off(o, b.i32(o + 12)?)?;
            let mut c = IkChain { name: b.rel_str(o, 0)?, linktype: b.i32(o + 4)?, links: Vec::new() };
            for k in 0..nl.min(16) {
                let p = li + k * 28;
                c.links.push((b.i32(p)?, v3(b.vec3(p + 4)?)));
            }
            self.ik_chains.push(c);
        }

        // pose params
        let (n, idx) = cnt(300)?;
        for i in 0..n {
            let o = idx + i * 20;
            self.pose_params.push(PoseParam { name: b.rel_str(o, 0)?, flags: b.i32(o + 4)?, start: b.f32(o + 8)?, end: b.f32(o + 12)?, loop_range: b.f32(o + 16)? });
        }

        self.surfaceprop = b.cstr(b.i32(308)?.max(0) as usize);
        let kvi = b.i32(312)?;
        let kvs = b.i32(316)?;
        if kvi > 0 && kvs > 0 {
            if let Ok(bytes) = b.bytes(kvi as usize, kvs as usize) {
                self.keyvalues = String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string();
            }
        }
        self.mass = b.f32(328)?;
        self.contents = b.i32(332)?;
        let (n, idx) = cnt(336)?;
        for i in 0..n {
            let o = idx + i * 8;
            self.include_models.push(IncludeModel { label: b.rel_str(o, 0)?, name: b.rel_str(o, 4)? });
        }
        let abn = b.i32(348)?;
        if abn > 0 {
            self.anim_block_name = b.cstr(abn as usize);
        }
        let (n, idx) = cnt(352)?;
        for i in 0..n {
            self.anim_blocks.push((b.i32(idx + i * 8)?, b.i32(idx + i * 8 + 4)?));
        }
        self.constdirectionallightdot = b.u8(376)?;
        self.root_lod = b.u8(377)?;
        self.num_allowed_root_lods = b.u8(378)?;
        self.vert_anim_fixed_point_scale = b.f32(392).unwrap_or(0.0);
        self.illum_position_attachment = 0;
        let h2 = b.i32(400).unwrap_or(0);
        if h2 > 0 && (h2 as usize) + 16 <= data.len() {
            let h2 = h2 as usize;
            self.illum_position_attachment = b.i32(h2 + 8).unwrap_or(0);
            self.max_eye_deflection = b.f32(h2 + 12).unwrap_or(0.0);
        }
        Ok(())
    }

    pub fn bone_index(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name.eq_ignore_ascii_case(name))
    }

    /// Material name for a mesh given the active skin family.
    pub fn mesh_texture(&self, mesh_material: i32, skin: usize) -> Option<&Texture> {
        let fam = self.skins.get(skin).or_else(|| self.skins.first());
        let idx = fam.and_then(|f| f.get(mesh_material as usize).copied()).map(|v| v as usize).unwrap_or(mesh_material.max(0) as usize);
        self.textures.get(idx)
    }

    /// Decode the local (parent-space) transforms of every bone for one frame of an animation.
    /// `ani` is the content of the external `.ani` block file when the model uses one.
    pub fn decode_frame(&self, anim: usize, frame: i32, ani: Option<&[u8]>) -> Vec<(Vec3, Quat)> {
        let ad = &self.anims[anim];
        let delta = ad.flags & STUDIO_DELTA != 0;
        let mut out: Vec<(Vec3, Quat)> = self.bones.iter().map(|b| if delta { (Vec3::ZERO, Quat::IDENTITY) } else { (b.pos, b.quat) }).collect();
        if ad.flags & STUDIO_ALLZEROS != 0 {
            return out;
        }
        let frame = frame.clamp(0, (ad.numframes - 1).max(0));
        let Some((buf, start, local_frame)) = self.anim_data_location(ad, frame, ani) else { return out };
        let b = Bin::new(buf);
        let mut p = start;
        for _guard in 0..self.bones.len() + 1 {
            let Ok(bone) = b.u8(p) else { break };
            let Ok(flags) = b.u8(p + 1) else { break };
            let Ok(next) = b.i16(p + 2) else { break };
            let bi = bone as usize;
            if bi < self.bones.len() {
                if let Ok(v) = decode_bone(&b, p, flags, &self.bones[bi], local_frame) {
                    out[bi] = v;
                }
            }
            if next == 0 {
                break;
            }
            p = (p as i64 + next as i64) as usize;
        }
        out
    }

    /// Returns (buffer, offset of first mstudioanim_t, frame within that buffer section)
    fn anim_data_location<'a>(&'a self, ad: &AnimDesc, frame: i32, ani: Option<&'a [u8]>) -> Option<(&'a [u8], usize, i32)> {
        let b = Bin::new(&self.data);
        let (block, index, local) = if ad.sectionframes > 0 {
            let section = frame / ad.sectionframes;
            let so = add_off(ad.offset, ad.sectionindex).ok()? + section as usize * 8;
            (b.i32(so).ok()?, b.i32(so + 4).ok()?, frame - section * ad.sectionframes)
        } else {
            (ad.animblock, ad.animindex, frame)
        };
        if block == 0 {
            let start = add_off(ad.offset, index).ok()?;
            Some((&self.data, start, local))
        } else {
            let ani = ani?;
            let (start, _end) = *self.anim_blocks.get(block as usize)?;
            Some((ani, (start + index) as usize, local))
        }
    }

    /// Whether the animation needs an external .ani file.
    pub fn anim_uses_block(&self, anim: usize) -> bool {
        let ad = &self.anims[anim];
        if ad.sectionframes > 0 {
            let b = Bin::new(&self.data);
            let nsec = ad.numframes / ad.sectionframes + 2;
            for s in 0..nsec {
                if let Ok(so) = add_off(ad.offset, ad.sectionindex) {
                    if b.i32(so + s as usize * 8).unwrap_or(0) != 0 {
                        return true;
                    }
                }
            }
            false
        } else {
            ad.animblock != 0
        }
    }

    /// Piecewise root motion at a given frame (position, yaw in degrees).
    pub fn movement_at(&self, anim: usize, frame: f32) -> (Vec3, f32) {
        let ad = &self.anims[anim];
        let mut pos = Vec3::ZERO;
        let mut yaw = 0.0;
        let mut prev = 0.0f32;
        for mv in &ad.movements {
            let end = mv.endframe as f32;
            if end >= frame {
                let span = (end - prev).max(1e-6);
                let f = ((frame - prev) / span).clamp(0.0, 1.0);
                let total = 0.5 * (mv.v0 + mv.v1);
                let g = if total.abs() > 1e-6 { (mv.v0 * f + 0.5 * (mv.v1 - mv.v0) * f * f) / total } else { f };
                let piece = mv.position - pos;
                return (pos + piece * g, yaw + (mv.angle - yaw) * f);
            }
            prev = end;
            pos = mv.position;
            yaw = mv.angle;
        }
        (pos, yaw)
    }
}

fn read_ikrule(b: &Bin, o: usize) -> FResult<IkRule> {
    // mstudioikrule_t (152 bytes)
    let q = b.vec4(o + 44)?;
    Ok(IkRule {
        index: b.i32(o)?,
        kind: b.i32(o + 4)?,
        chain: b.i32(o + 8)?,
        bone: b.i32(o + 12)?,
        slot: b.i32(o + 16)?,
        height: b.f32(o + 20)?,
        radius: b.f32(o + 24)?,
        floor: b.f32(o + 28)?,
        pos: Vec3::from_array(b.vec3(o + 32)?),
        q: Quat::from_xyzw(q[0], q[1], q[2], q[3]),
        start: b.f32(o + 76)?,
        peak: b.f32(o + 80)?,
        tail: b.f32(o + 84)?,
        end: b.f32(o + 88)?,
        contact: b.f32(o + 96)?,
        drop: b.f32(o + 100)?,
        top: b.f32(o + 104)?,
        attachment: b.rel_str(o, 120).unwrap_or_default(),
    })
}

/// mstudioanimvalue_t RLE extraction (ExtractAnimValue from bone_setup.cpp)
fn extract_anim_value(b: &Bin, mut p: usize, frame: i32, scale: f32) -> FResult<f32> {
    let mut k = frame;
    for _ in 0..100_000 {
        let valid = b.u8(p)? as i32;
        let total = b.u8(p + 1)? as i32;
        if total == 0 {
            return Ok(0.0);
        }
        if total <= k {
            k -= total;
            p += (valid as usize + 1) * 2;
            continue;
        }
        return Ok(if valid > k { b.i16(p + (k as usize + 1) * 2)? as f32 * scale } else { b.i16(p + valid as usize * 2)? as f32 * scale });
    }
    err("runaway animation value stream")
}

fn valueptr(b: &Bin, ptr: usize, frame: i32, scale: Vec3) -> FResult<Vec3> {
    let mut v = [0.0f32; 3];
    for (j, out) in v.iter_mut().enumerate() {
        let off = b.i16(ptr + j * 2)?;
        if off > 0 {
            *out = extract_anim_value(b, ptr + off as usize, frame, scale[j])?;
        }
    }
    Ok(Vec3::from_array(v))
}

pub fn quat48(b: &Bin, p: usize) -> FResult<Quat> {
    let x = b.u16(p)? as f32;
    let y = b.u16(p + 2)? as f32;
    let zw = b.u16(p + 4)?;
    let z = (zw & 0x7fff) as f32;
    let wneg = zw >> 15 != 0;
    let x = (x - 32768.0) * (1.0 / 32768.0);
    let y = (y - 32768.0) * (1.0 / 32768.0);
    let z = (z - 16384.0) * (1.0 / 16384.0);
    let mut w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
    if wneg {
        w = -w;
    }
    Ok(Quat::from_xyzw(x, y, z, w))
}

pub fn quat64(b: &Bin, p: usize) -> FResult<Quat> {
    let v = b.u64(p)?;
    let xi = (v & 0x1fffff) as f64;
    let yi = ((v >> 21) & 0x1fffff) as f64;
    let zi = ((v >> 42) & 0x1fffff) as f64;
    let wneg = (v >> 63) != 0;
    let x = ((xi - 1048576.0) * (1.0 / 1048576.5)) as f32;
    let y = ((yi - 1048576.0) * (1.0 / 1048576.5)) as f32;
    let z = ((zi - 1048576.0) * (1.0 / 1048576.5)) as f32;
    let mut w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
    if wneg {
        w = -w;
    }
    Ok(Quat::from_xyzw(x, y, z, w))
}

/// RadianEuler (x=roll, y=pitch, z=yaw) -> quaternion, matching Source's AngleQuaternion.
pub fn euler_to_quat(e: Vec3) -> Quat {
    Quat::from_euler(glam::EulerRot::ZYX, e.z, e.y, e.x)
}

/// Quaternion -> RadianEuler (x, y, z) matching Source's QuaternionAngles.
pub fn quat_to_euler(q: Quat) -> Vec3 {
    // computed in f64: near pitch = +-90 degrees f32 loses ~0.07 degrees
    let d = glam::DQuat::from_xyzw(q.x as f64, q.y as f64, q.z as f64, q.w as f64).normalize();
    let (z, y, x) = d.to_euler(glam::EulerRot::ZYX);
    Vec3::new(x as f32, y as f32, z as f32)
}

fn decode_bone(b: &Bin, p: usize, flags: u8, bone: &Bone, frame: i32) -> FResult<(Vec3, Quat)> {
    let data = p + 4;
    let delta = flags & STUDIO_ANIM_DELTA != 0;
    let q = if flags & STUDIO_ANIM_RAWROT != 0 {
        quat48(b, data)?
    } else if flags & STUDIO_ANIM_RAWROT2 != 0 {
        quat64(b, data)?
    } else if flags & STUDIO_ANIM_ANIMROT != 0 {
        let mut ang = valueptr(b, data, frame, bone.rotscale)?;
        if !delta {
            ang += bone.rot;
        }
        euler_to_quat(ang)
    } else if delta {
        Quat::IDENTITY
    } else {
        bone.quat
    };
    let pos_off = data
        + if flags & STUDIO_ANIM_ANIMROT != 0 { 6 } else { 0 }
        + if flags & STUDIO_ANIM_RAWROT != 0 { 6 } else { 0 }
        + if flags & STUDIO_ANIM_RAWROT2 != 0 { 8 } else { 0 };
    let pos = if flags & STUDIO_ANIM_RAWPOS != 0 {
        Vec3::new(f16_to_f32(b.u16(pos_off)?), f16_to_f32(b.u16(pos_off + 2)?), f16_to_f32(b.u16(pos_off + 4)?))
    } else if flags & STUDIO_ANIM_ANIMPOS != 0 {
        let mut v = valueptr(b, pos_off, frame, bone.posscale)?;
        if !delta {
            v += bone.pos;
        }
        v
    } else if delta {
        Vec3::ZERO
    } else {
        bone.pos
    };
    Ok((pos, q))
}

// ---------------------------------------------------------------------------
// VVD
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex {
    pub weights: [f32; 3],
    pub bones: [u8; 3],
    pub num_bones: u8,
    pub pos: Vec3,
    pub normal: Vec3,
    pub uv: [f32; 2],
}

#[derive(Clone, Debug, Default)]
pub struct Vvd {
    pub checksum: i32,
    pub num_lods: i32,
    /// LOD0 vertices with fixups applied
    pub vertices: Vec<Vertex>,
    pub tangents: Vec<[f32; 4]>,
}

impl Vvd {
    pub fn parse(data: &[u8]) -> FResult<Vvd> {
        Self::parse_lod(data, 0)
    }

    pub fn parse_lod(data: &[u8], lod: i32) -> FResult<Vvd> {
        let b = Bin::new(data);
        if b.bytes(0, 4)? != b"IDSV" {
            return err("not a VVD file");
        }
        let checksum = b.i32(8)?;
        let num_lods = b.i32(12)?;
        let lod0_count = b.i32(16)?.max(0) as usize;
        let num_fixups = b.i32(48)?.max(0) as usize;
        let fixup_start = b.i32(52)? as usize;
        let vstart = b.i32(56)? as usize;
        let tstart = b.i32(60)? as usize;
        let read_v = |i: usize| -> FResult<Vertex> {
            let o = vstart + i * 48;
            Ok(Vertex {
                weights: [b.f32(o)?, b.f32(o + 4)?, b.f32(o + 8)?],
                bones: [b.u8(o + 12)?, b.u8(o + 13)?, b.u8(o + 14)?],
                num_bones: b.u8(o + 15)?,
                pos: v3(b.vec3(o + 16)?),
                normal: v3(b.vec3(o + 28)?),
                uv: [b.f32(o + 40)?, b.f32(o + 44)?],
            })
        };
        let read_t = |i: usize| -> [f32; 4] { b.vec4(tstart + i * 16).unwrap_or([1.0, 0.0, 0.0, 1.0]) };
        let mut vertices = Vec::new();
        let mut tangents = Vec::new();
        if num_fixups == 0 {
            let count = b.i32(16 + lod.clamp(0, 7) as usize * 4)?.max(0) as usize;
            let count = if lod == 0 { lod0_count } else { count };
            for i in 0..count {
                vertices.push(read_v(i)?);
                tangents.push(read_t(i));
            }
        } else {
            for f in 0..num_fixups {
                let o = fixup_start + f * 12;
                let flod = b.i32(o)?;
                let src = b.i32(o + 4)?.max(0) as usize;
                let n = b.i32(o + 8)?.max(0) as usize;
                if flod >= lod {
                    for i in 0..n {
                        vertices.push(read_v(src + i)?);
                        tangents.push(read_t(src + i));
                    }
                }
            }
        }
        Ok(Vvd { checksum, num_lods, vertices, tangents })
    }
}

// ---------------------------------------------------------------------------
// VTX (.dx90.vtx)
// ---------------------------------------------------------------------------

/// Triangles of one mesh, as indices into the mesh's vertex range
/// (mesh-local vertex ids: add model.first_vertex + mesh.vertex_offset).
#[derive(Clone, Debug, Default)]
pub struct VtxMesh {
    pub tris: Vec<[u16; 3]>,
}

#[derive(Clone, Debug, Default)]
pub struct VtxModel {
    /// lods[lod][mesh]
    pub lods: Vec<Vec<VtxMesh>>,
    pub switch_points: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct Vtx {
    pub checksum: i32,
    pub num_lods: i32,
    /// bodyparts[bp][model]
    pub bodyparts: Vec<Vec<VtxModel>>,
}

impl Vtx {
    pub fn parse(data: &[u8]) -> FResult<Vtx> {
        // Try the SDK 2013 strip-group layout first, then the newer (CS:GO-era) one.
        match Self::parse_with(data, false) {
            Ok(v) => Ok(v),
            Err(e) => Self::parse_with(data, true).map_err(|_| e),
        }
    }

    fn parse_with(data: &[u8], topology: bool) -> FResult<Vtx> {
        let b = Bin::new(data);
        let version = b.i32(0)?;
        if version != 7 {
            return err(format!("unsupported VTX version {version}"));
        }
        let checksum = b.i32(16)?;
        let num_lods = b.i32(20)?;
        let nbp = b.i32(28)?.max(0) as usize;
        let bpo = b.i32(32)? as usize;
        let sg_size = if topology { 33 } else { 25 };
        let strip_size = if topology { 35 } else { 27 };
        let mut bodyparts = Vec::new();
        for bp in 0..nbp {
            let bpp = bpo + bp * 8;
            let nm = b.i32(bpp)?.max(0) as usize;
            let mo = add_off(bpp, b.i32(bpp + 4)?)?;
            let mut models = Vec::new();
            for m in 0..nm {
                let mp = mo + m * 8;
                let nl = b.i32(mp)?.max(0) as usize;
                let lo = add_off(mp, b.i32(mp + 4)?)?;
                let mut vm = VtxModel::default();
                for l in 0..nl.min(8) {
                    let lp = lo + l * 12;
                    let nmesh = b.i32(lp)?.max(0) as usize;
                    let meo = add_off(lp, b.i32(lp + 4)?)?;
                    vm.switch_points.push(b.f32(lp + 8)?);
                    let mut meshes = Vec::new();
                    for me in 0..nmesh {
                        let mep = meo + me * 9;
                        let nsg = b.i32(mep)?.max(0) as usize;
                        let sgo = add_off(mep, b.i32(mep + 4)?)?;
                        let mut tris = Vec::new();
                        for sg in 0..nsg {
                            let sgp = sgo + sg * sg_size;
                            let nverts = b.i32(sgp)?.max(0) as usize;
                            let vo = add_off(sgp, b.i32(sgp + 4)?)?;
                            let nidx = b.i32(sgp + 8)?.max(0) as usize;
                            let io = add_off(sgp, b.i32(sgp + 12)?)?;
                            let nstrips = b.i32(sgp + 16)?.max(0) as usize;
                            let so = add_off(sgp, b.i32(sgp + 20)?)?;
                            // sanity check
                            if nverts > 1_000_000 || nidx > 10_000_000 {
                                return err("corrupt VTX strip group");
                            }
                            let vert_orig = |i: usize| -> FResult<u16> { b.u16(vo + i * 9 + 4) };
                            for s in 0..nstrips {
                                let sp = so + s * strip_size;
                                let sni = b.i32(sp)?.max(0) as usize;
                                let sio = b.i32(sp + 4)?.max(0) as usize;
                                let flags = b.u8(sp + 18)?;
                                // indices address the strip group's vertex list directly
                                // (a strip's vertOffset is not added; studiorender does the same)
                                let idx_at = |k: usize| -> FResult<u16> {
                                    let vi = b.u16(io + (sio + k) * 2)? as usize;
                                    if vi >= nverts {
                                        return err("VTX index out of range");
                                    }
                                    vert_orig(vi)
                                };
                                if flags & 2 != 0 {
                                    // triangle strip
                                    for k in 0..sni.saturating_sub(2) {
                                        let (a, bb, c) = (idx_at(k)?, idx_at(k + 1)?, idx_at(k + 2)?);
                                        if a == bb || bb == c || a == c {
                                            continue;
                                        }
                                        if k % 2 == 0 { tris.push([a, bb, c]) } else { tris.push([bb, a, c]) }
                                    }
                                } else {
                                    for k in (0..sni / 3 * 3).step_by(3) {
                                        tris.push([idx_at(k)?, idx_at(k + 1)?, idx_at(k + 2)?]);
                                    }
                                }
                            }
                        }
                        meshes.push(VtxMesh { tris });
                    }
                    vm.lods.push(meshes);
                }
                models.push(vm);
            }
            bodyparts.push(models);
        }
        Ok(Vtx { checksum, num_lods, bodyparts })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn euler_roundtrip() {
        let e = Vec3::new(0.3, -0.7, 1.2);
        let q = euler_to_quat(e);
        let e2 = quat_to_euler(q);
        assert!((e - e2).length() < 1e-4, "{e:?} {e2:?}");
        // Source AngleMatrix: x-axis column = (cp*cy, cp*sy, -sp)
        let m = glam::Mat3::from_quat(q);
        let (sp, cp) = e.y.sin_cos();
        let (sy, cy) = e.z.sin_cos();
        assert!((m.x_axis - Vec3::new(cp * cy, cp * sy, -sp)).length() < 1e-5);
    }
}
