//! The scenario context. Task 1 holds the pass and fail counters, `ok`, `not_ported` and the row
//! type of the scenario table; Task 13 adds the launch, the one wait primitive and the shots.

/// What a scenario gets: its name and its counters.
#[derive(Debug)]
pub struct Ctx {
    pub name: &'static str,
    pub checks: u32,
    pub fails: Vec<String>,
}

impl Ctx {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            checks: 0,
            fails: Vec::new(),
        }
    }

    /// Prints a pass or a FAIL line and counts it.
    pub fn ok(&mut self, cond: bool, msg: impl AsRef<str>) {
        self.checks += 1;
        let msg = msg.as_ref();
        if cond {
            println!("  pass  {msg}");
        } else {
            println!("  FAIL  {msg}");
            self.fails.push(msg.to_string());
        }
    }

    /// Records one failed check saying the scenario is not ported yet, and returns it as the error.
    pub fn not_ported(&mut self, task: u32) -> Result<(), String> {
        let msg = format!("not ported yet (Task {task})");
        self.ok(false, &msg);
        Err(msg)
    }
}

/// One row of the scenario table.
pub struct Scenario {
    pub name: &'static str,
    pub group: &'static str,
    pub run: fn(&mut Ctx) -> Result<(), String>,
    pub limit_secs: u64,
}
