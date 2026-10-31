// Auto rig from layer names. Each layer of a paint document is sorted into
// a face part by its own name, then by its folders' names (English,
// Japanese and Korean words), and Cubism's standard rig is built over the
// parts with the editor's own operations (`ayatsuri_core::edit`), so the
// result opens in the editor as an ordinary model to keep rigging.
//
// The tree, parents first:
//
//   WarpBody (ParamBodyAngleX, ParamBreath)
//     body meshes
//     RotHead (ParamAngleZ), pivot at the bottom of the face
//       WarpHead (ParamAngleX x ParamAngleY)
//         face meshes
//         WarpEyeL / WarpEyeR (ParamEyeLOpen / ParamEyeROpen)
//         WarpBrowL / WarpBrowR (ParamBrowLY / ParamBrowRY)
//         WarpMouth (ParamMouthOpenY x ParamMouthForm)
//         WarpHairFront / Side / Back (ParamHair*, swung by physics)
//
// L and R are the character's own: the left eye is the one on the
// viewer's right. PSD models are in canvas-height units, y down.

use ayatsuri_core::edit::{self, Child, NewDeformer, Target};
use ayatsuri_format::model::{
    Binding, DeformerKind, Model, Parameter, PhysicsGroup, PhysicsInput, PhysicsIo, PhysicsNode,
    PhysicsOutput, PhysicsRange, PhysicsRig,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Face,
    EyeL,
    EyeR,
    BrowL,
    BrowR,
    Mouth,
    HairFront,
    HairSide,
    HairBack,
    Body,
    WingL,
    WingR,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Face => "face",
            Role::EyeL => "eye L",
            Role::EyeR => "eye R",
            Role::BrowL => "brow L",
            Role::BrowR => "brow R",
            Role::Mouth => "mouth",
            Role::HairFront => "hair front",
            Role::HairSide => "hair side",
            Role::HairBack => "hair back",
            Role::Body => "body",
            Role::WingL => "wing L",
            Role::WingR => "wing R",
        }
    }
}

/// What a name says, before eyes and brows get their side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Brow,
    Eye,
    Mouth,
    Hair,
    Face,
    Wing,
    Body,
}

/// Checked in this order: lashes first (속눈썹 holds 눈썹), then brows
/// before eyes (eyebrow, 눈썹 hold eye words), hair before face (머리카락
/// holds 머리, head), wings before body ("body wing L" is a wing). ASCII
/// words of four letters or more match inside a word ("eyebrows",
/// "hairback"), shorter ones and those in [`AT_START`] only at a word's
/// start ("eyelid", "armor", "wings"), so "ear" is not found in "pearl",
/// "arm" in "warm" nor "wing" in "drawing".
const WORDS: &[(Kind, &[&str])] = &[
    (Kind::Eye, &["속눈썹", "まつ毛", "まつげ"]),
    (Kind::Brow, &["brow", "眉", "まゆ", "눈썹"]),
    (
        Kind::Eye,
        &["eye", "iris", "pupil", "lash", "sclera", "目", "瞳", "睫", "瞼", "まぶた", "눈"],
    ),
    (Kind::Mouth, &["mouth", "lip", "teeth", "tongue", "口", "唇", "歯", "舌", "입", "혀", "치아"]),
    (Kind::Hair, &["hair", "bang", "髪", "もみあげ", "머리카락", "앞머리", "옆머리", "뒷머리"]),
    (
        Kind::Face,
        &[
            "face", "head", "nose", "ear", "cheek", "blush", "顔", "輪郭", "鼻", "耳", "頬",
            "얼굴", "머리", "코", "귀", "볼",
        ],
    ),
    (Kind::Wing, &["wing", "翼", "羽", "날개"]),
    (
        Kind::Body,
        &[
            "body", "neck", "torso", "arm", "hand", "leg", "cloth", "shirt", "chest", "体", "首",
            "腕", "手", "服", "胸", "몸", "목", "팔", "손", "옷", "다리", "가슴",
        ],
    ),
];

const HAIR_SIDE: &[&str] = &["side", "横", "もみあげ", "옆"];
const HAIR_BACK: &[&str] = &["back", "rear", "後", "うしろ", "뒤", "뒷"];

/// Lowercase ASCII words, camelCase split ("HairBack" is "hair", "back").
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in text.chars() {
        if !ch.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if ch.is_ascii_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = ch.is_ascii_lowercase();
        cur.push(ch.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Long words that still only match at a word's start.
const AT_START: &[&str] = &["wing"];

fn has_word(text: &str, toks: &[String], word: &str) -> bool {
    if !word.is_ascii() {
        return text.contains(word);
    }
    let inside = word.len() >= 4 && !AT_START.contains(&word);
    toks.iter().any(|t| if inside { t.contains(word) } else { t.starts_with(word) })
}

fn kind_of_text(text: &str) -> Option<Kind> {
    let toks = tokens(text);
    WORDS.iter().find(|(_, words)| words.iter().any(|w| has_word(text, &toks, w))).map(|&(k, _)| k)
}

fn hair_role(text: &str) -> Role {
    let toks = tokens(text);
    if HAIR_SIDE.iter().any(|w| has_word(text, &toks, w)) {
        Role::HairSide
    } else if HAIR_BACK.iter().any(|w| has_word(text, &toks, w)) {
        Role::HairBack
    } else {
        Role::HairFront
    }
}

/// What an eye layer is, so it shuts the way riggers shut it: the upper
/// lash comes down onto the lower one, the eyeball (white, iris,
/// highlight) folds away under it, the lower lash stays. A layer that says
/// none of these (an eye drawn as one piece) is squashed whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EyePart {
    UpperLash,
    LowerLash,
    Ball,
    Other,
}

impl EyePart {
    pub fn name(self) -> &'static str {
        match self {
            EyePart::UpperLash => "upper lash",
            EyePart::LowerLash => "lower lash",
            EyePart::Ball => "eyeball",
            EyePart::Other => "",
        }
    }
}

