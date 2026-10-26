//! Toolbar and button icons drawn as vectors on a 16x16 grid, so they stay
//! crisp at any DPI and follow the light/dark theme.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Refresh,
    Brief,
    Full,
    QuickView,
    Thumbs,
    Branch,
    Invert,
    Back,
    Forward,
    Pack,
    Unpack,
    Search,
    MultiRename,
    Compare,
    CopyNames,
    Hidden,
    Theme,
    Terminal,
    View,
    Edit,
    Copy,
    Move,
    MkDir,
    Delete,
    Hotlist,
    Network,
    ParentDir,
    Folder,
    File,
    Tree,
    Gear,
    /// A button without a picture of its own.
    Command,
    /// A button that opens another bar as a menu.
    Menu,
}

struct G {
    r: Rect,
    k: f32,
}

impl G {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        pos2(self.r.left() + x * self.k, self.r.top() + y * self.k)
    }
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(self.p(x0, y0), self.p(x1, y1))
    }
    fn s(&self, w: f32, c: Color32) -> Stroke {
        Stroke::new((w * self.k).max(1.0), c)
    }
}

const BLUE: Color32 = Color32::from_rgb(0x2B, 0x6C, 0xC4);
const LIGHT_BLUE: Color32 = Color32::from_rgb(0x8F, 0xC1, 0xF2);
const GREEN: Color32 = Color32::from_rgb(0x2E, 0x9E, 0x3E);
const DARK_GREEN: Color32 = Color32::from_rgb(0x1E, 0x6E, 0x2A);
const YELLOW: Color32 = Color32::from_rgb(0xF4, 0xC4, 0x3A);
const DARK_YELLOW: Color32 = Color32::from_rgb(0xC8, 0x92, 0x10);
const RED: Color32 = Color32::from_rgb(0xD8, 0x3B, 0x2B);
const BROWN: Color32 = Color32::from_rgb(0xC4, 0x8A, 0x3C);
const GREY: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);

fn folder(pt: &Painter, g: &G, x: f32, y: f32, w: f32, h: f32) {
    let tab = vec![g.p(x, y + 1.0), g.p(x + w * 0.4, y + 1.0), g.p(x + w * 0.5, y + 2.2), g.p(x, y + 2.2)];
    pt.add(Shape::convex_polygon(tab, DARK_YELLOW, Stroke::NONE));
    pt.rect_filled(g.rect(x, y + 2.0, x + w, y + h), 1.0, YELLOW);
    pt.rect_stroke(g.rect(x, y + 2.0, x + w, y + h), 1.0, g.s(0.7, DARK_YELLOW), StrokeKind::Inside);
}

fn page(pt: &Painter, g: &G, x: f32, y: f32, w: f32, h: f32, fill: Color32, edge: Color32) {
    let fold = w * 0.3;
    let pts = vec![g.p(x, y), g.p(x + w - fold, y), g.p(x + w, y + fold), g.p(x + w, y + h), g.p(x, y + h)];
    pt.add(Shape::convex_polygon(pts, fill, g.s(0.8, edge)));
    pt.line_segment([g.p(x + w - fold, y), g.p(x + w - fold, y + fold)], g.s(0.6, edge));
    pt.line_segment([g.p(x + w - fold, y + fold), g.p(x + w, y + fold)], g.s(0.6, edge));
}

fn arrow(pt: &Painter, g: &G, from: (f32, f32), to: (f32, f32), c: Color32, w: f32) {
    let a = g.p(from.0, from.1);
    let b = g.p(to.0, to.1);
    let dir = (b - a).normalized();
    let n = vec2(-dir.y, dir.x);
    let head = 3.2 * g.k;
    let base = b - dir * head;
    pt.line_segment([a, base], Stroke::new(w * g.k, c));
    pt.add(Shape::convex_polygon(vec![b, base + n * head * 0.75, base - n * head * 0.75], c, Stroke::NONE));
}

