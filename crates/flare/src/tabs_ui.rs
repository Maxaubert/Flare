//! rows, strip layout, the strip and sidebar drag (Task 26).
//!
//! Sources: `TabStrip.tsx`, `TabSidebar.tsx`, `tabList.ts`, `useTabReorder.ts`, `App.tsx`
//! (`tabProps`). Everything here is pure except [`to_slint_row`] and [`sync_rows`], which are the
//! one edge where a row meets Slint.
#![allow(dead_code)] // the controller (Task 30) is the caller

use flare_tabs::model::TabModel;
use flare_tabs::sidebar_rows::{find_keeps, folder_line};
use flare_tabs::strip_layout::{layout_strip, room_for_plan};
use flare_tabs::strip_plan::{StripSlot, plan_strip};
use flare_tabs::tab_drag::{Axis, Release, TabDrag};
use flare_tabs::tab_drop::Lane;
use flare_tabs::tabs::{Pending, TabKind};
use slint::Model;
use std::collections::{HashMap, HashSet};

/// The pitch and height of a sidebar row, in css pixels.
const SIDE_PITCH: f64 = 50.0;
const SIDE_ROW_H: f64 = 46.0;

/// What a tab that needs you says it needs (M6 colours them; M3 never has any).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkRole {
    Working,
    Done,
    Question,
    Failed,
}

/// One tab as the strip, the sidebar and the +N list draw it.
#[derive(Clone, Debug, PartialEq)]
pub struct RowData {
    pub id: String,
    pub label: String,
    pub tip: String,
    pub folder: String,
    pub settings: bool,
    pub active: bool,
    pub loading: bool,
    /// 0 named, 1 icon, 2 hidden.
    pub mode: i32,
    pub x: i32,
    pub w: i32,
    pub kept: bool,
    pub offset_css: f32,
    pub carried: bool,
    /// What this tab needs from you, if anything (the +N list sorts by it, the dots show it).
    pub need: Option<MarkRole>,
}

/// The +N button, the + button and the dots on the +N button.
#[derive(Clone, Debug, PartialEq)]
pub struct GeomData {
    pub more_x: i32,
    pub more_w: i32,
    pub plus_x: i32,
    pub plus_w: i32,
    pub more_count: i32,
    pub dots: Vec<MarkRole>,
}

pub struct ViewInput<'a> {
    pub model: &'a TabModel,
    pub home: &'a str,
    pub query: &'a str,
    pub strip_phys: i32,
    pub scale: f64,
    pub needs: &'a HashMap<String, MarkRole>,
    pub offsets: &'a HashMap<String, f32>,
    pub carried: Option<&'a str>,
}

pub struct View {
    pub rows: Vec<RowData>,
    pub geom: GeomData,
    pub has_tabs: bool,
    pub front_kind: i32,
}

/// Most dots the +N button shows.
const MAX_DOTS: usize = 4;