const LASH: &[&str] =
    &["lash", "lid", "まつ毛", "まつげ", "睫", "瞼", "まぶた", "속눈썹", "눈꺼풀", "아이라인"];
const LOWER: &[&str] = &["lower", "bottom", "under", "下", "아래", "밑"];
const BALL: &[&str] = &[
    "white",
    "sclera",
    "iris",
    "pupil",
    "highlight",
    "白目",
    "瞳",
    "黒目",
    "ハイライト",
    "흰자",
    "눈동자",
    "동공",
    "하이라이트",
];

/// The first name that says which part, layer name before folders.
fn eye_part(names: &[String]) -> EyePart {
    let says = |text: &str, words: &[&str]| {
        let toks = tokens(text);
        words.iter().any(|w| has_word(text, &toks, w))
    };
    for n in names {
        if says(n, LASH) {
            return if says(n, LOWER) { EyePart::LowerLash } else { EyePart::UpperLash };
        }
        if says(n, BALL) {
            return EyePart::Ball;
        }
    }
    EyePart::Other
}

/// Each eye layer's part (from [`roles`]); `Other` for every other layer.
pub fn eye_parts(
    model: &Model,
    roles: &[Role],
    mesh_names: &[String],
    part_names: &[String],
) -> Vec<EyePart> {
    roles
        .iter()
        .enumerate()
        .map(|(i, r)| match r {
            Role::EyeL | Role::EyeR => eye_part(&names_of(model, i, mesh_names, part_names)),
            _ => EyePart::Other,
        })
        .collect()
}

/// The layer's name, then its folders from the innermost out.
fn names_of(
    model: &Model,
    mesh: usize,
    mesh_names: &[String],
    part_names: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = mesh_names.get(mesh).cloned().into_iter().collect();
    let mut part = model.art_meshes.get(mesh).and_then(|m| m.parent_part);
    let mut hops = 0;
    while let Some(p) = part
        && hops < 64
    {
        out.extend(part_names.get(p as usize).cloned());
        part = model.parts.get(p as usize).and_then(|x| x.parent_part);
        hops += 1;
    }
    out
}

/// The first name that says something, with the name that said it.
fn classify(names: &[String]) -> Option<(Kind, &str)> {
    names.iter().find_map(|n| kind_of_text(n).map(|k| (k, n.as_str())))
}

type Box4 = [f32; 4];

fn mesh_box(model: &Model, mesh: usize) -> Option<Box4> {
    let m = model.art_meshes.get(mesh)?;
    let n = (m.vertex_count as usize).saturating_mul(2);
    let (pairs, _) = m.positions.get(..n)?.as_chunks::<2>();
    pairs.iter().fold(None, |b, &[x, y]| Some(grow(b, [x, y, x, y])))
}

fn grow(b: Option<Box4>, [x0, y0, x1, y1]: Box4) -> Box4 {
    match b {
        None => [x0, y0, x1, y1],
        Some([a, b, c, d]) => [a.min(x0), b.min(y0), c.max(x1), d.max(y1)],
    }
}

/// Every layer's role: what its names say, eyes and brows sided by where
/// they sit on the face, and nameless layers by whether they sit on it.
pub fn roles(model: &Model, mesh_names: &[String], part_names: &[String]) -> Vec<Role> {
    let n = model.art_meshes.len();
    let said: Vec<Option<(Kind, String)>> = (0..n)
        .map(|i| {
            let names = names_of(model, i, mesh_names, part_names);
            classify(&names).map(|(k, t)| (k, t.to_string()))
        })
        .collect();
    let boxes: Vec<Option<Box4>> = (0..n).map(|i| mesh_box(model, i)).collect();
    let union = |pick: &dyn Fn(Kind) -> bool| {
        said.iter().zip(&boxes).fold(None, |acc, (s, b)| match (s, b) {
            (Some((k, _)), Some(b)) if pick(*k) => Some(grow(acc, *b)),
            _ => acc,
        })
    };
    let face = union(&|k| k == Kind::Face)
        .or_else(|| union(&|k| matches!(k, Kind::Eye | Kind::Brow | Kind::Mouth)));
    let cx = face.map_or(0.0, |[x0, _, x1, _]| (x0 + x1) * 0.5);
    // a pair splits at its own middle: a face drawn turned is not centred
    // between its eyes. One eye alone is sided by the face.
    let split = |k: Kind| {
        let widest = said
            .iter()
            .zip(&boxes)
            .filter_map(|(s, b)| match (s, b) {
                (Some((x, _)), Some([x0, _, x1, _])) if *x == k => Some(x1 - x0),
                _ => None,
            })
            .fold(0.0f32, f32::max);
        match union(&|x| x == k) {
            Some([x0, _, x1, _]) if x1 - x0 > widest * 1.5 => (x0 + x1) * 0.5,
            _ => cx,
        }
    };
    let (eye_x, brow_x) = (split(Kind::Eye), split(Kind::Brow));
    let eye_box = |left: bool| {
        said.iter().zip(&boxes).fold(None, |acc, (s, b)| match (s, b) {
            (Some((Kind::Eye, _)), Some(b)) if ((b[0] + b[2]) * 0.5 > eye_x) == left => {
                Some(grow(acc, *b))
            }
            _ => acc,
        })
    };
    let (eye_l, eye_r) = (eye_box(true), eye_box(false));
    let holds = |b: Option<Box4>, x: f32, y: f32| {
        b.is_some_and(|[x0, y0, x1, y1]| x >= x0 && x <= x1 && y >= y0 && y <= y1)
    };
    said.iter()
        .zip(&boxes)
        .map(|(s, b)| {
            let mid_x = b.map_or(cx, |[x0, _, x1, _]| (x0 + x1) * 0.5);
            let mid_y = b.map_or(0.0, |[_, y0, _, y1]| (y0 + y1) * 0.5);
            match s {
                Some((Kind::Eye, _)) if mid_x > eye_x => Role::EyeL,
                Some((Kind::Eye, _)) => Role::EyeR,
                Some((Kind::Brow, _)) if mid_x > brow_x => Role::BrowL,
                Some((Kind::Brow, _)) => Role::BrowR,
                Some((Kind::Mouth, _)) => Role::Mouth,
                Some((Kind::Hair, t)) => hair_role(t),
                Some((Kind::Face, _)) => Role::Face,
                Some((Kind::Wing, _)) if mid_x > cx => Role::WingL,
                Some((Kind::Wing, _)) => Role::WingR,
                Some((Kind::Body, _)) => Role::Body,
                // no word for it ("highlight", "sigil"): an eye's if it
                // sits in one, else the face's if it sits on the face
                None if holds(eye_l, mid_x, mid_y) => Role::EyeL,
                None if holds(eye_r, mid_x, mid_y) => Role::EyeR,
                None if holds(face, mid_x, mid_y) => Role::Face,
                None => Role::Body,
            }
        })
        .collect()
}

