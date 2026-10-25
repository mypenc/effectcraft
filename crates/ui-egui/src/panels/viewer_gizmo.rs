//! The 3D transform gizmo of the Composition viewer.
//!
//! * **Move** (every tool but Rotation): an arrow per axis (drag to move along it) and a small
//!   square between each pair of arrows. Dragging a square sweeps the layer across the plane
//!   those two axes span, as if it slid over an invisible sheet: the pointer is projected onto
//!   that plane, so the layer follows it exactly under perspective.
//! * **Rotate** (Rotation tool, W): an X / Y / Z ring around the layer. Dragging a ring turns
//!   the layer about that axis (X / Y / Z Rotation). Hold Alt to edit Orientation instead and
//!   Shift to snap to 15 degrees. The ring axes follow the order the engine composes the
//!   angles in, so a ring always turns the property it is named for.
//!
//! It works on every 3D layer: solids, footage, shapes, models, nulls, cameras and lights. The
//! rings and squares keep one size on screen whatever the layer's Scale.

use effectcraft_engine::geom::{Mat3, Mat4, Vec3, vec2 as gv2, vec3};
use effectcraft_engine::project::{Layer, LayerId};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::render::three_d;
use egui::{Color32, Painter, Pos2, Rect, Stroke, pos2, vec2};
use serde_json::json;

use super::viewer::{NEAR_W, ViewerMap, axis_drag, cam_state, parent_world_inverse, project_segment};
use crate::EffectcraftApp;
use crate::state::Tool;

/// Ring radius and arrow length on screen, in points.
const SIZE: f32 = 70.0;
/// Move arrow length on screen, in points.
const ARROW: f64 = 60.0;
/// Segments per ring.
const RING_STEPS: usize = 72;
/// How close (points) the pointer has to be to a ring to grab it.
const RING_GRAB: f32 = 7.0;
/// Rotation snap with Shift, in degrees.
const SNAP_DEG: f64 = 15.0;

const AXIS_COLORS: [Color32; 3] = [Color32::from_rgb(0xe0, 0x50, 0x50), Color32::from_rgb(0x60, 0xd0, 0x60), Color32::from_rgb(0x50, 0x8c, 0xf0)];
const AXIS_NAMES: [&str; 3] = ["x", "y", "z"];
/// The plane squares: the two axes a square spans, its name and the axis it is perpendicular
/// to (which gives it its colour).
const PLANES: [([usize; 2], &str, usize); 3] = [([0, 1], "xy", 2), ([1, 2], "yz", 0), ([0, 2], "xz", 1)];

/// A move arrow: layer, grabbable screen segment, position change per comp px in x and y.
pub(super) type AxisHit = (LayerId, [Pos2; 2], [f64; 3], [f64; 3]);

/// The grabbable parts of the gizmos drawn this frame.
#[derive(Default)]
pub(super) struct Hits {
    pub rings: Vec<RingHit>,
    pub planes: Vec<PlaneHit>,
}

/// One rotation ring as drawn: the screen polyline (None where it passes behind the camera).
#[derive(Clone, Debug)]
pub(super) struct RingHit {
    layer: LayerId,
    axis: usize,
    orientation: bool,
    /// The angles the ring edits (X, Y, Z) when the frame was built.
    start_vals: [f64; 3],
    /// Closed polyline: screen points and the same in comp px.
    screen: Vec<Option<Pos2>>,
    comp: Vec<Option<[f64; 2]>>,
}

/// One plane square as drawn.
#[derive(Clone, Debug)]
pub(super) struct PlaneHit {
    layer: LayerId,
    quad: [Pos2; 4],
    start_pos: [f64; 3],
    /// Comp px to plane coordinates (s, t), and the parent-space position change per unit of each.
    inv: Mat3,
    dirs: [[f64; 3]; 2],
}

/// A ring drag in progress.
#[derive(Clone, Debug)]
pub(super) struct RingDrag {
    layer: LayerId,
    axis: usize,
    orientation: bool,
    start_vals: [f64; 3],
    start: [f64; 2],
    /// Comp px the pointer covers per radian of turn, where the ring was grabbed.
    tangent: [f64; 2],
}

