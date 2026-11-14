//! The BASIC debugger (`basic_*` commands): listing, variables and stack
//! come from the machine's [`emu_core::basic::BasicDebug`]; breakpoints on
//! lines and the BASIC steps are decided here.
//!
//! The machine names the places where its interpreter begins a statement
//! or a line (stop points). While a BASIC breakpoint or step is active the
//! session keeps hidden breakpoints there; at each hit it asks the machine
//! where the program is and resumes silently unless a breakpoint or the
//! step is satisfied. With nothing active nothing is armed, so a machine
//! running BASIC pays nothing for the debugger (golden rule 5).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use emu_core::basic::{
    BasicDebug, BasicFrame, BasicGranularity, BasicMode, BasicPos, BasicState, BasicValue,
};
use emu_core::{BpId, BreakpointKind, StopReason};

use super::Session;
use crate::protocol::{base64_encode, MAX_BASIC_ARRAY};

#[derive(Clone, Copy, Debug)]
struct BasicBreak {
    line: u32,
    /// `None`: wherever the program enters the line.
    statement: Option<u16>,
    enabled: bool,
    hits: u32,
    /// `basic_run_to`: removed when the run ends.
    temporary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BasicStep {
    Statement,
    Line {
        from: Option<BasicPos>,
    },
    /// The next line with at most `depth` GOSUBs open.
    Over {
        from: Option<BasicPos>,
        depth: u32,
    },
    /// Until fewer than `depth` GOSUBs are open.
    Out {
        depth: u32,
    },
}

impl BasicStep {
    fn name(self) -> &'static str {
        match self {
            BasicStep::Statement => "statement",
            BasicStep::Line { .. } => "line",
            BasicStep::Over { .. } => "over",
            BasicStep::Out { .. } => "out",
        }
    }
}

/// The session's BASIC debugging state.
#[derive(Debug, Default)]
pub(super) struct BasicDbg {
    breaks: BTreeMap<u32, BasicBreak>,
    next: u32,
    step: Option<BasicStep>,
    /// The hidden breakpoints at the machine's stop points.
    armed: BTreeMap<BpId, BreakpointKind>,
    /// Where the program was at the last stop point seen.
    prev: Option<BasicPos>,
    /// The reason of the stop just decided, for the next stop state.
    reason: Option<Value>,
}

impl BasicDbg {
    /// The BASIC reason of the stop being reported, if BASIC caused it.
    pub(super) fn take_reason(&mut self) -> Option<Value> {
        self.reason.take()
    }
}

fn value_json(v: &BasicValue) -> Value {
    match v {
        BasicValue::Int(n) => json!(n),
        // Not finite: null.
        BasicValue::Float(f) => json!(f),
        BasicValue::Str(s) => json!(s),
    }
}

fn pos_json(v: &mut Value, pos: Option<BasicPos>) {
    v["line"] = json!(pos.map(|p| p.line));
    v["statement"] = json!(pos.map(|p| p.statement));
}

/// The program left the line of `from` or began a line anew.
fn line_changed(from: Option<BasicPos>, state: &BasicState) -> bool {
    state.pos.map(|p| p.line) != from.map(|p| p.line) || state.pos.is_some_and(|p| p.statement == 0)
}

impl Session {
    fn basic(&mut self) -> Result<&mut dyn BasicDebug, String> {
        let id = &self.machine_id;
        self.machine
            .basic()
            .ok_or_else(|| format!("{id} has no BASIC the debugger can follow"))
    }

    pub(super) fn has_basic(&mut self) -> bool {
        self.machine.basic().is_some()
    }

    /// Bring the hidden breakpoints in line with what is active, from the
    /// machine's current stop points.
    fn basic_rearm(&mut self) {
        let wanted = self.basic.step.is_some() || self.basic.breaks.values().any(|b| b.enabled);
        if !wanted && self.basic.armed.is_empty() {
            return;
        }
        let points = match self.machine.basic() {
            Some(b) if wanted => b.stop_points(),
            _ => Vec::new(),
        };
        if points.iter().map(|p| &p.kind).eq(self.basic.armed.values()) {
            return;
        }
        if self.basic.armed.is_empty() {
            self.basic.prev = None;
        }
        for (id, _) in std::mem::take(&mut self.basic.armed) {
            self.hooks.remove_breakpoint(id);
        }
        for p in points {
            let id = self.hooks.add_breakpoint(p.kind);
            self.basic.armed.insert(id, p.kind);
        }
    }