/// id, min, default, max, and the roles it moves: made only when one of
/// them is drawn, so no parameter is left dead. Live2D's Standard
/// Parameter List, plus ParamWing. Face, eye and mouth ones are always
/// made: trackers drive them and model3.json's EyeBlink and LipSync
/// groups name them.
const PARAMS: &[(&str, f32, f32, f32, &[Role])] = &[
    ("ParamAngleX", -30.0, 0.0, 30.0, &[]),
    ("ParamAngleY", -30.0, 0.0, 30.0, &[]),
    ("ParamAngleZ", -30.0, 0.0, 30.0, &[]),
    ("ParamEyeLOpen", 0.0, 1.0, 1.0, &[]),
    ("ParamEyeROpen", 0.0, 1.0, 1.0, &[]),
    ("ParamBrowLY", -1.0, 0.0, 1.0, &[Role::BrowL]),
    ("ParamBrowRY", -1.0, 0.0, 1.0, &[Role::BrowR]),
    ("ParamMouthForm", -1.0, 0.0, 1.0, &[]),
    ("ParamMouthOpenY", 0.0, 0.0, 1.0, &[]),
    ("ParamBodyAngleX", -10.0, 0.0, 10.0, &[]),
    ("ParamBreath", 0.0, 0.0, 1.0, &[]),
    ("ParamHairFront", -1.0, 0.0, 1.0, &[Role::HairFront]),
    ("ParamHairSide", -1.0, 0.0, 1.0, &[Role::HairSide]),
    ("ParamHairBack", -1.0, 0.0, 1.0, &[Role::HairBack]),
    ("ParamWing", -1.0, 0.0, 1.0, &[Role::WingL, Role::WingR]),
];

/// role, warp id, rows, cols
const FEATURES: &[(Role, &str, u32, u32)] = &[
    (Role::EyeL, "WarpEyeL", 2, 2),
    (Role::EyeR, "WarpEyeR", 2, 2),
    (Role::BrowL, "WarpBrowL", 1, 2),
    (Role::BrowR, "WarpBrowR", 1, 2),
    (Role::Mouth, "WarpMouth", 2, 4),
    (Role::HairFront, "WarpHairFront", 3, 3),
    (Role::HairSide, "WarpHairSide", 3, 3),
    (Role::HairBack, "WarpHairBack", 3, 3),
    (Role::WingL, "WarpWingL", 3, 3),
    (Role::WingR, "WarpWingR", 3, 3),
];

/// Wing warps hang on the body: a lean carries them, a head turn does not.
const WINGS: &[&str] = &["WarpWingL", "WarpWingR"];

fn param(model: &Model, id: &str) -> Option<usize> {
    model.parameters.iter().position(|p| p.id == id)
}

fn deformer(model: &Model, id: &str) -> Option<usize> {
    model.deformers.iter().position(|d| d.id == id)
}

/// Each axis's value at keyform `k` (first axis fastest), in `params` order.
fn values_at(binding: &Binding, params: &[usize], k: usize) -> Vec<f32> {
    let mut at = vec![0.0; params.len()];
    let mut rest = k;
    for a in &binding.axes {
        let len = a.keys.len().max(1);
        let v = a.keys.get(rest % len).copied().unwrap_or(0.0);
        rest /= len;
        if let Some(slot) =
            params.iter().position(|&p| p == a.parameter as usize).and_then(|i| at.get_mut(i))
        {
            *slot = v;
        }
    }
    at
}

/// Bind deformer `di` to `axes` and return each keyform's values, in
/// `axes` order. The new keyforms all start as the rest form.
fn key(model: &mut Model, di: usize, axes: &[(&str, &[f32])]) -> Option<Vec<Vec<f32>>> {
    let mut params = Vec::with_capacity(axes.len());
    for (id, keys) in axes {
        let p = param(model, id)?;
        edit::add_keys(model, Target::Deformer(di), p, keys);
        params.push(p);
    }
    let binding = &model.deformers.get(di)?.binding;
    Some((0..binding.keyform_count()).map(|k| values_at(binding, &params, k)).collect())
}

