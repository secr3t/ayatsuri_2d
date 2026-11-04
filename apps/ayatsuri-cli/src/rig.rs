// Auto rig from layer names. Each layer of a paint document is sorted into
// a face part by its own name, then by its folders' names (English,
// Japanese and Korean words), and Cubism's standard rig is built over the
// parts with the editor's own operations (`ayatsuri_core::edit`), so the
// result opens in the editor as an ordinary model to keep rigging.
//
// The tree, parents first:
//
//   WarpBody (ParamBodyAngleX, Y, Z, ParamBreath)
//     body meshes
//     RotHead (ParamAngleZ), pivot at the bottom of the face
//       WarpHead (ParamAngleX x ParamAngleY)
//         face meshes
//         WarpEyeL / WarpEyeR, their meshes keyed on ParamEyeLOpen /
//           ParamEyeROpen and the irises on ParamEyeBallX x Y
//         WarpBrowL / WarpBrowR (ParamBrowLY / ParamBrowRY)
//         WarpMouth (ParamMouthOpenY x ParamMouthForm)
//         WarpHairFront / Side / Back (ParamHair*, swung by physics)
//       long WarpHairSide / Back, following ParamAngleX x Y at the top
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

/// What an eye or mouth layer is, so it moves the way riggers move it:
/// the upper lash comes down and the lower lash up to meet it, the white
/// folds shut between them and clips the iris (with the pupil and
/// highlight), which looks about; the mouth's inside shows only when it
/// opens. An eye layer that says none of these (an eye drawn as one piece)
/// is squashed whole.
///
/// The swaps are whole drawings of a shut or half shut eye or an open
/// mouth: they hang on the head, bend with nothing else, and fade in over
/// the parts as the parameter gets there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    UpperLash,
    LowerLash,
    White,
    Iris,
    MouthInside,
    EyeClosed,
    EyeHalf,
    MouthOpen,
    Other,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::UpperLash => "upper lash",
            Part::LowerLash => "lower lash",
            Part::White => "eye white",
            Part::Iris => "iris",
            Part::MouthInside => "inside",
            Part::EyeClosed => "closed",
            Part::EyeHalf => "half",
            Part::MouthOpen => "open",
            Part::Other => "",
        }
    }

    fn is_swap(self) -> bool {
        matches!(self, Part::EyeClosed | Part::EyeHalf | Part::MouthOpen)
    }
}

/// Half before shut: 半開き holds 開.
const HALF: &[&str] = &["half", "半目", "半開", "반감", "반쯤"];
const CLOSED: &[&str] = &["closed", "close", "shut", "閉", "감은", "감김", "닫힌"];
const OPEN: &[&str] = &["open", "開", "벌린", "벌림", "열린"];

const LASH: &[&str] =
    &["lash", "lid", "まつ毛", "まつげ", "睫", "瞼", "まぶた", "속눈썹", "눈꺼풀", "아이라인"];
const LOWER: &[&str] = &["lower", "bottom", "under", "下", "아래", "밑"];
const WHITE: &[&str] = &["white", "sclera", "白目", "흰자"];
const IRIS: &[&str] =
    &["iris", "pupil", "highlight", "瞳", "黒目", "ハイライト", "눈동자", "동공", "하이라이트"];
const INSIDE: &[&str] = &[
    "inside",
    "inner",
    "teeth",
    "tongue",
    "口内",
    "口の中",
    "歯",
    "舌",
    "입안",
    "입 안",
    "입속",
    "치아",
    "혀",
];

fn says(text: &str, words: &[&str]) -> bool {
    let toks = tokens(text);
    words.iter().any(|w| has_word(text, &toks, w))
}

/// The first name that says which eye part, layer name before folders.
fn eye_part(names: &[String]) -> Part {
    for n in names {
        if says(n, HALF) {
            return Part::EyeHalf;
        }
        if says(n, CLOSED) {
            return Part::EyeClosed;
        }
        if says(n, LASH) {
            return if says(n, LOWER) { Part::LowerLash } else { Part::UpperLash };
        }
        if says(n, WHITE) {
            return Part::White;
        }
        if says(n, IRIS) {
            return Part::Iris;
        }
    }
    Part::Other
}