/// label: tab_labels; tip: the folder path (Settings: ""); folder: folder_line (Settings: "");
/// kept: find_keeps, or true when no query (the order never changes); loading: pending is an
/// agent; mode/x/w: plan_strip then layout_strip; dots: the roles of up to four hidden `needs`
/// tabs, in strip order. front_kind: nothing in front: 2 (start screen) once `restore_done`,
/// else 3; Settings 1; a placeholder 3; else 0.
pub fn build_view(inp: &ViewInput) -> View {
    let scale = if inp.scale.is_nan() || inp.scale <= 0.0 {
        1.0
    } else {
        inp.scale
    };
    let tabs = &inp.model.state.tabs;
    let active_id = inp.model.state.active_id.as_deref();
    let labels = inp.model.labels();
    let ids: Vec<String> = tabs.iter().map(|t| t.id.clone()).collect();
    let needy: HashSet<String> = inp.needs.keys().cloned().collect();
    let room = room_for_plan(f64::from(inp.strip_phys) / scale);
    let plan = plan_strip(&ids, active_id, &needy, room);
    let layout = layout_strip(&plan, inp.strip_phys, scale);

    let mut rows = Vec::with_capacity(tabs.len());
    for (i, tab) in tabs.iter().enumerate() {
        let settings = tab.kind == TabKind::Settings;
        let slot = layout.slots.get(i).copied().flatten();
        let mode = match plan[i] {
            StripSlot::Full => 0,
            StripSlot::Icon => 1,
            StripSlot::Hidden => 2,
        };
        rows.push(RowData {
            id: tab.id.clone(),
            label: labels[i].clone(),
            tip: if settings {
                String::new()
            } else {
                tab.cwd.clone()
            },
            folder: if settings {
                String::new()
            } else {
                folder_line(&tab.cwd, inp.home)
            },
            settings,
            active: Some(tab.id.as_str()) == active_id,
            loading: tab.pending == Some(Pending::Agent),
            mode,
            x: slot.map_or(0, |s| s.x),
            w: slot.map_or(0, |s| s.w),
            kept: inp.query.trim().is_empty() || find_keeps(inp.query, &labels[i], &tab.cwd),
            offset_css: inp.offsets.get(&tab.id).copied().unwrap_or(0.0),
            carried: inp.carried == Some(tab.id.as_str()),
            need: inp.needs.get(&tab.id).copied(),
        });
    }

    let dots: Vec<MarkRole> = rows
        .iter()
        .filter(|r| r.mode == 2)
        .filter_map(|r| r.need)
        .take(MAX_DOTS)
        .collect();
    let geom = GeomData {
        more_x: layout.more.map_or(0, |s| s.x),
        more_w: layout.more.map_or(0, |s| s.w),
        plus_x: layout.plus.x,
        plus_w: layout.plus.w,
        more_count: rows.iter().filter(|r| r.mode == 2).count() as i32,
        dots,
    };
    let front_kind = match inp.model.active() {
        None => {
            if inp.model.restore_done() {
                2
            } else {
                3
            }
        }
        Some(t) if t.kind == TabKind::Settings => 1,
        Some(t) if t.pending.is_some() => 3,
        Some(_) => 0,
    };
    View {
        has_tabs: !rows.is_empty(),
        rows,
        geom,
        front_kind,
    }
}

/// The edge: nothing else touches Slint.
pub fn to_slint_row(
    row: &RowData,
    mark_colour: &dyn Fn(Option<MarkRole>) -> slint::Color,
) -> crate::TabRow {
    crate::TabRow {
        id: row.id.as_str().into(),
        label: row.label.as_str().into(),
        tip: row.tip.as_str().into(),
        folder: row.folder.as_str().into(),
        settings: row.settings,
        active: row.active,
        loading: row.loading,
        mode: row.mode,
        x: row.x,
        w: row.w,
        offset: row.offset_css,
        carried: row.carried,
        kept: row.kept,
        mark_kind: 0,
        mark_color: mark_colour(row.need),
    }
}

/// Updates the ONE `VecModel` that `tab-rows` was given at startup IN PLACE (a new model would
/// destroy the rows' TouchAreas and end a carry mid-drag): rows that differ are set, extra rows
/// are pushed or removed from the end. Never `set_vec`, never a new `ModelRc`.
pub fn sync_rows(
    target: &slint::VecModel<crate::TabRow>,
    rows: &[RowData],
    mark_colour: &dyn Fn(Option<MarkRole>) -> slint::Color,
) {
    let shared = target.row_count().min(rows.len());
    for (i, row) in rows.iter().enumerate().take(shared) {
        let new = to_slint_row(row, mark_colour);
        if target.row_data(i).as_ref() != Some(&new) {
            target.set_row_data(i, new);
        }
    }
    for row in rows.iter().skip(shared) {
        target.push(to_slint_row(row, mark_colour));
    }
    while target.row_count() > rows.len() {
        target.remove(target.row_count() - 1);
    }
}