    /// End a BASIC step or `basic_run_to` (with every run that ends).
    pub(super) fn basic_cancel(&mut self) {
        self.basic.step = None;
        self.basic.breaks.retain(|_, b| !b.temporary);
        self.basic_rearm();
    }

    /// Before a run starts: the stop points may have moved.
    pub(super) fn basic_before_run(&mut self) {
        self.basic.reason = None;
        self.basic_rearm();
    }

    /// A breakpoint stopped the machine. `None`: not a stop point.
    /// `Some(None)`: a stop point nobody waits for, keep running.
    pub(super) fn basic_hit(&mut self, id: BpId) -> Option<Option<StopReason>> {
        let kind = *self.basic.armed.get(&id)?;
        let Some(state) = self.machine.basic().map(|b| b.state()) else {
            return Some(None);
        };
        let prev = std::mem::replace(&mut self.basic.prev, state.pos);
        let entered = match (state.pos, prev) {
            (Some(p), Some(q)) => p.statement == 0 || p.line != q.line,
            (Some(_), None) => true,
            (None, _) => false,
        };
        let mut reason = None;
        if let (BasicMode::Running, Some(pos)) = (state.mode, state.pos) {
            let hit = self.basic.breaks.iter_mut().find(|(_, b)| {
                b.enabled
                    && b.line == pos.line
                    && match b.statement {
                        Some(s) => s == pos.statement,
                        None => entered,
                    }
            });
            if let Some((&bp, b)) = hit {
                b.hits = b.hits.saturating_add(1);
                reason = Some(if b.temporary {
                    json!({"kind": "basic_step", "step": "run_to"})
                } else {
                    json!({"kind": "basic_break", "bp": bp, "hits": b.hits})
                });
            }
        }
        if reason.is_none() {
            let done = match self.basic.step {
                None => false,
                Some(BasicStep::Statement) => true,
                Some(BasicStep::Line { from }) => line_changed(from, &state),
                Some(BasicStep::Over { from, depth }) => {
                    state.depth <= depth && line_changed(from, &state)
                }
                Some(BasicStep::Out { depth }) => state.depth < depth,
            };
            if done {
                let step = self.basic.step.map_or("", BasicStep::name);
                reason = Some(json!({"kind": "basic_step", "step": step}));
            }
        }
        match reason {
            Some(mut r) => {
                r["mode"] = json!(state.mode.name());
                pos_json(&mut r, state.pos);
                self.basic.reason = Some(r);
                Some(Some(StopReason::StepDone))
            }
            None => {
                // A breakpoint of the user's at the same place still counts.
                let other = self
                    .hooks
                    .breakpoints()
                    .find(|(other, bp)| {
                        bp.kind == kind && bp.enabled && self.bp_meta.contains_key(other)
                    })
                    .map(|(other, _)| other);
                Some(match (other, kind) {
                    (Some(other), BreakpointKind::Exec { .. }) => {
                        self.counts(StopReason::Breakpoint(other))
                    }
                    _ => None,
                })
            }
        }
    }

    pub(super) fn basic_info(&mut self) -> Result<Value, String> {
        let info = self.basic()?.info();
        Ok(json!({
            "dialect": info.dialect,
            "granularity": info.granularity.name(),
            "space": info.space.0,
        }))
    }

    pub(super) fn basic_state(&mut self) -> Result<Value, String> {
        let s = self.basic()?.state();
        let mut v = json!({"mode": s.mode.name(), "addr": s.text_addr, "depth": s.depth});
        pos_json(&mut v, s.pos);
        Ok(v)
    }

    pub(super) fn basic_list(
        &mut self,
        from: Option<u32>,
        to: Option<u32>,
    ) -> Result<Value, String> {
        let basic = self.basic()?;
        let (from, to) = (from.unwrap_or(0), to.unwrap_or(u32::MAX));
        let lines: Vec<Value> = basic
            .program()
            .iter()
            .filter(|l| (from..=to).contains(&l.number))
            .map(|l| {
                json!({
                    "line": l.number, "addr": l.addr, "text": l.text,
                    "statements": l.statements, "raw": base64_encode(&l.raw),
                })
            })
            .collect();
        let state = basic.state();
        let mut current = json!({"mode": state.mode.name()});
        pos_json(&mut current, state.pos);
        Ok(json!({"lines": lines, "current": current}))
    }

