//! Gate Array (40007/40008/40010): video, palette, ROM/RAM paging,
//! interrupts — and the arbitration of the bus between the Z80 and video.
//!
//! **CPU wait states.** Video and the Z80 share the RAM; the Gate Array
//! holds the Z80's /WAIT low on three of every four 4 MHz T-states, so
//! every bus access is stretched until the T-state the Z80 samples /WAIT
//! on (T2 of opcode fetches and memory cycles, the automatic TW of I/O
//! cycles) is the free one. Internal T-states are not stretched. With the
//! Z80's M-cycle placement this gives the well-known instruction lengths in
//! whole microseconds ("NOPs"), e.g. `OUT (C),r` 4 µs, `EX (SP),HL` 6 µs
//! (cpcwiki, "Z80" — CPC instruction timings; checked by
//! `machine-cpc/tests/z80_timing.rs`).

/// Wait states for a Z80 access sampling /WAIT on master T-state `t`
/// (4 MHz T-states since power-on): /WAIT is released on T-states with
/// `t % 4 == 0`.
pub fn cpu_wait_states(t: u64) -> u32 {
    ((4 - t % 4) % 4) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_until_the_free_t_state() {
        assert_eq!(cpu_wait_states(8), 0);
        assert_eq!(cpu_wait_states(9), 3);
        assert_eq!(cpu_wait_states(10), 2);
        assert_eq!(cpu_wait_states(11), 1);
    }
}
