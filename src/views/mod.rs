pub mod columns;
pub mod grid;
pub mod list;
pub mod pathbar;
pub mod sidebar;
pub mod toolbar;

use crate::app::App;
use crate::layout::Rect;
use crate::theme::Color;

pub fn paint(app: &mut App) {
    {
        let (g, t, l) = (&app.gfx, &app.theme, &app.layout);
        // Sidebar column runs from the top edge to the path bar, like Finder. Transparent shows Mica.
        let sidebar_col = Rect::new(0.0, 0.0, l.sidebar.w, l.pathbar.y);
        g.fill_rect(sidebar_col, if app.mica { Color::TRANSPARENT } else { t.window_bg });
        g.fill_rect(l.content, t.content_bg);
    }
    toolbar::paint(app);
    sidebar::paint(app);
    match app.view {
        crate::app::View::Columns => {
            columns::paint(app);
            list::paint_message(app);
        }
        crate::app::View::Icon => {
            grid::paint(app);
            list::paint_message(app);
        }
        crate::app::View::List => list::paint(app),
    }
    {
        let (g, t, l) = (&app.gfx, &app.theme, &app.layout);
        g.fill_rect(l.pathbar, t.window_bg);
        g.line(0.0, l.pathbar.y + 0.5, l.pathbar.w, l.pathbar.y + 0.5, t.separator);
    }
    pathbar::paint(app);
}
