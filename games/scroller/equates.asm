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
; Inside those cells the artwork is written rather than masked in, and its own
; zeros are the mask: every pixel of every stamped cell is the ship's, so a
; clash there is not mitigated but impossible. That asks of the artwork that it
; fill its square, which is what the shapes in data.asm are drawn to.
;
; Around them the ship carries a black border, and the border is a *shape*: the
; artwork grown MASK_GROW pixels in every direction. It spills out of the
; ship's own cells into the ones next door — where no clash could happen and
; nothing requires it — because a border that stopped at the cell boundary
; would draw the boundary. What should read as an outline, or a shadow, would
; read as the rectangle it was clipped to.
;
; So the drawn region is four bytes across and SPR_H+2*MASK_GROW rows tall: the
; artwork written over the middle two bytes, the grown shape ANDed away either
; side of it and above and below.

SPR_W           equ 2                   ; bytes across: two cells
SPR_H           equ 16                  ; rows, which is two cells
SPR_CELLS       equ SPR_H/8             ; cells down
SPR_ART         equ SPR_H*SPR_W         ; bytes of artwork

MASK_GROW       equ 3                   ; pixels of border around the artwork
HALO_W          equ SPR_W+2             ; bytes across the region drawn
HALO_H          equ SPR_H+2*MASK_GROW   ; rows down it
HALO_EDGE       equ MASK_GROW*2*HALO_W  ; the rows above and below the artwork
HALO_MID        equ SPR_H*HALO_W        ; the rows the artwork is on

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