/// A plane sweep in progress.
#[derive(Clone, Debug)]
pub(super) struct PlaneDrag {
    layer: LayerId,
    start_pos: [f64; 3],
    inv: Mat3,
    dirs: [[f64; 3]; 2],
    start_st: [f64; 2],
}

/// `p` through the 4x4 `pc` to comp px; None at or behind the camera's near plane.
fn project(pc: &Mat4, p: Vec3) -> Option<[f64; 2]> {
    let h = |i: usize| pc.0.get(i).map_or(0.0, |row| row[0] * p.x + row[1] * p.y + row[2] * p.z + row[3]);
    let w = h(3);
    let q = [h(0) / w, h(1) / w];
    (w >= NEAR_W && q.iter().all(|v| v.is_finite())).then_some(q)
}

fn unit(v: Vec3) -> Option<Vec3> {
    let l = v.length();
    (l.is_finite() && l > 1e-9).then(|| vec3(v.x / l, v.y / l, v.z / l))
}

/// The world directions of the X, Y and Z rings of `l` and the three angles they edit.
///
/// The engine composes `parent * Orientation * Rz * Ry * Rx` (`render::eval::local_matrix`), so
/// the Z ring turns about the frame before any of the three angles, the Y ring about the frame
/// after Z, and the X ring about the frame after Z and Y. Editing Orientation is the same with
/// its own angles and without X/Y/Z Rotation. Cameras and lights add their auto-orient towards
/// the Point of Interest in front of both.
fn ring_axes(ctx: &EvalCtx, l: &Layer, orientation: bool) -> Option<([Vec3; 3], [f64; 3])> {
    let tr = l.transform()?;
    let o = ctx.v3(l, tr, "orientation", [0.0; 3]);
    let r = [ctx.f(l, tr, "rotationX", 0.0), ctx.f(l, tr, "rotationY", 0.0), ctx.f(l, tr, "rotation", 0.0)];
    let ro = Mat4::orientation(Vec3::from(o));
    let rr = Mat4::rotate_z(r[2]) * Mat4::rotate_y(r[1]) * Mat4::rotate_x(r[0]);
    let own = ro * rr;
    let total = if l.is_camera() || l.is_light() { three_d::camera::rig_rotation(ctx, l) } else { own };
    let pre = total * own.inverse()?;
    let (pre, a) = if orientation { (pre, o) } else { (pre * ro, r) };
    let (ez, ey, ex) = (vec3(0.0, 0.0, 1.0), vec3(0.0, 1.0, 0.0), vec3(1.0, 0.0, 0.0));
    let local = [
        (pre * Mat4::rotate_z(a[2]) * Mat4::rotate_y(a[1])).apply_vec(ex),
        (pre * Mat4::rotate_z(a[2])).apply_vec(ey),
        pre.apply_vec(ez),
    ];
    let pw = three_d::camera::parent_world(ctx, l);
    let w = |v: Vec3| unit(pw.apply_vec(v));
    Some(([w(local[0])?, w(local[1])?, w(local[2])?], a))
}

/// Two unit vectors spanning the plane perpendicular to `axis`, with `u x v = axis`.
fn plane_basis(axis: Vec3) -> (Vec3, Vec3) {
    let h = if axis.x.abs() < 0.9 { vec3(1.0, 0.0, 0.0) } else { vec3(0.0, 1.0, 0.0) };
    let d = h.dot(axis);
    let u = unit(vec3(h.x - axis.x * d, h.y - axis.y * d, h.z - axis.z * d)).unwrap_or(vec3(1.0, 0.0, 0.0));
    (u, axis.cross(u))
}

/// The homography from plane coordinates (s, t) of `o + s*ua + t*ub` to comp px under the
/// world to comp projection `p`, inverted: comp px to (s, t). None when the plane is edge-on.
fn plane_homography(p: &Mat4, o: Vec3, ua: Vec3, ub: Vec3) -> Option<Mat3> {
    let row = |i: usize| p.0.get(i);
    let mut m = [[0.0; 3]; 3];
    for (dst, src) in [0usize, 1, 3].into_iter().enumerate() {
        let r = row(src)?;
        m[dst] = [r[0] * ua.x + r[1] * ua.y + r[2] * ua.z, r[0] * ub.x + r[1] * ub.y + r[2] * ub.z, r[0] * o.x + r[1] * o.y + r[2] * o.z + r[3]];
    }
    let h = Mat3(m);
    let inv = h.inverse()?;
    inv.0.iter().flatten().all(|v| v.is_finite()).then_some(inv)
}