    pub(super) fn basic_vars(&mut self) -> Result<Value, String> {
        let vars: Vec<Value> = self
            .basic()?
            .variables()
            .iter()
            .map(|var| {
                let mut v = json!({"name": var.name, "type": var.ty.name(), "addr": var.addr});
                match &var.value {
                    Some(value) => v["value"] = value_json(value),
                    None => v["dims"] = json!(var.dims),
                }
                v
            })
            .collect();
        Ok(json!({"vars": vars}))
    }

    pub(super) fn basic_array(
        &mut self,
        name: &str,
        first: Option<u32>,
        count: Option<u32>,
    ) -> Result<Value, String> {
        let first = first.unwrap_or(0);
        let count = count.unwrap_or(256);
        if count > MAX_BASIC_ARRAY {
            return Err(format!("count exceeds {MAX_BASIC_ARRAY}"));
        }
        let values = self
            .basic()?
            .array(name, first, count)
            .map_err(|e| format!("{name}: {e}"))?;
        let values: Vec<Value> = values.iter().map(value_json).collect();
        Ok(json!({"name": name, "first": first, "values": values}))
    }

    pub(super) fn basic_set_var(
        &mut self,
        name: &str,
        index: Option<u32>,
        value: &Value,
    ) -> Result<Value, String> {
        let value = match value {
            Value::String(s) => BasicValue::Str(s.clone()),
            Value::Number(n) => match (n.as_i64(), n.as_f64()) {
                (Some(i), _) => BasicValue::Int(i),
                (None, Some(f)) => BasicValue::Float(f),
                (None, None) => return Err("value out of range".into()),
            },
            _ => return Err("value must be a number or a string".into()),
        };
        self.basic()?
            .set_variable(name, index, &value)
            .map_err(|e| format!("{name}: {e}"))?;
        Ok(json!({}))
    }

    pub(super) fn basic_stack(&mut self) -> Result<Value, String> {
        let basic = self.basic()?;
        let frames: Vec<Value> = basic
            .stack()
            .iter()
            .map(|f| match f {
                BasicFrame::Gosub { from } => {
                    json!({"kind": "gosub", "line": from.line, "statement": from.statement})
                }
                BasicFrame::For {
                    var,
                    limit,
                    step,
                    from,
                } => json!({
                    "kind": "for", "var": var, "limit": value_json(limit),
                    "step": value_json(step), "line": from.line, "statement": from.statement,
                }),
            })
            .collect();
        Ok(json!({"frames": frames, "depth": basic.state().depth}))
    }

    pub(super) fn basic_break_add(
        &mut self,
        line: u32,
        statement: Option<u16>,
        enabled: Option<bool>,
    ) -> Result<Value, String> {
        let info = self.basic()?.info();
        if info.granularity == BasicGranularity::Line && statement.is_some_and(|s| s != 0) {
            return Err(format!("{} only stops at lines", info.dialect));
        }
        let bp = self.basic_add(line, statement, enabled != Some(false), false);
        self.basic_rearm();
        Ok(json!({"bp": bp}))
    }

    fn basic_add(
        &mut self,
        line: u32,
        statement: Option<u16>,
        enabled: bool,
        temporary: bool,
    ) -> u32 {
        self.basic.next += 1;
        self.basic.breaks.insert(
            self.basic.next,
            BasicBreak {
                line,
                statement,
                enabled,
                hits: 0,
                temporary,
            },
        );
        self.basic.next
    }

    pub(super) fn basic_break_remove(&mut self, bp: u32) -> Result<Value, String> {
        self.basic
            .breaks
            .remove(&bp)
            .ok_or_else(|| format!("no BASIC breakpoint {bp}"))?;
        self.basic_rearm();
        Ok(json!({}))
    }

    pub(super) fn basic_break_enable(&mut self, bp: u32, enabled: bool) -> Result<Value, String> {
        self.basic
            .breaks
            .get_mut(&bp)
            .ok_or_else(|| format!("no BASIC breakpoint {bp}"))?
            .enabled = enabled;
        self.basic_rearm();
        Ok(json!({}))
    }

    pub(super) fn basic_break_list(&self) -> Value {
        let list: Vec<Value> = self
            .basic
            .breaks
            .iter()
            .filter(|(_, b)| !b.temporary)
            .map(|(bp, b)| {
                json!({
                    "bp": bp, "line": b.line, "statement": b.statement,
                    "enabled": b.enabled, "hits": b.hits,
                })
            })
            .collect();
        json!(list)
    }