/// Paints `icon` into `rect` (square). `fg` is the theme's text colour,
/// used for monochrome parts.
pub fn paint(pt: &Painter, rect: Rect, icon: Icon, fg: Color32) {
    let side = rect.width().min(rect.height());
    let r = Rect::from_center_size(rect.center(), vec2(side, side));
    let g = G { r, k: side / 16.0 };
    let paper = Color32::from_rgb(0xFA, 0xFA, 0xFA);
    match icon {
        Icon::Refresh => {
            let c = g.p(8.0, 8.0);
            let rad = 5.0 * g.k;
            let arc = |start: f32, end: f32| -> Vec<Pos2> {
                (0..=12).map(|i| {
                    let t = start + (end - start) * i as f32 / 12.0;
                    c + vec2(t.cos(), t.sin()) * rad
                }).collect()
            };
            pt.add(Shape::line(arc(3.6, 5.9), g.s(2.0, GREEN)));
            pt.add(Shape::line(arc(0.45, 2.75), g.s(2.0, GREEN)));
            let tip1 = c + vec2(5.9f32.cos(), 5.9f32.sin()) * rad;
            pt.add(Shape::convex_polygon(vec![tip1 + vec2(2.6, -1.6) * g.k, tip1 + vec2(1.4, 2.8) * g.k, tip1 + vec2(-2.4, 0.2) * g.k], GREEN, Stroke::NONE));
            let tip2 = c + vec2(2.75f32.cos(), 2.75f32.sin()) * rad;
            pt.add(Shape::convex_polygon(vec![tip2 + vec2(-2.6, 1.6) * g.k, tip2 + vec2(-1.4, -2.8) * g.k, tip2 + vec2(2.4, -0.2) * g.k], GREEN, Stroke::NONE));
        }
        Icon::Brief => {
            for col in 0..3 {
                for row in 0..4 {
                    let x = 1.5 + col as f32 * 4.8;
                    let y = 1.5 + row as f32 * 3.4;
                    pt.rect_filled(g.rect(x, y, x + 1.8, y + 1.8), 0.0, BLUE);
                    pt.line_segment([g.p(x + 2.3, y + 0.9), g.p(x + 4.0, y + 0.9)], g.s(0.9, GREY));
                }
            }
        }
        Icon::Full => {
            for row in 0..5 {
                let y = 1.5 + row as f32 * 2.8;
                pt.rect_filled(g.rect(1.5, y, 3.5, y + 2.0), 0.0, BLUE);
                pt.line_segment([g.p(4.6, y + 1.0), g.p(14.5, y + 1.0)], g.s(1.0, GREY));
            }
        }
        Icon::Thumbs => {
            pt.rect_filled(g.rect(1.0, 2.0, 15.0, 14.0), 1.0, Color32::from_rgb(0x5A, 0x9B, 0x5A));
            pt.add(Shape::convex_polygon(vec![g.p(2.0, 13.0), g.p(7.0, 6.0), g.p(11.0, 13.0)], Color32::from_rgb(0x3E, 0x6B, 0x3E), Stroke::NONE));
            pt.circle_filled(g.p(11.5, 5.5), 1.8 * g.k, YELLOW);
            pt.rect_stroke(g.rect(1.0, 2.0, 15.0, 14.0), 1.0, g.s(1.2, BROWN), StrokeKind::Inside);
        }
        Icon::QuickView => {
            // Two panels; the right one previews a picture.
            pt.rect_stroke(g.rect(0.5, 2.0, 15.5, 14.0), 1.0, g.s(1.0, GREY), StrokeKind::Inside);
            pt.line_segment([g.p(8.0, 2.0), g.p(8.0, 14.0)], g.s(1.0, GREY));
            for i in 0..4 {
                let y = 4.5 + i as f32 * 2.4;
                pt.line_segment([g.p(2.0, y), g.p(6.5, y)], g.s(0.8, BLUE));
            }
            pt.rect_filled(g.rect(9.2, 4.0, 14.3, 12.0), 0.0, Color32::from_rgb(0x5A, 0x9B, 0x5A));
            pt.circle_filled(g.p(12.6, 5.8), 0.9 * g.k, YELLOW);
            pt.add(Shape::convex_polygon(vec![g.p(9.2, 12.0), g.p(11.0, 8.0), g.p(13.0, 12.0)], Color32::from_rgb(0x3E, 0x6B, 0x3E), Stroke::NONE));
        }
        Icon::Branch => {
            folder(pt, &g, 1.0, 0.5, 7.0, 5.5);
            folder(pt, &g, 7.5, 5.5, 7.0, 5.0);
            folder(pt, &g, 7.5, 10.5, 7.0, 5.0);
            pt.line_segment([g.p(3.5, 6.0), g.p(3.5, 13.5)], g.s(0.9, GREY));
            pt.line_segment([g.p(3.5, 9.0), g.p(7.5, 9.0)], g.s(0.9, GREY));
            pt.line_segment([g.p(3.5, 13.5), g.p(7.5, 13.5)], g.s(0.9, GREY));
        }
        Icon::Invert => {
            pt.rect_stroke(g.rect(1.5, 1.5, 8.5, 8.5), 1.0, g.s(1.0, fg), StrokeKind::Inside);
            pt.line_segment([g.p(3.0, 5.0), g.p(4.6, 6.8)], g.s(1.4, GREEN));
            pt.line_segment([g.p(4.6, 6.8), g.p(7.4, 2.8)], g.s(1.4, GREEN));
            pt.rect_stroke(g.rect(7.5, 7.5, 14.5, 14.5), 1.0, g.s(1.0, fg), StrokeKind::Inside);
            arrow(pt, &g, (12.5, 2.5), (12.5, 6.5), RED, 1.2);
            arrow(pt, &g, (3.5, 13.5), (3.5, 9.5), RED, 1.2);
        }
        Icon::Back => {
            pt.add(Shape::convex_polygon(vec![g.p(1.5, 8.0), g.p(8.0, 2.0), g.p(8.0, 14.0)], GREY, Stroke::NONE));
            pt.rect_filled(g.rect(7.5, 5.5, 14.5, 10.5), 0.0, GREY);
        }
        Icon::Forward => {
            pt.add(Shape::convex_polygon(vec![g.p(14.5, 8.0), g.p(8.0, 14.0), g.p(8.0, 2.0)], GREY, Stroke::NONE));
            pt.rect_filled(g.rect(1.5, 5.5, 8.5, 10.5), 0.0, GREY);
        }
        Icon::Pack | Icon::Unpack => {
            pt.rect_filled(g.rect(2.0, 6.0, 14.0, 15.0), 1.0, BROWN);
            pt.rect_filled(g.rect(1.5, 5.0, 14.5, 7.5), 1.0, DARK_YELLOW);
            pt.line_segment([g.p(8.0, 7.5), g.p(8.0, 15.0)], g.s(1.0, DARK_YELLOW));
            pt.line_segment([g.p(2.0, 11.0), g.p(14.0, 11.0)], g.s(1.0, DARK_YELLOW));
            if icon == Icon::Pack {
                arrow(pt, &g, (8.0, 0.0), (8.0, 5.2), RED, 2.0);
            } else {
                arrow(pt, &g, (8.0, 5.2), (8.0, 0.0), GREEN, 2.0);
            }
        }
        Icon::Search => {
            pt.circle_filled(g.p(6.5, 6.5), 4.6 * g.k, LIGHT_BLUE.gamma_multiply(0.5));
            pt.circle_stroke(g.p(6.5, 6.5), 4.6 * g.k, g.s(1.6, fg));
            pt.line_segment([g.p(9.8, 9.8), g.p(14.5, 14.5)], g.s(2.6, fg));
        }
        Icon::MultiRename => {
            pt.text(g.p(0.5, 4.0), egui::Align2::LEFT_CENTER, "ab", egui::FontId::proportional(6.5 * g.k), BLUE);
            pt.line_segment([g.p(1.0, 7.5), g.p(8.0, 7.5)], g.s(0.8, GREY));
            pt.line_segment([g.p(1.0, 10.5), g.p(6.0, 10.5)], g.s(0.8, GREY));
            pt.line_segment([g.p(1.0, 13.5), g.p(7.0, 13.5)], g.s(0.8, GREY));
            pt.line_segment([g.p(14.5, 2.5), g.p(7.5, 12.5)], g.s(2.4, Color32::from_rgb(0xE8, 0x9A, 0x2B)));
            pt.line_segment([g.p(7.5, 12.5), g.p(6.8, 14.5)], g.s(1.2, fg));
            pt.line_segment([g.p(14.5, 2.5), g.p(13.6, 3.8)], g.s(2.4, RED));
        }
        Icon::Compare => {
            folder(pt, &g, 0.5, 1.0, 6.5, 5.5);
            folder(pt, &g, 9.0, 1.0, 6.5, 5.5);
            arrow(pt, &g, (4.0, 9.0), (12.5, 9.0), BLUE, 1.4);
            arrow(pt, &g, (12.0, 13.0), (3.5, 13.0), GREEN, 1.4);
        }
        Icon::CopyNames => {
            pt.rect_filled(g.rect(2.5, 2.0, 13.5, 15.0), 1.0, BROWN);
            pt.rect_filled(g.rect(4.0, 3.5, 12.0, 14.0), 0.0, paper);
            pt.rect_filled(g.rect(5.5, 1.0, 10.5, 3.5), 1.0, GREY);
            for i in 0..4 {
                let y = 6.0 + i as f32 * 2.0;
                pt.line_segment([g.p(5.0, y), g.p(11.0, y)], g.s(0.8, BLUE));
            }
        }
        Icon::Hidden => {
            page(pt, &g, 3.0, 1.0, 10.0, 14.0, paper.gamma_multiply(0.6), GREY);
            pt.circle_filled(g.p(11.5, 11.5), 3.6 * g.k, RED);
            pt.text(g.p(11.5, 11.6), egui::Align2::CENTER_CENTER, "!", egui::FontId::proportional(6.0 * g.k), Color32::WHITE);
        }
        Icon::Theme => {
            pt.circle_filled(g.p(8.0, 8.0), 6.0 * g.k, YELLOW);
            pt.circle_stroke(g.p(8.0, 8.0), 6.0 * g.k, g.s(0.8, DARK_YELLOW));
            // Dark half drawn as a polygon over the right side.
            let c = g.p(8.0, 8.0);
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let t = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 16.0;
                    c + vec2(t.cos(), t.sin()) * 6.0 * g.k
                })
                .collect();
            pt.add(Shape::convex_polygon(pts, Color32::from_rgb(0x2E, 0x36, 0x48), Stroke::NONE));
        }
        Icon::Terminal => {
            pt.rect_filled(g.rect(1.0, 2.0, 15.0, 14.0), 1.5, Color32::from_rgb(0x1E, 0x1E, 0x1E));
            pt.rect_filled(g.rect(1.0, 2.0, 15.0, 4.0), 1.5, Color32::from_rgb(0x55, 0x55, 0x55));
            pt.line_segment([g.p(3.5, 6.5), g.p(6.0, 8.5)], g.s(1.3, Color32::from_rgb(0xD0, 0xD0, 0xD0)));
            pt.line_segment([g.p(6.0, 8.5), g.p(3.5, 10.5)], g.s(1.3, Color32::from_rgb(0xD0, 0xD0, 0xD0)));
            pt.line_segment([g.p(7.5, 11.0), g.p(11.5, 11.0)], g.s(1.3, Color32::from_rgb(0xD0, 0xD0, 0xD0)));
        }
        Icon::View => {
            page(pt, &g, 1.0, 0.5, 10.0, 13.0, paper, GREY);
            for i in 0..3 {
                let y = 4.0 + i as f32 * 2.2;
                pt.line_segment([g.p(2.5, y), g.p(8.5, y)], g.s(0.8, GREY));
            }
            pt.circle_filled(g.p(10.0, 10.0), 3.2 * g.k, LIGHT_BLUE.gamma_multiply(0.6));
            pt.circle_stroke(g.p(10.0, 10.0), 3.2 * g.k, g.s(1.3, BLUE));
            pt.line_segment([g.p(12.3, 12.3), g.p(15.0, 15.0)], g.s(2.0, BLUE));
        }
        Icon::Edit => {
            page(pt, &g, 1.0, 0.5, 10.0, 13.0, paper, GREY);
            for i in 0..3 {
                let y = 4.0 + i as f32 * 2.2;
                pt.line_segment([g.p(2.5, y), g.p(8.5, y)], g.s(0.8, GREY));
            }
            pt.line_segment([g.p(15.0, 5.0), g.p(8.0, 13.5)], g.s(2.6, Color32::from_rgb(0xE8, 0x9A, 0x2B)));
            pt.line_segment([g.p(8.0, 13.5), g.p(7.2, 15.5)], g.s(1.3, fg));
            pt.line_segment([g.p(15.0, 5.0), g.p(14.0, 6.2)], g.s(2.6, RED));
        }
        Icon::Copy => {
            page(pt, &g, 1.0, 0.5, 8.5, 11.0, paper, BLUE);
            page(pt, &g, 6.0, 4.5, 8.5, 11.0, LIGHT_BLUE, BLUE);
        }
        Icon::Move => {
            page(pt, &g, 1.0, 0.5, 8.5, 11.0, paper.gamma_multiply(0.7), GREY);
            page(pt, &g, 6.0, 4.5, 8.5, 11.0, Color32::from_rgb(0xFB, 0xC8, 0xC0), RED);
            arrow(pt, &g, (3.0, 13.5), (8.5, 8.0), RED, 1.4);
        }
        Icon::MkDir => {
            folder(pt, &g, 0.5, 2.5, 13.0, 11.0);
            pt.circle_filled(g.p(12.0, 4.5), 3.6 * g.k, GREEN);
            pt.line_segment([g.p(12.0, 2.6), g.p(12.0, 6.4)], g.s(1.3, Color32::WHITE));
            pt.line_segment([g.p(10.1, 4.5), g.p(13.9, 4.5)], g.s(1.3, Color32::WHITE));
        }
        Icon::Delete => {
            page(pt, &g, 2.0, 0.5, 10.0, 13.0, paper, GREY);
            pt.line_segment([g.p(7.5, 7.5), g.p(14.5, 14.5)], g.s(2.4, RED));
            pt.line_segment([g.p(14.5, 7.5), g.p(7.5, 14.5)], g.s(2.4, RED));
        }
        Icon::Hotlist => {
            let c = g.p(8.0, 8.2);
            let pts: Vec<Pos2> = (0..10)
                .map(|i| {
                    let t = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 5.0;
                    let rr = if i % 2 == 0 { 7.0 } else { 3.0 } * g.k;
                    c + vec2(t.cos(), t.sin()) * rr
                })
                .collect();
            pt.add(Shape::convex_polygon(pts, YELLOW, g.s(0.8, DARK_YELLOW)).into_concave_fix());
        }
        Icon::Network => {
            pt.rect_filled(g.rect(1.0, 1.5, 11.0, 8.5), 1.0, Color32::from_rgb(0x3B, 0x7D, 0xC9));
            pt.rect_filled(g.rect(2.0, 2.5, 10.0, 7.5), 0.0, LIGHT_BLUE);
            pt.rect_filled(g.rect(4.5, 8.5, 7.5, 10.0), 0.0, GREY);
            pt.rect_filled(g.rect(2.5, 10.0, 9.5, 11.0), 0.0, GREY);
            pt.circle_filled(g.p(11.5, 11.5), 4.0 * g.k, Color32::from_rgb(0x2D, 0x9C, 0xDB));
            pt.circle_stroke(g.p(11.5, 11.5), 4.0 * g.k, g.s(0.7, DARK_GREEN));
            pt.line_segment([g.p(7.5, 11.5), g.p(15.5, 11.5)], g.s(0.6, Color32::WHITE));
            pt.line_segment([g.p(11.5, 7.5), g.p(11.5, 15.5)], g.s(0.6, Color32::WHITE));
        }
        Icon::ParentDir => {
            let c = Color32::from_rgb(0x90, 0x9A, 0xA8);
            pt.add(Shape::convex_polygon(vec![g.p(8.0, 1.5), g.p(14.0, 8.0), g.p(2.0, 8.0)], c, Stroke::NONE));
            pt.rect_filled(g.rect(5.5, 7.5, 10.5, 14.5), 0.0, c);
        }
        Icon::Folder => folder(pt, &g, 0.5, 1.5, 15.0, 13.0),
        Icon::File => page(pt, &g, 2.5, 0.5, 11.0, 15.0, paper, GREY),
        Icon::Tree => {
            folder(pt, &g, 0.5, 0.5, 6.5, 5.0);
            folder(pt, &g, 6.0, 5.5, 6.0, 4.5);
            folder(pt, &g, 9.5, 11.0, 6.0, 4.5);
            pt.line_segment([g.p(2.5, 5.5), g.p(2.5, 8.5)], g.s(0.9, GREY));
            pt.line_segment([g.p(2.5, 8.5), g.p(6.0, 8.5)], g.s(0.9, GREY));
            pt.line_segment([g.p(8.0, 10.0), g.p(8.0, 13.5)], g.s(0.9, GREY));
            pt.line_segment([g.p(8.0, 13.5), g.p(9.5, 13.5)], g.s(0.9, GREY));
        }
        Icon::Gear => {
            let c = g.p(8.0, 8.0);
            let teeth: Vec<Pos2> = (0..32)
                .map(|i| {
                    let t = std::f32::consts::TAU * i as f32 / 32.0;
                    let rr = if (i / 2) % 2 == 0 { 7.2 } else { 5.4 } * g.k;
                    c + vec2(t.cos(), t.sin()) * rr
                })
                .collect();
            pt.add(Shape::convex_polygon(teeth, GREY, Stroke::NONE).into_concave_fix());
            pt.circle_filled(c, 4.6 * g.k, Color32::from_rgb(0x9A, 0x9A, 0x9A));
            pt.circle_filled(c, 2.0 * g.k, paper);
        }
        Icon::Command => {
            pt.rect(g.rect(1.5, 3.0, 14.5, 13.0), 2.0 * g.k, LIGHT_BLUE.gamma_multiply(0.35), g.s(1.0, BLUE), StrokeKind::Inside);
            for x in [5.0, 8.0, 11.0] {
                pt.circle_filled(g.p(x, 8.0), 1.1 * g.k, BLUE);
            }
        }
        Icon::Menu => {
            pt.rect(g.rect(1.0, 2.0, 15.0, 11.0), 1.0, paper, g.s(1.0, GREY), StrokeKind::Inside);
            for y in [4.5, 6.8, 9.0] {
                pt.line_segment([g.p(3.0, y), g.p(12.5, y)], g.s(0.9, BLUE));
            }
            pt.add(Shape::convex_polygon(vec![g.p(9.0, 12.0), g.p(15.0, 12.0), g.p(12.0, 15.5)], fg, Stroke::NONE));
        }
    }
}

/// Concave polygons are not supported by `convex_polygon`; triangulate
/// star-shaped polygons as a fan from their centroid instead.
trait ConcaveFix {
    fn into_concave_fix(self) -> Shape;
}

impl ConcaveFix for Shape {
    fn into_concave_fix(self) -> Shape {
        let Shape::Path(path) = self else { return self };
        let n = path.points.len() as f32;
        let centroid = path.points.iter().fold(Pos2::ZERO, |acc, p| acc + p.to_vec2() / n);
        let fill = path.fill;
        let mut shapes = Vec::new();
        for i in 0..path.points.len() {
            let a = path.points[i];
            let b = path.points[(i + 1) % path.points.len()];
            shapes.push(Shape::convex_polygon(vec![centroid, a, b], fill, Stroke::NONE));
        }
        if path.stroke.width > 0.0 {
            let mut pts = path.points.clone();
            pts.push(path.points[0]);
            shapes.push(Shape::line(pts, path.stroke.clone()));
        }
        Shape::Vec(shapes)
    }
}