/// The drag of a tab along the strip.
pub struct StripDrag {
    drag: TabDrag,
}

impl Default for StripDrag {
    fn default() -> Self {
        Self::new()
    }
}

impl StripDrag {
    pub fn new() -> Self {
        Self {
            drag: TabDrag::new(Axis::X),
        }
    }

    /// `row_index` = the row's index in `view.rows` (Slint's `for` index); this converts it to the
    /// index among the DRAWN tabs (hidden rows have no lane). Lanes are every drawn slot in WINDOW
    /// css pixels (`origin_css + slot.x / scale`); the container is the strip's width from the
    /// origin. Returns whether a press was taken.
    #[allow(clippy::too_many_arguments)]
    pub fn press(
        &mut self,
        id: &str,
        row_index: usize,
        pointer_x_css: f64,
        origin_css: f64,
        view: &View,
        strip_phys: i32,
        scale: f64,
    ) -> bool {
        let scale = if scale.is_nan() || scale <= 0.0 {
            1.0
        } else {
            scale
        };
        let Some(row) = view.rows.get(row_index) else {
            return false;
        };
        if row.mode == 2 || row.id != id {
            return false;
        }
        let drawn: Vec<&RowData> = view.rows.iter().filter(|r| r.mode != 2).collect();
        let Some(index) = drawn.iter().position(|r| r.id == id) else {
            return false;
        };
        let lanes: Vec<Lane> = drawn
            .iter()
            .map(|r| {
                let left = origin_css + f64::from(r.x) / scale;
                let width = f64::from(r.w) / scale;
                Lane {
                    left,
                    width,
                    mid: left + width / 2.0,
                }
            })
            .collect();
        let container = (origin_css, origin_css + f64::from(strip_phys) / scale);
        self.drag
            .press(id, index, pointer_x_css, lanes, container, false)
    }

    pub fn moved(&mut self, pointer_x_css: f64) {
        self.drag.moved(pointer_x_css);
    }

    pub fn release(&mut self) -> Release {
        self.drag.release()
    }

    pub fn cancel(&mut self) {
        self.drag.cancel();
    }

    /// `TabDrag::offset` per drawn row, in css pixels.
    pub fn offsets(&self, view: &View) -> HashMap<String, f32> {
        view.rows
            .iter()
            .filter(|r| r.mode != 2)
            .enumerate()
            .map(|(i, r)| (r.id.clone(), self.drag.offset(&r.id, i) as f32))
            .collect()
    }

    /// The id of the tab being carried, once the pointer has travelled past the slop.
    pub fn carried(&self) -> Option<String> {
        self.drag.carry().filter(|c| c.live).map(|c| c.id.clone())
    }

    pub fn carrying(&self) -> bool {
        self.drag.carry().is_some_and(|c| c.live)
    }

    /// The slot being aimed at, translated from the drawn index to the model's tab index.
    pub fn drop_index_in_model(&self, model: &TabModel, view: &View) -> Option<usize> {
        if !self.carrying() {
            return None;
        }
        slot_to_model(model, view, self.drag.drop_at()?)
    }
}

/// A slot among the DRAWN tabs (in `reorder_tabs` terms) as a slot in the model's list. A slot
/// past the last drawn tab lands right after it.
pub fn slot_to_model(model: &TabModel, view: &View, slot: usize) -> Option<usize> {
    let position = |id: &str| model.state.tabs.iter().position(|t| t.id == id);
    let drawn: Vec<usize> = view
        .rows
        .iter()
        .filter(|r| r.mode != 2)
        .filter_map(|r| position(&r.id))
        .collect();
    match drawn.get(slot) {
        Some(&at) => Some(at),
        None => drawn.last().map(|&last| last + 1),
    }
}

/// The drag of a tab down the sidebar.
pub struct SideDrag {
    drag: TabDrag,
}