/// Key warp `id` on `axes` and shape every keyform: `f` gets the keyform's
/// values (in `axes` order) and a grid point's (u, v) across the grid, 0 to
/// 1, and says how far the point moves in grid widths and heights.
fn shape_warp(
    model: &mut Model,
    id: &str,
    axes: &[(&str, &[f32])],
    f: impl Fn(&[f32], f32, f32) -> (f32, f32),
) -> Option<()> {
    let di = deformer(model, id)?;
    let values = key(model, di, axes)?;
    let DeformerKind::Warp(w) = &mut model.deformers.get_mut(di)?.kind else { return None };
    let (rows, cols) = (w.rows.max(1) as usize, w.cols.max(1) as usize);
    let per = (rows + 1) * (cols + 1) * 2;
    let base = w.positions.get(..per)?.to_vec();
    let (pairs, _) = base.as_chunks::<2>();
    let [x0, y0, x1, y1] = pairs.iter().fold(None, |b, &[x, y]| Some(grow(b, [x, y, x, y])))?;
    let (bw, bh) = (x1 - x0, y1 - y0);
    for (k, vals) in values.iter().enumerate() {
        let Some(form) = w.positions.get_mut(k * per..(k + 1) * per) else { continue };
        let (points, _) = form.as_chunks_mut::<2>();
        for (i, (p, &[bx, by])) in points.iter_mut().zip(pairs).enumerate() {
            let u = (i % (cols + 1)) as f32 / cols as f32;
            let v = (i / (cols + 1)) as f32 / rows as f32;
            let (dx, dy) = f(vals, u, v);
            *p = [bx + dx * bw, by + dy * bh];
        }
    }
    Some(())
}

/// Key rotation `id` on `axes`, `f` giving each keyform's angle in degrees.
fn shape_rotation(
    model: &mut Model,
    id: &str,
    axes: &[(&str, &[f32])],
    f: impl Fn(&[f32]) -> f32,
) -> Option<()> {
    let di = deformer(model, id)?;
    let values = key(model, di, axes)?;
    let DeformerKind::Rotation(r) = &mut model.deformers.get_mut(di)?.kind else { return None };
    for (a, vals) in r.angles.iter_mut().zip(&values) {
        *a = f(vals);
    }
    Some(())
}

/// Key art mesh `mi` on parameter `id` at `keys` and shape every keyform:
/// `f` gets the key and the rest form's points, and returns the keyform's
/// points and opacity.
fn shape_mesh(
    model: &mut Model,
    mi: usize,
    id: &str,
    keys: &[f32],
    f: impl Fn(f32, &[[f32; 2]]) -> (Vec<[f32; 2]>, f32),
) -> Option<()> {
    let p = param(model, id)?;
    edit::add_keys(model, Target::ArtMesh(mi), p, keys);
    let m = model.art_meshes.get_mut(mi)?;
    let values: Vec<f32> = (0..m.binding.keyform_count())
        .map(|k| values_at(&m.binding, &[p], k).first().copied().unwrap_or(0.0))
        .collect();
    let n = (m.vertex_count as usize).saturating_mul(2);
    let base = m.positions.get(..n)?.to_vec();
    let (rest, _) = base.as_chunks::<2>();
    for (k, &v) in values.iter().enumerate() {
        let (points, opacity) = f(v, rest);
        if let Some(form) = m.positions.get_mut(k * n..(k + 1) * n) {
            for (dst, src) in form.as_chunks_mut::<2>().0.iter_mut().zip(&points) {
                *dst = *src;
            }
        }
        if let Some(o) = m.opacities.get_mut(k) {
            *o = opacity;
        }
    }
    Some(())
}

/// Shut each eye by its parts (see [`EyePart`]), on keys 0, 0.5 and 1 so
/// the eyeball stays solid until the lids are half way.
fn shut_eyes(model: &mut Model, roles: &[Role], parts: &[EyePart]) {
    for (side, id) in [(Role::EyeL, "ParamEyeLOpen"), (Role::EyeR, "ParamEyeROpen")] {
        let eye: Vec<usize> =
            roles.iter().enumerate().filter(|&(_, &r)| r == side).map(|(i, _)| i).collect();
        let part = |i: usize| parts.get(i).copied().unwrap_or(EyePart::Other);
        let union = |model: &Model, pick: &dyn Fn(EyePart) -> bool| {
            eye.iter()
                .filter(|&&i| pick(part(i)))
                .filter_map(|&i| mesh_box(model, i))
                .fold(None, |b, m| Some(grow(b, m)))
        };
        let Some([_, y0, _, y1]) = union(model, &|_| true) else { continue };
        // the lids meet on the lower lash, else 60% down the eye
        let line = union(model, &|p| p == EyePart::LowerLash)
            .map_or(y0 + (y1 - y0) * 0.6, |[_, a, _, b]| (a + b) * 0.5);
        let squash = move |pts: &[[f32; 2]], by: f32| -> Vec<[f32; 2]> {
            pts.iter().map(|&[x, y]| [x, line + (y - line) * (1.0 - by)]).collect()
        };
        for &mi in &eye {
            let keys = [0.0, 0.5, 1.0];
            match part(mi) {
                EyePart::LowerLash => {}
                EyePart::UpperLash => {
                    shape_mesh(model, mi, id, &keys, |v, pts| {
                        let shut = 1.0 - v;
                        let (lo, hi) = pts
                            .iter()
                            .fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[1]), hi.max(p[1])));
                        let cy = (lo + hi) * 0.5;
                        // drops onto the line and thins to half
                        let moved = pts
                            .iter()
                            .map(|&[x, y]| {
                                [x, cy + (y - cy) * (1.0 - 0.5 * shut) + shut * (line - cy)]
                            })
                            .collect();
                        (moved, 1.0)
                    });
                }
                EyePart::Ball => {
                    shape_mesh(model, mi, id, &keys, |v, pts| {
                        (squash(pts, 0.95 * (1.0 - v)), if v <= 0.0 { 0.0 } else { 1.0 })
                    });
                }
                EyePart::Other => {
                    shape_mesh(model, mi, id, &keys, |v, pts| (squash(pts, 0.9 * (1.0 - v)), 1.0));
                }
            }
        }
    }
}

