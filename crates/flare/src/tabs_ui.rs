//! rows, strip layout, the strip and sidebar drag (Task 26).
//!
//! Sources: `TabStrip.tsx`, `TabSidebar.tsx`, `tabList.ts`, `useTabReorder.ts`, `App.tsx`
//! (`tabProps`). Everything here is pure except [`to_slint_row`] and [`sync_rows`], which are the
//! one edge where a row meets Slint.
#![allow(dead_code)] // the controller (Task 30) is the caller

use crate::mark_colours::{UiMarks, colour_of};
use flare_agent::icon::{IconKind, icon_kind};
use flare_agent::kinds::DetectedAgent;
use flare_agent::tab_look::{Switches, TabSets, tab_look, tab_title};
use flare_agent::tab_mark::{MarkColour, MarkPlace, MarkState};
use flare_tabs::model::TabModel;
use flare_tabs::sidebar_rows::{find_keeps, folder_line};
use flare_tabs::strip_layout::{layout_strip_with, room_for_plan};
use flare_tabs::strip_plan::{MORE_BUTTON, StripSlot, more_button_css, plan_strip};
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
    /// 0 shell, 1 claude, 2 codex (the tab icon).
    pub agent: i32,
    /// 0 none, 1 working, 2 question, 3 finished, 4 failed.
    pub mark_state: i32,
    /// 0 none, 1 ring, 2 line, 3 run.
    pub mark_kind: i32,
    /// The state's line colour; transparent while there is no state.
    pub mark_color: slint::Color,
    /// A finished Bar line wears the flowing rainbow.
    pub mark_rainbow: bool,
}

/// One dot on the +N button: the hidden tab's need, wearing the look's colour for it (a state
/// whose switch is off draws a transparent dot).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dot {
    pub role: MarkRole,
    pub colour: slint::Color,
}

/// The +N button, the + button and the dots on the +N button.
#[derive(Clone, Debug, PartialEq)]
pub struct GeomData {
    pub more_x: i32,
    pub more_w: i32,
    pub plus_x: i32,
    pub plus_w: i32,
    pub more_count: i32,
    pub dots: Vec<Dot>,
}

/// What the agent indicator says about the tabs (borrowed from the hub for one `build_view`).
pub struct AgentRows<'a> {
    pub sets: TabSets<'a>,
    pub sw: Switches,
    pub marks: &'a UiMarks,
    pub kinds: HashMap<String, DetectedAgent>,
    pub failed_kinds: HashMap<String, String>,
    /// Tabs coming back to an agent that the model does not know about yet.
    pub loading: HashSet<String>,
}

fn role_of(state: MarkState) -> MarkRole {
    match state {
        MarkState::Working => MarkRole::Working,
        MarkState::Done => MarkRole::Done,
        MarkState::Question => MarkRole::Question,
        MarkState::Failed => MarkRole::Failed,
    }
}

/// The `needs` of the strip: the done, question and failed tabs, RAW (before the switches). A
/// tab's role is its look's state when it has one, else the highest raw set it is in (question,
/// failed, done).
pub fn needs_of<'i>(
    ids: impl Iterator<Item = &'i str>,
    agent: &AgentRows<'_>,
) -> HashMap<String, MarkRole> {
    let mut out = HashMap::new();
    for id in ids {
        let (q, f, d) = (
            agent.sets.question.contains(id),
            agent.sets.failed.contains(id),
            agent.sets.done.contains(id),
        );
        if !(q || f || d) {
            continue;
        }
        let role = match tab_look(id, &agent.sets, &agent.sw).state {
            Some(state) => role_of(state),
            None if q => MarkRole::Question,
            None if f => MarkRole::Failed,
            None => MarkRole::Done,
        };
        out.insert(id.to_string(), role);
    }
    out
}