/// Whether `p` is inside the convex quad.
fn in_quad(q: &[Pos2; 4], p: Pos2) -> bool {
    let side = |a: Pos2, b: Pos2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    let s: Vec<f32> = (0..4).map(|i| side(q[i], q[(i + 1) % 4])).collect();
    s.iter().all(|v| *v >= 0.0) || s.iter().all(|v| *v <= 0.0)
}

fn quad_area(q: &[Pos2; 4]) -> f32 {
    (0..4).map(|i| q[i].x * q[(i + 1) % 4].y - q[(i + 1) % 4].x * q[i].y).sum::<f32>().abs() / 2.0
}

/// Draws the gizmo of the selected 3D layer `l` and records what can be grabbed.
///
/// `comp` is the comp's size in px, `hover` the pointer, `alt` whether Alt is down (Orientation
/// rings instead of Rotation rings).
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    app: &mut EffectcraftApp,
    painter: &Painter,
    map: &ViewerMap,
    ectx: &EvalCtx,
    l: &Layer,
    comp: [f64; 2],
    hover: Option<Pos2>,
    alt: bool,
    axis_hits: &mut Vec<AxisHit>,
    hits: &mut Hits,
) {
    if !l.is_3d() {
        return;
    }
    let Some(tr) = l.transform() else { return };
    let rig = l.is_camera() || l.is_light();
    // Cameras and lights have no anchor point: the gizmo sits on their position.
    let a = if rig { [0.0; 3] } else { ectx.v3(l, tr, "anchor", [0.0; 3]) };
    let a3 = vec3(a[0], a[1], a[2]);
    let zoom = f64::from(map.zoom);
    let cam = cam_state(ectx);
    let proj = cam.projection(comp[0], comp[1]);
    let w = ectx.world_matrix(l);
    let origin = w.apply(a3);
    let rotate = app.ui.tool == Tool::Rotate;
    if rotate {
        draw_rings(app, painter, map, ectx, l, &cam, &proj, origin, zoom, hover, alt, hits);
    } else {
        draw_arrows(app, painter, map, ectx, l, &proj, &w, a3, zoom, axis_hits);
        draw_planes(app, painter, map, ectx, l, &proj, &w, a3, zoom, hits);
    }
}

/// The move arrows (clipped at the camera's near plane, #572).
#[allow(clippy::too_many_arguments)]
fn draw_arrows(
    app: &mut EffectcraftApp,
    painter: &Painter,
    map: &ViewerMap,
    ectx: &EvalCtx,
    l: &Layer,
    proj: &Mat4,
    w: &Mat4,
    a3: Vec3,
    zoom: f64,
    axis_hits: &mut Vec<AxisHit>,
) {
    let pc = *proj * *w;
    let len = ARROW / zoom;
    let pinv = parent_world_inverse(ectx, l);
    for (i, (axis, col)) in [vec3(len, 0.0, 0.0), vec3(0.0, len, 0.0), vec3(0.0, 0.0, len)].into_iter().zip(AXIS_COLORS).enumerate() {
        let Some([o, e]) = project_segment(&pc, a3, a3 + axis) else { continue };
        // An axis pointing at the camera (Z in the default front view) projects to a dot: draw
        // it as a short diagonal stub so it can still be grabbed.
        let mut s = [e[0] - o[0], e[1] - o[1]];
        if (s[0] * s[0] + s[1] * s[1]).sqrt() * zoom < 12.0 {
            let k = 24.0 / zoom;
            s = [k, -k];
        }
        let (so, se) = (map.to_screen(o), map.to_screen([o[0] + s[0], o[1] + s[1]]));
        painter.arrow(so, se - so, Stroke::new(2.0, col));
        if !l.switches.locked {
            let dir = pinv.apply_vec(w.apply_vec(axis));
            let (ax, ay) = axis_drag([dir.x, dir.y, dir.z], s);
            // The inner part stays on the anchor point, which drags the layer freely.
            axis_hits.push((l.id, [so + (se - so) * 0.35, se], ax, ay));
            let name = AXIS_NAMES.get(i).copied().unwrap_or("x");
            app.auto.add(&format!("viewer.gizmo.{}.{name}", l.id.0), Rect::from_center_size(se, vec2(12.0, 12.0)), "3D axis");
        }
    }
}