fn hair_physics(n: usize, name: &str, output: &str) -> PhysicsGroup {
    let input = |param: &str, weight: f32, kind: PhysicsIo| PhysicsInput {
        param: param.into(),
        weight,
        kind,
        reflect: false,
    };
    let node = |y: f32, mobility: f32, delay: f32, acceleration: f32, radius: f32| PhysicsNode {
        x: 0.0,
        y,
        mobility,
        delay,
        acceleration,
        radius,
    };
    let range = PhysicsRange { min: -10.0, default: 0.0, max: 10.0 };
    PhysicsGroup {
        id: format!("PhysicsSetting{n}"),
        name: name.into(),
        inputs: vec![
            input("ParamAngleX", 60.0, PhysicsIo::X),
            input("ParamAngleZ", 60.0, PhysicsIo::Angle),
            input("ParamBodyAngleX", 40.0, PhysicsIo::X),
        ],
        outputs: vec![PhysicsOutput {
            param: output.into(),
            vertex: 1,
            scale: 1.5,
            weight: 100.0,
            kind: PhysicsIo::Angle,
            reflect: false,
        }],
        // Cubism's new-setting defaults: an anchor and one swinging node
        nodes: vec![node(0.0, 1.0, 1.0, 1.0, 0.0), node(3.0, 0.95, 0.9, 1.5, 3.0)],
        norm_position: range,
        norm_angle: range,
    }
}

/// Swap every layer's quad for an outline mesh, as Mesh Edit's Automatic
/// Mesh does, so the warps bend the art and not just its four corners.
/// `pages` are the decoded texture pages. Returns how many were meshed.
pub fn automesh(model: &mut Model, pages: &[image::RgbaImage]) -> usize {
    let params = ayatsuri_core::automesh::Params::default();
    let mut done = 0;
    for mi in 0..model.art_meshes.len() {
        let Some(m) = model.art_meshes.get(mi) else { continue };
        let Some(page) = pages.get(m.texture as usize) else { continue };
        let (uvs, _) = m.uvs.as_chunks::<2>();
        let Some([u0, v0, u1, v1]) =
            uvs.iter().fold(None, |b, &[u, v]| Some(grow(b, [u, v, u, v])))
        else {
            continue;
        };
        let (pw, ph) = (page.width(), page.height());
        // the atlas leaves 16 px clear round each layer: the outline may
        // take half of it and never reach the next layer's art
        let (gu, gv) = (8.0 / pw.max(1) as f32, 8.0 / ph.max(1) as f32);
        let room = (u0 - gu, v0 - gv, u1 + gu, v1 + gv);
        let alpha_at = |x: u32, y: u32| page.get_pixel_checked(x, y).map_or(0, |p| p.0[3]);
        let meshed = ayatsuri_core::automesh::for_patch(
            &alpha_at,
            (pw, ph),
            (u0, v0, u1, v1),
            room,
            &params,
        );
        let Some((new_uvs, indices)) = meshed else { continue };
        if edit::remesh(model, mi, &new_uvs, &indices) {
            done += 1;
        }
    }
    done
}

/// A new deformer over `children`, under their shared parent.
fn wrap(model: &mut Model, kind: NewDeformer, children: &[Child], id: &str) -> Result<(), String> {
    edit::create_deformer(model, kind, children, None, (0.0, 0.0, 0.0, 0.0), None, id.into())
        .map(|_| ())
        .ok_or_else(|| format!("could not make {id}"))
}

