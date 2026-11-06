; Boiler's kernel in assembly: the vector table in RAM, the task switch on
; the timer interrupt, interrupts on and off, the new task's start.
;
; A task's registers live at the start of its Task (kernel.brass):
; R1-R12 (0-22), S0 (24), S1 (26), S2 (28), S3 (30), PC (32), SR (34).
; The interrupt saves them there, calls boiler.schedule (which may choose
; another task as `current`), and loads `current`'s.

BOILER_VEC_SEG = $003c          ; the vector table at 003C:0040 (VBR)
BOILER_ROM_VEC = $0040          ; the ROM's table at FFFB:0040
BOILER_TIMER_VECTOR = 13        ; level 5: Telegraph's timers

; installVectors(): the ROM's table copied to RAM, the timer's vector to
; the task switch, VBR set; the runtime's concurrency vectors to Boiler.
boiler__installVectors:
        li      r1, #$fffb
        li      r2, #BOILER_VEC_SEG
        li      r3, #BOILER_ROM_VEC
        movi    r4, #64         ; 32 vectors of two words
.copy:
        mts     s3, r1
        lwx     r9, (r3)
        mts     s3, r2
        swx     r9, (r3)+
        addi    r4, #-1
        bne     .copy
        mts     s3, r2
        li      r3, #BOILER_ROM_VEC+4*BOILER_TIMER_VECTOR
        li      r9, #seg(boiler_irq)
        swx     r9, (r3)+
        li      r9, #off(boiler_irq)
        swx     r9, (r3)
        mtsc    #6, r2          ; VBR
        ; the runtime's vectors (offset, segment)
        li      r3, #seg(rtd_vec_chan_make)
        mts     s3, r3
        li      r3, #rtd_vec_chan_make
        li      r9, #off(boiler__chanMake)
        swx     r9, (r3)+
        li      r9, #seg(boiler__chanMake)
        swx     r9, (r3)+
        li      r9, #off(boiler__chanSend)
        swx     r9, (r3)+
        li      r9, #seg(boiler__chanSend)
        swx     r9, (r3)+
        li      r9, #off(boiler__chanRecv)
        swx     r9, (r3)+
        li      r9, #seg(boiler__chanRecv)
        swx     r9, (r3)+
        li      r9, #off(boiler__chanClose)
        swx     r9, (r3)+
        li      r9, #seg(boiler__chanClose)
        swx     r9, (r3)+
        li      r9, #off(boiler__spawn)
        swx     r9, (r3)+
        li      r9, #seg(boiler__spawn)
        swx     r9, (r3)+
        li      r9, #off(boiler__selectCases)
        swx     r9, (r3)+
        li      r9, #seg(boiler__selectCases)
        swx     r9, (r3)
        retf

; Interrupts off (lock) and on (unlock); Halt waits for an interrupt.
boiler__disable:
        di
        retf
boiler__enable:
        ei
        retf
boiler__halt:
        halt
        retf

; funcAddr(f func()) uintptr: a function value is its address.
boiler__funcAddr:
        retf

; dataSegment() uint16: the program's data segment (S1).
boiler__dataSegment:
        li      r1, #seg(rtd_data_start)
        retf

; The address of the task-start trampoline (spawn puts it in a new task).
boiler__entryPoint:
        li      r1, #seg(boiler_task_entry)
        li      r2, #off(boiler_task_entry)
        retf

; A new task starts here: R5:R6 its function, R1-R4 its arguments.
boiler_task_entry:
        callf   r5:r6
        li      r9, #seg(boiler__exitTask)
        li      r10, #off(boiler__exitTask)
        callf   r9:r10
.never:
        bra     .never

; The timer interrupt (and Yield, which requests it): save, schedule, load.
boiler_irq:
        push    r9
        push    r10
        mfs     r9, s3
        push    r9              ; 0: S3, 2: R10, 4: R9
        li      r9, #seg(v_boiler__current)
        mts     s3, r9
        li      r9, #off(v_boiler__current)
        lwx     r10, (r9)+      ; the Task: offset, segment
        lwx     r9, (r9)
        mts     s3, r9
        mov     r9, r10
        swx     r1, (r9)+
        swx     r2, (r9)+
        swx     r3, (r9)+
        swx     r4, (r9)+
        swx     r5, (r9)+
        swx     r6, (r9)+
        swx     r7, (r9)+
        swx     r8, (r9)+
        lw      r10, 4(r12)     ; R9
        swx     r10, (r9)+
        lw      r10, 2(r12)     ; R10
        swx     r10, (r9)+
        swx     r11, (r9)+
        mov     r10, r12        ; R12 before the pushes
        addi    r10, #6
        swx     r10, (r9)+
        mfsc    r10, #1         ; S0
        swx     r10, (r9)+
        mfs     r10, s1
        swx     r10, (r9)+
        mfs     r10, s2
        swx     r10, (r9)+
        lw      r10, 0(r12)     ; S3
        swx     r10, (r9)+
        mfsc    r10, #0         ; PC
        swx     r10, (r9)+
        mfsc    r10, #2         ; SR
        swx     r10, (r9)
        ; schedule on the interrupted task's stack (its registers are safe)
        li      r9, #seg(rtd_data_start)
        mts     s1, r9
        li      r9, #seg(boiler__schedule)
        li      r10, #off(boiler__schedule)
        callf   r9:r10
        ; load `current`
        li      r9, #seg(v_boiler__current)
        mts     s3, r9
        li      r9, #off(v_boiler__current)
        lwx     r10, (r9)+
        lwx     r9, (r9)
        mts     s3, r9
        mov     r9, r10         ; S3:R9 = the Task
        addi    r10, #24        ; S0, S1, S2 (S3 later), PC, SR
        lwx     r1, (r10)+
        mtsc    #1, r1
        lwx     r1, (r10)+
        mts     s1, r1
        lwx     r1, (r10)+
        mts     s2, r1
        lwx     r2, (r10)+      ; S3, for later
        lwx     r1, (r10)+
        mtsc    #0, r1
        lwx     r1, (r10)
        mtsc    #2, r1
        mov     r10, r9         ; R12, R11
        addi    r10, #22
        lwx     r12, (r10)
        addi    r10, #-2
        lwx     r11, (r10)
        addi    r10, #-2        ; R10 and R9 onto the task's stack, S3 last
        lwx     r1, (r10)
        push    r1              ; R10
        addi    r10, #-2
        lwx     r1, (r10)
        push    r1              ; R9
        push    r2              ; S3
        lwx     r1, (r9)+       ; R1-R8
        lwx     r2, (r9)+
        lwx     r3, (r9)+
        lwx     r4, (r9)+
        lwx     r5, (r9)+
        lwx     r6, (r9)+
        lwx     r7, (r9)+
        lwx     r8, (r9)
        pop     r10
        mts     s3, r10
        pop     r9
        pop     r10
        rti

; memcpy(dst, src uintptr, n int): the runtime's copy. n is the fifth
; argument word: in the caller's outgoing area (R12 + 4 after the pushes).
boiler__memcpy:
        push    r10
        push    r11
        mfs     r10, s2
        mts     s3, r10
        mov     r10, r12
        addi    r10, #4
        lwx     r9, (r10)
        li      r10, #@rt_memcpy
        jalr    r11, r10
        pop     r11
        pop     r10
        retf
