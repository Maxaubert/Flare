//! One agent call on the web panel as a small state machine (Surfaces stage 4): look, click, type,
//! press, scroll, wait, eval. It never touches the browser. It says which DevTools Protocol calls to
//! make ([`Out::Calls`]), is told each answer ([`Job::on_reply`]) and the passing of time
//! ([`Job::on_tick`]), and finishes with a result or an error ([`Out::Done`]). The window code owns
//! the browser, the clock and the timers (rule 34: nothing here waits).
//!
//! Calls go out in order; where two do not depend on each other (the layout and the accessibility
//! tree, the boxes of 600 elements) they go out together and the job waits for all of them.

use std::collections::{HashMap, VecDeque};

use serde_json::{Value, json};

use crate::agent_web::{
    self as web, Button, Candidate, EVAL_RESULT_MAX, Viewport, WaitFor, click_events, click_point,
    element_rect, key_events, png_size_b64, shot_params, shot_scale, viewport_of, wheel_event,
};

/// How long the pieces of a call may take before it is answered `timeout`.
pub const LOOK_BUDGET_MS: u64 = 20_000;
pub const ACT_BUDGET_MS: u64 = 12_000;
/// How often a wait looks again.
pub const POLL_MS: u64 = 250;
/// A picture that has not come in this long after the boxes were asked for is given up: the list goes
/// without it (a hidden or hung page never paints).
pub const SHOT_BUDGET_MS: u64 = 6_000;
/// Why a look has no picture when the page is not on screen.
pub const HIDDEN_NOTE: &str =
    "the page is not on screen (its tab is behind or a Flare menu is open over it)";
/// A wait's check is a script of its own; a page that cannot run it in this long counts as not yet.
const CHECK_TIMEOUT_MS: u64 = 2_000;
/// The longest an `eval` may run in the page.
const EVAL_TIMEOUT_MS: u64 = 10_000;

/// A DevTools Protocol call to make. Ids are unique across the window so a late answer for a finished
/// job cannot be mistaken for a new one's.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub id: u64,
    pub method: String,
    pub params: Value,
}

/// Why a call failed, in the channel's own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fail {
    BadParams,
    NotFound,
    Unavailable,
    Timeout,
    /// The person took over the page.
    Interrupted,
    Internal,
}

#[derive(Clone, Debug, PartialEq)]
pub struct JobError {
    pub fail: Fail,
    pub message: String,
}

fn err<T>(fail: Fail, message: impl Into<String>) -> Result<T, JobError> {
    Err(JobError {
        fail,
        message: message.into(),
    })
}

/// What a click or scroll points at.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Ref(String),
    /// A point in CSS pixels from the page's top left corner as it is shown (the `rect`s of a look).
    Point(f64, f64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// `picture` is false when the page is not on screen (its tab is behind, or a menu hides it): a
    /// hidden page paints nothing, so a screenshot would never come. The list is still made.
    Look {
        picture: bool,
    },
    Click {
        target: Target,
        button: Button,
        double: bool,
    },
    Type {
        text: String,
        target: Option<String>,
    },
    Press {
        key: web::KeySpec,
    },
    Scroll {
        dx: f64,
        dy: f64,
        target: Option<String>,
    },
    Wait {
        what: WaitFor,
        timeout_ms: u64,
    },
    Eval {
        expression: String,
    },
}

impl Op {
    /// The method's name, for the ribbon and the log.
    pub fn name(&self) -> &'static str {
        match self {
            Op::Look { .. } => "look",
            Op::Click { .. } => "click",
            Op::Type { .. } => "type",
            Op::Press { .. } => "press",
            Op::Scroll { .. } => "scroll",
            Op::Wait { .. } => "wait",
            Op::Eval { .. } => "eval",
        }
    }
}

/// The element numbers of the last look: `e1` is the browser's node number so far, until the next look
/// or navigation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefMap(HashMap<String, i64>);

impl RefMap {
    pub fn get(&self, r: &str) -> Option<i64> {
        self.0.get(r).copied()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What the page is, for the answer of a look.
#[derive(Clone, Copy, Debug)]
pub struct Page<'a> {
    pub url: &'a str,
    pub title: &'a str,
}

/// What a job asks of the window next.
#[derive(Debug, PartialEq)]
pub enum Out {
    /// Nothing until the next answer or tick.
    Idle,
    Calls(Vec<Call>),
    Done(Result<Value, JobError>),
}

/// Hands out call ids.
pub struct Ids<'a>(pub &'a mut u64);

impl Ids<'_> {
    fn next(&mut self) -> u64 {
        *self.0 += 1;
        *self.0
    }
}

type Slot = (u64, Option<Result<Value, String>>);

enum Then {
    Click { button: Button, double: bool },
    Wheel { dx: f64, dy: f64 },
}

enum Stage {
    /// Layout metrics, the accessibility tree and the device pixel ratio (and DOM.enable, whose
    /// answer does not matter).
    LookA {
        layout: Slot,
        ax: Slot,
        dpr: Slot,
        enable: u64,
        picture: bool,
    },
    /// The screenshot and the boxes of the chosen elements.
    LookB {
        vp: Viewport,
        dpr: f64,
        cands: Vec<Candidate>,
        truncated: bool,
        shot: Slot,
        boxes: Vec<Slot>,
        /// When the picture is given up on.
        shot_by_ms: u64,
    },
    /// Bring the element into view.
    PrepA {
        backend: i64,
        then: Then,
        enable: u64,
        scroll: u64,
        got: usize,
    },
    /// Where it is now.
    PrepB {
        then: Then,
        layout: Slot,
        quads: Slot,
    },
    /// Only the page's size (a wheel with nothing named).
    Metrics {
        then: Then,
        layout: Slot,
    },
    /// Input events, one at a time.
    Seq {
        queue: VecDeque<(&'static str, Value)>,
        cur: u64,
        /// The event in flight: if it is a release and fails, the renderer still holds what it let go of.
        cur_call: Call,
        result: Value,
    },
    Focus {
        text: String,
        enable: u64,
        focus: u64,
        got: usize,
    },
    Wait {
        what: WaitFor,
        end_ms: u64,
        next_ms: u64,
        in_flight: Option<u64>,
        /// At least one check has been made (a wait of zero still looks once).
        checked: bool,
    },
    One {
        id: u64,
        eval: bool,
    },
}

pub struct Job {
    op: &'static str,
    stage: Option<Stage>,
    deadline_ms: u64,
    refs: Option<RefMap>,
    /// The page's address and title as the last answer found them (a look given up on the picture
    /// names the page from these).
    page: (String, String),
    /// The releases of a press the job was cut short in (a timeout takes the stage before the window
    /// asks); see [`Job::release_calls`].
    release: Vec<Call>,
}

fn call(ids: &mut Ids<'_>, method: &str, params: Value) -> Call {
    Call {
        id: ids.next(),
        method: method.to_string(),
        params,
    }
}

fn slot(c: &Call) -> Slot {
    (c.id, None)
}

fn parse(reply: Result<String, String>) -> Result<Value, String> {
    match reply {
        Ok(s) if s.trim().is_empty() => Ok(json!({})),
        Ok(s) => {
            serde_json::from_str(&s).map_err(|e| format!("the browser's answer was not JSON: {e}"))
        }
        Err(e) => Err(e),
    }
}

fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

impl Job {
    pub fn op(&self) -> &'static str {
        self.op
    }

    /// The numbers a finished look handed out; the window keeps them for the clicks that follow.
    pub fn take_refs(&mut self) -> Option<RefMap> {
        self.refs.take()
    }

    /// The calls that let go of what a job cut short had pressed: the mouse button or key whose release
    /// was still queued. The renderer would otherwise believe it is still held (the person's next moves
    /// would drag or select). The window sends them and waits for no answer.
    pub fn release_calls(&mut self, ids: &mut Ids<'_>) -> Vec<Call> {
        let mut out = std::mem::take(&mut self.release);
        if let Some(Stage::Seq { queue, .. }) = &self.stage {
            out.extend(releases_of(queue, ids));
        }
        out
    }

    /// The person took over the page: the call ends.
    pub fn interrupted() -> JobError {
        JobError {
            fail: Fail::Interrupted,
            message: "the user took over the page; look again before doing anything more"
                .to_string(),
        }
    }