/// The plane squares between the arrows.
#[allow(clippy::too_many_arguments)]
fn draw_planes(
    app: &mut EffectcraftApp,
    painter: &Painter,
    map: &ViewerMap,
    ectx: &EvalCtx,
    l: &Layer,
    proj: &Mat4,
    w: &Mat4,
    a3: Vec3,
    zoom: f64,
    hits: &mut Hits,
) {
    let len = ARROW / zoom;
    let o = w.apply(a3);
    let axes = [w.apply_vec(vec3(len, 0.0, 0.0)), w.apply_vec(vec3(0.0, len, 0.0)), w.apply_vec(vec3(0.0, 0.0, len))];
    let pinv = parent_world_inverse(ectx, l);
    let Some(tr) = l.transform() else { return };
    let start_pos = ectx.v3(l, tr, "position", [0.0; 3]);
    for ([ia, ib], name, normal) in PLANES {
        let (Some(ua), Some(ub)) = (axes.get(ia).copied(), axes.get(ib).copied()) else { continue };
        let at = |s: f64, t: f64| project(proj, vec3(o.x + ua.x * s + ub.x * t, o.y + ua.y * s + ub.y * t, o.z + ua.z * s + ub.z * t)).map(|p| map.to_screen(p));
        let (Some(p0), Some(p1), Some(p2), Some(p3)) = (at(0.25, 0.25), at(0.55, 0.25), at(0.55, 0.55), at(0.25, 0.55)) else { continue };
        let quad = [p0, p1, p2, p3];
        // Edge-on to the view: nothing to grab, and the sweep has no solution.
        if quad_area(&quad) < 40.0 {
            continue;
        }
        let col = AXIS_COLORS.get(normal).copied().unwrap_or(Color32::WHITE);
        let hot = hover.is_some_and(|h| in_quad(&quad, h));
        painter.add(egui::Shape::convex_polygon(quad.to_vec(), col.gamma_multiply(if hot { 0.6 } else { 0.32 }), Stroke::new(1.0, col)));
        if l.switches.locked {
            continue;
        }
        let Some(inv) = plane_homography(proj, o, ua, ub) else { continue };
        let (da, db) = (pinv.apply_vec(ua), pinv.apply_vec(ub));
        hits.planes.push(PlaneHit { layer: l.id, quad, start_pos, inv, dirs: [[da.x, da.y, da.z], [db.x, db.y, db.z]] });
        let centre = pos2((p0.x + p2.x) / 2.0, (p0.y + p2.y) / 2.0);
        app.auto.add(&format!("viewer.gizmo.{}.plane.{name}", l.id.0), Rect::from_center_size(centre, vec2(12.0, 12.0)), "3D plane");
    }
}

