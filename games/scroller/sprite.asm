; Two kinds of thing get drawn over the terrain, and they are different kinds.
;
; # Ships stand on the character grid and wear a black outline
;
; A ship stamps the cells it covers with its own colour, so it has to cover
; whole cells: it moves eight pixels at a time in both directions, and the two
; by two it stands on are its own. Nothing else is in them to be recoloured.
;
; What is left is the terrain inside those same cells, which would be drawn in
; the ship's ink because the ink is now the ship's. The mask is what deals with
; that, and it is the shape of the ship rather than the shape of its cells: the
; artwork grown by MASK_GROW pixels in every direction, cleared to black. The
; ship then flies over the terrain with a black outline hugging it, and the
; nearest surviving terrain is MASK_GROW pixels away — far enough to read as
; the ship's own outline rather than as scenery in the wrong colour.
;
; It is not a proof, it is a distance. Terrain in the far corner of a covered
; cell is still terrain in the ship's ink, and MASK_GROW is the dial between
; how much of that is left and how fat the ship looks. A blackout of the whole
; cell would be the proof, and would put the ship in a box.
;
; The mask is *grown from the artwork at startup* rather than drawn beside it,
; so there is one copy of the shape and the outline cannot disagree with it.
;
; # Bullets are pixel-positioned and carry nothing
;
; A bullet is two pixels wide and stamps no attribute, so it has no cell to
; keep clean, needs no outline, and can stand anywhere: it is OR-plotted from a
; pre-shifted phase in whatever colour it is flying through.
;
; Neither plotter erases. The blit rewrites every pixel of the playfield every
; frame, so last frame's sprites are already gone by the time these run. The
; attributes are the exception — the blit does not touch them — which is why
; the stamped cells are handed back explicitly when a ship moves off them.

; An actor: where it is, on the character grid, and the cells it stamped last.
ACT_CX          equ 0                   ; character column
ACT_CY          equ 1                   ; character row
ACT_ATTR        equ 2                   ; word: the cells stamped last frame
ACT_SIZE        equ 4

; -- growing the masks ------------------------------------------------------

; Every ship's artwork turned into a plot table: mask, data, mask, data, one
; row after another, which is the order the plotter walks.
build_sprites:
                ld hl,ship_gfx
                ld de,ship_plot
                call build_sprite
                ld hl,enemy_gfx
                ld de,enemy_plot
                jp build_sprite

; hl = artwork, de = where the plot table goes.
build_sprite:
                push hl
                push de
                call grow               ; `work` = the artwork, MASK_GROW fatter
                pop de
                pop hl

                ld ix,work
                ld b,SPR_H
.row:
                ; The mask keeps what the grown shape does not cover.
                ld a,(ix+0)
                cpl
                ld (de),a
                inc de
                ld a,(hl)
                ld (de),a
                inc de
                inc hl
                ld a,(ix+1)
                cpl
                ld (de),a
                inc de
                ld a,(hl)
                ld (de),a
                inc de
                inc hl
                inc ix
                inc ix
                djnz .row
                ret

; Copy the artwork at hl into `work` and grow it MASK_GROW pixels in every
; direction: MASK_GROW passes of "every pixel and its neighbours".
grow:
                ld de,work
                ld bc,SPR_ART
                ldir
                ld b,MASK_GROW
.pass:
                push bc
                call grow_across
                call grow_down
                pop bc
                djnz .pass
                ret

; Each row becomes itself and its two horizontal neighbours.
grow_across:
                ld ix,work
                ld c,SPR_H
.row:
                ld h,(ix+0)
                ld l,(ix+1)
                push hl
                add hl,hl               ; a pixel to the left
                ex de,hl
                pop hl
                push hl
                srl h
                rr l                    ; a pixel to the right
                ld a,h
                or d
                ld d,a
                ld a,l
                or e
                ld e,a
                pop hl
                ld a,h
                or d
                ld (ix+0),a
                ld a,l
                or e
                ld (ix+1),a
                inc ix
                inc ix
                dec c
                jr nz,.row
                ret

; Each row becomes itself and the rows above and below it. The row above has to
; be kept as it *was*, or the growth cascades down the sprite instead of
; spreading by one.
grow_down:
                ld ix,work
                ld de,0                 ; above the first row is nothing
                ld c,SPR_H
.row:
                ld h,(ix+0)
                ld l,(ix+1)
                push hl                 ; this row, as it stands, for the next
                ld a,h
                or d
                ld h,a
                ld a,l
                or e
                ld l,a
                ; The row below, where there is one.
                ld a,c
                dec a
                jr z,.store
                ld a,(ix+2)
                or h
                ld h,a
                ld a,(ix+3)
                or l
                ld l,a