/// Each eye and mouth layer's part (from [`roles`]); `Other` for the rest.
pub fn parts(
    model: &Model,
    roles: &[Role],
    mesh_names: &[String],
    part_names: &[String],
) -> Vec<Part> {
    roles
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let names = || names_of(model, i, mesh_names, part_names);
            match r {
                Role::EyeL | Role::EyeR => eye_part(&names()),
                Role::Mouth if names().iter().any(|n| says(n, OPEN)) => Part::MouthOpen,
                Role::Mouth if names().iter().any(|n| says(n, INSIDE)) => Part::MouthInside,
                _ => Part::Other,
            }
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
/// groups name them. The eyeballs' are made when an iris is drawn.
const PARAMS: &[(&str, f32, f32, f32, &[Role])] = &[
    ("ParamAngleX", -30.0, 0.0, 30.0, &[]),
    ("ParamAngleY", -30.0, 0.0, 30.0, &[]),
    ("ParamAngleZ", -30.0, 0.0, 30.0, &[]),
    ("ParamEyeLOpen", 0.0, 1.0, 1.0, &[]),
    ("ParamEyeROpen", 0.0, 1.0, 1.0, &[]),
    ("ParamEyeBallX", -1.0, 0.0, 1.0, &[]),
    ("ParamEyeBallY", -1.0, 0.0, 1.0, &[]),
    ("ParamBrowLY", -1.0, 0.0, 1.0, &[Role::BrowL]),
    ("ParamBrowRY", -1.0, 0.0, 1.0, &[Role::BrowR]),
    ("ParamMouthForm", -1.0, 0.0, 1.0, &[]),
    ("ParamMouthOpenY", 0.0, 0.0, 1.0, &[]),
    ("ParamBodyAngleX", -10.0, 0.0, 10.0, &[]),
    ("ParamBodyAngleY", -10.0, 0.0, 10.0, &[]),
    ("ParamBodyAngleZ", -10.0, 0.0, 10.0, &[]),
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

/// ParamBodyAngleZ tips the chest the way ParamAngleZ tips the head.
const BODY_TILT: f32 = -1.0;

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
/// 1, and says how far the point moves in grid widths and heights, on top
/// of the shape the keyform already has (a warp keyed twice keeps both).
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
        for (i, p) in points.iter_mut().enumerate() {
            let u = (i % (cols + 1)) as f32 / cols as f32;
            let v = (i / (cols + 1)) as f32 / rows as f32;
            let (dx, dy) = f(vals, u, v);
            *p = [p[0] + dx * bw, p[1] + dy * bh];
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

/// Key art mesh `mi` on `axes` and shape every keyform: `f` gets the
/// keyform's values (in `axes` order) and the rest form's points, and
/// returns the keyform's points and opacity.
fn shape_mesh(
    model: &mut Model,
    mi: usize,
    axes: &[(&str, &[f32])],
    f: impl Fn(&[f32], &[[f32; 2]]) -> (Vec<[f32; 2]>, f32),
) -> Option<()> {
    let mut params = Vec::with_capacity(axes.len());
    for (id, keys) in axes {
        let p = param(model, id)?;
        edit::add_keys(model, Target::ArtMesh(mi), p, keys);
        params.push(p);
    }
    let m = model.art_meshes.get_mut(mi)?;
    let values: Vec<Vec<f32>> =
        (0..m.binding.keyform_count()).map(|k| values_at(&m.binding, &params, k)).collect();
    let n = (m.vertex_count as usize).saturating_mul(2);
    let base = m.positions.get(..n)?.to_vec();
    let (rest, _) = base.as_chunks::<2>();
    for (k, vals) in values.iter().enumerate() {
        let (points, opacity) = f(vals, rest);
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

/// Shut each eye by its parts (see [`Part`]), on keys 0, 0.5 and 1 so the
/// white stays solid until the lids are half way, and turn its irises in
/// the white on ParamEyeBallX and Y.
fn shut_eyes(model: &mut Model, roles: &[Role], parts: &[Part]) {
    let sides = [(Role::EyeL, "ParamEyeLOpen"), (Role::EyeR, "ParamEyeROpen")];
    for (side, id) in sides {
        let part = |i: usize| parts.get(i).copied().unwrap_or(Part::Other);
        let all: Vec<usize> =
            roles.iter().enumerate().filter(|&(_, &r)| r == side).map(|(i, _)| i).collect();
        let (swaps, eye): (Vec<usize>, Vec<usize>) = all.iter().partition(|&&i| part(i).is_swap());
        let has = |p: Part| swaps.iter().any(|&i| part(i) == p);
        // drawn shut, the parts give way to the drawing from half open to
        // a fifth; drawn half shut, that drawing shows from 0.35 to 0.65
        let shut = |v: f32| ((0.5 - v) / 0.3).clamp(0.0, 1.0);
        let half = |v: f32| (((v - 0.2) / 0.15).min((0.8 - v) / 0.15)).clamp(0.0, 1.0);
        let swapped = has(Part::EyeClosed) || has(Part::EyeHalf);
        let gone = move |v: f32| if has(Part::EyeClosed) { 1.0 - shut(v) } else { 1.0 };
        let keys: &[f32] =
            if swapped { &[0.0, 0.2, 0.35, 0.5, 0.65, 0.8, 1.0] } else { &[0.0, 0.5, 1.0] };
        for &mi in &swaps {
            let show = if part(mi) == Part::EyeClosed { shut } else { half };
            shape_mesh(model, mi, &[(id, keys)], |v, pts| {
                (pts.to_vec(), show(v.first().copied().unwrap_or(1.0)))
            });
        }
        let union = |model: &Model, pick: &dyn Fn(Part) -> bool| {
            eye.iter()
                .filter(|&&i| pick(part(i)))
                .filter_map(|&i| mesh_box(model, i))
                .fold(None, |b, m| Some(grow(b, m)))
        };
        let Some([_, y0, _, y1]) = union(model, &|_| true) else { continue };
        let mid_y = |b: Option<Box4>| b.map(|[_, a, _, b]| (a + b) * 0.5);
        // the lids meet a quarter of the way up from the lower lash, else
        // 60% down the eye
        let line = match (
            mid_y(union(model, &|p| p == Part::UpperLash)),
            mid_y(union(model, &|p| p == Part::LowerLash)),
        ) {
            (Some(up), Some(low)) => low - (low - up) * 0.25,
            (None, Some(low)) => low,
            _ => y0 + (y1 - y0) * 0.6,
        };
        let squash = move |pts: &[[f32; 2]], by: f32| -> Vec<[f32; 2]> {
            pts.iter().map(|&[x, y]| [x, line + (y - line) * (1.0 - by)]).collect()
        };
        // a lash keeps its shape: it slides onto the line, the upper one
        // thinning to half
        let slide = move |pts: &[[f32; 2]], shut: f32, thin: f32| -> Vec<[f32; 2]> {
            let (lo, hi) =
                pts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[1]), hi.max(p[1])));
            let cy = (lo + hi) * 0.5;
            let k = 1.0 - thin * shut;
            pts.iter().map(|&[x, y]| [x, cy + (y - cy) * k + shut * (line - cy)]).collect()
        };
        let whites: Vec<u32> =
            eye.iter().filter(|&&i| part(i) == Part::White).map(|&i| i as u32).collect();
        let white = union(model, &|p| p == Part::White);
        // the irises look about a quarter of the white's width
        let reach = white
            .or_else(|| union(model, &|_| true))
            .map_or((0.0, 0.0), |[x0, y0, x1, y1]| ((x1 - x0) * 0.25, (y1 - y0) * 0.2));
        let gaze: &[f32] = &[-1.0, 0.0, 1.0];
        for &mi in &eye {
            match part(mi) {
                Part::UpperLash => {
                    shape_mesh(model, mi, &[(id, keys)], |v, pts| {
                        (slide(pts, 1.0 - v.first().copied().unwrap_or(1.0), 0.5), 1.0)
                    });
                }
                Part::LowerLash => {
                    shape_mesh(model, mi, &[(id, keys)], |v, pts| {
                        (slide(pts, 1.0 - v.first().copied().unwrap_or(1.0), 0.0), 1.0)
                    });
                }
                Part::White => {
                    // shut it is a line: nothing shows, nothing it clips
                    // shows, and half shut it is not see-through
                    shape_mesh(model, mi, &[(id, keys)], |v, pts| {
                        (squash(pts, 1.0 - v.first().copied().unwrap_or(1.0)), 1.0)
                    });
                }
                Part::Iris => {
                    if let Some(m) = model.art_meshes.get_mut(mi)
                        && m.masks.is_empty()
                    {
                        m.masks = whites.clone();
                    }
                    // clipped by the white it shuts with; without one it
                    // folds away and fades itself
                    let alone = whites.is_empty();
                    let fold = if alone { 0.95 } else { 0.0 };
                    let axes = [(id, keys), ("ParamEyeBallX", gaze), ("ParamEyeBallY", gaze)];
                    shape_mesh(model, mi, &axes, |v, pts| {
                        let at = |i: usize| v.get(i).copied().unwrap_or(0.0);
                        let (dx, dy) = (at(1) * reach.0, -at(2) * reach.1);
                        let moved: Vec<[f32; 2]> = squash(pts, fold * (1.0 - at(0)))
                            .iter()
                            .map(|&[x, y]| [x + dx, y + dy])
                            .collect();
                        (moved, if alone && at(0) <= 0.0 { 0.0 } else { 1.0 })
                    });
                }
                _ => {
                    shape_mesh(model, mi, &[(id, keys)], |v, pts| {
                        (squash(pts, 0.9 * (1.0 - v.first().copied().unwrap_or(1.0))), 1.0)
                    });
                }
            }
            fade_mesh(model, mi, id, gone);
        }
    }
}

/// Scale each of mesh `mi`'s keyform opacities by `f` of its value on `id`.
fn fade_mesh(model: &mut Model, mi: usize, id: &str, f: impl Fn(f32) -> f32) -> Option<()> {
    let p = param(model, id)?;
    let m = model.art_meshes.get_mut(mi)?;
    for k in 0..m.binding.keyform_count() {
        let v = values_at(&m.binding, &[p], k).first().copied().unwrap_or(0.0);
        if let Some(o) = m.opacities.get_mut(k) {
            *o *= f(v);
        }
    }
    Some(())
}

/// The mouth's inside shows only once the mouth opens. Drawn open, the
/// mouth gives way to the drawing from 0.3 to 0.6.
fn open_mouth(model: &mut Model, roles: &[Role], parts: &[Part]) {
    let id = "ParamMouthOpenY";
    let swapped = parts.contains(&Part::MouthOpen);
    let open = |v: f32| ((v - 0.3) / 0.3).clamp(0.0, 1.0);
    let keys: &[f32] = if swapped { &[0.0, 0.3, 0.6, 1.0] } else { &[0.0, 0.3, 1.0] };
    for (mi, (&r, &p)) in roles.iter().zip(parts).enumerate() {
        match p {
            _ if r != Role::Mouth => continue,
            Part::Other if swapped => {
                shape_mesh(model, mi, &[(id, keys)], |_, pts| (pts.to_vec(), 1.0));
            }
            Part::MouthInside => {
                shape_mesh(model, mi, &[(id, keys)], |v, pts| {
                    (pts.to_vec(), if v.first().copied().unwrap_or(0.0) > 0.0 { 1.0 } else { 0.0 })
                });
            }
            Part::MouthOpen => {
                shape_mesh(model, mi, &[(id, keys)], |v, pts| {
                    (pts.to_vec(), open(v.first().copied().unwrap_or(0.0)))
                });
                continue;
            }
            _ => continue,
        }
        if swapped {
            fade_mesh(model, mi, id, |v| 1.0 - open(v));
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
            input("ParamBodyAngleZ", 40.0, PhysicsIo::Angle),
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
/// eyes' and mouth's `parts` (from [`parts`]): the standard parameters, the
/// deformer tree, the keyforms and hair and wing physics.
pub fn rig(model: &mut Model, roles: &[Role], parts: &[Part]) -> Result<(), String> {
    let drawn = |any: &[Role]| any.is_empty() || roles.iter().any(|r| any.contains(r));
    let iris = parts.contains(&Part::Iris);
    for &(id, min, default, max, moves) in PARAMS {
        let wanted = if id.starts_with("ParamEyeBall") { iris } else { drawn(moves) };
        if wanted && param(model, id).is_none() {
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
    let long: Vec<(Role, &str)> =
        [(Role::HairSide, "WarpHairSide"), (Role::HairBack, "WarpHairBack")]
            .into_iter()
            .filter(|&(role, _)| match (face, union(model, &|r| r == role)) {
                (Some([_, y0, _, y1]), Some([_, _, _, bottom])) => bottom > y1 + (y1 - y0) * 0.5,
                _ => false,
            })
            .collect();
    let is_long = |r: Role| long.iter().any(|&(x, _)| x == r);
    // where the warps' grids lie on the canvas, for the long hair to
    // follow the head's turn (create_warp_deformer's 5% margin)
    let grid = |[x0, y0, x1, y1]: Box4| {
        let (mx, my) = ((x1 - x0) * 0.05, (y1 - y0) * 0.05);
        [x0 - mx, y0 - my, x1 + mx, y1 + my]
    };
    let head_grid =
        union(model, &|r| !is_long(r) && !matches!(r, Role::Body | Role::WingL | Role::WingR))
            .map(grid);
    let long_grids: Vec<(&str, Box4)> = long
        .iter()
        .filter_map(|&(role, id)| union(model, &|r| r == role).map(|b| (id, grid(b))))
        .collect();

    // the feature warps, straight over their meshes; the swap drawings
    // stay out of them and only turn with the head
    let swap = |i: &usize| parts.get(*i).is_some_and(|p| p.is_swap());
    let mut made: Vec<&str> = Vec::new();
    for &(role, id, rows, cols) in FEATURES {
        let ms: Vec<usize> = meshes(role).into_iter().filter(|i| !swap(i)).collect();
        if ms.is_empty() {
            continue;
        }
        edit::create_warp_deformer(model, &ms, rows, cols, id.into())
            .ok_or_else(|| format!("could not make {id}"))?;
        made.push(id);
    }

    // the head: a warp over the face and its features, a rotation over that
    let mut head: Vec<Child> = meshes(Role::Face).into_iter().map(Child::Mesh).collect();
    head.extend((0..roles.len()).filter(swap).map(Child::Mesh));
    let short = made.iter().filter(|id| !long.iter().any(|(_, l)| l == *id) && !WINGS.contains(id));
    head.extend(short.filter_map(|id| deformer(model, id)).map(Child::Deformer));
    let mut body: Vec<Child> = meshes(Role::Body).into_iter().map(Child::Mesh).collect();
    if !head.is_empty() {
        wrap(model, NewDeformer::Warp { rows: 4, cols: 4 }, &head, "WarpHead")?;
        let mut turned: Vec<Child> =
            deformer(model, "WarpHead").map(Child::Deformer).into_iter().collect();
        turned.extend(long.iter().filter_map(|(_, id)| deformer(model, id)).map(Child::Deformer));
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
    open_mouth(model, roles, parts);
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
    // the middle of the face travels further than its edges
    let head_move = move |v: &[f32], x: f32, y: f32| {
        let (tx, ty) = (at(v, 0) / 30.0, at(v, 1) / 30.0);
        (tx * (0.03 + 0.08 * (pi * x).sin()), -ty * (0.02 + 0.06 * (pi * y).sin()))
    };
    shape_warp(model, "WarpHead", turn, head_move);
    // long hair follows the head where it lies on it, less and less below
    // it, so no gap opens between it and the turned face
    if let Some([hx0, hy0, hx1, hy1]) = head_grid {
        let (hw, hh) = ((hx1 - hx0).max(1e-4), (hy1 - hy0).max(1e-4));
        for &(id, [x0, y0, x1, y1]) in &long_grids {
            let (w, h) = ((x1 - x0).max(1e-4), (y1 - y0).max(1e-4));
            shape_warp(model, id, turn, |v, u, t| {
                let (cx, cy) = (x0 + u * w, y0 + t * h);
                let hu = ((cx - hx0) / hw).clamp(0.0, 1.0);
                let hv = ((cy - hy0) / hh).clamp(0.0, 1.0);
                let (dx, dy) = head_move(v, hu, hv);
                let follow = 1.0 - ((cy - hy1) / (y1 - hy1).max(1e-4)).clamp(0.0, 1.0);
                (dx * hw / w * follow, dy * hh / h * follow)
            });
        }
    }
    shape_rotation(model, "RotHead", &[("ParamAngleZ", &[-30.0, 0.0, 30.0])], |v| -at(v, 0) / 3.0);
    let lean: &[(&str, &[f32])] = &[
        ("ParamBodyAngleX", &[-10.0, 0.0, 10.0]),
        ("ParamBreath", &[0.0, 1.0]),
        ("ParamBodyAngleY", &[-10.0, 0.0, 10.0]),
        ("ParamBodyAngleZ", &[-10.0, 0.0, 10.0]),
    ];
    shape_warp(model, "WarpBody", lean, |v, x, y| {
        let up = 1.0 - y; // the hips stay put
        let lean = 0.06 * at(v, 0) / 10.0 * up;
        let breath = -0.015 * at(v, 1) * up;
        // bent back the chest rises, bent forward it sinks
        let bend = -0.03 * at(v, 2) / 10.0 * up;
        // tilted sideways the shoulders go over, the far one higher
        let tz = BODY_TILT * at(v, 3) / 10.0;
        (lean + 0.06 * tz * up, breath + bend + 0.04 * tz * (x - 0.5) * up)
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

/// How `bust` shapes a chest: a soft ellipse on the warp's grid (0 to 1)
/// that rises and swells as the parameter goes up and sinks and narrows,
/// at 0.6 of the reach, as it goes down.
pub struct Bust {
    pub center: [f32; 2],
    pub radius: [f32; 2],
    pub lift: f32,
    pub swell: f32,
    pub rows: u32,
    pub cols: u32,
}

impl Default for Bust {
    fn default() -> Self {
        Bust { center: [0.5, 0.4], radius: [0.35, 0.2], lift: 0.03, swell: 0.05, rows: 8, cols: 8 }
    }
}

/// Wrap art mesh `mesh` of a finished model in a new warp, "WarpBust",
/// under the mesh's own parent, and key it on `param` at -1, 0 and 1,
/// the parameter made (-1 to 1) if the model has none.
pub fn bust(model: &mut Model, mesh: &str, param_id: &str, b: &Bust) -> Result<(), String> {
    const ID: &str = "WarpBust";
    if deformer(model, ID).is_some() {
        return Err(format!("the model has a {ID} already"));
    }
    let mi = model.art_meshes.iter().position(|m| m.id == mesh).ok_or(format!("no mesh {mesh}"))?;
    if param(model, param_id).is_none() {
        model.parameters.push(Parameter {
            id: param_id.into(),
            min: -1.0,
            max: 1.0,
            default: 0.0,
            repeat: false,
            decimals: 1,
            blend_shape: false,
        });
    }
    edit::create_warp_deformer(model, &[mi], b.rows, b.cols, ID.into())
        .ok_or(format!("{mesh} cannot take a warp"))?;
    let ([cu, cv], [ru, rv]) = (b.center, b.radius.map(|r| r.max(1e-4)));
    shape_warp(model, ID, &[(param_id, &[-1.0, 0.0, 1.0])], |vals, u, v| {
        let val = vals.first().copied().unwrap_or(0.0);
        let d = ((u - cu) / ru).hypot((v - cv) / rv);
        let w = if d < 1.0 { (1.0 - d).powi(2) * (1.0 + 2.0 * d) } else { 0.0 };
        // down, the same moves at 0.6; a negative swell narrows (up is -y)
        let k = if val > 0.0 { val } else { 0.6 * val };
        let (lift, swell) = (b.lift * w * k, b.swell * w * k);
        ((u - cu) * swell, -lift + (v - cv) * swell)
    })
    .ok_or(format!("{ID} cannot be keyed on {param_id}"))
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
        assert_eq!(part(&["上まつ毛"]), Part::UpperLash);
        assert_eq!(part(&["아래 속눈썹"]), Part::LowerLash);
        assert_eq!(part(&["eye lower lash L"]), Part::LowerLash);
        assert_eq!(part(&["L", "白目"]), Part::White, "the folder says it");
        assert_eq!(part(&["eye iris L"]), Part::Iris);
        assert_eq!(part(&["eye_L"]), Part::Other);
        assert_eq!(part(&["eye closed L"]), Part::EyeClosed);
        assert_eq!(part(&["目 半開き"]), Part::EyeHalf, "half before 開");
        assert_eq!(part(&["감은 눈"]), Part::EyeClosed);
        assert!(says("mouth open", OPEN) && says("口 開き", OPEN) && says("벌린 입", OPEN));
        assert!(says("mouth inside", INSIDE) && says("입 안", INSIDE));
    }

    /// Each drawn mesh's (mean x, mean y, opacity), in canvas pixels.
    fn look(p: &ayatsuri_core::Puppet) -> Vec<(usize, f32, f32, f32)> {
        p.drawables()
            .map(|d| {
                let (pairs, _) = d.positions.as_chunks::<2>();
                let n = pairs.len().max(1) as f32;
                let x = pairs.iter().map(|p| p[0]).sum::<f32>() / n;
                let y = pairs.iter().map(|p| p[1]).sum::<f32>() / n;
                (d.mesh, x * 600.0 + 200.0, y * 600.0 + 300.0, d.opacity)
            })
            .collect()
    }

    fn get(v: &[(usize, f32, f32, f32)], m: usize) -> Option<(f32, f32, f32)> {
        v.iter().find(|x| x.0 == m).map(|x| (x.1, x.2, x.3))
    }

    #[test]
    fn an_eye_in_parts_shuts_its_lids_together_and_looks_about() {
        let (model, _, _) = rigged();
        let (white, iris, upper, lower, highlight) = (4, 5, 6, 7, 8);
        assert_eq!(model.art_meshes[iris].masks, [white as u32], "the white clips the iris");
        assert_eq!(model.art_meshes[highlight].masks, [white as u32]);
        let mut puppet = ayatsuri_core::Puppet::new(model.clone());
        puppet.update();
        let open = look(&puppet);
        puppet.set_value(param(&model, "ParamEyeLOpen").unwrap(), 0.0);
        puppet.update();
        let shut = look(&puppet);
        let tall = |p: &ayatsuri_core::Puppet, m: usize| {
            let d = p.drawables().find(|d| d.mesh == m).unwrap();
            let (pairs, _) = d.positions.as_chunks::<2>();
            let ys = pairs.iter().map(|p| p[1] * 600.0);
            ys.clone().fold(f32::MIN, f32::max) - ys.fold(f32::MAX, f32::min)
        };
        assert!(tall(&puppet, white) < 0.5, "the white folds to a line, clipping the iris away");
        let y = |v: &[_], m| get(v, m).unwrap().1;
        assert!((y(&open, upper) - 188.0).abs() < 3.0, "{}", y(&open, upper));
        assert!((y(&open, lower) - 213.0).abs() < 3.0, "{}", y(&open, lower));
        // a quarter of the way up from the lower lash, both lids there
        assert!((y(&shut, upper) - 207.0).abs() < 3.0, "{}", y(&shut, upper));
        assert!((y(&shut, lower) - 207.0).abs() < 3.0, "{}", y(&shut, lower));

        puppet.set_value(param(&model, "ParamEyeLOpen").unwrap(), 1.0);
        puppet.set_value(param(&model, "ParamEyeBallX").unwrap(), 1.0);
        puppet.update();
        let right = look(&puppet);
        let x = |v: &[_], m| get(v, m).unwrap().0;
        // a quarter of the white's mesh (44 px of art, its outline round
        // it) to the viewer's right; the white itself stays
        for m in [iris, highlight] {
            let d = x(&right, m) - x(&open, m);
            assert!((10.0..17.0).contains(&d), "{m}: {d}");
        }
        assert_eq!(get(&right, white), get(&open, white));
    }

    #[test]
    fn the_mouths_inside_shows_only_when_it_opens() {
        let (model, _, r) = rigged();
        let inside = 12;
        assert_eq!(r[inside], Role::Mouth);
        let mut puppet = ayatsuri_core::Puppet::new(model.clone());
        puppet.update();
        assert!(get(&look(&puppet), inside).is_none_or(|x| x.2 == 0.0));
        puppet.set_value(param(&model, "ParamMouthOpenY").unwrap(), 1.0);
        puppet.update();
        assert_eq!(get(&look(&puppet), inside).map(|x| x.2), Some(1.0));
    }

    #[test]
    fn swap_drawings_fade_in_over_the_parts_and_bend_with_the_head_only() {
        let mut doc = doll();
        doc.layers.push(blob("eye closed L", "", (240.0, 200.0, 27.0, 18.0), [240, 200, 190]));
        doc.layers.push(blob("mouth open", "", (200.0, 268.0, 22.0, 10.0), [150, 40, 50]));
        let (model, _, _) = rigged_from(&doc);
        let n = doc.layers.len();
        let (closed, open, white, lash, mouth) = (n - 2, n - 1, 4, 6, 13);
        let warp_head = deformer(&model, "WarpHead").map(|d| d as u32);
        for m in [closed, open] {
            assert_eq!(model.art_meshes[m].parent_deformer, warp_head, "{m}");
        }
        let mut puppet = ayatsuri_core::Puppet::new(model.clone());
        let at = |puppet: &mut ayatsuri_core::Puppet, id: &str, v: f32| {
            puppet.set_value(param(&model, id).unwrap(), v);
            puppet.update();
            look(puppet)
        };
        let opa = |v: &[_], m| get(v, m).map_or(0.0, |x| x.2);
        let eye = "ParamEyeLOpen";
        let o = at(&mut puppet, eye, 1.0);
        assert_eq!((opa(&o, closed), opa(&o, white), opa(&o, lash)), (0.0, 1.0, 1.0));
        let half = at(&mut puppet, eye, 0.35);
        assert!((opa(&half, closed) - 0.5).abs() < 0.01 && (opa(&half, white) - 0.5).abs() < 0.01);
        let s = at(&mut puppet, eye, 0.0);
        assert_eq!((opa(&s, closed), opa(&s, white), opa(&s, lash)), (1.0, 0.0, 0.0));
        let still = |a: &[_], b: &[_], m| {
            let (a, b) = (get(a, m).unwrap(), get(b, m).unwrap());
            (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
        };
        assert!(still(&s, &half, closed), "the shut drawing does not bend");
        at(&mut puppet, eye, 1.0);
        let m = "ParamMouthOpenY";
        let shut = at(&mut puppet, m, 0.0);
        assert_eq!((opa(&shut, open), opa(&shut, mouth)), (0.0, 1.0));
        let mid = at(&mut puppet, m, 0.45);
        assert!((opa(&mid, open) - 0.5).abs() < 0.01 && (opa(&mid, mouth) - 0.5).abs() < 0.01);
        let wide = at(&mut puppet, m, 1.0);
        assert_eq!((opa(&wide, open), opa(&wide, mouth)), (1.0, 0.0));
        assert!(still(&wide, &mid, open), "nor the open one");
    }

    #[test]
    fn the_body_tilts_the_way_the_head_does() {
        let (model, _, _) = rigged();
        let face = 3;
        let mut puppet = ayatsuri_core::Puppet::new(model.clone());
        puppet.update();
        let rest = get(&look(&puppet), face).unwrap().0;
        let shift = |puppet: &mut ayatsuri_core::Puppet, id: &str, v: f32| {
            let p = param(&model, id).unwrap();
            puppet.set_value(p, v);
            puppet.update();
            let x = get(&look(puppet), face).unwrap().0 - rest;
            puppet.set_value(p, 0.0);
            puppet.update();
            x
        };
        let (head, body) =
            (shift(&mut puppet, "ParamAngleZ", 30.0), shift(&mut puppet, "ParamBodyAngleZ", 10.0));
        assert!(head.abs() > 1.0 && head.signum() == body.signum(), "{head} {body}");
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
            blob("mouth inside", "", (200.0, 266.0, 16.0, 4.0), [90, 30, 40]),
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
        let parts = super::parts(&model, &r, &imported.mesh_names, &imported.part_names);
        rig(&mut model, &r, &parts).unwrap();
        (model, imported.textures, r)
    }

    #[test]
    fn a_doll_rigs_and_each_parameter_moves_its_own_layers() {
        use Role::*;
        let (model, _, r) = rigged();
        let want = [HairBack, Body, Body, Face, EyeL, EyeL, EyeL, EyeL, EyeL, EyeR, BrowL, BrowR];
        assert_eq!(r, [&want[..], &[Mouth, Mouth, HairFront, HairSide, WingL, WingR]].concat());
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
            // a mesh shown or hidden (the mouth's inside) counts as moved
            let mut out: Vec<Role> = now
                .iter()
                .filter(|(m, b)| {
                    rest.iter()
                        .find(|(x, _)| x == m)
                        .is_none_or(|(_, a)| a.iter().zip(b).any(|(x, y)| (x - y).abs() > 1e-4))
                })
                .map(|(m, _)| r[*m])
                .collect();
            out.sort_by_key(|x| *x as u8);
            out.dedup();
            out
        };
        assert_eq!(moved("ParamEyeLOpen", 0.0), vec![EyeL]);
        assert_eq!(moved("ParamEyeROpen", 0.0), vec![EyeR]);
        assert_eq!(moved("ParamBrowLY", 1.0), vec![BrowL]);
        assert_eq!(moved("ParamMouthOpenY", 1.0), vec![Mouth]);
        assert_eq!(moved("ParamHairFront", 1.0), vec![HairFront]);
        assert_eq!(moved("ParamHairSide", 1.0), vec![HairSide]);
        assert_eq!(moved("ParamEyeBallX", 1.0), vec![EyeL], "the right eye has no iris");
        // the long twintails stay out of the head warp but follow its turn
        // at the top, and tilt with the head
        let tilted = vec![Face, EyeL, EyeR, BrowL, BrowR, Mouth, HairFront, HairSide, HairBack];
        assert_eq!(moved("ParamAngleX", 30.0), tilted);
        assert_eq!(moved("ParamAngleY", 30.0), tilted);
        assert_eq!(moved("ParamAngleZ", 30.0), tilted);
        // wings flap together and lean with the body, not with the head
        assert_eq!(moved("ParamWing", 1.0), vec![WingL, WingR]);
        let mut all = tilted.clone();
        all.extend([Body, WingL, WingR]);
        for id in ["ParamBodyAngleX", "ParamBodyAngleY", "ParamBodyAngleZ"] {
            assert_eq!(moved(id, 10.0), all, "{id}");
        }
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

    #[test]
    fn a_bust_warp_lifts_and_swells_the_chest_and_survives_a_round_trip() {
        let (model, _, roles) = rigged();
        let moc = |m: &Model| ayatsuri_import::to_moc3(m, ayatsuri_import::ExportVersion::DEFAULT);
        let mut model = ayatsuri_import::from_moc3(&moc(&model).unwrap()).unwrap();
        let body = roles.iter().position(|&r| r == Role::Body).unwrap();
        let id = model.art_meshes[body].id.clone();
        let parent = model.art_meshes[body].parent_deformer;
        bust(&mut model, &id, "ParamBustY", &Bust::default()).unwrap();
        assert!(bust(&mut model, &id, "ParamBustY", &Bust::default()).is_err(), "only one");

        let di = deformer(&model, "WarpBust").unwrap();
        let warp = &model.deformers[di];
        assert_eq!(warp.parent_deformer, parent, "under the mesh's old parent");
        assert_eq!(model.art_meshes[body].parent_deformer, Some(di as u32));
        let DeformerKind::Warp(w) = &warp.kind else { panic!() };
        let per = 81 * 2;
        let form = |k: usize| &w.positions[k * per..(k + 1) * per];
        // the grid point at (0.5, 0.375), in the ellipse's middle
        let mid = (3 * 9 + 4) * 2;
        let (low, rest, high) = (form(0), form(1), form(2));
        assert!(
            high[mid + 1] < rest[mid + 1] && low[mid + 1] > rest[mid + 1],
            "up at 1, down at -1"
        );
        // a point off the middle goes out at 1 and in at -1
        let side = (3 * 9 + 6) * 2;
        assert!(high[side] > rest[side] && low[side] < rest[side]);
        // the corners stay put
        assert_eq!(&high[..2], &rest[..2]);

        let back = ayatsuri_import::from_moc3(&moc(&model).unwrap()).unwrap();
        let ids = |m: &Model| m.parameters.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&back), ids(&model));
        assert!(ids(&back).contains(&"ParamBustY".to_string()));
        let shape = |m: &Model| {
            m.art_meshes
                .iter()
                .map(|a| {
                    (
                        a.id.clone(),
                        a.binding.keyform_count(),
                        a.masks.clone(),
                        a.draw_orders.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(&back), shape(&model));
        assert_eq!(back.deformers.len(), model.deformers.len());
    }
}