pub struct ViewInput<'a> {
    pub model: &'a TabModel,
    pub home: &'a str,
    pub query: &'a str,
    pub strip_phys: i32,
    pub scale: f64,
    pub needs: &'a HashMap<String, MarkRole>,
    pub agent: &'a AgentRows<'a>,
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
    // The +N button grows with its dots (Prism Terminal sizes it by its content): the plan is made
    // again with that much less room, once, so the strip still fits.
    let more_of = |plan: &[StripSlot]| {
        let hidden = ids
            .iter()
            .zip(plan)
            .filter(|(_, s)| **s == StripSlot::Hidden);
        let dots = hidden.clone().filter(|(id, _)| needy.contains(*id)).count();
        more_button_css(hidden.count(), dots)
    };
    let mut plan = plan_strip(&ids, active_id, &needy, room);
    let mut more_css = more_of(&plan);
    if more_css > MORE_BUTTON {
        plan = plan_strip(&ids, active_id, &needy, room - (more_css - MORE_BUTTON));
        more_css = more_of(&plan);
    }
    let layout = layout_strip_with(&plan, inp.strip_phys, scale, more_css);

    let mut rows = Vec::with_capacity(tabs.len());
    for (i, tab) in tabs.iter().enumerate() {
        let settings = tab.kind == TabKind::Settings;
        let slot = layout.slots.get(i).copied().flatten();
        let mode = match plan[i] {
            StripSlot::Full => 0,
            StripSlot::Icon => 1,
            StripSlot::Hidden => 2,
        };
        let look = tab_look(&tab.id, &inp.agent.sets, &inp.agent.sw);
        let tab_agent = inp
            .agent
            .kinds
            .get(&tab.id)
            .filter(|_| inp.agent.sets.agents.contains(&tab.id))
            .copied();
        let tip = tab_title(
            &tab.cwd,
            settings,
            look.failed,
            inp.agent.failed_kinds.get(&tab.id).map(String::as_str),
        )
        .unwrap_or_default();
        rows.push(RowData {
            id: tab.id.clone(),
            label: labels[i].clone(),
            tip,
            folder: if settings {
                String::new()
            } else {
                folder_line(&tab.cwd, inp.home)
            },
            settings,
            active: Some(tab.id.as_str()) == active_id,
            loading: tab.pending == Some(Pending::Agent) || inp.agent.loading.contains(&tab.id),
            mode,
            x: slot.map_or(0, |s| s.x),
            w: slot.map_or(0, |s| s.w),
            kept: inp.query.trim().is_empty() || find_keeps(inp.query, &labels[i], &tab.cwd),
            offset_css: inp.offsets.get(&tab.id).copied().unwrap_or(0.0),
            carried: inp.carried == Some(tab.id.as_str()),
            need: inp.needs.get(&tab.id).copied(),
            agent: match icon_kind(tab_agent, settings) {
                IconKind::Claude => 1,
                IconKind::Codex => 2,
                IconKind::Shell | IconKind::Settings => 0,
            },
            mark_state: look.state.map_or(0, state_int),
            mark_kind: match look.mark.place {
                MarkPlace::None => 0,
                MarkPlace::Ring => 1,
                MarkPlace::Line => 2,
                MarkPlace::Run => 3,
            },
            mark_color: look
                .state
                .map_or(slint::Color::from_argb_u8(0, 0, 0, 0), |s| {
                    colour_of(inp.agent.marks, s)
                }),
            mark_rainbow: look.mark.colour == Some(MarkColour::Rainbow),
        });
    }

    let dots: Vec<Dot> = rows
        .iter()
        .filter(|r| r.mode == 2)
        .filter_map(|r| {
            r.need.map(|role| Dot {
                role,
                colour: r.mark_color,
            })
        })
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

/// 1 working, 2 question, 3 finished, 4 failed.
fn state_int(s: MarkState) -> i32 {
    match s {
        MarkState::Working => 1,
        MarkState::Question => 2,
        MarkState::Done => 3,
        MarkState::Failed => 4,
    }
}

