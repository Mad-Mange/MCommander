//! Light and dark palettes. The light palette is sampled from Total
//! Commander 11 on Windows 11; the dark one follows TC's dark mode.

use egui::{Color32, CornerRadius, Stroke, Visuals};

#[derive(Clone, Debug)]
pub struct Palette {
    pub dark: bool,
    /// Window chrome: menu, toolbar and drive bars.
    pub bar: Color32,
    /// Dialog / status line / F-key bar background.
    pub face: Color32,
    pub list_bg: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub marked: Color32,
    pub cursor_fill: Color32,
    pub cursor_border: Color32,
    pub header_active: Color32,
    pub header_inactive: Color32,
    pub header_text: Color32,
    pub col_header_bg: Color32,
    pub col_header_sorted: Color32,
    pub col_sep: Color32,
    pub border: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub button_border: Color32,
    pub input_bg: Color32,
    pub accent: Color32,
    pub error: Color32,
    pub tab_active: Color32,
    pub tab_inactive: Color32,
    pub scrollbar_track: Color32,
    pub scrollbar_thumb: Color32,
}

impl Palette {
    pub fn light() -> Palette {
        Palette {
            dark: false,
            bar: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            face: Color32::from_rgb(0xF0, 0xF0, 0xF0),
            list_bg: Color32::WHITE,
            text: Color32::BLACK,
            text_dim: Color32::from_rgb(0x6D, 0x6D, 0x6D),
            marked: Color32::from_rgb(0xE8, 0x11, 0x23),
            cursor_fill: Color32::from_rgb(0xCC, 0xE8, 0xFF),
            cursor_border: Color32::from_rgb(0x1E, 0x1E, 0x1E),
            header_active: Color32::from_rgb(0x99, 0xB4, 0xD1),
            header_inactive: Color32::from_rgb(0xBF, 0xCD, 0xDB),
            header_text: Color32::BLACK,
            col_header_bg: Color32::WHITE,
            col_header_sorted: Color32::from_rgb(0xBC, 0xDC, 0xF4),
            col_sep: Color32::from_rgb(0xE5, 0xE5, 0xE5),
            border: Color32::from_rgb(0xA0, 0xA0, 0xA0),
            hover: Color32::from_rgb(0xE5, 0xF3, 0xFF),
            pressed: Color32::from_rgb(0xCC, 0xE8, 0xFF),
            button_border: Color32::from_rgb(0x99, 0xD1, 0xFF),
            input_bg: Color32::WHITE,
            accent: Color32::from_rgb(0x00, 0x78, 0xD4),
            error: Color32::from_rgb(0xC4, 0x2B, 0x1C),
            tab_active: Color32::WHITE,
            tab_inactive: Color32::from_rgb(0xE6, 0xE6, 0xE6),
            scrollbar_track: Color32::from_rgb(0xF0, 0xF0, 0xF0),
            scrollbar_thumb: Color32::from_rgb(0xC2, 0xC2, 0xC2),
        }
    }