/// The X / Y / Z rotation rings, the back half of each dimmed.
#[allow(clippy::too_many_arguments)]
fn draw_rings(
    app: &mut EffectcraftApp,
    painter: &Painter,
    map: &ViewerMap,
    ectx: &EvalCtx,
    l: &Layer,
    cam: &three_d::CameraState,
    proj: &Mat4,
    origin: Vec3,
    zoom: f64,
    hover: Option<Pos2>,
    alt: bool,
    hits: &mut Hits,
) {
    let Some((axes, start_vals)) = ring_axes(ectx, l, alt) else { return };
    // World units per comp px at the layer's depth, so the rings keep their screen size.
    let k = if cam.ortho { 1.0 / cam.zoom.max(1e-9) } else { cam.depth(origin).max(1.0) / cam.zoom.max(1e-9) };
    let radius = f64::from(SIZE) / zoom * k;
    let fwd = cam.forward();
    let mut drawn: Vec<(usize, Vec<Option<Pos2>>, Vec<Option<[f64; 2]>>, Vec<bool>)> = Vec::new();
    for (axis_i, axis) in axes.iter().enumerate() {
        let (u, v) = plane_basis(*axis);
        let (mut screen, mut comp, mut back) = (Vec::new(), Vec::new(), Vec::new());
        for s in 0..RING_STEPS {
            let t = s as f64 / RING_STEPS as f64 * std::f64::consts::TAU;
            let (sn, cs) = t.sin_cos();
            let d = vec3(u.x * cs + v.x * sn, u.y * cs + v.y * sn, u.z * cs + v.z * sn);
            let p = vec3(origin.x + d.x * radius, origin.y + d.y * radius, origin.z + d.z * radius);
            let c = project(proj, p);
            comp.push(c);
            screen.push(c.map(|c| map.to_screen(c)));
            back.push(d.dot(fwd) > 0.0);
        }
        drawn.push((axis_i, screen, comp, back));
    }
    // Rings nearest the pointer light up.
    let near = |screen: &[Option<Pos2>], p: Pos2| ring_distance(screen, p).map(|(d, _)| d);
    let hot = hover.and_then(|h| drawn.iter().filter_map(|(i, s, ..)| near(s, h).filter(|d| *d < RING_GRAB).map(|d| (*i, d))).min_by(|a, b| a.1.total_cmp(&b.1))).map(|(i, _)| i);
    for (axis_i, screen, comp, back) in drawn {
        let col = AXIS_COLORS.get(axis_i).copied().unwrap_or(Color32::WHITE);
        let width = if hot == Some(axis_i) { 3.5 } else { 2.0 };
        for i in 0..screen.len() {
            let j = (i + 1) % screen.len();
            if let (Some(Some(a)), Some(Some(b))) = (screen.get(i), screen.get(j)) {
                let dim = back.get(i).copied().unwrap_or(false);
                painter.line_segment([*a, *b], Stroke::new(width, if dim { col.gamma_multiply(0.35) } else { col }));
            }
        }
        if l.switches.locked {
            continue;
        }
        if let Some(Some(p)) = screen.first() {
            let name = AXIS_NAMES.get(axis_i).copied().unwrap_or("x");
            let which = if alt { "orientation" } else { "rotate" };
            app.auto.add(&format!("viewer.gizmo.{}.{which}.{name}", l.id.0), Rect::from_center_size(*p, vec2(12.0, 12.0)), "3D rotation ring");
        }
        hits.rings.push(RingHit { layer: l.id, axis: axis_i, orientation: alt, start_vals, screen, comp });
    }
}

