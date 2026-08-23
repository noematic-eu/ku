use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthStr;

use crate::app::View;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingField {
    Theme,
    Refresh,
    Retention,
    Warn,
    Crit,
    Snapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Tab(View),
    FooterHelp,
    FooterQuit,
    TableRow(usize),
    CycleSort,
    Setting(SettingField),
    DashDisk(usize),
    GrowthWindow,
    OverlayAction(usize),
    OverlayYes,
    OverlayNo,
    OverlayDismiss,
}

#[derive(Debug, Default, Clone)]
pub struct HitMap {
    regions: Vec<(Rect, Hit)>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    pub fn push(&mut self, rect: Rect, hit: Hit) {
        if rect.width > 0 && rect.height > 0 {
            self.regions.push((rect, hit));
        }
    }

    pub fn at(&self, x: u16, y: u16) -> Option<Hit> {
        let pos = Position::new(x, y);
        self.regions
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(_, hit)| *hit)
    }
}

pub fn tab_label(index: usize, view: View) -> String {
    format!("{} {}", index + 1, view.title())
}

/// Hit rectangles matching ratatui `Tabs` (1-space padding on each side, `│` divider).
pub fn tab_rects(area: Rect) -> Vec<(Rect, View)> {
    let mut x = area.x;
    let mut out = Vec::new();
    for (i, view) in View::ALL.iter().copied().enumerate() {
        if x >= area.right() {
            break;
        }
        let label = tab_label(i, view);
        let width = 1u16.saturating_add(label.width() as u16).saturating_add(1);
        let w = width.min(area.right().saturating_sub(x));
        if w == 0 {
            break;
        }
        out.push((
            Rect {
                x,
                y: area.y,
                width: w,
                height: area.height.max(1),
            },
            view,
        ));
        x = x.saturating_add(width);
        if i + 1 < View::ALL.len() {
            x = x.saturating_add(1);
        }
    }
    out
}

/// Map table body rows (border + header) to `TableRow` hits.
pub fn register_table_rows(
    hits: &mut HitMap,
    area: Rect,
    offset: usize,
    len: usize,
    header: Option<Hit>,
) {
    if area.width < 3 || area.height < 3 {
        return;
    }
    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    if let Some(header) = header {
        hits.push(
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            },
            header,
        );
    }
    let start_y = inner.y.saturating_add(1);
    let visible = inner.height.saturating_sub(1);
    for vis in 0..visible {
        let idx = offset + vis as usize;
        if idx >= len {
            break;
        }
        hits.push(
            Rect {
                x: inner.x,
                y: start_y.saturating_add(vis),
                width: inner.width,
                height: 1,
            },
            Hit::TableRow(idx),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_region_wins() {
        let mut map = HitMap::default();
        map.push(Rect::new(0, 0, 10, 5), Hit::FooterHelp);
        map.push(Rect::new(2, 1, 3, 1), Hit::FooterQuit);
        assert_eq!(map.at(3, 1), Some(Hit::FooterQuit));
        assert_eq!(map.at(0, 0), Some(Hit::FooterHelp));
        assert_eq!(map.at(20, 20), None);
    }

    #[test]
    fn tabs_cover_each_view() {
        let area = Rect::new(0, 0, 80, 1);
        let tabs = tab_rects(area);
        assert_eq!(tabs.len(), View::ALL.len());
        for (i, view) in View::ALL.iter().copied().enumerate() {
            let (rect, v) = tabs[i];
            assert_eq!(v, view);
            assert_eq!(map_at(&tabs, rect.x + 1), Some(view));
        }
    }

    fn map_at(tabs: &[(Rect, View)], x: u16) -> Option<View> {
        tabs.iter()
            .rev()
            .find(|(r, _)| r.contains(Position::new(x, 0)))
            .map(|(_, v)| *v)
    }
}