    /// Starts a job. `refs` are the numbers of the last look; a ref that is not among them fails here.
    pub fn start(op: Op, refs: &RefMap, now_ms: u64, ids: &mut Ids<'_>) -> (Option<Job>, Out) {
        let name = op.name();
        let finish = |r: Result<Value, JobError>| (None, Out::Done(r));
        let backend_of = |r: &str| -> Result<i64, JobError> {
            refs.get(r).ok_or_else(|| JobError {
                fail: Fail::NotFound,
                message: format!("there is no element {r}; refs last until the next look or page load, so look again"),
            })
        };
        let job = |stage: Stage, budget: u64, calls: Vec<Call>| {
            (
                Some(Job {
                    op: name,
                    stage: Some(stage),
                    deadline_ms: now_ms + budget,
                    refs: None,
                    page: Default::default(),
                    release: Vec::new(),
                }),
                Out::Calls(calls),
            )
        };
        match op {
            Op::Look { picture } => {
                let enable = call(ids, "DOM.enable", json!({}));
                let layout = call(ids, "Page.getLayoutMetrics", json!({}));
                let ax = call(ids, "Accessibility.getFullAXTree", json!({}));
                // A fixed question, not the agent's: how many device pixels make a CSS pixel here.
                let dpr = call(
                    ids,
                    "Runtime.evaluate",
                    json!({
                        "expression": "window.devicePixelRatio",
                        "returnByValue": true,
                        // A page that hangs or lies in its own getter costs the ratio, not the look.
                        "timeout": CHECK_TIMEOUT_MS,
                    }),
                );
                job(
                    Stage::LookA {
                        layout: slot(&layout),
                        ax: slot(&ax),
                        dpr: slot(&dpr),
                        enable: enable.id,
                        picture,
                    },
                    LOOK_BUDGET_MS,
                    vec![enable, layout, ax, dpr],
                )
            }
            Op::Click {
                target: Target::Point(x, y),
                button,
                double,
            } => {
                if !(x.is_finite() && y.is_finite()) || x < 0.0 || y < 0.0 {
                    return finish(err(
                        Fail::BadParams,
                        "x and y are page pixels from the top left, zero or more",
                    ));
                }
                let (queue, first) = seq_of(
                    click_events(x, y, button, double),
                    "Input.dispatchMouseEvent",
                    ids,
                );
                job(
                    Stage::Seq {
                        queue,
                        cur: first.id,
                        cur_call: first.clone(),
                        result: json!({"clicked": {"x": r1(x), "y": r1(y)}}),
                    },
                    ACT_BUDGET_MS,
                    vec![first],
                )
            }
            Op::Click {
                target: Target::Ref(r),
                button,
                double,
            } => match backend_of(&r) {
                Err(e) => finish(Err(e)),
                Ok(b) => prep_a(b, Then::Click { button, double }, now_ms, name, ids),
            },
            Op::Scroll { dx, dy, target } => {
                if !(dx.is_finite() && dy.is_finite()) {
                    return finish(err(Fail::BadParams, "dx and dy must be numbers"));
                }
                match target {
                    Some(r) => match backend_of(&r) {
                        Err(e) => finish(Err(e)),
                        Ok(b) => prep_a(b, Then::Wheel { dx, dy }, now_ms, name, ids),
                    },
                    None => {
                        let layout = call(ids, "Page.getLayoutMetrics", json!({}));
                        job(
                            Stage::Metrics {
                                then: Then::Wheel { dx, dy },
                                layout: slot(&layout),
                            },
                            ACT_BUDGET_MS,
                            vec![layout],
                        )
                    }
                }
            }
            Op::Type { text, target } => match target {
                Some(r) => match backend_of(&r) {
                    Err(e) => finish(Err(e)),
                    Ok(b) => {
                        let enable = call(ids, "DOM.enable", json!({}));
                        let focus = call(ids, "DOM.focus", json!({"backendNodeId": b}));
                        job(
                            Stage::Focus {
                                text,
                                enable: enable.id,
                                focus: focus.id,
                                got: 0,
                            },
                            ACT_BUDGET_MS,
                            vec![enable, focus],
                        )
                    }
                },
                None => insert_text(text, now_ms, name, ids),
            },
            Op::Press { key } => {
                let (queue, first) = seq_of(key_events(&key), "Input.dispatchKeyEvent", ids);
                job(
                    Stage::Seq {
                        queue,
                        cur: first.id,
                        cur_call: first.clone(),
                        result: json!({}),
                    },
                    ACT_BUDGET_MS,
                    vec![first],
                )
            }
            Op::Wait { what, timeout_ms } => {
                let end = now_ms + timeout_ms;
                // The first check is made at once (the text may be there already); a pause has none.
                let mut j = Job {
                    op: name,
                    stage: Some(Stage::Wait {
                        what,
                        end_ms: end,
                        next_ms: now_ms,
                        in_flight: None,
                        checked: false,
                    }),
                    deadline_ms: end + 5_000,
                    refs: None,
                    page: Default::default(),
                    release: Vec::new(),
                };
                let out = j.on_tick(now_ms, ids);
                (Some(j), out)
            }
            Op::Eval { expression } => {
                let c = call(
                    ids,
                    "Runtime.evaluate",
                    json!({
                        "expression": expression,
                        "returnByValue": true,
                        "awaitPromise": true,
                        "userGesture": false,
                        "timeout": EVAL_TIMEOUT_MS,
                    }),
                );
                job(
                    Stage::One {
                        id: c.id,
                        eval: true,
                    },
                    EVAL_TIMEOUT_MS + 5_000,
                    vec![c],
                )
            }
        }
    }

    /// Time has passed (and a wait may be due to look again).
    pub fn on_tick(&mut self, now_ms: u64, ids: &mut Ids<'_>) -> Out {
        if let Some(Stage::Wait {
            what,
            end_ms,
            next_ms,
            in_flight,
            checked,
        }) = self.stage.as_mut()
        {
            if matches!(what, WaitFor::Ms(_)) {
                if now_ms >= *end_ms {
                    self.stage = None;
                    return Out::Done(Ok(json!({"met": true})));
                }
                return Out::Idle;
            }
            if in_flight.is_none() && *checked && now_ms >= *end_ms {
                self.stage = None;
                return Out::Done(Ok(json!({"met": false})));
            }
            if in_flight.is_none() && now_ms >= *next_ms {
                let expr = web::wait_expression(what).unwrap_or_default();
                let c = call(
                    ids,
                    "Runtime.evaluate",
                    json!({
                        "expression": expr,
                        "returnByValue": true,
                        "timeout": CHECK_TIMEOUT_MS,
                    }),
                );
                *in_flight = Some(c.id);
                *next_ms = now_ms + POLL_MS;
                *checked = true;
                return Out::Calls(vec![c]);
            }
            // A check that never answers does not hold the wait past its end.
            if now_ms >= *end_ms + 3_000 {
                self.stage = None;
                return Out::Done(Ok(json!({"met": false})));
            }
            return Out::Idle;
        }
        // A picture that never comes is given up on; the list goes without it.
        if let Some(Stage::LookB {
            shot, shot_by_ms, ..
        }) = self.stage.as_mut()
            && shot.1.is_none()
            && now_ms >= *shot_by_ms
        {
            shot.1 = Some(Err(
                "the browser did not paint a picture in time (is the page on screen?)".to_string(),
            ));
            return self.finish_if_looked();
        }
        if self.stage.is_some() && now_ms >= self.deadline_ms {
            if let Some(Stage::Seq { queue, .. }) = &self.stage {
                self.release = releases_of(queue, ids);
            }
            self.stage = None;
            return Out::Done(err(
                Fail::Timeout,
                "the page did not answer in time (is its tab in front and the page responding?)",
            ));
        }
        Out::Idle
    }

    /// A look whose picture was just given up on: done if every box is in too.
    fn finish_if_looked(&mut self) -> Out {
        let Some(Stage::LookB {
            vp,
            dpr,
            cands,
            truncated,
            shot,
            boxes,
            ..
        }) = self.stage.take()
        else {
            return Out::Idle;
        };
        if boxes.iter().any(|b| b.1.is_none()) {
            // The boxes are still coming: the stage goes back, now without a picture to wait for.
            self.stage = Some(Stage::LookB {
                vp,
                dpr,
                cands,
                truncated,
                shot,
                boxes,
                shot_by_ms: u64::MAX,
            });
            return Out::Idle;
        }
        let page = std::mem::take(&mut self.page);
        let result = self.finish_look(
            &vp,
            dpr,
            cands,
            truncated,
            &shot,
            &boxes,
            Page {
                url: &page.0,
                title: &page.1,
            },
        );
        Out::Done(Ok(result))
    }