    /// `basic_step`: the step to run; the caller starts the run.
    pub(super) fn basic_step(&mut self, kind: Option<&str>) -> Result<(), String> {
        let state = self.basic()?.state();
        if state.mode == BasicMode::Inactive {
            return Err("BASIC is not active".into());
        }
        let step = match kind.unwrap_or("line") {
            "statement" => BasicStep::Statement,
            "line" => BasicStep::Line { from: state.pos },
            "over" => BasicStep::Over {
                from: state.pos,
                depth: state.depth,
            },
            "out" if state.depth == 0 => return Err("not inside a GOSUB".into()),
            "out" => BasicStep::Out { depth: state.depth },
            other => {
                return Err(format!(
                    "unknown step '{other}' (statement, line, over, out)"
                ))
            }
        };
        self.basic.step = Some(step);
        Ok(())
    }

    /// `basic_run_to`: a breakpoint for one run; the caller starts it.
    pub(super) fn basic_run_to(&mut self, line: u32, statement: Option<u16>) -> Result<(), String> {
        self.basic()?;
        self.basic_add(line, statement, true, true);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use emu_core::testing::basic::{ToyBasic, ToyStmt, ToyStmt::*, DISPATCH};

    use super::*;
    use crate::protocol::{BreakSpec, Command, Request};

    /// ```text
    /// 10 A=5:GOSUB 100:A=A-2
    /// 20 FOR I=1 TO 3:B=B+10:NEXT
    /// 30 C=C+1
    /// 40 GOTO 10
    /// 100 A=A+1:GOSUB 200
    /// 110 RETURN
    /// 200 D=D+1:RETURN
    /// ```
    const PROGRAM: &[(u16, &[ToyStmt])] = &[
        (10, &[Let('A', 5), Gosub(100), Add('A', -2)]),
        (20, &[For('I', 1, 3), Add('B', 10), Next]),
        (30, &[Add('C', 1)]),
        (40, &[Goto(10)]),
        (100, &[Add('A', 1), Gosub(200)]),
        (110, &[Return]),
        (200, &[Add('D', 1), Return]),
    ];

    fn session() -> Session {
        Session::new(Box::new(ToyBasic::new(PROGRAM)), "toy")
    }

    fn call(s: &mut Session, cmd: Command) -> Value {
        s.handle(Request { id: 1, cmd })
    }

    fn ok(s: &mut Session, cmd: Command) -> Value {
        let v = call(s, cmd);
        assert_eq!(v["ok"], true, "{v}");
        v["result"].clone()
    }

    fn stopped(s: &mut Session) -> Value {
        for _ in 0..50 {
            s.run_slice();
            if let Some(ev) = s
                .take_events()
                .into_iter()
                .find(|e| e["event"] == "stopped")
            {
                return ev;
            }
        }
        panic!("no stop");
    }

    /// Run `cmd` (asynchronous) to its stop: (kind, line, statement).
    fn run(s: &mut Session, cmd: Command) -> (String, u64, u64) {
        assert_eq!(ok(s, cmd)["running"], true);
        let ev = stopped(s);
        let r = &ev["reason"];
        (
            r["kind"].as_str().unwrap().to_string(),
            r["line"].as_u64().unwrap(),
            r["statement"].as_u64().unwrap(),
        )
    }

    fn step(s: &mut Session, kind: &str) -> (u64, u64) {
        let (k, line, statement) = run(
            s,
            Command::BasicStep {
                kind: Some(kind.into()),
            },
        );
        assert_eq!(k, "basic_step");
        (line, statement)
    }

    fn break_at(line: u32, statement: Option<u16>) -> Command {
        Command::BasicBreakAdd {
            line,
            statement,
            enabled: None,
        }
    }

    fn var(s: &mut Session, name: &str) -> Value {
        let vars = ok(s, Command::BasicVars);
        vars["vars"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == name)
            .map_or(Value::Null, |v| v["value"].clone())
    }

    fn armed(s: &Session) -> usize {
        s.hooks.breakpoints().count()
    }

    #[test]
    fn a_line_breakpoint_stops_once_per_entry() {
        let mut s = session();
        assert_eq!(armed(&s), 0);
        let bp = ok(&mut s, break_at(20, None))["bp"].clone();
        assert_eq!(armed(&s), 1);
        assert_eq!(run(&mut s, Command::Resume), ("basic_break".into(), 20, 0));
        assert_eq!(var(&mut s, "A"), json!(4));
        assert_eq!(var(&mut s, "C"), Value::Null);
        // The loop returns into line 20 three times without stopping.
        assert_eq!(run(&mut s, Command::Resume), ("basic_break".into(), 20, 0));
        assert_eq!(var(&mut s, "B"), json!(30));
        assert_eq!(var(&mut s, "C"), json!(1));
        let list = ok(&mut s, Command::BasicBreakList);
        assert_eq!(
            list,
            json!([{"bp": bp, "line": 20, "statement": null, "enabled": true, "hits": 2}])
        );
        // Disabled or removed: nothing armed, the program runs on.
        ok(
            &mut s,
            Command::BasicBreakEnable {
                bp: 1,
                enabled: false,
            },
        );
        assert_eq!(armed(&s), 0);
        ok(&mut s, Command::BasicBreakRemove { bp: 1 });
        assert_eq!(
            call(&mut s, Command::BasicBreakRemove { bp: 1 })["ok"],
            false
        );
        assert_eq!(ok(&mut s, Command::BasicBreakList), json!([]));
        ok(&mut s, Command::Resume);
        s.run_slice();
        assert!(s.is_running());
    }

    #[test]
    fn a_statement_breakpoint_stops_at_every_pass() {
        let mut s = session();
        ok(&mut s, break_at(20, Some(1)));
        for b in [Value::Null, json!(10), json!(20)] {
            assert_eq!(run(&mut s, Command::Resume), ("basic_break".into(), 20, 1));
            assert_eq!(var(&mut s, "B"), b);
        }
        assert_eq!(ok(&mut s, Command::BasicBreakList)[0]["hits"], 3);
    }

    #[test]
    fn steps_by_statement_line_over_and_out() {
        let mut s = session();
        // The machine sits before the first statement's dispatch.
        assert_eq!(step(&mut s, "statement"), (10, 0));
        assert_eq!(step(&mut s, "statement"), (10, 1));
        // Over the GOSUB (two levels deep) to the next line.
        assert_eq!(step(&mut s, "over"), (20, 0));
        assert_eq!(var(&mut s, "A"), json!(4));
        assert_eq!(var(&mut s, "D"), json!(1));
        // By line: the loop stays in line 20, so the next line is 30.
        assert_eq!(step(&mut s, "line"), (30, 0));
        assert_eq!(step(&mut s, "line"), (40, 0));
        assert_eq!(step(&mut s, "line"), (10, 0));
        // Into the subroutines by line, out of them one by one.
        assert_eq!(step(&mut s, "line"), (100, 0));
        assert_eq!(step(&mut s, "line"), (200, 0));
        let stack = ok(&mut s, Command::BasicStack);
        assert_eq!(stack["depth"], 2);
        assert_eq!(
            stack["frames"],
            json!([
                {"kind": "gosub", "line": 100, "statement": 1},
                {"kind": "gosub", "line": 10, "statement": 1},
            ])
        );
        assert_eq!(step(&mut s, "out"), (110, 0));
        assert_eq!(step(&mut s, "out"), (10, 2));
        let out = Command::BasicStep {
            kind: Some("out".into()),
        };
        assert_eq!(call(&mut s, out)["error"], "not inside a GOSUB");
        let up = Command::BasicStep {
            kind: Some("up".into()),
        };
        assert_eq!(call(&mut s, up)["ok"], false);
        // A finished step leaves nothing armed.
        assert_eq!(armed(&s), 0);
        assert!(!s.is_running());
    }

    #[test]
    fn run_to_is_a_breakpoint_for_one_run() {
        let mut s = session();
        let cmd = Command::BasicRunTo {
            line: 20,
            statement: Some(2),
        };
        assert_eq!(run(&mut s, cmd), ("basic_step".into(), 20, 2));
        assert_eq!(ok(&mut s, Command::BasicBreakList), json!([]));
        assert_eq!(armed(&s), 0);
        // Paused before it is reached: gone as well.
        ok(
            &mut s,
            Command::BasicRunTo {
                line: 999,
                statement: None,
            },
        );
        s.run_slice();
        ok(&mut s, Command::Pause);
        assert_eq!(armed(&s), 0);
        // The stop state of a pause is a plain one.
        let state = ok(&mut s, Command::Pause);
        assert_eq!(state["reason"]["kind"], "step_done");
    }

    #[test]
    fn listing_state_variables_and_arrays() {
        let mut s = session();
        assert_eq!(
            ok(&mut s, Command::BasicInfo),
            json!({"dialect": "Toy BASIC", "granularity": "statement", "space": 0})
        );
        let state = ok(&mut s, Command::BasicState);
        assert_eq!(state["mode"], "running");
        assert_eq!(state["line"], 10);
        assert_eq!(state["depth"], 0);
        let all = Command::BasicList {
            from: None,
            to: None,
        };
        let list = ok(&mut s, all);
        let lines = list["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[1]["line"], 20);
        assert_eq!(lines[1]["text"], "FOR I=1 TO 3:B=B+10:NEXT");
        assert_eq!(lines[1]["statements"], json!([0, 13, 20]));
        assert_eq!(list["current"]["line"], 10);
        let raw = crate::protocol::base64_decode(lines[6]["raw"].as_str().unwrap()).unwrap();
        assert_eq!(raw, [8, 200, 0, 2, 3, 1, 0, 5]);
        let part = Command::BasicList {
            from: Some(30),
            to: Some(100),
        };
        assert_eq!(ok(&mut s, part)["lines"].as_array().unwrap().len(), 3);

        let set = |name: &str, index, value| Command::BasicSetVar {
            name: name.into(),
            index,
            value,
        };
        ok(&mut s, set("Q", None, json!(-7)));
        ok(&mut s, set("S$", None, json!("HI")));
        ok(&mut s, set("X!", None, json!(2.5)));
        ok(&mut s, set("@", Some(3), json!(12)));
        assert_eq!(var(&mut s, "Q"), json!(-7));
        assert_eq!(var(&mut s, "S$"), json!("HI"));
        assert_eq!(var(&mut s, "X!"), json!(2.5));
        let vars = ok(&mut s, Command::BasicVars);
        let array = vars["vars"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == "@")
            .unwrap();
        assert_eq!(array["dims"], json!([8]));
        assert_eq!(array["type"], "int");
        let a = ok(
            &mut s,
            Command::BasicArray {
                name: "@".into(),
                first: Some(2),
                count: Some(3),
            },
        );
        assert_eq!(a, json!({"name": "@", "first": 2, "values": [0, 12, 0]}));
        for bad in [
            set("Q", None, json!(70000)),
            set("Q", None, json!("x")),
            set("Q", None, json!(null)),
            set("NOPE", None, json!(1)),
            Command::BasicArray {
                name: "Q".into(),
                first: None,
                count: None,
            },
            Command::BasicArray {
                name: "@".into(),
                first: None,
                count: Some(MAX_BASIC_ARRAY + 1),
            },
        ] {
            assert_eq!(call(&mut s, bad)["ok"], false);
        }
    }

    #[test]
    fn a_dialect_with_line_stops_steps_by_lines() {
        let machine = ToyBasic::new(PROGRAM).with_line_stops();
        let mut s = Session::new(Box::new(machine), "toy");
        assert_eq!(ok(&mut s, Command::BasicInfo)["granularity"], "line");
        assert_eq!(call(&mut s, break_at(20, Some(1)))["ok"], false);
        assert_eq!(step(&mut s, "statement"), (10, 0));
        assert_eq!(step(&mut s, "statement"), (100, 0));
        // Over the GOSUB 200 inside the subroutine, then back out of it.
        assert_eq!(step(&mut s, "over"), (110, 0));
        assert_eq!(step(&mut s, "over"), (20, 0));
        ok(&mut s, break_at(30, Some(0)));
        assert_eq!(run(&mut s, Command::Resume), ("basic_break".into(), 30, 0));
    }

    #[test]
    fn a_breakpoint_of_the_user_at_the_stop_point_still_stops() {
        let mut s = session();
        ok(&mut s, break_at(30, None));
        let spec = BreakSpec {
            addr: Some(DISPATCH),
            ..BreakSpec::default()
        };
        let bp = ok(&mut s, Command::BreakAdd(spec))["bp"].clone();
        ok(&mut s, Command::Resume);
        let ev = stopped(&mut s);
        assert_eq!(
            ev["reason"],
            json!({"kind": "breakpoint", "bp": bp, "hits": 1})
        );
        // The session's own breakpoints are not in the user's list.
        assert_eq!(ok(&mut s, Command::BreakList).as_array().unwrap().len(), 1);
        let bp = bp.as_u64().unwrap() as u32;
        ok(&mut s, Command::BreakRemove { bp });
        assert_eq!(run(&mut s, Command::Resume), ("basic_break".into(), 30, 0));
    }

    #[test]
    fn capabilities_announce_basic() {
        let mut s = session();
        let caps = ok(&mut s, Command::Capabilities);
        assert_eq!(caps["basic"], true);
        assert!(caps["commands"]
            .as_array()
            .unwrap()
            .contains(&json!("basic_step")));
    }
}
