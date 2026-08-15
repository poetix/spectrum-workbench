; The numbers every other file in the game reads.
;
; The screen is divided once, here: the top two thirds are the playfield and
; the bottom third is the panel. Nothing below this file knows a pixel row
; from a character row without asking it.

; -- the display ------------------------------------------------------------

SCREEN          equ $4000
ATTRS           equ $5800

PF_COL          equ 4                   ; leftmost character column
PF_W            equ 24                  ; bytes across, so 192 pixels
PF_H            equ 128                 ; pixel rows: the top two thirds
PF_ROWS         equ PF_H/8              ; character rows

ATTR_FIELD      equ 0b00000101           ; cyan on black: the terrain
ATTR_PANEL      equ 0b00001111           ; white on blue: the surround
ATTR_SHIP       equ 0b01000111           ; bright white
ATTR_ENEMY      equ 0b01000010           ; bright red

; -- sprites ----------------------------------------------------------------
;
; A ship stands on the character grid, both ways, so the cells it covers are
; exactly its own two by two and stamping them with its colour takes nothing
; from anything else. Moving on the grid is what buys that: a ship anywhere
; else straddles cells it would have to claim and cannot fill.
;
; The mask follows the shape. It is the artwork grown by MASK_GROW pixels in
; every direction, so what the plotter clears is a black outline hugging the
; ship rather than a black square around it — which is the difference between
; a ship flying over the terrain and a ship carrying a hole in it.
;
; What that outline is for is the colour. Inside a stamped cell, any terrain
; left standing is drawn in the ship's ink; the outline is what pushes the
; terrain far enough back that there is none of it near the ship to notice.
; Terrain in the far corner of a covered cell is the residue this leaves, and
; MASK_GROW is the dial: wider outline, less residue, fatter ship.

SPR_W           equ 2                   ; bytes across: two cells
SPR_H           equ 16                  ; rows, which is two cells
SPR_CELLS       equ SPR_H/8             ; cells down
MASK_GROW       equ 3                   ; pixels of black around the artwork

SPR_ART         equ SPR_H*SPR_W         ; bytes of artwork
SPR_PLOT        equ SPR_H*SPR_W*2       ; bytes of mask and data, interleaved

; The furthest a ship can stand and still be inside the playfield.
SPR_CX_MAX      equ PF_W-SPR_W
SPR_CY_MAX      equ PF_ROWS-SPR_CELLS

BULLET_H        equ 4                   ; rows in a bullet

; -- the terrain ring -------------------------------------------------------
;
; 256 lines of PF_W bytes, written at the top edge as the window walks
; backwards through it. The window is PF_H lines and a blit that had to wrap
; part way down would need a second source pointer and a branch per row, so the
; first PF_H lines are *mirrored* after the end of the ring: a window starting
; anywhere in the ring proper is then contiguous, and the blit is a straight
; run through memory. The cost is 24 bytes copied per frame, in terrain.asm.

BUF             equ $8000
BUF_LINES       equ 256
BUF_BYTES       equ BUF_LINES*PF_W      ; 6144
MIRROR          equ BUF_BYTES           ; a line to its mirror
MIRRORED        equ BUF+PF_H*PF_W       ; lines below this address are mirrored
BUF_TOTAL       equ (BUF_LINES+PF_H)*PF_W

ROCK            equ $FF                 ; solid terrain
DASH            equ 0b10011001           ; the marker rows, every 16th line

; -- everything else --------------------------------------------------------

CODE            equ $B000
STACK           equ $FDF0

; IM 2 wants 257 bytes of the same value, and the vector it lands on is that
; value in both halves: $FD * 257 = $FDFD, which is where the jump goes.
IM2_TABLE       equ $FE00
IM2_FILL        equ $FD
IM2_JUMP        equ IM2_FILL*257

; -- where the frame went ---------------------------------------------------
;
; Each phase paints the border, so a frame's stripe down the side of the screen
; is a measurement of it. rkw-shot counts the lines (crates/rkw-shot/src/lib.rs).

BDR_IDLE        equ 0                   ; black: waiting for the interrupt
BDR_SCROLL      equ 1                   ; blue: the ring, and the new line
BDR_LOGIC       equ 4                   ; green: keys and actor movement
BDR_BLIT        equ 2                   ; red: the ring to the screen
BDR_SPRITES     equ 6                   ; yellow: sprites, bullets, attributes

                macro BORDER colour
                ld a,colour
                out ($FE),a
                endm
