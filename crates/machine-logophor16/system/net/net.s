; Package net's link to Foundry's network services (SPEC.md 11, TRAP #7-9).
; The TRAPs use R1-R6, R9 and R10: R5/R6 are kept for the caller, R10
; holds the return segment for RETF.

; net.trapOpen() int: rings up, transmit and receive on; NETSTAT.
net__trapOpen:
        push    r10
        push    r5
        push    r6
        trap    #7
        pop     r6
        pop     r5
        pop     r10
        retf

; net.trapSend(off uint16, n int) int: send the frame at S1:off; 0 or 1.
net__trapSend:
        push    r10
        push    r5
        push    r6
        trap    #8
        pop     r6
        pop     r5
        pop     r10
        retf

; net.trapRecv(off uint16) (int, int): a received frame to S1:off;
; its length (0: none) and descriptor flags.
net__trapRecv:
        push    r10
        push    r5
        push    r6
        trap    #9
        pop     r6
        pop     r5
        pop     r10
        retf