impl Default for SideDrag {
    fn default() -> Self {
        Self::new()
    }
}

impl SideDrag {
    pub fn new() -> Self {
        Self {
            drag: TabDrag::new(Axis::Y),
        }
    }

    /// Lanes are `first_row_top + i * 50`, 46 high; the container is `rows_top..rows_bottom`.
    /// `refuse` (the find query is typed) starts nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn press(
        &mut self,
        id: &str,
        index: usize,
        pointer_y: f64,
        first_row_top: f64,
        rows_top: f64,
        rows_bottom: f64,
        count: usize,
        refuse: bool,
    ) -> bool {
        let lanes: Vec<Lane> = (0..count)
            .map(|i| {
                let top = first_row_top + i as f64 * SIDE_PITCH;
                Lane {
                    left: top,
                    width: SIDE_ROW_H,
                    mid: top + SIDE_ROW_H / 2.0,
                }
            })
            .collect();
        self.drag
            .press(id, index, pointer_y, lanes, (rows_top, rows_bottom), refuse)
    }

    pub fn moved(&mut self, pointer_y: f64) {
        self.drag.moved(pointer_y);
    }

    pub fn release(&mut self) -> Release {
        self.drag.release()
    }

    pub fn cancel(&mut self) {
        self.drag.cancel();
    }

    /// The offset along y of each listed row, in css pixels (the list is the rows in order).
    pub fn offsets(&self, ids: &[String]) -> HashMap<String, f32> {
        ids.iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), self.drag.offset(id, i) as f32))
            .collect()
    }

    pub fn drop_at(&self) -> Option<usize> {
        self.drag.drop_at()
    }

    pub fn carried(&self) -> Option<String> {
        self.drag.carry().filter(|c| c.live).map(|c| c.id.clone())
    }

    pub fn carrying(&self) -> bool {
        self.drag.carry().is_some_and(|c| c.live)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_agent::kinds::AgentKind;
    use flare_tabs::restore::{Restored, SavedTab, SavedTabs};

    const HOME: &str = r"C:\Users\Admin";

    fn model_of(paths: &[&str]) -> TabModel {
        let mut m = TabModel::empty(0);
        for p in paths {
            m.open_tab(p);
        }
        m
    }

    fn view_of(m: &TabModel, strip_phys: i32, scale: f64, query: &str) -> View {
        let (needs, offsets) = (HashMap::new(), HashMap::new());
        build_view(&ViewInput {
            model: m,
            home: HOME,
            query,
            strip_phys,
            scale,
            needs: &needs,
            offsets: &offsets,
            carried: None,
        })
    }

    fn three() -> TabModel {
        model_of(&[r"C:\w\api", r"C:\w\web", r"D:\x\api"])
    }

    fn forty() -> TabModel {
        let paths: Vec<String> = (0..40).map(|i| format!(r"C:\w\p{i}")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        model_of(&refs)
    }

    fn colour(_: Option<MarkRole>) -> slint::Color {
        slint::Color::from_rgb_u8(1, 2, 3)
    }

    fn ids_of(m: &TabModel) -> Vec<String> {
        m.state.tabs.iter().map(|t| t.id.clone()).collect()
    }

    #[test]
    fn rows_carry_label_tip_folder_and_flags() {
        let m = three();
        let v = view_of(&m, 1000, 1.0, "");
        let labels: Vec<&str> = v.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["api - w", "web", "api - x"]);
        let tips: Vec<&str> = v.rows.iter().map(|r| r.tip.as_str()).collect();
        assert_eq!(tips, [r"C:\w\api", r"C:\w\web", r"D:\x\api"]);
        let folders: Vec<&str> = v.rows.iter().map(|r| r.folder.as_str()).collect();
        assert_eq!(folders, [r"C:\w", r"C:\w", r"D:\x"]);
        assert!(v.rows[2].active && !v.rows[0].active && !v.rows[1].active);
        assert!(v.rows.iter().all(|r| !r.loading && r.kept));
        assert!(v.has_tabs);
    }

    #[test]
    fn the_settings_tab_has_no_tip_or_folder() {
        let mut m = three();
        m.open_settings();
        let v = view_of(&m, 1000, 1.0, "");
        let s = v.rows.iter().find(|r| r.settings).expect("settings row");
        assert_eq!(s.label, "Settings");
        assert_eq!(s.tip, "");
        assert_eq!(s.folder, "");
    }

    fn saved_two() -> SavedTabs {
        SavedTabs {
            tabs: vec![
                SavedTab {
                    cwd: r"C:\a".into(),
                    agent: Some(AgentKind::Claude),
                },
                SavedTab {
                    cwd: r"C:\b".into(),
                    agent: None,
                },
            ],
            active: 0,
        }
    }

    #[test]
    fn a_placeholder_row_is_loading_only_for_an_agent() {
        let m = TabModel::boot(&saved_two(), 0);
        let v = view_of(&m, 1000, 1.0, "");
        assert!(v.rows[0].loading);
        assert!(!v.rows[1].loading);
    }

    #[test]
    fn sixteen_tabs_in_a_thousand_pixels_are_one_name_and_icons() {
        let paths: Vec<String> = (0..16).map(|i| format!(r"C:\w\p{i}")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let m = model_of(&refs);
        let v = view_of(&m, 1000, 1.0, "");
        let named: Vec<&RowData> = v.rows.iter().filter(|r| r.mode == 0).collect();
        assert_eq!(named.len(), 1);
        assert!(named[0].active);
        assert_eq!(v.rows.iter().filter(|r| r.mode == 1).count(), 15);
        assert_eq!(v.geom.more_count, 0);
        for r in &v.rows {
            assert!([150, 40, 84].contains(&r.w), "width {}", r.w);
        }
    }

    #[test]
    fn forty_tabs_split_into_drawn_and_listed() {
        let m = forty();
        let v = view_of(&m, 1000, 1.0, "");
        let drawn = v.rows.iter().filter(|r| r.mode != 2).count() as i32;
        assert_eq!(drawn + v.geom.more_count, 40);
        assert!(v.geom.more_count > 0);
        assert!(v.rows.iter().find(|r| r.active).unwrap().mode != 2);
    }

    #[test]
    fn dots_are_the_roles_of_hidden_needy_tabs_at_most_four() {
        let m = forty();
        let ids = ids_of(&m);
        // Most tabs need you, so some needy ones are pushed into the list.
        let roles = [
            MarkRole::Done,
            MarkRole::Question,
            MarkRole::Failed,
            MarkRole::Working,
        ];
        let needs: HashMap<String, MarkRole> = ids
            .iter()
            .enumerate()
            .skip(1)
            .take(35)
            .map(|(i, id)| (id.clone(), roles[i % 4]))
            .collect();
        let offsets = HashMap::new();
        let v = build_view(&ViewInput {
            model: &m,
            home: HOME,
            query: "",
            strip_phys: 1000,
            scale: 1.0,
            needs: &needs,
            offsets: &offsets,
            carried: None,
        });
        let hidden_needy: Vec<MarkRole> = v
            .rows
            .iter()
            .filter(|r| r.mode == 2)
            .filter_map(|r| r.need)
            .collect();
        assert!(hidden_needy.len() > 4);
        assert_eq!(v.geom.dots.len(), 4);
        assert_eq!(v.geom.dots, hidden_needy[..4].to_vec());
        // None hidden needy: no dots.
        let plain = view_of(&m, 1000, 1.0, "");
        assert!(plain.geom.dots.is_empty());
    }

    #[test]
    fn find_filters_without_reordering() {
        let m = model_of(&[r"C:\w\api", r"C:\w\web", r"C:\x\snap", r"C:\ap\zed"]);
        let v = view_of(&m, 1000, 1.0, "ap");
        let kept: Vec<&str> = v
            .rows
            .iter()
            .filter(|r| r.kept)
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(kept, ["api", "snap", "zed"]);
        let order: Vec<&str> = v.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(order, ["api", "web", "snap", "zed"]);
        let all = view_of(&m, 1000, 1.0, "");
        assert!(all.rows.iter().all(|r| r.kept));
    }

    #[test]
    fn front_kind_follows_the_front_tab() {
        let empty = TabModel::empty(0);
        assert_eq!(view_of(&empty, 1000, 1.0, "").front_kind, 3);
        let mut done = TabModel::empty(0);
        done.settle(&Restored {
            tabs: vec![],
            active: 0,
        });
        assert_eq!(view_of(&done, 1000, 1.0, "").front_kind, 2);
        let mut s = three();
        s.open_settings();
        assert_eq!(view_of(&s, 1000, 1.0, "").front_kind, 1);
        let placeholder = TabModel::boot(&saved_two(), 0);
        assert_eq!(view_of(&placeholder, 1000, 1.0, "").front_kind, 3);
        assert_eq!(view_of(&three(), 1000, 1.0, "").front_kind, 0);
    }

    #[test]
    fn rows_use_the_layouts_device_pixel_slots() {
        let v = view_of(&three(), 2250, 2.25, "");
        let slots: Vec<(i32, i32)> = v.rows.iter().map(|r| (r.x, r.w)).collect();
        assert_eq!(slots, [(0, 338), (338, 337), (675, 338)]);
    }

    #[test]
    fn strip_lanes_are_the_slots_in_window_coordinates() {
        let m = three();
        let v = view_of(&m, 1000, 1.0, "");
        let ids = ids_of(&m);
        let mut d = StripDrag::new();
        assert!(d.press(&ids[1], 1, 260.0, 50.0, &v, 1000, 1.0));
        d.moved(380.0);
        assert!(d.carrying());
        assert_eq!(d.carried().as_deref(), Some(ids[1].as_str()));
        assert_eq!(d.offsets(&v)[&ids[1]], 120.0);
        assert_eq!(
            d.release(),
            Release::Drop {
                id: ids[1].clone(),
                slot: 3
            }
        );
    }

    #[test]
    fn dragging_the_second_tab_left_past_the_first_drops_in_slot_zero() {
        let m = three();
        let v = view_of(&m, 1000, 1.0, "");
        let ids = ids_of(&m);
        let mut d = StripDrag::new();
        d.press(&ids[1], 1, 260.0, 50.0, &v, 1000, 1.0);
        d.moved(100.0);
        assert_eq!(d.drop_index_in_model(&m, &v), Some(0));
        assert_eq!(
            d.release(),
            Release::Drop {
                id: ids[1].clone(),
                slot: 0
            }
        );
    }

    #[test]
    fn a_press_without_travel_is_a_pick_and_a_two_pixel_wobble_too() {
        let m = three();
        let v = view_of(&m, 1000, 1.0, "");
        let ids = ids_of(&m);
        let mut d = StripDrag::new();
        d.press(&ids[1], 1, 260.0, 50.0, &v, 1000, 1.0);
        assert_eq!(d.release(), Release::Pick);
        d.press(&ids[1], 1, 260.0, 50.0, &v, 1000, 1.0);
        d.moved(262.0);
        assert!(!d.carrying());
        assert_eq!(d.release(), Release::Pick);
        d.press(&ids[1], 1, 260.0, 50.0, &v, 1000, 1.0);
        d.moved(262.0);
        assert_eq!(d.drop_index_in_model(&m, &v), None);
    }

    #[test]
    fn hidden_tabs_are_not_lanes() {
        let m = forty();
        let v = view_of(&m, 1000, 1.0, "");
        let ids = ids_of(&m);
        let drawn: Vec<usize> = (0..40).filter(|&i| v.rows[i].mode != 2).collect();
        assert!(drawn.len() < 40);
        let first = drawn[0];
        let mut d = StripDrag::new();
        assert!(d.press(&ids[first], first, 25.0, 0.0, &v, 1000, 1.0));
        d.moved(5000.0);
        // Past every drawn tab: right after the last drawn one, in MODEL indices (the naive
        // drawn slot would be far smaller).
        assert_eq!(d.drop_index_in_model(&m, &v), Some(40));
        // A hidden row cannot be pressed.
        let hidden = (0..40).find(|&i| v.rows[i].mode == 2).unwrap();
        let mut d2 = StripDrag::new();
        assert!(!d2.press(&ids[hidden], hidden, 25.0, 0.0, &v, 1000, 1.0));
        // A slot among the drawn tabs maps to that drawn tab's model index.
        assert_eq!(slot_to_model(&m, &v, 1), Some(drawn[1]));
        assert_eq!(slot_to_model(&m, &v, drawn.len() - 1), Some(39));
    }

    #[test]
    fn sidebar_lanes_step_by_fifty() {
        let mut d = SideDrag::new();
        assert!(d.press("b", 1, 160.0, 100.0, 100.0, 300.0, 4, false));
        d.moved(220.0);
        assert!(d.carrying());
        // 60 px down puts the row's lower edge (256) past the next row's middle (223) and short of
        // the one after (273): in front of row 3 in `reorder_tabs` terms (the plan's table says
        // Some(2), which would be "stay put" for row 1; the lanes make it 3).
        assert_eq!(d.drop_at(), Some(3));
        let ids: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        let off = d.offsets(&ids);
        assert_eq!(off["b"], 60.0);
        assert_eq!(off["c"], -46.0);
        assert_eq!(off["a"], 0.0);
        assert_eq!(
            d.release(),
            Release::Drop {
                id: "b".into(),
                slot: 3
            }
        );
    }

    #[test]
    fn a_sidebar_drag_is_refused_while_the_find_query_is_typed() {
        let mut d = SideDrag::new();
        assert!(!d.press("b", 1, 160.0, 100.0, 100.0, 300.0, 4, true));
        assert!(!d.carrying());
        assert_eq!(d.release(), Release::Nothing);
    }

    fn row(id: &str, offset: f32) -> RowData {
        RowData {
            id: id.into(),
            label: id.into(),
            tip: String::new(),
            folder: String::new(),
            settings: false,
            active: false,
            loading: false,
            mode: 0,
            x: 0,
            w: 150,
            kept: true,
            offset_css: offset,
            carried: false,
            need: None,
        }
    }

    #[test]
    fn sync_rows_updates_in_place_and_only_grows_or_shrinks_at_the_end() {
        let target = slint::VecModel::<crate::TabRow>::default();
        let three = [row("a", 0.0), row("b", 0.0), row("c", 0.0)];
        sync_rows(&target, &three, &colour);
        assert_eq!(target.row_count(), 3);
        let before: Vec<crate::TabRow> = (0..3).map(|i| target.row_data(i).unwrap()).collect();
        let moved = [row("a", 0.0), row("b", 12.5), row("c", 0.0)];
        sync_rows(&target, &moved, &colour);
        assert_eq!(target.row_count(), 3);
        assert_eq!(target.row_data(1).unwrap().offset, 12.5);
        assert_eq!(target.row_data(0).unwrap(), before[0]);
        assert_eq!(target.row_data(2).unwrap(), before[2]);
        let five = [
            row("a", 0.0),
            row("b", 12.5),
            row("c", 0.0),
            row("d", 0.0),
            row("e", 0.0),
        ];
        sync_rows(&target, &five, &colour);
        assert_eq!(target.row_count(), 5);
        assert_eq!(target.row_data(1).unwrap().offset, 12.5);
        sync_rows(&target, &five[..2], &colour);
        assert_eq!(target.row_count(), 2);
        assert_eq!(target.row_data(1).unwrap().id, "b");
    }
}