/// The edge: nothing else touches Slint.
pub fn to_slint_row(row: &RowData) -> crate::TabRow {
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
        mark_kind: row.mark_kind,
        mark_color: row.mark_color,
        mark_state: row.mark_state,
        mark_rainbow: row.mark_rainbow,
        agent: row.agent,
        failed_tip: "".into(),
    }
}

/// Updates the ONE `VecModel` that `tab-rows` was given at startup IN PLACE (a new model would
/// destroy the rows' TouchAreas and end a carry mid-drag): rows that differ are set, extra rows
/// are pushed or removed from the end. Never `set_vec`, never a new `ModelRc`.
pub fn sync_rows(target: &slint::VecModel<crate::TabRow>, rows: &[RowData]) {
    let shared = target.row_count().min(rows.len());
    for (i, row) in rows.iter().enumerate().take(shared) {
        let new = to_slint_row(row);
        if target.row_data(i).as_ref() != Some(&new) {
            target.set_row_data(i, new);
        }
    }
    for row in rows.iter().skip(shared) {
        target.push(to_slint_row(row));
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
    use flare_agent::tab_mark::Indicator;
    use flare_tabs::restore::{Restored, SavedTab, SavedTabs};

    const WORKING: (u8, u8, u8) = (1, 1, 1);
    const DONE: (u8, u8, u8) = (2, 2, 2);
    const QUESTION: (u8, u8, u8) = (3, 3, 3);
    const FAILED: (u8, u8, u8) = (4, 4, 4);

    fn rgb(c: (u8, u8, u8)) -> slint::Color {
        slint::Color::from_rgb_u8(c.0, c.1, c.2)
    }

    fn marks() -> UiMarks {
        UiMarks {
            working: rgb(WORKING),
            finished: rgb(DONE),
            question: rgb(QUESTION),
            failed: rgb(FAILED),
            rainbow: Vec::new(),
            ink: rgb((9, 9, 9)),
        }
    }

    fn switches(indicator: Indicator) -> Switches {
        Switches {
            indicator,
            done_on: true,
            question_on: true,
            failed_on: true,
            rainbow: true,
        }
    }

    /// What the hub would hold, as owned sets.
    #[derive(Default)]
    struct World {
        agents: HashSet<String>,
        working: HashSet<String>,
        question: HashSet<String>,
        failed: HashSet<String>,
        done: HashSet<String>,
        kinds: HashMap<String, DetectedAgent>,
        failed_kinds: HashMap<String, String>,
    }

    impl World {
        fn rows<'a>(&'a self, sw: Switches, m: &'a UiMarks) -> AgentRows<'a> {
            AgentRows {
                sets: TabSets {
                    agents: &self.agents,
                    working: &self.working,
                    question: &self.question,
                    failed: &self.failed,
                    done: &self.done,
                },
                sw,
                marks: m,
                kinds: self.kinds.clone(),
                failed_kinds: self.failed_kinds.clone(),
                loading: HashSet::new(),
            }
        }
    }

    fn set_of(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn view_in(m: &TabModel, strip_phys: i32, w: &World, sw: Switches) -> View {
        let marks = marks();
        let agent = w.rows(sw, &marks);
        let ids: Vec<&str> = m.state.tabs.iter().map(|t| t.id.as_str()).collect();
        let needs = needs_of(ids.into_iter(), &agent);
        let offsets = HashMap::new();
        build_view(&ViewInput {
            model: m,
            home: HOME,
            query: "",
            strip_phys,
            scale: 1.0,
            needs: &needs,
            offsets: &offsets,
            carried: None,
            agent: &agent,
        })
    }

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
        let world = World::default();
        let marks = marks();
        let agent = world.rows(switches(Indicator::Ring), &marks);
        build_view(&ViewInput {
            model: m,
            home: HOME,
            query,
            strip_phys,
            scale,
            needs: &needs,
            offsets: &offsets,
            carried: None,
            agent: &agent,
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
    fn dots_are_the_needs_of_hidden_tabs_up_to_four() {
        let m = forty();
        let ids = ids_of(&m);
        let strs: Vec<&str> = ids.iter().map(String::as_str).collect();
        // Most tabs need you, so some needy ones are pushed into the list.
        let mut w = World::default();
        for (i, id) in strs.iter().enumerate().skip(1).take(35) {
            match i % 3 {
                0 => w.done.insert(id.to_string()),
                1 => w.question.insert(id.to_string()),
                _ => w.failed.insert(id.to_string()),
            };
        }
        let v = view_in(&m, 1000, &w, switches(Indicator::Ring));
        let hidden_needy: Vec<(MarkRole, slint::Color)> = v
            .rows
            .iter()
            .filter(|r| r.mode == 2)
            .filter_map(|r| r.need.map(|n| (n, r.mark_color)))
            .collect();
        assert!(hidden_needy.len() > 4);
        assert_eq!(v.geom.dots.len(), 4);
        let dots: Vec<(MarkRole, slint::Color)> =
            v.geom.dots.iter().map(|d| (d.role, d.colour)).collect();
        assert_eq!(dots, hidden_needy[..4].to_vec());
        for (role, colour) in dots {
            let want = match role {
                MarkRole::Done => rgb(DONE),
                MarkRole::Question => rgb(QUESTION),
                MarkRole::Failed => rgb(FAILED),
                MarkRole::Working => rgb(WORKING),
            };
            assert_eq!(colour, want);
        }
        // None hidden needy: no dots.
        let plain = view_of(&m, 1000, 1.0, "");
        assert!(plain.geom.dots.is_empty());
    }

    #[test]
    fn a_switched_off_state_draws_a_transparent_dot() {
        let m = forty();
        let ids = ids_of(&m);
        let mut w = World::default();
        for id in ids.iter().skip(1) {
            w.question.insert(id.clone());
        }
        let mut sw = switches(Indicator::Ring);
        sw.question_on = false;
        let v = view_in(&m, 1000, &w, sw);
        assert_eq!(v.geom.dots.len(), 4);
        for d in &v.geom.dots {
            assert_eq!(d.role, MarkRole::Question);
            assert_eq!(d.colour.alpha(), 0);
        }
    }

    #[test]
    fn needs_are_the_raw_sets_and_the_role_is_the_look_state() {
        let m = three();
        let ids = ids_of(&m);
        let mut w = World::default();
        w.done.insert(ids[0].clone());
        w.question.insert(ids[1].clone());
        w.agents.insert(ids[2].clone());
        w.working.insert(ids[2].clone());
        w.done.insert(ids[2].clone());
        let marks = marks();
        let strs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut sw = switches(Indicator::Ring);
        let rows = w.rows(sw, &marks);
        let needs = needs_of(strs.iter().copied(), &rows);
        assert_eq!(needs[&ids[0]], MarkRole::Done);
        assert_eq!(needs[&ids[1]], MarkRole::Question);
        // Working wins in the look, and the tab is still a need through the done set.
        assert_eq!(needs[&ids[2]], MarkRole::Working);
        // A switched-off question still keeps its tab visible (the raw set), as a question.
        sw.question_on = false;
        let rows = w.rows(sw, &marks);
        assert_eq!(
            needs_of(strs.iter().copied(), &rows)[&ids[1]],
            MarkRole::Question
        );
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
            agent: 0,
            mark_state: 0,
            mark_kind: 0,
            mark_color: slint::Color::from_argb_u8(0, 0, 0, 0),
            mark_rainbow: false,
        }
    }

    #[test]
    fn sync_rows_updates_in_place_and_only_grows_or_shrinks_at_the_end() {
        let target = slint::VecModel::<crate::TabRow>::default();
        let three = [row("a", 0.0), row("b", 0.0), row("c", 0.0)];
        sync_rows(&target, &three);
        assert_eq!(target.row_count(), 3);
        let before: Vec<crate::TabRow> = (0..3).map(|i| target.row_data(i).unwrap()).collect();
        let moved = [row("a", 0.0), row("b", 12.5), row("c", 0.0)];
        sync_rows(&target, &moved);
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
        sync_rows(&target, &five);
        assert_eq!(target.row_count(), 5);
        assert_eq!(target.row_data(1).unwrap().offset, 12.5);
        sync_rows(&target, &five[..2]);
        assert_eq!(target.row_count(), 2);
        assert_eq!(target.row_data(1).unwrap().id, "b");
    }

    fn one_tab_world(state: &str) -> (TabModel, String, World) {
        let m = model_of(&[r"C:\a"]);
        let id = ids_of(&m)[0].clone();
        let mut w = World::default();
        w.agents.insert(id.clone());
        match state {
            "working" => w.working.insert(id.clone()),
            "question" => w.question.insert(id.clone()),
            "done" => w.done.insert(id.clone()),
            _ => w.failed.insert(id.clone()),
        };
        (m, id, w)
    }

    #[test]
    fn a_working_tab_gets_a_ring_mark_row_under_ring_and_a_run_bar_under_bar() {
        let (m, _, w) = one_tab_world("working");
        let r = view_in(&m, 1000, &w, switches(Indicator::Ring))
            .rows
            .remove(0);
        assert_eq!((r.mark_kind, r.mark_state), (1, 1));
        let r = view_in(&m, 1000, &w, switches(Indicator::Bar))
            .rows
            .remove(0);
        assert_eq!((r.mark_kind, r.mark_state), (3, 1));
        assert_eq!(r.mark_color, rgb(WORKING));
        assert!(!r.mark_rainbow);

        let (m, _, w) = one_tab_world("question");
        let r = view_in(&m, 1000, &w, switches(Indicator::Ring))
            .rows
            .remove(0);
        assert_eq!((r.mark_kind, r.mark_state), (1, 2));
        let r = view_in(&m, 1000, &w, switches(Indicator::Bar))
            .rows
            .remove(0);
        assert_eq!((r.mark_kind, r.mark_state), (2, 2));
        assert_eq!(r.mark_color, rgb(QUESTION));

        let (m, _, w) = one_tab_world("done");
        let r = view_in(&m, 1000, &w, switches(Indicator::Bar))
            .rows
            .remove(0);
        assert_eq!((r.mark_kind, r.mark_state), (2, 3));
        assert!(
            r.mark_rainbow,
            "a finished line flows when the rainbow is on"
        );
        let mut sw = switches(Indicator::Bar);
        sw.rainbow = false;
        let r = view_in(&m, 1000, &w, sw).rows.remove(0);
        assert!(!r.mark_rainbow);
        assert_eq!(r.mark_color, rgb(DONE));

        let (m, _, w) = one_tab_world("failed");
        let r = view_in(&m, 1000, &w, switches(Indicator::Ring))
            .rows
            .remove(0);
        assert_eq!((r.mark_kind, r.mark_state), (1, 4));
        assert_eq!(r.mark_color, rgb(FAILED));

        let plain = view_of(&three(), 1000, 1.0, "");
        assert!(
            plain
                .rows
                .iter()
                .all(|r| r.mark_kind == 0 && r.mark_state == 0)
        );
    }

    #[test]
    fn the_switches_hide_their_marks() {
        let (m, _, w) = one_tab_world("question");
        let mut sw = switches(Indicator::Ring);
        sw.question_on = false;
        let r = view_in(&m, 1000, &w, sw).rows.remove(0);
        assert_eq!((r.mark_kind, r.mark_state), (0, 0));
        assert_eq!(r.mark_color.alpha(), 0);

        // Failed off: the tab shows finished when it is also in done.
        let (m, id, mut w) = one_tab_world("failed");
        w.done.insert(id);
        let mut sw = switches(Indicator::Ring);
        sw.failed_on = false;
        let r = view_in(&m, 1000, &w, sw).rows.remove(0);
        assert_eq!(r.mark_state, 3);
        // Done off too: none.
        sw.done_on = false;
        let r = view_in(&m, 1000, &w, sw).rows.remove(0);
        assert_eq!(r.mark_state, 0);
    }

    #[test]
    fn a_failed_tab_tooltip_carries_the_kind() {
        let (m, id, mut w) = one_tab_world("failed");
        w.failed_kinds.insert(id.clone(), "rate_limit".into());
        let r = view_in(&m, 1000, &w, switches(Indicator::Ring))
            .rows
            .remove(0);
        assert_eq!(r.tip, "C:\\a\nFailed: rate limit");
        w.failed_kinds.clear();
        let r = view_in(&m, 1000, &w, switches(Indicator::Ring))
            .rows
            .remove(0);
        assert_eq!(r.tip, "C:\\a\nFailed");
        // A tab that is not failed keeps its plain path.
        let (m, _, w) = one_tab_world("done");
        let r = view_in(&m, 1000, &w, switches(Indicator::Ring))
            .rows
            .remove(0);
        assert_eq!(r.tip, "C:\\a");
        // The Settings tab has none.
        let mut m = three();
        m.open_settings();
        let v = view_in(&m, 1000, &World::default(), switches(Indicator::Ring));
        assert_eq!(v.rows.iter().find(|r| r.settings).unwrap().tip, "");
    }

    #[test]
    fn the_icon_follows_the_agent_only_while_present() {
        let m = model_of(&[r"C:\a"]);
        let id = ids_of(&m)[0].clone();
        let mut w = World::default();
        w.kinds.insert(id.clone(), DetectedAgent::Claude);
        let agent_of = |w: &World| view_in(&m, 1000, w, switches(Indicator::Ring)).rows[0].agent;
        assert_eq!(agent_of(&w), 0, "a Claude kind without a present agent");
        w.agents.insert(id.clone());
        assert_eq!(agent_of(&w), 1);
        w.kinds.insert(id.clone(), DetectedAgent::Codex);
        assert_eq!(agent_of(&w), 2);
        w.kinds.insert(id, DetectedAgent::Other);
        assert_eq!(agent_of(&w), 0);
        let mut m = three();
        m.open_settings();
        let v = view_in(&m, 1000, &World::default(), switches(Indicator::Ring));
        assert_eq!(v.rows.iter().find(|r| r.settings).unwrap().agent, 0);
    }

    #[test]
    fn sixteen_tabs_six_hidden_five_marked_gives_four_dots_in_strip_order() {
        let paths: Vec<String> = (0..16).map(|i| format!(r"C:\w\p{i}")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let m = model_of(&refs);
        // Every tab but the first needs you, so the plan cannot keep them all visible.
        let ids = ids_of(&m);
        let mut w = World::default();
        for (i, id) in ids.iter().enumerate().skip(1) {
            match i % 3 {
                0 => w.done.insert(id.clone()),
                1 => w.question.insert(id.clone()),
                _ => w.failed.insert(id.clone()),
            };
        }
        let v = (200..=700)
            .rev()
            .step_by(20)
            .map(|px| view_in(&m, px, &w, switches(Indicator::Ring)))
            .find(|v| {
                v.rows
                    .iter()
                    .filter(|r| r.mode == 2 && r.need.is_some())
                    .count()
                    >= 5
            })
            .expect("a width that hides five tabs");
        let hidden_needs: Vec<(MarkRole, slint::Color)> = v
            .rows
            .iter()
            .filter(|r| r.mode == 2)
            .filter_map(|r| r.need.map(|n| (n, r.mark_color)))
            .collect();
        assert!(hidden_needs.len() >= 5);
        let dots: Vec<(MarkRole, slint::Color)> =
            v.geom.dots.iter().map(|d| (d.role, d.colour)).collect();
        assert_eq!(dots, hidden_needs[..4].to_vec(), "four, in strip order");
    }
}
