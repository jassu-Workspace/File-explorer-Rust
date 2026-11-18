//! Settings sheet (Ctrl+,): a card over the window with Explorer's options and Files' own.
use crate::app::App;
use crate::layout::Rect;
use crate::settings::{ClickMode, PanelStyle};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Click(ClickMode),
    Ext,
    Hidden,
    System,
    PathBar,
    Panel(PanelStyle),
    UseWindows,
    Done,
}

const CARD_W: f32 = 480.0;
const TITLE_H: f32 = 56.0;
const ROW_H: f32 = 48.0;
const FOOT_H: f32 = 64.0;
const PAD: f32 = 20.0;
const ROWS: usize = 6;

fn row_titles() -> [&'static str; ROWS] {
    ["Open items", "Show file name extensions", "Show hidden items", "Show protected operating system files", "Show path bar", "Copy and move style"]
}

/// The card and every control in it, in window coordinates.
pub fn sheet_layout(window: Rect) -> (Rect, Vec<(Action, Rect)>) {
    let h = TITLE_H + ROWS as f32 * ROW_H + FOOT_H;
    let card = Rect::new(window.x + (window.w - CARD_W) / 2.0, window.y + ((window.h - h) / 2.0).max(8.0), CARD_W, h);
    let row = |i: usize| Rect::new(card.x + PAD, card.y + TITLE_H + i as f32 * ROW_H, card.w - 2.0 * PAD, ROW_H);
    let switch = |i: usize| {
        let r = row(i);
        Rect::new(r.right() - 40.0, r.y + (ROW_H - 22.0) / 2.0, 40.0, 22.0)
    };
    let segments = |i: usize, n: usize, w: f32| {
        let r = row(i);
        (0..n).map(move |k| Rect::new(r.right() - n as f32 * w + k as f32 * w, r.y + (ROW_H - 28.0) / 2.0, w, 28.0))
    };
    let mut c = Vec::new();
    for (k, m) in segments(0, 2, 96.0).zip([ClickMode::Single, ClickMode::Double]) {
        c.push((Action::Click(m), k));
    }
    c.push((Action::Ext, switch(1)));
    c.push((Action::Hidden, switch(2)));
    c.push((Action::System, switch(3)));
    c.push((Action::PathBar, switch(4)));
    for (k, p) in segments(5, 3, 64.0).zip([PanelStyle::Pill, PanelStyle::Card, PanelStyle::Stack]) {
        c.push((Action::Panel(p), k));
    }
    let fy = card.bottom() - FOOT_H + (FOOT_H - 32.0) / 2.0;
    c.push((Action::UseWindows, Rect::new(card.x + PAD, fy, 170.0, 32.0)));
    c.push((Action::Done, Rect::new(card.right() - PAD - 90.0, fy, 90.0, 32.0)));
    (card, c)
}

pub fn sheet_hit(window: Rect, x: f32, y: f32) -> Option<Action> {
    sheet_layout(window).1.into_iter().find(|(_, r)| r.contains(x, y)).map(|(a, _)| a)
}