    /// The browser answered call `id`.
    pub fn on_reply(
        &mut self,
        id: u64,
        reply: Result<String, String>,
        page: Page<'_>,
        now_ms: u64,
        ids: &mut Ids<'_>,
    ) -> Out {
        let Some(stage) = self.stage.take() else {
            return Out::Idle;
        };
        self.page = (page.url.to_string(), page.title.to_string());
        let r = parse(reply);
        let (stage, out) = self.step(stage, id, r, page, now_ms, ids);
        match out {
            Out::Done(_) => self.stage = None,
            _ => self.stage = stage,
        }
        out
    }

    fn step(
        &mut self,
        stage: Stage,
        id: u64,
        r: Result<Value, String>,
        page: Page<'_>,
        now_ms: u64,
        ids: &mut Ids<'_>,
    ) -> (Option<Stage>, Out) {
        match stage {
            Stage::LookA {
                mut layout,
                mut ax,
                mut dpr,
                enable,
                picture,
            } => {
                if id == layout.0 {
                    layout.1 = Some(r);
                } else if id == ax.0 {
                    ax.1 = Some(r);
                } else if id == dpr.0 {
                    dpr.1 = Some(r);
                } else if id != enable {
                    return (
                        Some(Stage::LookA {
                            layout,
                            ax,
                            dpr,
                            enable,
                            picture,
                        }),
                        Out::Idle,
                    );
                }
                let (Some(l), Some(a), Some(d)) = (layout.1.clone(), ax.1.clone(), dpr.1.clone())
                else {
                    return (
                        Some(Stage::LookA {
                            layout,
                            ax,
                            dpr,
                            enable,
                            picture,
                        }),
                        Out::Idle,
                    );
                };
                let dpr = web::clean_dpr(
                    d.ok()
                        .and_then(|v| v.pointer("/result/value").and_then(Value::as_f64)),
                );
                let vp = match l {
                    Ok(v) => viewport_of(&v),
                    Err(_) => None,
                };
                let Some(vp) = vp else {
                    return done(err(
                        Fail::Unavailable,
                        "the page's size could not be read; is a page open and loaded?",
                    ));
                };
                let tree = match a {
                    Ok(v) => v,
                    Err(e) => {
                        return done(err(
                            Fail::Unavailable,
                            format!("the page could not be read: {e}"),
                        ));
                    }
                };
                let (cands, truncated) = web::candidates(&tree);
                let mut calls = Vec::new();
                let shot: Slot = if picture {
                    let c = call(ids, "Page.captureScreenshot", shot_params(&vp, dpr));
                    let sl = slot(&c);
                    calls.push(c);
                    sl
                } else {
                    (0, Some(Err(HIDDEN_NOTE.to_string())))
                };
                let mut boxes = Vec::with_capacity(cands.len());
                for c in &cands {
                    let q = call(
                        ids,
                        "DOM.getContentQuads",
                        json!({"backendNodeId": c.backend_id}),
                    );
                    boxes.push(slot(&q));
                    calls.push(q);
                }
                (
                    Some(Stage::LookB {
                        vp,
                        dpr,
                        cands,
                        truncated,
                        shot,
                        boxes,
                        shot_by_ms: now_ms + SHOT_BUDGET_MS,
                    }),
                    Out::Calls(calls),
                )
            }
            Stage::LookB {
                vp,
                dpr,
                cands,
                truncated,
                mut shot,
                mut boxes,
                shot_by_ms,
            } => {
                if id == shot.0 {
                    shot.1 = Some(r);
                } else if let Some(b) = boxes.iter_mut().find(|b| b.0 == id) {
                    b.1 = Some(r);
                }
                if shot.1.is_none() || boxes.iter().any(|b| b.1.is_none()) {
                    return (
                        Some(Stage::LookB {
                            vp,
                            dpr,
                            cands,
                            truncated,
                            shot,
                            boxes,
                            shot_by_ms,
                        }),
                        Out::Idle,
                    );
                }
                let result = self.finish_look(&vp, dpr, cands, truncated, &shot, &boxes, page);
                (None, Out::Done(Ok(result)))
            }
            Stage::PrepA {
                backend,
                then,
                enable,
                scroll,
                mut got,
            } => {
                // The answers of DOM.enable and of the scroll do not matter (a text node cannot be scrolled
                // to, and that is no reason to stop); both must be in before the position is asked.
                if id == enable || id == scroll {
                    got += 1;
                }
                if got < 2 {
                    return (
                        Some(Stage::PrepA {
                            backend,
                            then,
                            enable,
                            scroll,
                            got,
                        }),
                        Out::Idle,
                    );
                }
                let layout = call(ids, "Page.getLayoutMetrics", json!({}));
                let quads = call(
                    ids,
                    "DOM.getContentQuads",
                    json!({"backendNodeId": backend}),
                );
                (
                    Some(Stage::PrepB {
                        then,
                        layout: slot(&layout),
                        quads: slot(&quads),
                    }),
                    Out::Calls(vec![layout, quads]),
                )
            }
            Stage::PrepB {
                then,
                mut layout,
                mut quads,
            } => {
                if id == layout.0 {
                    layout.1 = Some(r);
                } else if id == quads.0 {
                    quads.1 = Some(r);
                }
                let (Some(l), Some(q)) = (layout.1.clone(), quads.1.clone()) else {
                    return (
                        Some(Stage::PrepB {
                            then,
                            layout,
                            quads,
                        }),
                        Out::Idle,
                    );
                };
                let gone = || {
                    err(
                        Fail::NotFound,
                        "that element is gone or has no place on the page; look again",
                    )
                };
                let Some(vp) = l.ok().and_then(|v| viewport_of(&v)) else {
                    return done(err(Fail::Unavailable, "the page's size could not be read"));
                };
                let Some((x, y)) = q.ok().and_then(|q| click_point(&q, &vp)) else {
                    return done(gone());
                };
                self.pointer_then(then, x, y, ids)
            }
            Stage::Metrics { then, mut layout } => {
                if id == layout.0 {
                    layout.1 = Some(r);
                }
                let Some(res) = layout.1.clone() else {
                    return (Some(Stage::Metrics { then, layout }), Out::Idle);
                };
                let Some(vp) = res.ok().and_then(|v| viewport_of(&v)) else {
                    return done(err(Fail::Unavailable, "the page's size could not be read"));
                };
                self.pointer_then(then, vp.w / 2.0, vp.h / 2.0, ids)
            }
            Stage::Seq {
                mut queue,
                cur,
                cur_call,
                result,
            } => {
                if id != cur {
                    return (
                        Some(Stage::Seq {
                            queue,
                            cur,
                            cur_call,
                            result,
                        }),
                        Out::Idle,
                    );
                }
                if let Err(e) = r {
                    // What the sequence pressed is let go of all the same: the releases still queued, and the
                    // one that just failed (a press went out, its release did not).
                    self.release = releases_of(&queue, ids);
                    if is_release(&cur_call.params) {
                        self.release
                            .insert(0, call(ids, &cur_call.method, cur_call.params.clone()));
                    }
                    return done(err(
                        Fail::Internal,
                        format!("the page refused the input: {e}"),
                    ));
                }
                match queue.pop_front() {
                    Some((method, params)) => {
                        let c = call(ids, method, params);
                        (
                            Some(Stage::Seq {
                                queue,
                                cur: c.id,
                                cur_call: c.clone(),
                                result,
                            }),
                            Out::Calls(vec![c]),
                        )
                    }
                    None => (None, Out::Done(Ok(result))),
                }
            }
            Stage::Focus {
                text,
                enable,
                focus,
                mut got,
            } => {
                if id == focus {
                    if let Err(e) = &r {
                        return done(err(
                            Fail::NotFound,
                            format!(
                                "that element could not be given the keyboard ({e}); click it first or look again"
                            ),
                        ));
                    }
                    got += 1;
                } else if id == enable {
                    got += 1;
                }
                if got < 2 {
                    return (
                        Some(Stage::Focus {
                            text,
                            enable,
                            focus,
                            got,
                        }),
                        Out::Idle,
                    );
                }
                let name = self.op;
                let (job, out) = insert_text(text, now_ms, name, ids);
                (job.and_then(|j| j.stage), out)
            }
            Stage::Wait {
                what,
                end_ms,
                next_ms,
                in_flight,
                checked,
            } => {
                if in_flight != Some(id) {
                    return (
                        Some(Stage::Wait {
                            what,
                            end_ms,
                            next_ms,
                            in_flight,
                            checked,
                        }),
                        Out::Idle,
                    );
                }
                let met = r
                    .ok()
                    .and_then(|v| v.pointer("/result/value").and_then(Value::as_bool))
                    .unwrap_or(false);
                if met {
                    return (None, Out::Done(Ok(json!({"met": true}))));
                }
                (
                    Some(Stage::Wait {
                        what,
                        end_ms,
                        next_ms,
                        in_flight: None,
                        checked,
                    }),
                    Out::Idle,
                )
            }
            Stage::One { id: want, eval } => {
                if id != want {
                    return (Some(Stage::One { id: want, eval }), Out::Idle);
                }
                let v = match r {
                    Ok(v) => v,
                    Err(e) => {
                        return done(err(Fail::Internal, format!("the page did not run it: {e}")));
                    }
                };
                if let Some(ex) = v.get("exceptionDetails") {
                    let text = ex
                        .pointer("/exception/description")
                        .and_then(Value::as_str)
                        .or_else(|| ex.get("text").and_then(Value::as_str))
                        .unwrap_or("the script threw");
                    return done(err(
                        Fail::BadParams,
                        format!("the script threw: {}", web::clean_text(text, 400)),
                    ));
                }
                let value = v.pointer("/result/value").cloned().unwrap_or(Value::Null);
                (None, Out::Done(Ok(eval_answer(value))))
            }
        }
    }

