//! Concurrency (BRASS.md 9): channels, `go`, `select`. The runtime turns
//! each operation into a call of Boiler's implementation through its
//! vectors (rt.s); without Boiler they panic.
//!
//! **[B]** The calls (the registers of Brass's convention, so Boiler
//! implements them as ordinary functions):
//! `chanMake(elemSize, cap int) *Chan`; `chanSend(c *Chan, value uintptr)`
//! (the address of the value); `chanRecv(c *Chan, into uintptr) bool`
//! (false: closed and empty); `chanClose(c *Chan)`; `spawn(fn uintptr,
//! args uintptr)` (args: the number of words, then up to four words, which
//! the new task gets in R1–R4); `selectCases(table uintptr, n int,
//! hasDefault bool) (int, bool)` (the chosen case, or n for the default;
//! for a receive, ok). A select table entry is 10 bytes: kind (0 receive,
//! 1 send), the channel, the address of the value.

use super::expr::CallTarget;
use super::*;

impl Compiler {
    /// `<-c`: the value in R1.. (a struct: its address); with `with_ok`
    /// also ok in the next register.
    pub(super) fn recv(&mut self, c: &Expr, with_ok: bool) -> CResult<Ty> {
        let ct = self.expr(c, None)?;
        let Ty::Chan(el) = self.types.under(&ct).clone() else {
            return err(
                c.pos,
                format!("cannot receive from {}", self.types.name(&ct)),
            );
        };
        let size = self.types.size(&el).max(1);
        let buf = self.slot(size.next_multiple_of(2));
        self.frame_addr_to(buf, 3);
        self.rt("rt_chan_recv");
        let ok = self.slot(2);
        self.st(1, ok);
        let words = self.types.words(&el);
        if words == 0 {
            if with_ok {
                return err(c.pos, "v, ok := <-c with a struct: receive a pointer");
            }
            self.frame_addr(buf);
        } else {
            if with_ok && words == 4 {
                return err(
                    c.pos,
                    "v, ok := <-c with a slice or interface: receive a pointer",
                );
            }
            self.x_frame(buf);
            self.load_x(&el);
            if with_ok {
                self.ld(words as u8 + 1, ok);
            }
        }
        Ok(if with_ok {
            Ty::Tuple(vec![*el, Ty::Bool])
        } else {
            *el
        })
    }

    pub(super) fn frame_addr_pub(&mut self, off: u32, r: u8) {
        self.frame_addr_to(off, r)
    }

    /// R`r`:R`r+1` = the address of frame offset `off`.
    fn frame_addr_to(&mut self, off: u32, r: u8) {
        self.emit(format!("mfs     r{r}, s2"));
        self.li(r + 1, i64::from(off));
        self.emit(format!("add     r{}, r12", r + 1));
    }

    /// `c <- v`.
    pub(super) fn send(&mut self, c: &Expr, v: &Expr, pos: Pos) -> CResult<()> {
        let ct = self.type_peek(c)?;
        let Ty::Chan(el) = self.types.under(&ct).clone() else {
            return err(pos, format!("cannot send to {}", self.types.name(&ct)));
        };
        let m = self.mark();
        self.expr(c, None)?;
        let ch = self.spill_addr();
        let size = self.types.size(&el).max(1);
        let buf = self.slot(size.next_multiple_of(2));
        let vt = match &v.kind {
            ExprKind::Composite(None, elems) => self.composite(v.pos, &el, elems)?,
            _ => self.expr(v, Some(&el))?,
        };
        self.coerce(&vt, &el, v.pos)?;
        if self.types.words(&el) == 0 {
            self.emit("mov     r3, r1");
            self.emit("mov     r4, r2");
            self.frame_addr(buf);
            self.memcpy(size);
        } else {
            self.x_frame(buf);
            self.store_x(&el);
        }
        self.ld(1, ch);
        self.ld(2, ch + 2);
        self.frame_addr_to(buf, 3);
        self.rt("rt_chan_send");
        self.release(m);
        Ok(())
    }

    /// `make(chan T, n)`.
    pub(super) fn make_chan(&mut self, t: &Ty, el: &Ty, cap: Option<&Expr>) -> CResult<Ty> {
        match cap {
            Some(n) => {
                let nt = self.expr(n, Some(&Ty::I16))?;
                if !self.types.is_int(&nt) {
                    return err(n.pos, "a channel's capacity is an integer");
                }
                self.emit("mov     r2, r1");
            }
            None => self.li(2, 0),
        }
        self.li(1, i64::from(self.types.size(el).max(1)));
        self.rt("rt_chan_make");
        Ok(t.clone())
    }

    /// `go f(args)`: the arguments (at most four words) into a block, then
    /// the new task.
    pub(super) fn go(&mut self, call: &Expr, pos: Pos) -> CResult<()> {
        let ExprKind::Call(f, args, spread) = &call.kind else {
            return err(pos, "go needs a function call");
        };
        let m = self.mark();
        let words = self.call_args(call.pos, f, args, *spread)?;
        let (target, words) = words;
        if words.len() > 4 {
            return err(pos, "go passes at most four argument words");
        }
        // block: count, words
        let block = self.slot(10);
        self.li(10, words.len() as i64);
        self.st(10, block);
        for (k, w) in words.iter().enumerate() {
            self.ld(10, *w);
            self.st(10, block + 2 + 2 * k as u32);
        }
        match target {
            CallTarget::Static(sym) => {
                self.emit(format!("li      r1, #seg({sym})"));
                self.emit(format!("li      r2, #off({sym})"));
            }
            CallTarget::Value(fv) => {
                self.ld(1, fv);
                self.ld(2, fv + 2);
            }
        }
        self.frame_addr_to(block, 3);
        self.rt("rt_go");
        self.release(m);
        Ok(())
    }

