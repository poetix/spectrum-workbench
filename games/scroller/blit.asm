; The playfield: PF_H rows of PF_W bytes, ring buffer to screen, every frame.
;
; This is the whole cost of a smooth vertical scroll on this machine. There is
; no hardware scroll register, so a picture that has moved by one pixel is a
; picture that has been written again — 3072 bytes of it, 50 times a second.
;
; # Why the source is a stack pointer and the destination is not
;
; Reading through `POP` is 5 T-states a byte and costs nothing to maintain: the
; window is contiguous (see the mirror in equates.asm), so `SP` walks the whole
; 3072 bytes without ever being reloaded. That is what makes the ring layout
; worth its 3 KB of mirror.
;
; Writing through `PUSH` would be 5.5 T-states a byte, which is where the usual
; "stack blit" gets its speed, but it cannot be had here: `SP` is already the
; source, so each row would have to swap it to the destination and back, twice,
; because only 12 bytes of registers can be carried across a swap. Costed out,
; that is 324 T-states a row plus the self-modified addresses it needs, against
; 390 for the loop below — three per cent, for self-modifying code and a
; per-frame patch pass. It is not worth it, and the reason it is not is that
; the rows are only 24 bytes long. The trade would change with a wider one.
;
; So the destination is written the plain way, `LD (HL),r` and `INC L`, and the
; row's address is an immediate because the rows are unrolled. Per row:
;
;   ld hl,nn                       10
;   12 x (pop de, ld/inc, ld/inc)  12 * 32 = 384
;   less the last INC L nobody needs      -4
;                                  ------------
;                                  390 T-states, 16.25 a byte
;
; times PF_H rows is 49,920 T-states of a 69,888 T-state frame — before the ULA
; charges for the screen writes it contends. The measurement is the border.

blit:
                ld (blit_sp),sp
                ld sp,(src)

                rept PF_H,y
                ld hl,SCREEN + ((y&$C0)<<5) + ((y&$07)<<8) + ((y&$38)<<2) + PF_COL
                rept PF_W/2-1
                pop de
                ld (hl),e
                inc l
                ld (hl),d
                inc l
                endr
                ; The last byte of the row: no pointer to advance afterwards.
                pop de
                ld (hl),e
                inc l
                ld (hl),d
                endr

                ld sp,(blit_sp)
                ret

; Where the real stack goes while SP is reading the terrain. Interrupts are off
; for the whole of the blit — an interrupt taken here would push a return
; address into the middle of the ring buffer.
blit_sp:        dw 0