    pub fn dark() -> Palette {
        Palette {
            dark: true,
            bar: Color32::from_rgb(0x2B, 0x2B, 0x2B),
            face: Color32::from_rgb(0x20, 0x20, 0x20),
            list_bg: Color32::from_rgb(0x19, 0x19, 0x19),
            text: Color32::from_rgb(0xE8, 0xE8, 0xE8),
            text_dim: Color32::from_rgb(0x9A, 0x9A, 0x9A),
            marked: Color32::from_rgb(0xFF, 0x6B, 0x6B),
            cursor_fill: Color32::from_rgb(0x26, 0x4F, 0x78),
            cursor_border: Color32::from_rgb(0x8C, 0xB4, 0xDC),
            header_active: Color32::from_rgb(0x3D, 0x5A, 0x80),
            header_inactive: Color32::from_rgb(0x33, 0x3B, 0x45),
            header_text: Color32::from_rgb(0xF0, 0xF0, 0xF0),
            col_header_bg: Color32::from_rgb(0x2B, 0x2B, 0x2B),
            col_header_sorted: Color32::from_rgb(0x34, 0x4B, 0x63),
            col_sep: Color32::from_rgb(0x3C, 0x3C, 0x3C),
            border: Color32::from_rgb(0x50, 0x50, 0x50),
            hover: Color32::from_rgb(0x3A, 0x3A, 0x3A),
            pressed: Color32::from_rgb(0x26, 0x4F, 0x78),
            button_border: Color32::from_rgb(0x5A, 0x7A, 0x9A),
            input_bg: Color32::from_rgb(0x1E, 0x1E, 0x1E),
            accent: Color32::from_rgb(0x4C, 0xA8, 0xFF),
            error: Color32::from_rgb(0xFF, 0x7A, 0x6E),
            tab_active: Color32::from_rgb(0x19, 0x19, 0x19),
            tab_inactive: Color32::from_rgb(0x2B, 0x2B, 0x2B),
            scrollbar_track: Color32::from_rgb(0x20, 0x20, 0x20),
            scrollbar_thumb: Color32::from_rgb(0x55, 0x55, 0x55),
        }
    }

    /// egui visuals matching the palette, so stock widgets (dialogs, text
    /// fields, menus) blend in with the custom-drawn parts.
    pub fn visuals(&self) -> Visuals {
        let mut v = if self.dark { Visuals::dark() } else { Visuals::light() };
        let radius = CornerRadius::same(2);
        v.override_text_color = Some(self.text);
        v.panel_fill = self.face;
        v.window_fill = self.face;
        v.window_stroke = Stroke::new(1.0, self.border);
        v.window_corner_radius = CornerRadius::same(4);
        v.menu_corner_radius = CornerRadius::same(4);
        v.extreme_bg_color = self.input_bg;
        v.text_edit_bg_color = Some(self.input_bg);
        v.faint_bg_color = if self.dark { Color32::from_rgb(0x24, 0x24, 0x24) } else { Color32::from_rgb(0xF7, 0xF7, 0xF7) };
        v.selection.bg_fill = if self.dark { Color32::from_rgb(0x26, 0x4F, 0x78) } else { Color32::from_rgb(0x99, 0xC9, 0xEF) };
        v.selection.stroke = Stroke::new(1.0, self.text);
        v.hyperlink_color = self.accent;
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = self.face;
        w.noninteractive.weak_bg_fill = self.face;
        w.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
        let button = if self.dark { Color32::from_rgb(0x33, 0x33, 0x33) } else { Color32::from_rgb(0xFD, 0xFD, 0xFD) };
        let button_border = if self.dark { Color32::from_rgb(0x55, 0x55, 0x55) } else { Color32::from_rgb(0xD0, 0xD0, 0xD0) };
        w.inactive.bg_fill = button;
        w.inactive.weak_bg_fill = button;
        w.inactive.bg_stroke = Stroke::new(1.0, button_border);
        w.inactive.fg_stroke = Stroke::new(1.0, self.text);
        w.hovered.bg_fill = self.hover;
        w.hovered.weak_bg_fill = self.hover;
        w.hovered.bg_stroke = Stroke::new(1.0, self.button_border);
        w.hovered.fg_stroke = Stroke::new(1.0, self.text);
        w.active.bg_fill = self.pressed;
        w.active.weak_bg_fill = self.pressed;
        w.active.bg_stroke = Stroke::new(1.0, self.accent);
        w.active.fg_stroke = Stroke::new(1.0, self.text);
        w.open.bg_fill = self.pressed;
        w.open.weak_bg_fill = self.pressed;
        w.open.fg_stroke = Stroke::new(1.0, self.text);
        for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            s.corner_radius = radius;
            s.expansion = 0.0;
        }
        v
    }
}