/// Build the rig over `roles` (one per art mesh, from [`roles`]) and the
/// eyes' `parts` (from [`eye_parts`]): the standard parameters, the
/// deformer tree, the keyforms and hair and wing physics.
pub fn rig(model: &mut Model, roles: &[Role], parts: &[EyePart]) -> Result<(), String> {
    let drawn = |any: &[Role]| any.is_empty() || roles.iter().any(|r| any.contains(r));
    for &(id, min, default, max, moves) in PARAMS {
        if drawn(moves) && param(model, id).is_none() {
            model.parameters.push(Parameter {
                id: id.into(),
                min,
                max,
                default,
                repeat: false,
                decimals: 1,
                blend_shape: false,
            });
        }
    }
    let meshes = |r: Role| -> Vec<usize> {
        roles.iter().enumerate().filter(|&(_, &x)| x == r).map(|(i, _)| i).collect()
    };
    let union = |model: &Model, pick: &dyn Fn(Role) -> bool| {
        roles
            .iter()
            .enumerate()
            .filter(|&(_, &r)| pick(r))
            .filter_map(|(i, _)| mesh_box(model, i))
            .fold(None, |b, m| Some(grow(b, m)))
    };
    // the neck: the bottom middle of the face, else of everything on it
    let neck = union(model, &|r| r == Role::Face)
        .or_else(|| union(model, &|r| r != Role::Body))
        .map(|[x0, _, x1, y1]| ((x0 + x1) * 0.5, y1));
    // long side or back hair (twintails, hair to the waist) hangs past the
    // face by more than half its height: inside the head warp a turn would
    // stretch it to the knees, so it only turns with the head's rotation
    let face = union(model, &|r| r == Role::Face);
    let long: Vec<&str> = [(Role::HairSide, "WarpHairSide"), (Role::HairBack, "WarpHairBack")]
        .into_iter()
        .filter(|&(role, _)| match (face, union(model, &|r| r == role)) {
            (Some([_, y0, _, y1]), Some([_, _, _, bottom])) => bottom > y1 + (y1 - y0) * 0.5,
            _ => false,
        })
        .map(|(_, id)| id)
        .collect();

    // the feature warps, straight over their meshes
    let mut made: Vec<&str> = Vec::new();
    for &(role, id, rows, cols) in FEATURES {
        let ms = meshes(role);
        if ms.is_empty() {
            continue;
        }
        edit::create_warp_deformer(model, &ms, rows, cols, id.into())
            .ok_or_else(|| format!("could not make {id}"))?;
        made.push(id);
    }

    // the head: a warp over the face and its features, a rotation over that
    let mut head: Vec<Child> = meshes(Role::Face).into_iter().map(Child::Mesh).collect();
    let short = made.iter().filter(|id| !long.contains(id) && !WINGS.contains(id));
    head.extend(short.filter_map(|id| deformer(model, id)).map(Child::Deformer));
    let mut body: Vec<Child> = meshes(Role::Body).into_iter().map(Child::Mesh).collect();
    if !head.is_empty() {
        wrap(model, NewDeformer::Warp { rows: 4, cols: 4 }, &head, "WarpHead")?;
        let mut turned: Vec<Child> =
            deformer(model, "WarpHead").map(Child::Deformer).into_iter().collect();
        turned.extend(long.iter().filter_map(|id| deformer(model, id)).map(Child::Deformer));
        let rotation = NewDeformer::Rotation { origin: neck };
        wrap(model, rotation, &turned, "RotHead")?;
        body.extend(deformer(model, "RotHead").map(Child::Deformer));
    }
    // looked up only now: making the head deformers shifted every index
    body.extend(WINGS.iter().filter_map(|id| deformer(model, id)).map(Child::Deformer));
    if !body.is_empty() {
        wrap(model, NewDeformer::Warp { rows: 4, cols: 4 }, &body, "WarpBody")?;
    }

    // keyforms. Moves are in grid widths and heights; up is -y.
    let pi = std::f32::consts::PI;
    let at = |v: &[f32], i: usize| v.get(i).copied().unwrap_or(0.0);
    shut_eyes(model, roles, parts);
    for (id, p) in [("WarpBrowL", "ParamBrowLY"), ("WarpBrowR", "ParamBrowRY")] {
        shape_warp(model, id, &[(p, &[-1.0, 0.0, 1.0])], |v, _, _| (0.0, -0.4 * at(v, 0)));
    }
    let mouth: &[(&str, &[f32])] =
        &[("ParamMouthOpenY", &[0.0, 1.0]), ("ParamMouthForm", &[-1.0, 0.0, 1.0])];
    shape_warp(model, "WarpMouth", mouth, |v, x, y| {
        let open = at(v, 0) * 0.8 * y; // the jaw drops, the top lip stays
        let smile = -at(v, 1) * 0.25 * ((2.0 * x - 1.0).powi(2) - 0.4); // corners up
        (0.0, open + smile)
    });
    for (id, p) in [
        ("WarpHairFront", "ParamHairFront"),
        ("WarpHairSide", "ParamHairSide"),
        ("WarpHairBack", "ParamHairBack"),
    ] {
        // rooted at the top, the tips swing most
        shape_warp(model, id, &[(p, &[-1.0, 0.0, 1.0])], |v, _, y| (0.12 * at(v, 0) * y * y, 0.0));
    }
    // wings flap as a pair: the root by the body stays, the far tip rises
    // and tucks in. The left wing is on the viewer's right, its root at u 0.
    for (id, root_at_left) in [("WarpWingL", true), ("WarpWingR", false)] {
        shape_warp(model, id, &[("ParamWing", &[-1.0, 0.0, 1.0])], |v, x, _| {
            let reach = if root_at_left { x } else { 1.0 - x };
            let tuck = if root_at_left { -1.0 } else { 1.0 };
            let f = at(v, 0) * reach * reach;
            (tuck * 0.04 * f, -0.12 * f)
        });
    }
    let turn: &[(&str, &[f32])] =
        &[("ParamAngleX", &[-30.0, 0.0, 30.0]), ("ParamAngleY", &[-30.0, 0.0, 30.0])];
    shape_warp(model, "WarpHead", turn, |v, x, y| {
        let (tx, ty) = (at(v, 0) / 30.0, at(v, 1) / 30.0);
        // the middle of the face travels further than its edges
        let dx = tx * (0.03 + 0.08 * (pi * x).sin());
        let dy = -ty * (0.02 + 0.06 * (pi * y).sin());
        (dx, dy)
    });
    shape_rotation(model, "RotHead", &[("ParamAngleZ", &[-30.0, 0.0, 30.0])], |v| -at(v, 0) / 3.0);
    let lean: &[(&str, &[f32])] =
        &[("ParamBodyAngleX", &[-10.0, 0.0, 10.0]), ("ParamBreath", &[0.0, 1.0])];
    shape_warp(model, "WarpBody", lean, |v, _, y| {
        let lean = 0.06 * at(v, 0) / 10.0 * (1.0 - y); // the hips stay put
        let breath = -0.015 * at(v, 1) * (1.0 - y);
        (lean, breath)
    });

    let groups: Vec<PhysicsGroup> = [
        (&[Role::HairFront][..], "Hair Front", "ParamHairFront"),
        (&[Role::HairSide], "Hair Side", "ParamHairSide"),
        (&[Role::HairBack], "Hair Back", "ParamHairBack"),
        (&[Role::WingL, Role::WingR], "Wing", "ParamWing"),
    ]
    .iter()
    .filter(|(any, _, _)| drawn(any))
    .enumerate()
    .map(|(n, &(_, name, out))| hair_physics(n + 1, name, out))
    .collect();
    if !groups.is_empty() {
        model.physics = Some(PhysicsRig {
            fps: 60.0,
            gravity_x: 0.0,
            gravity_y: -1.0,
            wind_x: 0.0,
            wind_y: 0.0,
            groups,
        });
    }
    if model.draw_groups.is_empty() {
        model.draw_groups = ayatsuri_core::draworder::synthesize(model);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayatsuri_import::{Blend, Document, Layer};

    #[test]
    fn names_sort_into_roles() {
        let k = |s: &str| kind_of_text(s);
        assert_eq!(k("Eyebrow_L"), Some(Kind::Brow));
        assert_eq!(k("눈썹"), Some(Kind::Brow));
        assert_eq!(k("eye white"), Some(Kind::Eye));
        assert_eq!(k("白目"), Some(Kind::Eye));
        assert_eq!(k("앞머리"), Some(Kind::Hair));
        assert_eq!(k("머리"), Some(Kind::Face), "머리 alone is the head");
        assert_eq!(k("pearl"), None, "ear is a whole word only");
        assert_eq!(k("background"), None);
        assert_eq!(hair_role("HairBack"), Role::HairBack);
        assert_eq!(hair_role("横髪"), Role::HairSide);
        assert_eq!(hair_role("bangs"), Role::HairFront);
        assert_eq!(k("속눈썹"), Some(Kind::Eye), "an eyelash, not a brow");
        assert_eq!(k("eyelid_L"), Some(Kind::Eye));
        assert_eq!(k("armor"), Some(Kind::Body));
        assert_eq!(k("body wing L"), Some(Kind::Wing), "a wing before the body word");
        assert_eq!(k("날개"), Some(Kind::Wing));
        assert_eq!(k("line drawing"), None, "wing only at a word's start");
        let part =
            |names: &[&str]| eye_part(&names.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(part(&["上まつ毛"]), EyePart::UpperLash);
        assert_eq!(part(&["아래 속눈썹"]), EyePart::LowerLash);
        assert_eq!(part(&["L", "白目"]), EyePart::Ball, "the folder says it");
        assert_eq!(part(&["eye_L"]), EyePart::Other);
    }

    #[test]
    fn an_eye_in_parts_shuts_onto_its_lower_lash() {
        let (model, _, _) = rigged();
        let mut puppet = ayatsuri_core::Puppet::new(model.clone());
        puppet.update();
        // mesh index -> (mean y, opacity), in canvas pixels
        let look = |p: &ayatsuri_core::Puppet| -> Vec<(usize, f32, f32)> {
            p.drawables()
                .map(|d| {
                    let (pairs, _) = d.positions.as_chunks::<2>();
                    let y = pairs.iter().map(|p| p[1]).sum::<f32>() / pairs.len().max(1) as f32;
                    (d.mesh, y * 600.0 + 300.0, d.opacity)
                })
                .collect()
        };
        let get =
            |v: &[(usize, f32, f32)], m: usize| v.iter().find(|x| x.0 == m).map(|x| (x.1, x.2));
        let open = look(&puppet);
        puppet.set_value(param(&model, "ParamEyeLOpen").unwrap(), 0.0);
        puppet.update();
        let shut = look(&puppet);
        let (white, iris, upper, lower, highlight) = (4, 5, 6, 7, 8);
        for ball in [white, iris, highlight] {
            assert_eq!(get(&open, ball).map(|x| x.1), Some(1.0));
            assert!(get(&shut, ball).is_none_or(|x| x.1 == 0.0), "the eyeball folds away");
        }
        let (up_open, up_shut) = (get(&open, upper).unwrap().0, get(&shut, upper).unwrap().0);
        assert!((up_open - 188.0).abs() < 3.0, "{up_open}");
        assert!((up_shut - 213.0).abs() < 3.0, "the upper lash lands on the lower: {up_shut}");
        assert_eq!(get(&open, lower), get(&shut, lower), "the lower lash stays");
    }

    /// A filled ellipse on a full-canvas layer.
    fn blob(name: &str, folder: &str, ellipse: (f32, f32, f32, f32), rgb: [u8; 3]) -> Layer {
        let (w, h) = (400u32, 600u32);
        let (cx, cy, rx, ry) = ellipse;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for (i, px) in rgba.chunks_mut(4).enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            if ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2) <= 1.0 {
                px.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        let groups = if folder.is_empty() { vec![] } else { vec![folder.into()] };
        Layer {
            name: name.into(),
            groups,
            x: 0,
            y: 0,
            width: w,
            height: h,
            rgba,
            visible: true,
            opacity: 1.0,
            clip_to_below: false,
            blend: Blend::NORMAL,
        }
    }

    /// A doll in the usual layer order, bottom first, on a 400x600 canvas.
    pub(crate) fn doll() -> Document {
        let layers = vec![
            blob("back hair", "Hair", (200.0, 190.0, 125.0, 150.0), [70, 40, 30]),
            blob("body", "", (200.0, 470.0, 90.0, 130.0), [80, 110, 200]),
            blob("neck", "", (200.0, 330.0, 25.0, 40.0), [240, 210, 190]),
            // turned: its middle is right of both eyes (x 160 and 240)
            blob("face", "Head", (250.0, 200.0, 115.0, 120.0), [250, 225, 205]),
            // the left eye in parts, the right one drawn whole
            blob("white", "eye L", (240.0, 200.0, 22.0, 14.0), [250, 250, 250]),
            blob("iris", "eye L", (240.0, 200.0, 9.0, 11.0), [40, 60, 120]),
            blob("upper lash", "eye L", (240.0, 188.0, 24.0, 4.0), [30, 20, 20]),
            blob("lower lash", "eye L", (240.0, 213.0, 20.0, 2.0), [30, 20, 20]),
            // no folder, no eye word: it sits in the left eye
            blob("highlight", "", (236.0, 196.0, 3.0, 3.0), [255, 255, 255]),
            blob("R", "eye", (160.0, 200.0, 22.0, 14.0), [40, 60, 120]),
            blob("brow l", "", (240.0, 170.0, 24.0, 5.0), [60, 40, 30]),
            blob("brow r", "", (160.0, 170.0, 24.0, 5.0), [60, 40, 30]),
            blob("mouth", "", (200.0, 265.0, 20.0, 6.0), [200, 70, 80]),
            blob("bangs", "Hair", (200.0, 110.0, 105.0, 45.0), [90, 55, 40]),
            // twintails: down to y 580, far past the face (y 80 to 320)
            blob("side hair", "Hair", (200.0, 340.0, 130.0, 240.0), [90, 55, 40]),
            blob("body wing L", "", (345.0, 420.0, 50.0, 90.0), [230, 230, 240]),
            blob("body wing R", "", (55.0, 420.0, 50.0, 90.0), [230, 230, 240]),
        ];
        Document { width: 400, height: 600, layers, folders: vec![] }
    }

    pub(crate) fn rigged() -> (Model, Vec<Vec<u8>>, Vec<Role>) {
        rigged_from(&doll())
    }

    fn rigged_from(doc: &Document) -> (Model, Vec<Vec<u8>>, Vec<Role>) {
        let imported = ayatsuri_import::from_layers_atlas(doc).unwrap();
        let mut model = imported.model;
        let pages: Vec<image::RgbaImage> = imported
            .textures
            .iter()
            .map(|p| image::load_from_memory(p).unwrap().to_rgba8())
            .collect();
        assert_eq!(automesh(&mut model, &pages), model.art_meshes.len(), "every layer meshes");
        let r = roles(&model, &imported.mesh_names, &imported.part_names);
        let parts = eye_parts(&model, &r, &imported.mesh_names, &imported.part_names);
        rig(&mut model, &r, &parts).unwrap();
        (model, imported.textures, r)
    }

    #[test]
    fn a_doll_rigs_and_each_parameter_moves_its_own_layers() {
        use Role::*;
        let (model, _, r) = rigged();
        let want = [HairBack, Body, Body, Face, EyeL, EyeL, EyeL, EyeL, EyeL, EyeR, BrowL, BrowR];
        assert_eq!(r, [&want[..], &[Mouth, HairFront, HairSide, WingL, WingR]].concat());
        let mut puppet = ayatsuri_core::Puppet::new(model.clone());
        puppet.update();
        let snap = |p: &ayatsuri_core::Puppet| -> Vec<(usize, Vec<f32>)> {
            let mut v: Vec<_> = p.drawables().map(|d| (d.mesh, d.positions.to_vec())).collect();
            v.sort_by_key(|(m, _)| *m);
            v
        };
        let rest = snap(&puppet);
        // the roles of the meshes that move when one parameter is set
        let mut moved = |id: &str, value: f32| -> Vec<Role> {
            let p = param(&model, id).unwrap();
            puppet.set_value(p, value);
            puppet.update();
            let now = snap(&puppet);
            puppet.set_value(p, model.parameters[p].default);
            puppet.update();
            let mut out: Vec<Role> = rest
                .iter()
                .zip(&now)
                .filter(|((_, a), (_, b))| a.iter().zip(b).any(|(x, y)| (x - y).abs() > 1e-4))
                .map(|((m, _), _)| r[*m])
                .collect();
            out.sort_by_key(|x| *x as u8);
            out.dedup();
            out
        };
        let head = vec![Face, EyeL, EyeR, BrowL, BrowR, Mouth, HairFront, HairBack];
        assert_eq!(moved("ParamEyeLOpen", 0.0), vec![EyeL]);
        assert_eq!(moved("ParamEyeROpen", 0.0), vec![EyeR]);
        assert_eq!(moved("ParamBrowLY", 1.0), vec![BrowL]);
        assert_eq!(moved("ParamMouthOpenY", 1.0), vec![Mouth]);
        assert_eq!(moved("ParamHairFront", 1.0), vec![HairFront]);
        assert_eq!(moved("ParamHairSide", 1.0), vec![HairSide]);
        // the long twintails stay out of the turn but tilt with the head
        assert_eq!(moved("ParamAngleX", 30.0), head);
        let tilted = vec![Face, EyeL, EyeR, BrowL, BrowR, Mouth, HairFront, HairSide, HairBack];
        assert_eq!(moved("ParamAngleZ", 30.0), tilted);
        // wings flap together and lean with the body, not with the head
        assert_eq!(moved("ParamWing", 1.0), vec![WingL, WingR]);
        let mut all = tilted.clone();
        all.extend([Body, WingL, WingR]);
        assert_eq!(moved("ParamBodyAngleX", 10.0), all);
        assert_eq!(model.physics.as_ref().map(|p| p.groups.len()), Some(4));
    }

    #[test]
    fn parameters_come_only_with_layers_to_move() {
        let mut doc = doll();
        doc.layers.retain(|l| {
            !l.name.contains("brow") && !l.name.contains("side") && !l.name.contains("wing")
        });
        let (model, _, _) = rigged_from(&doc);
        let ids: Vec<&str> = model.parameters.iter().map(|p| p.id.as_str()).collect();
        for gone in ["ParamBrowLY", "ParamBrowRY", "ParamHairSide", "ParamWing"] {
            assert!(!ids.contains(&gone), "{gone} would be dead");
        }
        for kept in ["ParamEyeLOpen", "ParamMouthOpenY", "ParamHairFront", "ParamHairBack"] {
            assert!(ids.contains(&kept), "{kept}");
        }
        assert_eq!(model.physics.as_ref().map(|p| p.groups.len()), Some(2));
    }

    #[test]
    fn the_rig_survives_a_moc3_round_trip() {
        let (model, _, _) = rigged();
        let moc =
            ayatsuri_import::to_moc3(&model, ayatsuri_import::ExportVersion::DEFAULT).unwrap();
        let back = ayatsuri_import::from_moc3(&moc).unwrap();
        assert_eq!(back.parameters.len(), model.parameters.len());
        assert_eq!(back.deformers.len(), model.deformers.len());
        assert_eq!(back.art_meshes.len(), model.art_meshes.len());
    }
}