    pub(super) fn select(
        &mut self,
        cases: &[SelectCase],
        label: &Option<String>,
        pos: Pos,
    ) -> CResult<()> {
        let n = cases
            .iter()
            .filter(|c| !matches!(c.comm, Comm::Default))
            .count();
        if cases
            .iter()
            .filter(|c| matches!(c.comm, Comm::Default))
            .count()
            > 1
        {
            return err(pos, "select with two defaults");
        }
        self.fnc().scopes.push(HashMap::new());
        let table = self.slot((10 * n as u32).max(2));
        // Each case: its channel, a buffer, the value to send.
        let mut bufs = Vec::new();
        let mut k = 0u32;
        for c in cases {
            let (kind, chan) = match &c.comm {
                Comm::Default => continue,
                Comm::Recv { chan, .. } => (0, chan),
                Comm::Send(chan, _) => (1, chan),
            };
            let ct = self.expr(chan, None)?;
            let Ty::Chan(el) = self.types.under(&ct).clone() else {
                return err(
                    chan.pos,
                    format!("{} is not a channel", self.types.name(&ct)),
                );
            };
            let e = table + 10 * k;
            self.li(10, kind);
            self.st(10, e);
            self.st(2, e + 2); // channel: offset, segment (memory order)
            self.st(1, e + 4);
            let size = self.types.size(&el).max(1);
            let buf = self.slot(size.next_multiple_of(2));
            if let Comm::Send(_, v) = &c.comm {
                let vt = self.expr(v, Some(&el))?;
                self.coerce(&vt, &el, v.pos)?;
                if self.types.words(&el) == 0 {
                    self.emit("mov     r3, r1");
                    self.emit("mov     r4, r2");
                    self.frame_addr(buf);
                    self.memcpy(size);
                } else {
                    self.x_frame(buf);
                    self.store_x(&el);
                }
            }
            self.frame_addr(buf);
            self.st(2, e + 6);
            self.st(1, e + 8);
            bufs.push((buf, *el));
            k += 1;
        }
        self.frame_addr(table);
        self.li(3, n as i64);
        self.li(
            4,
            i64::from(cases.iter().any(|c| matches!(c.comm, Comm::Default))),
        );
        self.rt("rt_select");
        let chosen = self.slot(2);
        self.st(1, chosen);
        let ok = self.slot(2);
        self.st(2, ok);
        let brk = self.new_label();
        self.fnc().loops.push(LoopLabels {
            name: label.clone(),
            brk: brk.clone(),
            cont: None,
        });
        let mut k = 0usize;
        for c in cases {
            let next = self.new_label();
            let index = if matches!(c.comm, Comm::Default) {
                n
            } else {
                k
            };
            self.ld(1, chosen);
            self.li(3, index as i64);
            self.emit("cmp     r1, r3");
            let go = self.new_label();
            self.emit(format!("beq     {go}"));
            self.jump(&next);
            self.label(&go);
            self.fnc().scopes.push(HashMap::new());
            if let Comm::Recv { lhs, define, .. } = &c.comm {
                let (buf, el) = bufs[k].clone();
                let targets: Vec<(&Expr, Ty, u32)> = lhs
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        if i == 0 {
                            (e, el.clone(), buf)
                        } else {
                            (e, Ty::Bool, ok)
                        }
                    })
                    .collect();
                for (e, t, off) in targets {
                    // the value: in its buffer; spill it as assign_from expects
                    let s = if self.types.words(&t) == 0 {
                        self.frame_addr(off);
                        self.spill_addr()
                    } else {
                        if off == ok {
                            self.ld(1, ok);
                        } else {
                            // the buffer holds the value in memory order
                            self.x_frame(off);
                            self.load_x(&t);
                        }
                        self.spill(&t)
                    };
                    if *define {
                        if let ExprKind::Ident(name) = &e.kind {
                            if name != "_" {
                                let size = self.types.size(&t).max(1);
                                let slot = self.slot(size.next_multiple_of(2));
                                if self.types.words(&t) == 0 {
                                    self.ld(3, s);
                                    self.ld(4, s + 2);
                                    self.frame_addr(slot);
                                    self.memcpy(size);
                                } else {
                                    self.unspill(&t, s, 1);
                                    self.store_slot_pub(slot, &t);
                                }
                                self.declare_local(
                                    name,
                                    Local::Var {
                                        off: slot,
                                        ty: t.clone(),
                                        by_ref: false,
                                    },
                                );
                            }
                        } else {
                            return err(e.pos, "a name on the left of :=");
                        }
                    } else {
                        self.assign_from_pub(e, &t, s)?;
                    }
                }
            }
            for s in &c.body {
                self.stmt(s)?;
            }
            self.fnc().scopes.pop();
            self.jump(&brk);
            self.label(&next);
            if !matches!(c.comm, Comm::Default) {
                k += 1;
            }
        }
        self.fnc().loops.pop();
        self.label(&brk);
        self.fnc().scopes.pop();
        Ok(())
    }
}
