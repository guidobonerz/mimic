; files.readBlocks(blk uint32, count, seg int) int / writeBlocks: Foundry's
; TRAP #5 and #6 (R1:R2 block, R3 count, R4 segment; R1 = 0 or an error).
files__readBlocks:
        trap    #5
        retf
files__writeBlocks:
        trap    #6
        retf