pub fn paint(app: &App) {
    let (g, t) = (&app.gfx, &app.theme);
    let v = app.sheet_progress();
    // Fade keeps each color's own transparency (field backgrounds are translucent).
    let fade = |c: crate::theme::Color| c.with_alpha(c.a * v);
    let window = Rect::new(0.0, 0.0, app.w, app.h);
    g.fill_rect(window, crate::theme::Color::rgb(0x000000).with_alpha(0.28 * v));
    let (card0, controls) = sheet_layout(window);
    let dy = (1.0 - v) * 24.0;
    let mv = |r: Rect| Rect::new(r.x, r.y + dy, r.w, r.h);
    let card = mv(card0);
    g.fill_round(Rect::new(card.x - 1.0, card.y - 1.0, card.w + 2.0, card.h + 2.0), 15.0, fade(t.separator));
    g.fill_round(card, 14.0, fade(t.content_bg));
    g.text("Settings", Rect::new(card.x + PAD, card.y + 12.0, card.w - 2.0 * PAD, 32.0), &g.fonts.title, fade(t.text));
    let s = app.settings;
    let w = app.windows_defaults();
    let hints = [
        format!("Windows: {}", if w.double_click { "double-click" } else { "single-click" }),
        format!("Windows: {}", if w.show_ext { "on" } else { "off" }),
        format!("Windows: {}", if w.show_hidden { "on" } else { "off" }),
        format!("Windows: {}", if w.show_system { "on" } else { "off" }),
        "Ctrl+Alt+P".to_string(),
        "Pill, card or photo stack".to_string(),
    ];
    for (i, title) in row_titles().iter().enumerate() {
        let r = mv(Rect::new(card0.x + PAD, card0.y + TITLE_H + i as f32 * ROW_H, card0.w - 2.0 * PAD, ROW_H));
        g.text(title, Rect::new(r.x, r.y + 4.0, r.w - 200.0, 22.0), &g.fonts.body, fade(t.text));
        g.text(&hints[i], Rect::new(r.x, r.y + 24.0, r.w - 200.0, 18.0), &g.fonts.small, fade(t.text_dim));
        if i > 0 {
            g.line(r.x, r.y + 0.5, r.right(), r.y + 0.5, fade(t.separator));
        }
    }
    let on = |a: Action| match a {
        Action::Click(m) => s.click == m,
        Action::Ext => s.show_ext,
        Action::Hidden => s.show_hidden,
        Action::System => s.show_system,
        Action::PathBar => app.show_pathbar,
        Action::Panel(p) => s.panel == p,
        _ => false,
    };
    for (a, r0) in controls {
        let r = mv(r0);
        match a {
            Action::Ext | Action::Hidden | Action::System | Action::PathBar => {
                let lit = on(a);
                g.fill_round(r, r.h / 2.0, fade(if lit { t.accent } else { t.text.with_alpha(0.22) }));
                let k = r.h - 4.0;
                let kx = if lit { r.right() - k - 2.0 } else { r.x + 2.0 };
                g.fill_round(Rect::new(kx, r.y + 2.0, k, k), k / 2.0, fade(crate::theme::Color::rgb(0xFFFFFF)));
            }
            Action::Click(_) | Action::Panel(_) => {
                let label = match a {
                    Action::Click(ClickMode::Single) => "Single-click",
                    Action::Click(ClickMode::Double) => "Double-click",
                    Action::Panel(PanelStyle::Pill) => "Pill",
                    Action::Panel(PanelStyle::Card) => "Card",
                    _ => "Stack",
                };
                g.fill_rect(r, fade(t.field_bg));
                if on(a) {
                    g.fill_round(Rect::new(r.x + 2.0, r.y + 2.0, r.w - 4.0, r.h - 4.0), 6.0, fade(t.accent));
                }
                let fg = if on(a) { t.selection_text } else { t.text };
                g.text(label, Rect::new(r.x, r.y + 6.0, r.w, r.h - 6.0), &g.fonts.cell, fade(fg));
            }
            Action::UseWindows => {
                g.fill_round(r, 8.0, fade(t.field_bg));
                g.text("Use Windows settings", Rect::new(r.x, r.y + 8.0, r.w, r.h - 8.0), &g.fonts.cell, fade(t.text));
            }
            Action::Done => {
                g.fill_round(r, 8.0, fade(t.accent));
                g.text("Done", Rect::new(r.x, r.y + 8.0, r.w, r.h - 8.0), &g.fonts.cell, fade(t.selection_text));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Rect;

    #[test]
    fn sheet_controls_fit_and_hit() {
        let window = Rect::new(0.0, 0.0, 1100.0, 700.0);
        let (card, controls) = sheet_layout(window);
        assert!(card.w <= 520.0 && card.x > 0.0 && card.bottom() < 700.0);
        for (i, (_, a)) in controls.iter().enumerate() {
            assert!(a.x >= card.x && a.right() <= card.right() && a.y >= card.y && a.bottom() <= card.bottom(), "{a:?}");
            for (_, b) in &controls[i + 1..] {
                assert!(!(a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()), "{a:?} overlaps {b:?}");
            }
        }
        let (act, r) = controls[0];
        assert_eq!(sheet_hit(window, r.x + 2.0, r.y + 2.0), Some(act));
        assert_eq!(sheet_hit(window, card.x + 2.0, card.y + 2.0), None, "card padding is not a control");
        assert!(controls.iter().any(|c| c.0 == Action::Done) && controls.iter().any(|c| c.0 == Action::UseWindows));
    }

    #[test]
    fn narrow_window_still_fits() {
        let (card, _) = sheet_layout(Rect::new(0.0, 0.0, 800.0, 500.0));
        assert!(card.y >= 0.0 && card.bottom() <= 500.0);
    }
}