/// Distance from `p` to the closed polyline and the index of its nearest segment.
fn ring_distance(screen: &[Option<Pos2>], p: Pos2) -> Option<(f32, usize)> {
    let mut best: Option<(f32, usize)> = None;
    for i in 0..screen.len() {
        let (Some(Some(a)), Some(Some(b))) = (screen.get(i), screen.get((i + 1) % screen.len())) else { continue };
        let ab = *b - *a;
        let t = ((p - *a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
        let d = p.distance(*a + ab * t);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best
}

/// The ring under screen point `press` (comp px `at`), ready to drag.
pub(super) fn ring_at(rings: &[RingHit], press: Pos2, at: [f64; 2]) -> Option<RingDrag> {
    let step = std::f64::consts::TAU / RING_STEPS as f64;
    let (ring, i) = rings.iter().filter_map(|r| ring_distance(&r.screen, press).filter(|(d, _)| *d < RING_GRAB).map(|(d, i)| (r, i, d))).min_by(|a, b| a.2.total_cmp(&b.2)).map(|(r, i, _)| (r, i))?;
    let a = (*ring.comp.get(i)?)?;
    let b = (*ring.comp.get((i + 1) % ring.comp.len())?)?;
    Some(RingDrag {
        layer: ring.layer,
        axis: ring.axis,
        orientation: ring.orientation,
        start_vals: ring.start_vals,
        start: at,
        tangent: [(b[0] - a[0]) / step, (b[1] - a[1]) / step],
    })
}

/// The degrees a pointer move of `d` comp px turns a ring grabbed where it runs along `tangent`
/// (comp px per radian); `snap` rounds to 15 degrees.
fn ring_degrees(tangent: [f64; 2], d: [f64; 2], snap: bool) -> Option<f64> {
    let t2 = tangent[0] * tangent[0] + tangent[1] * tangent[1];
    if !(t2 > 1e-9) {
        return None;
    }
    let deg = ((d[0] * tangent[0] + d[1] * tangent[1]) / t2).to_degrees();
    let deg = if snap { (deg / SNAP_DEG).round() * SNAP_DEG } else { deg };
    deg.is_finite().then_some(deg)
}

pub(super) fn apply_ring(app: &mut EffectcraftApp, d: &RingDrag, cpt: [f64; 2], shift: bool, merge: &str) {
    let Some(deg) = ring_degrees(d.tangent, [cpt[0] - d.start[0], cpt[1] - d.start[1]], shift) else { return };
    let mut v = d.start_vals;
    if let Some(x) = v.get_mut(d.axis) {
        *x += deg;
    }
    let (path, value) = if d.orientation {
        ("transform/orientation", json!(v))
    } else {
        (["transform/rotationX", "transform/rotationY", "transform/rotation"].get(d.axis).copied().unwrap_or("transform/rotation"), json!(v.get(d.axis).copied().unwrap_or(0.0)))
    };
    let _ = app.session.execute("prop.set", json!({"layer": d.layer.0, "path": path, "value": value, "merge": merge}));
}

/// The plane square under screen point `press` (comp px `at`), ready to drag.
pub(super) fn plane_at(planes: &[PlaneHit], press: Pos2, at: [f64; 2]) -> Option<PlaneDrag> {
    let h = planes.iter().find(|p| in_quad(&p.quad, press))?;
    let st = h.inv.apply(gv2(at[0], at[1]));
    (st.x.is_finite() && st.y.is_finite()).then_some(PlaneDrag { layer: h.layer, start_pos: h.start_pos, inv: h.inv, dirs: h.dirs, start_st: [st.x, st.y] })
}

/// The position after sweeping a plane drag to comp px `cpt`: the pointer's point on the plane
/// minus the grabbed point, along the plane's two axes. Shift keeps to the axis moved most.
fn plane_position(d: &PlaneDrag, cpt: [f64; 2], shift: bool) -> Option<[f64; 3]> {
    let st = d.inv.apply(gv2(cpt[0], cpt[1]));
    // A plane seen nearly edge-on throws the pointer's point far away: hold still instead.
    let (mut ds, mut dt) = (st.x - d.start_st[0], st.y - d.start_st[1]);
    if !(ds.is_finite() && dt.is_finite()) || ds.abs() > 50.0 || dt.abs() > 50.0 {
        return None;
    }
    if shift {
        if ds.abs() >= dt.abs() {
            dt = 0.0;
        } else {
            ds = 0.0;
        }
    }
    let [a, b] = d.dirs;
    Some([0, 1, 2].map(|i| d.start_pos[i] + a[i] * ds + b[i] * dt))
}

pub(super) fn apply_plane(app: &mut EffectcraftApp, d: &PlaneDrag, cpt: [f64; 2], shift: bool, merge: &str) {
    let Some(p) = plane_position(d, cpt, shift) else { return };
    let _ = app.session.execute("prop.set", json!({"layer": d.layer.0, "path": "transform/position", "value": p, "merge": merge}));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pinhole camera at the origin looking down +z, 1000 px focal length, centred on (320, 180).
    fn pinhole() -> Mat4 {
        Mat4([[1000.0, 0.0, 320.0, 0.0], [0.0, 1000.0, 180.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 1.0, 0.0]])
    }

    #[test]
    fn plane_sweep_follows_the_pointer_on_the_plane() {
        let p = pinhole();
        let o = vec3(0.0, 0.0, 1000.0);
        // The XY plane of a layer 1000 px away, tilted about Y so perspective matters.
        let (ua, ub) = (vec3(80.0, 0.0, 40.0), vec3(0.0, 80.0, 0.0));
        let inv = plane_homography(&p, o, ua, ub).expect("not edge-on");
        for (s, t) in [(0.0, 0.0), (0.5, 0.25), (-1.0, 2.0), (3.0, -1.5)] {
            let world = vec3(o.x + ua.x * s + ub.x * t, o.y + ua.y * s + ub.y * t, o.z + ua.z * s + ub.z * t);
            let px = project(&p, world).expect("in front");
            let back = inv.apply(gv2(px[0], px[1]));
            assert!((back.x - s).abs() < 1e-6 && (back.y - t).abs() < 1e-6, "({s}, {t}) came back as ({}, {})", back.x, back.y);
        }
    }

    #[test]
    fn an_edge_on_plane_has_no_sweep() {
        // A plane containing the view direction projects to a line.
        let p = pinhole();
        assert!(plane_homography(&p, vec3(0.0, 0.0, 1000.0), vec3(0.0, 0.0, 100.0), vec3(100.0, 0.0, 0.0)).is_none());
    }

    #[test]
    fn plane_drag_moves_by_the_swept_distance_and_shift_locks_an_axis() {
        let p = pinhole();
        let o = vec3(0.0, 0.0, 1000.0);
        let (ua, ub) = (vec3(100.0, 0.0, 0.0), vec3(0.0, 100.0, 0.0));
        let inv = plane_homography(&p, o, ua, ub).expect("not edge-on");
        let start = project(&p, o).expect("in front");
        let d = PlaneDrag { layer: LayerId(1), start_pos: [10.0, 20.0, 30.0], inv, dirs: [[100.0, 0.0, 0.0], [0.0, 100.0, 0.0]], start_st: [0.0, 0.0] };
        // 100 px right on a plane 1000 px away at 1000 px focal length is 100 world px: one unit.
        let r = plane_position(&d, [start[0] + 100.0, start[1] + 50.0], false).expect("moves");
        assert!((r[0] - 110.0).abs() < 1e-6 && (r[1] - 70.0).abs() < 1e-6 && (r[2] - 30.0).abs() < 1e-9, "{r:?}");
        let r = plane_position(&d, [start[0] + 100.0, start[1] + 50.0], true).expect("moves");
        assert!((r[0] - 110.0).abs() < 1e-6 && (r[1] - 20.0).abs() < 1e-6, "shift keeps to x: {r:?}");
    }

    #[test]
    fn ring_turn_follows_the_pointer_along_the_ring_and_snaps() {
        // A ring that runs 100 comp px per radian to the right where it was grabbed.
        let t = [100.0, 0.0];
        let d = ring_degrees(t, [100.0, 30.0], false).expect("turns");
        assert!((d - 1.0_f64.to_degrees()).abs() < 1e-9, "one radian: {d}");
        assert_eq!(ring_degrees(t, [16.0, 0.0], true), Some(15.0), "9.2 degrees snaps to 15");
        assert_eq!(ring_degrees(t, [-100.0, 0.0], true), Some(-60.0), "-57.3 snaps to -60");
        assert_eq!(ring_degrees([0.0, 0.0], [10.0, 10.0], false), None, "a ring seen edge-on at the grab point holds still");
    }

    #[test]
    fn ring_basis_is_right_handed_and_perpendicular() {
        for axis in [vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, 1.0), unit(vec3(1.0, 2.0, -0.5)).expect("unit")] {
            let (u, v) = plane_basis(axis);
            assert!(u.dot(axis).abs() < 1e-9 && v.dot(axis).abs() < 1e-9 && u.dot(v).abs() < 1e-9);
            let c = u.cross(v);
            assert!((c.x - axis.x).abs() < 1e-9 && (c.y - axis.y).abs() < 1e-9 && (c.z - axis.z).abs() < 1e-9, "u x v = axis");
        }
    }

    #[test]
    fn ring_hit_test_picks_the_nearest_ring() {
        let circle = |r: f32| -> Vec<Option<Pos2>> { (0..RING_STEPS).map(|i| { let t = i as f32 / RING_STEPS as f32 * std::f32::consts::TAU; Some(pos2(r * t.cos(), r * t.sin())) }).collect() };
        let ring = |axis: usize, r: f32| {
            let screen = circle(r);
            let comp = screen.iter().map(|p| p.map(|p| [f64::from(p.x), f64::from(p.y)])).collect();
            RingHit { layer: LayerId(3), axis, orientation: false, start_vals: [1.0, 2.0, 3.0], screen, comp }
        };
        let rings = [ring(0, 50.0), ring(2, 60.0)];
        let hit = ring_at(&rings, pos2(61.0, 0.0), [61.0, 0.0]).expect("on the outer ring");
        assert_eq!(hit.axis, 2);
        // Counter-clockwise in the polyline's own orientation: the tangent at (r, 0) points down.
        assert!(hit.tangent[1] > 0.0 && hit.tangent[0].abs() < 12.0, "{:?}", hit.tangent);
        assert!(ring_at(&rings, pos2(0.0, 0.0), [0.0, 0.0]).is_none(), "the middle grabs nothing");
    }
}