    fn pointer_then(
        &mut self,
        then: Then,
        x: f64,
        y: f64,
        ids: &mut Ids<'_>,
    ) -> (Option<Stage>, Out) {
        let (events, method, result): (Vec<Value>, &'static str, Value) = match then {
            Then::Click { button, double } => (
                click_events(x, y, button, double),
                "Input.dispatchMouseEvent",
                json!({"clicked": {"x": r1(x), "y": r1(y)}}),
            ),
            Then::Wheel { dx, dy } => (
                vec![
                    json!({"type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0}),
                    wheel_event(x, y, dx, dy),
                ],
                "Input.dispatchMouseEvent",
                json!({"scrolled": {"x": r1(x), "y": r1(y)}}),
            ),
        };
        let (queue, first) = seq_of(events, method, ids);
        (
            Some(Stage::Seq {
                queue,
                cur: first.id,
                cur_call: first.clone(),
                result,
            }),
            Out::Calls(vec![first]),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_look(
        &mut self,
        vp: &Viewport,
        dpr: f64,
        cands: Vec<Candidate>,
        truncated: bool,
        shot: &Slot,
        boxes: &[Slot],
        page: Page<'_>,
    ) -> Value {
        let mut refs = HashMap::new();
        let mut elements = Vec::new();
        for (c, b) in cands.into_iter().zip(boxes) {
            let rect =
                b.1.as_ref()
                    .and_then(|r| r.as_ref().ok())
                    .and_then(element_rect);
            let Some(rect) = rect else { continue };
            let n = elements.len() + 1;
            let r = format!("e{n}");
            refs.insert(r.clone(), c.backend_id);
            let center = [r1(rect[0] + rect[2] / 2.0), r1(rect[1] + rect[3] / 2.0)];
            let offscreen = rect[0] + rect[2] <= 0.0
                || rect[1] + rect[3] <= 0.0
                || rect[0] >= vp.w
                || rect[1] >= vp.h;
            let mut e = json!({
                "ref": r,
                "role": c.role,
                "name": c.name,
                "rect": [r1(rect[0]), r1(rect[1]), r1(rect[2]), r1(rect[3])],
                "center": center,
            });
            if !c.value.is_empty() {
                e["value"] = json!(c.value);
            }
            if !c.states.is_empty() {
                e["state"] = json!(c.states);
            }
            if let Some(l) = c.level {
                e["level"] = json!(l);
            }
            if let Some(h) = c.href {
                e["href"] = json!(h);
            }
            if offscreen {
                e["offscreen"] = json!(true);
            }
            elements.push(e);
        }
        self.refs = Some(RefMap(refs));
        let mut out = json!({
            "url": page.url,
            "title": page.title,
            "viewport": {"width": r1(vp.w), "height": r1(vp.h), "device_scale": dpr},
            "elements": elements,
            "truncated": truncated,
        });
        match shot.1.as_ref() {
            Some(Ok(v)) => match v.get("data").and_then(Value::as_str) {
                Some(data) if data.len() / 4 * 3 <= web::SHOT_BYTES_MAX => {
                    let (w, h) = png_size_b64(data).unwrap_or((0, 0));
                    // Picture pixels per CSS pixel, as the picture really came out.
                    let scale = if w > 0 {
                        f64::from(w) / vp.w
                    } else {
                        dpr * shot_scale(vp.w, dpr)
                    };
                    out["image"] = json!({
                        "mime": "image/png",
                        "data": data,
                        "width": w,
                        "height": h,
                        "scale": (scale * 1000.0).round() / 1000.0,
                    });
                }
                Some(_) => out["image_error"] = json!("the picture was too large to send"),
                None => out["image_error"] = json!("the browser sent no picture"),
            },
            Some(Err(e)) => {
                out["image_error"] = json!(format!(
                    "the picture could not be taken: {}",
                    web::clean_text(e, 200)
                ));
            }
            None => {}
        }
        out
    }
}

fn done<T>(r: Result<Value, JobError>) -> (Option<T>, Out) {
    (None, Out::Done(r))
}

/// `Input.insertText` for the focused thing.
fn insert_text(
    text: String,
    now_ms: u64,
    name: &'static str,
    ids: &mut Ids<'_>,
) -> (Option<Job>, Out) {
    let n = text.chars().count();
    let c = call(ids, "Input.insertText", json!({"text": text}));
    (
        Some(Job {
            op: name,
            stage: Some(Stage::Seq {
                queue: VecDeque::new(),
                cur: c.id,
                cur_call: c.clone(),
                result: json!({"typed": n}),
            }),
            deadline_ms: now_ms + ACT_BUDGET_MS,
            refs: None,
            page: Default::default(),
            release: Vec::new(),
        }),
        Out::Calls(vec![c]),
    )
}

fn prep_a(
    backend: i64,
    then: Then,
    now_ms: u64,
    name: &'static str,
    ids: &mut Ids<'_>,
) -> (Option<Job>, Out) {
    let enable = call(ids, "DOM.enable", json!({}));
    let scroll = call(
        ids,
        "DOM.scrollIntoViewIfNeeded",
        json!({"backendNodeId": backend}),
    );
    (
        Some(Job {
            op: name,
            stage: Some(Stage::PrepA {
                backend,
                then,
                enable: enable.id,
                scroll: scroll.id,
                got: 0,
            }),
            deadline_ms: now_ms + ACT_BUDGET_MS,
            refs: None,
            page: Default::default(),
            release: Vec::new(),
        }),
        Out::Calls(vec![enable, scroll]),
    )
}

/// The events of a sequence: the first call, and the rest queued.
fn seq_of(
    events: Vec<Value>,
    method: &'static str,
    ids: &mut Ids<'_>,
) -> (VecDeque<(&'static str, Value)>, Call) {
    let mut it = events.into_iter();
    let first = call(ids, method, it.next().unwrap_or_else(|| json!({})));
    let queue = it.map(|p| (method, p)).collect();
    (queue, first)
}

/// Is this input event a release (a mouse button up, a key up)?
fn is_release(p: &Value) -> bool {
    matches!(
        p.get("type").and_then(Value::as_str),
        Some("mouseReleased" | "keyUp")
    )
}

/// The release events left in a queue of input (a mouse button up, a key up), as calls.
fn releases_of(queue: &VecDeque<(&'static str, Value)>, ids: &mut Ids<'_>) -> Vec<Call> {
    queue
        .iter()
        .filter(|(_, p)| is_release(p))
        .map(|(m, p)| call(ids, m, p.clone()))
        .collect()
}

/// What `browser.eval` answers: the value, cut when it is huge.
fn eval_answer(value: Value) -> Value {
    let text = value.to_string();
    if text.chars().count() <= EVAL_RESULT_MAX {
        return json!({"value": value});
    }
    let cut: String = text.chars().take(EVAL_RESULT_MAX).collect();
    json!({"value": cut, "truncated": true})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays the browser for a job: answers each call from `respond` in the order sent, with the clock
    /// stepping `step_ms` per answer, until the job is done.
    struct Rig {
        ids: u64,
        now: u64,
        log: Vec<(String, Value)>,
    }

    impl Rig {
        fn new() -> Self {
            Rig {
                ids: 0,
                now: 1_000,
                log: Vec::new(),
            }
        }

        fn run(
            &mut self,
            op: Op,
            refs: &RefMap,
            mut respond: impl FnMut(&str, &Value) -> Result<String, String>,
        ) -> (Result<Value, JobError>, Option<RefMap>) {
            let page = Page {
                url: "https://flare-test.local/index.html",
                title: "Test page",
            };
            let (job, mut out) = Job::start(op, refs, self.now, &mut Ids(&mut self.ids));
            let mut job = job;
            let mut guard = 0;
            loop {
                guard += 1;
                assert!(guard < 5_000, "the job never finished");
                match out {
                    Out::Done(r) => {
                        let refs = job.as_mut().and_then(Job::take_refs);
                        return (r, refs);
                    }
                    Out::Idle => {
                        self.now += POLL_MS;
                        out = job
                            .as_mut()
                            .expect("an idle job exists")
                            .on_tick(self.now, &mut Ids(&mut self.ids));
                    }
                    Out::Calls(calls) => {
                        let mut next = Out::Idle;
                        for c in calls {
                            self.log.push((c.method.clone(), c.params.clone()));
                            let reply = respond(&c.method, &c.params);
                            self.now += 3;
                            let o = job.as_mut().expect("calls need a job").on_reply(
                                c.id,
                                reply,
                                page,
                                self.now,
                                &mut Ids(&mut self.ids),
                            );
                            match o {
                                Out::Idle => {}
                                other => next = other,
                            }
                        }
                        out = next;
                    }
                }
            }
        }

        fn methods(&self) -> Vec<&str> {
            self.log.iter().map(|(m, _)| m.as_str()).collect()
        }
    }

    fn metrics(w: f64, h: f64) -> String {
        json!({"cssVisualViewport": {"pageX": 0, "pageY": 0, "clientWidth": w, "clientHeight": h}})
            .to_string()
    }

    fn quad(x: f64, y: f64, w: f64, h: f64) -> String {
        json!({"quads": [[x, y, x + w, y, x + w, y + h, x, y + h]]}).to_string()
    }

    fn page_ax() -> String {
        json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": ["2", "3", "4", "5"], "backendDOMNodeId": 1},
            {"nodeId": "2", "role": {"value": "heading"}, "name": {"value": "Shop"}, "properties": [{"name": "level", "value": {"value": 1}}], "backendDOMNodeId": 10},
            {"nodeId": "3", "role": {"value": "button"}, "name": {"value": "Buy"}, "backendDOMNodeId": 11},
            {"nodeId": "4", "role": {"value": "textbox"}, "name": {"value": "Quantity"}, "value": {"value": "1"}, "backendDOMNodeId": 12},
            {"nodeId": "5", "role": {"value": "link"}, "name": {"value": "Far away"}, "backendDOMNodeId": 13}
        ]}).to_string()
    }

    fn png_b64_small(w: u32, h: u32) -> String {
        // The header is enough for the size reader; the rest is padding.
        let mut b = vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
        ];
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in b.chunks(3) {
            let n = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            for i in 0..4 {
                s.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            }
        }
        s
    }

    /// The display's pixels per CSS pixel in these tests (a 225 percent display).
    const DPR: f64 = 2.25;

    fn browser(method: &str, params: &Value) -> Result<String, String> {
        match method {
            "Page.getLayoutMetrics" => Ok(metrics(800.0, 500.0)),
            "Accessibility.getFullAXTree" => Ok(page_ax()),
            "Runtime.evaluate" => {
                Ok(json!({"result": {"type": "number", "value": DPR}}).to_string())
            }
            // The browser draws at its device resolution and scales the clip by its own factor.
            "Page.captureScreenshot" => {
                let c = &params["clip"];
                let w =
                    c["width"].as_f64().unwrap_or(0.0) * c["scale"].as_f64().unwrap_or(1.0) * DPR;
                let h =
                    c["height"].as_f64().unwrap_or(0.0) * c["scale"].as_f64().unwrap_or(1.0) * DPR;
                Ok(json!({"data": png_b64_small(w.round() as u32, h.round() as u32)}).to_string())
            }
            "DOM.getContentQuads" => match params["backendNodeId"].as_i64() {
                Some(10) => Ok(quad(10.0, 10.0, 100.0, 30.0)),
                Some(11) => Ok(quad(100.0, 200.0, 80.0, 24.0)),
                Some(12) => Ok(quad(100.0, 250.0, 200.0, 24.0)),
                Some(13) => Ok(quad(100.0, 900.0, 80.0, 20.0)),
                _ => Err("Could not compute content quads.".into()),
            },
            _ => Ok("{}".into()),
        }
    }

    fn look(rig: &mut Rig) -> (Value, RefMap) {
        let (r, refs) = rig.run(Op::Look { picture: true }, &RefMap::default(), browser);
        (r.unwrap(), refs.unwrap())
    }

    #[test]
    fn a_look_reports_the_picture_the_elements_and_numbers_them() {
        let mut rig = Rig::new();
        let (v, refs) = look(&mut rig);
        assert_eq!(v["url"], json!("https://flare-test.local/index.html"));
        assert_eq!(v["viewport"]["width"], json!(800.0));
        assert_eq!(v["viewport"]["height"], json!(500.0));
        // 800 x 500 css pixels on a 2.25 display: 1800 device pixels wide, so the picture is scaled to 1600.
        assert_eq!(v["image"]["width"], json!(1600));
        assert_eq!(v["image"]["height"], json!(1000));
        assert_eq!(
            v["image"]["scale"],
            json!(2.0),
            "picture pixels per css pixel"
        );
        assert_eq!(v["viewport"]["device_scale"], json!(2.25));
        assert_eq!(v["image"]["mime"], json!("image/png"));
        let els = v["elements"].as_array().unwrap();
        let named: Vec<(&str, &str, &str)> = els
            .iter()
            .map(|e| {
                (
                    e["ref"].as_str().unwrap(),
                    e["role"].as_str().unwrap(),
                    e["name"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            named,
            [
                ("e1", "heading", "Shop"),
                ("e2", "button", "Buy"),
                ("e3", "textbox", "Quantity"),
                ("e4", "link", "Far away")
            ]
        );
        assert_eq!(els[1]["rect"], json!([100.0, 200.0, 80.0, 24.0]));
        assert_eq!(els[1]["center"], json!([140.0, 212.0]));
        assert_eq!(els[2]["value"], json!("1"));
        assert_eq!(els[0]["level"], json!(1));
        assert!(els[3]["offscreen"] == json!(true), "below the visible area");
        assert!(els[1].get("offscreen").is_none());
        assert_eq!(refs.get("e2"), Some(11));
        assert_eq!(refs.len(), 4);
        assert_eq!(v["truncated"], json!(false));
        // The first calls go out together, then the picture and the boxes together.
        let m = rig.methods();
        assert_eq!(
            &m[..4],
            [
                "DOM.enable",
                "Page.getLayoutMetrics",
                "Accessibility.getFullAXTree",
                "Runtime.evaluate"
            ]
        );
        assert_eq!(m[4], "Page.captureScreenshot");
        assert_eq!(m.iter().filter(|m| **m == "DOM.getContentQuads").count(), 4);
    }

    #[test]
    fn a_look_leaves_out_what_has_no_box_and_survives_a_missing_picture() {
        let mut rig = Rig::new();
        let (r, refs) = rig.run(
            Op::Look { picture: true },
            &RefMap::default(),
            |m, p| match m {
                "DOM.getContentQuads" if p["backendNodeId"] == json!(11) => Err("no box".into()),
                "Page.captureScreenshot" => Err("not visible".into()),
                _ => browser(m, p),
            },
        );
        let v = r.unwrap();
        assert!(v.get("image").is_none());
        assert!(v["image_error"].as_str().unwrap().contains("not visible"));
        let names: Vec<&str> = v["elements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Shop", "Quantity", "Far away"]);
        assert_eq!(
            refs.unwrap().get("e2"),
            Some(12),
            "refs count the elements that are shown"
        );
    }

    #[test]
    fn a_look_of_a_page_that_is_not_on_screen_takes_no_picture() {
        let mut rig = Rig::new();
        let (r, refs) = rig.run(Op::Look { picture: false }, &RefMap::default(), browser);
        let v = r.unwrap();
        assert!(v.get("image").is_none());
        assert!(v["image_error"].as_str().unwrap().contains(HIDDEN_NOTE));
        assert_eq!(
            v["elements"].as_array().unwrap().len(),
            4,
            "the list is made all the same"
        );
        assert_eq!(refs.unwrap().len(), 4);
        assert!(
            !rig.methods().contains(&"Page.captureScreenshot"),
            "a hidden page paints nothing, so the picture is not even asked for"
        );
    }

    #[test]
    fn a_picture_that_never_comes_is_given_up_on_and_the_list_goes_without_it() {
        let mut ids = 0;
        let (job, out) = Job::start(
            Op::Look { picture: true },
            &RefMap::default(),
            0,
            &mut Ids(&mut ids),
        );
        let mut job = job.unwrap();
        let page = Page {
            url: "https://a.example/",
            title: "A",
        };
        let Out::Calls(first) = out else { panic!() };
        let mut out = Out::Idle;
        for c in first {
            if c.method != "DOM.enable" {
                out = job.on_reply(
                    c.id,
                    browser(&c.method, &c.params),
                    page,
                    10,
                    &mut Ids(&mut ids),
                );
            }
        }
        let Out::Calls(second) = out else {
            panic!("{out:?}")
        };
        // Everything answers but the picture.
        let mut fin = Out::Idle;
        for c in &second {
            if c.method != "Page.captureScreenshot" {
                fin = job.on_reply(
                    c.id,
                    browser(&c.method, &c.params),
                    page,
                    20,
                    &mut Ids(&mut ids),
                );
            }
        }
        assert_eq!(fin, Out::Idle, "still waiting for the picture");
        assert_eq!(
            job.on_tick(SHOT_BUDGET_MS - 1, &mut Ids(&mut ids)),
            Out::Idle
        );
        let Out::Done(Ok(v)) = job.on_tick(SHOT_BUDGET_MS + 10, &mut Ids(&mut ids)) else {
            panic!("the look should end without its picture")
        };
        assert!(v.get("image").is_none());
        assert!(v["image_error"].as_str().unwrap().contains("did not paint"));
        assert_eq!(v["elements"].as_array().unwrap().len(), 4);
        assert_eq!(v["title"], json!("A"));
        assert!(job.take_refs().is_some());
    }

    #[test]
    fn a_picture_given_up_on_before_the_boxes_are_in_waits_for_them() {
        let mut ids = 0;
        let (job, out) = Job::start(
            Op::Look { picture: true },
            &RefMap::default(),
            0,
            &mut Ids(&mut ids),
        );
        let mut job = job.unwrap();
        let page = Page { url: "", title: "" };
        let Out::Calls(first) = out else { panic!() };
        let mut out = Out::Idle;
        for c in first {
            if c.method != "DOM.enable" {
                out = job.on_reply(
                    c.id,
                    browser(&c.method, &c.params),
                    page,
                    10,
                    &mut Ids(&mut ids),
                );
            }
        }
        let Out::Calls(second) = out else { panic!() };
        // Nothing but one box answers, then the picture's time is up: the look waits for the other boxes.
        let first_box = second
            .iter()
            .find(|c| c.method == "DOM.getContentQuads")
            .unwrap();
        job.on_reply(
            first_box.id,
            browser(&first_box.method, &first_box.params),
            page,
            20,
            &mut Ids(&mut ids),
        );
        assert_eq!(
            job.on_tick(SHOT_BUDGET_MS + 20, &mut Ids(&mut ids)),
            Out::Idle
        );
        let mut fin = Out::Idle;
        for c in second
            .iter()
            .filter(|c| c.method == "DOM.getContentQuads")
            .skip(1)
        {
            fin = job.on_reply(
                c.id,
                browser(&c.method, &c.params),
                page,
                SHOT_BUDGET_MS + 21,
                &mut Ids(&mut ids),
            );
        }
        assert!(matches!(fin, Out::Done(Ok(_))), "{fin:?}");
    }

    #[test]
    fn a_look_of_a_page_that_cannot_be_read_is_unavailable() {
        let mut rig = Rig::new();
        let (r, _) = rig.run(
            Op::Look { picture: true },
            &RefMap::default(),
            |m, p| match m {
                "Page.getLayoutMetrics" => Err("no page".into()),
                _ => browser(m, p),
            },
        );
        assert_eq!(r.unwrap_err().fail, Fail::Unavailable);
    }

    #[test]
    fn a_wide_page_is_pictured_at_a_scale_that_fits() {
        let mut rig = Rig::new();
        let (r, _) = rig.run(
            Op::Look { picture: true },
            &RefMap::default(),
            |m, p| match m {
                "Page.getLayoutMetrics" => Ok(metrics(3200.0, 1000.0)),
                _ => browser(m, p),
            },
        );
        let v = r.unwrap();
        // 3200 css px x 2.25 = 7200 device px, taken at 1600 / 7200 of the clip.
        let shot = rig
            .log
            .iter()
            .find(|(m, _)| m == "Page.captureScreenshot")
            .unwrap();
        assert!((shot.1["clip"]["scale"].as_f64().unwrap() - 1600.0 / 7200.0).abs() < 1e-9);
        assert_eq!(v["image"]["width"], json!(1600));
        assert_eq!(
            v["image"]["scale"],
            json!(0.5),
            "1600 picture px for 3200 css px"
        );
    }

    #[test]
    fn a_narrow_page_keeps_the_displays_full_resolution() {
        let mut rig = Rig::new();
        let (r, _) = rig.run(
            Op::Look { picture: true },
            &RefMap::default(),
            |m, p| match m {
                "Page.getLayoutMetrics" => Ok(metrics(400.0, 300.0)),
                _ => browser(m, p),
            },
        );
        let v = r.unwrap();
        assert_eq!(v["image"]["width"], json!(900));
        assert_eq!(v["image"]["scale"], json!(2.25));
        // And a browser that cannot say its ratio is taken as 1.
        let mut rig = Rig::new();
        let (r, _) = rig.run(
            Op::Look { picture: true },
            &RefMap::default(),
            |m, p| match m {
                "Runtime.evaluate" => Err("no".into()),
                "Page.captureScreenshot" => {
                    Ok(json!({"data": png_b64_small(800, 500)}).to_string())
                }
                _ => browser(m, p),
            },
        );
        let v = r.unwrap();
        assert_eq!(v["viewport"]["device_scale"], json!(1.0));
        assert_eq!(v["image"]["scale"], json!(1.0));
    }

    #[test]
    fn a_click_by_ref_scrolls_finds_the_middle_and_presses() {
        let mut rig = Rig::new();
        let (_, refs) = look(&mut rig);
        rig.log.clear();
        let (r, _) = rig.run(
            Op::Click {
                target: Target::Ref("e2".into()),
                button: Button::Left,
                double: false,
            },
            &refs,
            browser,
        );
        assert_eq!(r.unwrap()["clicked"], json!({"x": 140.0, "y": 212.0}));
        assert_eq!(
            rig.methods(),
            [
                "DOM.enable",
                "DOM.scrollIntoViewIfNeeded",
                "Page.getLayoutMetrics",
                "DOM.getContentQuads",
                "Input.dispatchMouseEvent",
                "Input.dispatchMouseEvent",
                "Input.dispatchMouseEvent",
            ]
        );
        let kinds: Vec<&str> = rig.log[4..]
            .iter()
            .map(|(_, p)| p["type"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["mouseMoved", "mousePressed", "mouseReleased"]);
        assert_eq!(rig.log[1].1["backendNodeId"], json!(11));
        assert_eq!(rig.log[5].1["x"], json!(140.0));
    }

    #[test]
    fn a_double_click_presses_twice() {
        let mut rig = Rig::new();
        let (_, refs) = look(&mut rig);
        rig.log.clear();
        let (r, _) = rig.run(
            Op::Click {
                target: Target::Ref("e2".into()),
                button: Button::Right,
                double: true,
            },
            &refs,
            browser,
        );
        assert!(r.is_ok());
        let presses = rig
            .log
            .iter()
            .filter(|(_, p)| p["type"] == json!("mousePressed"))
            .count();
        assert_eq!(presses, 2);
    }

    #[test]
    fn a_click_by_point_is_just_the_mouse_events() {
        let mut rig = Rig::new();
        let (r, _) = rig.run(
            Op::Click {
                target: Target::Point(33.0, 44.5),
                button: Button::Left,
                double: false,
            },
            &RefMap::default(),
            browser,
        );
        assert_eq!(r.unwrap()["clicked"], json!({"x": 33.0, "y": 44.5}));
        assert_eq!(rig.methods().len(), 3);
        for bad in [Target::Point(-1.0, 5.0), Target::Point(f64::NAN, 5.0)] {
            let mut rig = Rig::new();
            let (r, _) = rig.run(
                Op::Click {
                    target: bad,
                    button: Button::Left,
                    double: false,
                },
                &RefMap::default(),
                browser,
            );
            assert_eq!(r.unwrap_err().fail, Fail::BadParams);
            assert!(rig.log.is_empty());
        }
    }

    #[test]
    fn a_stale_ref_is_not_found_and_nothing_is_sent() {
        let mut rig = Rig::new();
        let (_, refs) = look(&mut rig);
        rig.log.clear();
        let (r, _) = rig.run(
            Op::Click {
                target: Target::Ref("e99".into()),
                button: Button::Left,
                double: false,
            },
            &refs,
            browser,
        );
        let e = r.unwrap_err();
        assert_eq!(e.fail, Fail::NotFound);
        assert!(e.message.contains("e99") && e.message.contains("look again"));
        assert!(rig.log.is_empty());
        // The same after the numbers were forgotten (a page load).
        let (r, _) = rig.run(
            Op::Click {
                target: Target::Ref("e2".into()),
                button: Button::Left,
                double: false,
            },
            &RefMap::default(),
            browser,
        );
        assert_eq!(r.unwrap_err().fail, Fail::NotFound);
    }

    #[test]
    fn an_element_that_vanished_is_not_found() {
        let mut rig = Rig::new();
        let (_, refs) = look(&mut rig);
        let (r, _) = rig.run(
            Op::Click {
                target: Target::Ref("e2".into()),
                button: Button::Left,
                double: false,
            },
            &refs,
            |m, p| match m {
                "DOM.getContentQuads" => Err("Could not compute content quads.".into()),
                "DOM.scrollIntoViewIfNeeded" => Err("Node is detached from document".into()),
                _ => browser(m, p),
            },
        );
        assert_eq!(r.unwrap_err().fail, Fail::NotFound);
    }

    #[test]
    fn typing_into_a_ref_focuses_it_then_inserts_the_text() {
        let mut rig = Rig::new();
        let (_, refs) = look(&mut rig);
        rig.log.clear();
        let (r, _) = rig.run(
            Op::Type {
                text: "héllo wörld".into(),
                target: Some("e3".into()),
            },
            &refs,
            browser,
        );
        assert_eq!(r.unwrap()["typed"], json!(11));
        assert_eq!(
            rig.methods(),
            ["DOM.enable", "DOM.focus", "Input.insertText"]
        );
        assert_eq!(rig.log[1].1["backendNodeId"], json!(12));
        assert_eq!(rig.log[2].1["text"], json!("héllo wörld"));
        // With no ref the text goes where the keyboard is.
        rig.log.clear();
        let (r, _) = rig.run(
            Op::Type {
                text: "x".into(),
                target: None,
            },
            &refs,
            browser,
        );
        assert!(r.is_ok());
        assert_eq!(rig.methods(), ["Input.insertText"]);
    }

    #[test]
    fn typing_into_something_that_cannot_take_the_keyboard_says_so() {
        let mut rig = Rig::new();
        let (_, refs) = look(&mut rig);
        let (r, _) = rig.run(
            Op::Type {
                text: "x".into(),
                target: Some("e1".into()),
            },
            &refs,
            |m, p| match m {
                "DOM.focus" => Err("Element is not focusable".into()),
                _ => browser(m, p),
            },
        );
        let e = r.unwrap_err();
        assert_eq!(e.fail, Fail::NotFound);
        assert!(e.message.contains("focus") || e.message.contains("keyboard"));
        assert!(!rig.methods().contains(&"Input.insertText"));
    }

    #[test]
    fn a_key_press_sends_down_then_up_in_order() {
        let mut rig = Rig::new();
        let key = web::parse_key("Enter").unwrap();
        let (r, _) = rig.run(Op::Press { key }, &RefMap::default(), browser);
        assert!(r.is_ok());
        let kinds: Vec<&str> = rig
            .log
            .iter()
            .map(|(_, p)| p["type"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["keyDown", "keyUp"]);
    }

    #[test]
    fn a_rejected_input_event_ends_the_call_with_the_reason() {
        let mut rig = Rig::new();
        let key = web::parse_key("a").unwrap();
        let (r, _) = rig.run(Op::Press { key }, &RefMap::default(), |_, _| {
            Err("page crashed".into())
        });
        let e = r.unwrap_err();
        assert_eq!(e.fail, Fail::Internal);
        assert!(e.message.contains("page crashed"));
        assert_eq!(rig.log.len(), 1, "nothing more is sent after a failure");
    }

    #[test]
    fn scrolling_without_a_ref_wheels_at_the_middle_of_the_page() {
        let mut rig = Rig::new();
        let (r, _) = rig.run(
            Op::Scroll {
                dx: 0.0,
                dy: 300.0,
                target: None,
            },
            &RefMap::default(),
            browser,
        );
        assert_eq!(r.unwrap()["scrolled"], json!({"x": 400.0, "y": 250.0}));
        let wheel = rig
            .log
            .iter()
            .find(|(_, p)| p["type"] == json!("mouseWheel"))
            .unwrap();
        assert_eq!(wheel.1["deltaY"], json!(300.0));
        assert_eq!(wheel.1["x"], json!(400.0));
    }

    #[test]
    fn scrolling_a_ref_wheels_over_it() {
        let mut rig = Rig::new();
        let (_, refs) = look(&mut rig);
        rig.log.clear();
        let (r, _) = rig.run(
            Op::Scroll {
                dx: 5.0,
                dy: -40.0,
                target: Some("e3".into()),
            },
            &refs,
            browser,
        );
        assert_eq!(r.unwrap()["scrolled"], json!({"x": 200.0, "y": 262.0}));
        let wheel = rig
            .log
            .iter()
            .find(|(_, p)| p["type"] == json!("mouseWheel"))
            .unwrap();
        assert_eq!(
            (wheel.1["deltaX"].clone(), wheel.1["deltaY"].clone()),
            (json!(5.0), json!(-40.0))
        );
        let mut rig = Rig::new();
        let (r, _) = rig.run(
            Op::Scroll {
                dx: f64::INFINITY,
                dy: 0.0,
                target: None,
            },
            &RefMap::default(),
            browser,
        );
        assert_eq!(r.unwrap_err().fail, Fail::BadParams);
    }

    #[test]
    fn a_wait_for_text_polls_until_it_is_there() {
        let mut rig = Rig::new();
        let mut checks = 0;
        let (r, _) = rig.run(
            Op::Wait {
                what: WaitFor::Text("Done".into()),
                timeout_ms: 10_000,
            },
            &RefMap::default(),
            |m, p| {
                assert_eq!(m, "Runtime.evaluate");
                let expr = p["expression"].as_str().unwrap();
                assert!(expr.contains("\"Done\""));
                assert_eq!(p["returnByValue"], json!(true));
                checks += 1;
                Ok(json!({"result": {"type": "boolean", "value": checks >= 4}}).to_string())
            },
        );
        assert_eq!(r.unwrap(), json!({"met": true}));
        assert_eq!(checks, 4);
    }

    #[test]
    fn a_wait_that_never_comes_true_ends_at_its_timeout_with_met_false() {
        let mut rig = Rig::new();
        let start = rig.now;
        let (r, _) = rig.run(
            Op::Wait {
                what: WaitFor::Selector("#nope".into()),
                timeout_ms: 2_000,
            },
            &RefMap::default(),
            |_, _| Ok(json!({"result": {"value": false}}).to_string()),
        );
        assert_eq!(r.unwrap(), json!({"met": false}));
        assert!(
            rig.now - start >= 2_000 && rig.now - start < 3_500,
            "{}",
            rig.now - start
        );
    }

    #[test]
    fn a_pause_waits_without_asking_the_page() {
        let mut rig = Rig::new();
        let (r, _) = rig.run(
            Op::Wait {
                what: WaitFor::Ms(700),
                timeout_ms: 700,
            },
            &RefMap::default(),
            |_, _| panic!("no call"),
        );
        assert_eq!(r.unwrap(), json!({"met": true}));
        assert!(rig.log.is_empty());
    }

    #[test]
    fn a_wait_with_a_hostile_text_sends_only_the_fixed_checker() {
        let mut rig = Rig::new();
        let hostile = "\");fetch('//evil')//";
        let _ = rig.run(
            Op::Wait {
                what: WaitFor::Text(hostile.into()),
                timeout_ms: 1,
            },
            &RefMap::default(),
            |_, _| Ok(json!({"result": {"value": true}}).to_string()),
        );
        let (_, p) = &rig.log[0];
        let expr = p["expression"].as_str().unwrap();
        let lit = serde_json::to_string(hostile).unwrap();
        assert_eq!(expr.replacen(&lit, "@", 1), web::WAIT_TEXT_TEMPLATE);
    }

    #[test]
    fn eval_returns_the_value_or_the_exception() {
        let mut rig = Rig::new();
        let (r, _) = rig.run(
            Op::Eval {
                expression: "1+1".into(),
            },
            &RefMap::default(),
            |m, p| {
                assert_eq!(m, "Runtime.evaluate");
                assert_eq!(p["expression"], json!("1+1"));
                assert_eq!(p["returnByValue"], json!(true));
                Ok(json!({"result": {"type": "number", "value": 2}}).to_string())
            },
        );
        assert_eq!(r.unwrap(), json!({"value": 2}));
        let mut rig = Rig::new();
        let (r, _) = rig.run(Op::Eval { expression: "boom()".into() }, &RefMap::default(), |_, _| {
            Ok(json!({"result": {"type": "object"}, "exceptionDetails": {"text": "Uncaught", "exception": {"description": "ReferenceError: boom is not defined\n    at <anonymous>:1:1"}}}).to_string())
        });
        let e = r.unwrap_err();
        assert_eq!(e.fail, Fail::BadParams);
        assert!(e.message.contains("ReferenceError: boom is not defined"));
        assert!(!e.message.contains('\n'));
    }

    #[test]
    fn a_huge_eval_value_is_cut() {
        let big = json!("x".repeat(EVAL_RESULT_MAX * 2));
        let a = eval_answer(big);
        assert_eq!(a["truncated"], json!(true));
        assert_eq!(
            a["value"].as_str().unwrap().chars().count(),
            EVAL_RESULT_MAX
        );
        assert_eq!(eval_answer(json!([1, 2])), json!({"value": [1, 2]}));
    }

    #[test]
    fn a_call_the_page_never_answers_times_out() {
        let mut ids = 0;
        let refs = RefMap::default();
        let (job, out) = Job::start(Op::Look { picture: true }, &refs, 0, &mut Ids(&mut ids));
        assert!(matches!(out, Out::Calls(_)));
        let mut job = job.unwrap();
        assert_eq!(
            job.on_tick(LOOK_BUDGET_MS - 1, &mut Ids(&mut ids)),
            Out::Idle
        );
        let out = job.on_tick(LOOK_BUDGET_MS, &mut Ids(&mut ids));
        let Out::Done(Err(e)) = out else {
            panic!("{out:?}")
        };
        assert_eq!(e.fail, Fail::Timeout);
        // A job that has finished ignores late answers.
        let late = job.on_reply(
            1,
            Ok("{}".into()),
            Page { url: "", title: "" },
            LOOK_BUDGET_MS + 1,
            &mut Ids(&mut ids),
        );
        assert_eq!(late, Out::Idle);
    }

    #[test]
    fn answers_for_other_calls_are_ignored() {
        let mut ids = 0;
        let (job, _) = Job::start(
            Op::Look { picture: true },
            &RefMap::default(),
            0,
            &mut Ids(&mut ids),
        );
        let mut job = job.unwrap();
        let out = job.on_reply(
            9_999,
            Ok("{}".into()),
            Page { url: "", title: "" },
            1,
            &mut Ids(&mut ids),
        );
        assert_eq!(out, Out::Idle);
    }

    #[test]
    fn the_interrupt_error_says_the_user_took_over() {
        let e = Job::interrupted();
        assert_eq!(e.fail, Fail::Interrupted);
        assert!(e.message.contains("took over"));
    }

    #[test]
    fn call_ids_never_repeat_across_jobs() {
        let mut rig = Rig::new();
        let _ = look(&mut rig);
        let after_first = rig.ids;
        let _ = look(&mut rig);
        assert!(rig.ids > after_first);
        let mut seen = std::collections::HashSet::new();
        let mut ids = 0u64;
        for _ in 0..3 {
            let (_, out) = Job::start(
                Op::Look { picture: true },
                &RefMap::default(),
                0,
                &mut Ids(&mut ids),
            );
            let Out::Calls(cs) = out else { panic!() };
            for c in cs {
                assert!(seen.insert(c.id));
            }
        }
    }

    fn click_job(ids: &mut u64) -> Job {
        let (job, out) = Job::start(
            Op::Click {
                target: Target::Point(10.0, 20.0),
                button: Button::Left,
                double: false,
            },
            &RefMap::default(),
            0,
            &mut Ids(ids),
        );
        assert!(matches!(out, Out::Calls(_)));
        job.unwrap()
    }

    #[test]
    fn a_click_cut_short_lets_go_of_the_button() {
        // The person takes over after the press went out: the release still queued is sent by the window.
        let mut ids = 0;
        let mut job = click_job(&mut ids);
        let cur = ids; // the first event (a move) is the call in flight
        let out = job.on_reply(
            cur,
            Ok("{}".into()),
            Page { url: "", title: "" },
            1,
            &mut Ids(&mut ids),
        );
        let Out::Calls(c) = out else {
            panic!("{out:?}")
        };
        assert_eq!(c[0].params["type"], "mousePressed");
        let rel = job.release_calls(&mut Ids(&mut ids));
        assert_eq!(rel.len(), 1);
        assert_eq!(rel[0].method, "Input.dispatchMouseEvent");
        assert_eq!(rel[0].params["type"], "mouseReleased");
        assert_eq!(rel[0].params["button"], "left");
        // A job that finished has nothing to let go of.
        let mut done = click_job(&mut ids);
        for _ in 0..4 {
            let cur = ids;
            let _ = done.on_reply(
                cur,
                Ok("{}".into()),
                Page { url: "", title: "" },
                1,
                &mut Ids(&mut ids),
            );
        }
        assert!(done.release_calls(&mut Ids(&mut ids)).is_empty());
    }

    #[test]
    fn a_click_whose_release_fails_still_lets_go_of_the_button() {
        // The press went out; the next event (the release) is refused: the renderer must still be told.
        let mut ids = 0;
        let mut job = click_job(&mut ids);
        let page = || Page { url: "", title: "" };
        // move ok, press ok, then the release is the call in flight.
        for _ in 0..2 {
            let cur = ids;
            let _ = job.on_reply(cur, Ok("{}".into()), page(), 1, &mut Ids(&mut ids));
        }
        let cur = ids;
        let out = job.on_reply(cur, Err("Target closed".into()), page(), 1, &mut Ids(&mut ids));
        assert!(matches!(out, Out::Done(Err(_))), "{out:?}");
        let rel = job.release_calls(&mut Ids(&mut ids));
        assert_eq!(rel.len(), 1, "{rel:?}");
        assert_eq!(rel[0].params["type"], "mouseReleased");
        assert!(job.release_calls(&mut Ids(&mut ids)).is_empty(), "once");
    }

    #[test]
    fn a_click_that_times_out_still_lets_go() {
        let mut ids = 0;
        let mut job = click_job(&mut ids);
        let out = job.on_tick(ACT_BUDGET_MS, &mut Ids(&mut ids));
        assert!(matches!(out, Out::Done(Err(_))), "{out:?}");
        let rel = job.release_calls(&mut Ids(&mut ids));
        assert_eq!(rel.len(), 1);
        assert_eq!(rel[0].params["type"], "mouseReleased");
        assert!(job.release_calls(&mut Ids(&mut ids)).is_empty(), "once");
    }

    #[test]
    fn the_pixel_ratio_question_has_a_time_limit_and_nothing_else_is_script() {
        let mut ids = 0;
        let (_, out) = Job::start(
            Op::Look { picture: true },
            &RefMap::default(),
            0,
            &mut Ids(&mut ids),
        );
        let Out::Calls(cs) = out else { panic!() };
        let evals: Vec<&Call> = cs
            .iter()
            .filter(|c| c.method == "Runtime.evaluate")
            .collect();
        assert_eq!(evals.len(), 1);
        assert_eq!(evals[0].params["expression"], "window.devicePixelRatio");
        assert!(evals[0].params["timeout"].as_u64().is_some());
    }
}