.store:
                ld (ix+0),h
                ld (ix+1),l
                pop de
                inc ix
                inc ix
                dec c
                jr nz,.row
                ret

; -- drawing ----------------------------------------------------------------

; The screen address of a byte in the playfield.
;   a = pixel row, 0..PF_H-1
;   c = character column, 0..PF_W-1
; -> hl
pf_addr:
                ld b,a
                and $07                 ; the pixel row within the cell
                ld h,a
                ld a,b
                and $38                 ; the character row, low two bits
                rlca
                rlca
                ld l,a
                ld a,b
                and $C0                 ; the third of the screen
                rrca
                rrca
                rrca
                or h
                or SCREEN/256
                ld h,a
                ld a,l
                add a,PF_COL
                add a,c
                ld l,a
                ret

; The attribute address of the cell a playfield pixel row and column is in.
;   a = pixel row, c = character column
; -> hl
pf_attr:
                and $F8                 ; the character row, times eight
                ld l,a
                ld h,0
                add hl,hl
                add hl,hl               ; times thirty-two
                ld a,l
                add a,PF_COL
                add a,c
                ld l,a
                ld a,h
                add a,ATTRS/256
                ld h,a
                ret

; One ship: the masked sprite, then the cells it stands on.
;   ix = the actor, hl = its plot table, a = the attribute to stamp
draw_actor:
                ld (stamp_with),a
                ex de,hl                ; de = the plot table

                ld c,(ix+ACT_CX)
                ld a,(ix+ACT_CY)
                add a,a
                add a,a
                add a,a                 ; the character row, in pixels
                push af
                call pf_addr
                ld b,SPR_H
                call draw_masked

                pop af
                ld c,(ix+ACT_CX)
                call pf_attr
                ld (ix+ACT_ATTR),l
                ld (ix+ACT_ATTR+1),h
                ld a,(stamp_with)
                ld e,a
                ld b,SPR_CELLS
                ld c,SPR_W
                jp stamp_rect

; Put an actor's cells back to the playfield's own colour, which is what makes
; it safe for it to be somewhere else this frame.
;   ix = the actor
release_cells:
                ld l,(ix+ACT_ATTR)
                ld h,(ix+ACT_ATTR+1)
                ld e,ATTR_FIELD
                ld b,SPR_CELLS
                ld c,SPR_W
                jp stamp_rect

stamp_with:     db 0

; The sprite itself: screen AND mask OR data, two bytes at a time.
;   hl = screen address of the top left byte
;   de = the plot table: mask, data, mask, data, one row after another
;   b  = rows
draw_masked:
.row:
                ld a,(de)
                inc de
                and (hl)
                ld c,a
                ld a,(de)
                inc de
                or c
                ld (hl),a
                inc l
                ld a,(de)
                inc de
                and (hl)
                ld c,a
                ld a,(de)
                inc de
                or c
                ld (hl),a
                dec l

                ; Down one pixel row: within the cell it is the high byte, and
                ; every eighth row it is the next character row instead — which
                ; only leaves the third when the low byte does not carry.
                inc h
                ld a,h
                and $07
                jr nz,.next
                ld a,l
                add a,32
                ld l,a
                jr c,.next
                ld a,h
                sub 8
                ld h,a
.next:          djnz .row
                ret

; A bullet, OR-plotted across two bytes from a pre-shifted graphic.
;   hl = screen address of the left byte
;   de = the phase to draw
;   b  = rows
draw_bullet:
.row:
                ld a,(de)
                inc de
                or (hl)
                ld (hl),a
                inc l
                ld a,(de)
                inc de
                or (hl)
                ld (hl),a
                dec l

                inc h
                ld a,h
                and $07
                jr nz,.next
                ld a,l
                add a,32
                ld l,a
                jr c,.next
                ld a,h
                sub 8
                ld h,a
.next:          djnz .row
                ret

; Fill a rectangle of attribute cells.
;   hl = top left cell, e = the attribute, b = rows, c = columns
stamp_rect:
.row:
                push hl
                push bc
                ld b,c
.cell:          ld (hl),e
                inc hl
                djnz .cell
                pop bc
                pop hl
                ld a,l
                add a,32
                ld l,a
                jr nc,.next
                inc h
.next:          djnz .row
                ret

; -- where the grown shapes live --------------------------------------------

work:           ds SPR_ART              ; one sprite, mid-growth
ship_plot:      ds SPR_PLOT
enemy_plot:     ds SPR_PLOT
