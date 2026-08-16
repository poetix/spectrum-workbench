; Two kinds of thing get drawn over the terrain, and they are different kinds.
;
; # Ships stand on the character grid, fill it, and spill a border out of it
;
; A ship stamps the cells it covers with its own colour, so it has to cover
; whole cells: it moves eight pixels at a time in both directions, and the two
; by two it stands on are its own.
;
; Inside those cells the artwork is *written* rather than masked in — sixteen
; rows of two bytes, replacing what was there — so every pixel of every stamped
; cell is the ship's, artwork where the artwork is and black where it is not. A
; clash inside them is not mitigated but impossible.
;
; Around them the ship carries a black border, and the border follows the
; shape: the artwork grown MASK_GROW pixels in every direction, ANDed out of
; the screen. It reaches into the cells next door, where no clash could happen
; and nothing requires it, and that spill is the whole point — a border cut off
; at the cell boundary draws the boundary, and the ship ends up sitting in a
; visible rectangle instead of wearing an outline.
;
; The cells it spills into are not stamped and keep the playfield's own colour.
; Clearing pixels there costs nothing to look at, because the playfield's paper
; is black, and costs nothing to undo, because the blit rewrites them next
; frame.
;
; The border is grown from the artwork at startup rather than drawn beside it,
; so there is one copy of the shape and the outline cannot disagree with it.
;
; # Bullets are pixel-positioned and carry nothing
;
; A bullet is two pixels wide and stamps no attribute, so it has no cell to
; keep clean, no border to carry, and can stand anywhere: it is OR-plotted from
; a pre-shifted phase in whatever colour it is flying through.
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

; -- growing the border -----------------------------------------------------
;
; Each sprite ends up with two tables the plotter walks a row at a time:
;
;   `mid`   SPR_H rows of border-left, artwork, artwork, border-right
;   `edge`  MASK_GROW rows above the artwork and MASK_GROW below, four
;           border bytes each
;
; Both come out of one grown copy of the shape, four bytes wide so that the
; growth has somewhere to spill.

build_sprites:
                ld hl,ship_gfx
                ld de,ship_mid
                call build_sprite
                ld hl,enemy_gfx
                ld de,enemy_mid
                jp build_sprite

; hl = artwork, de = its `mid` table, with `edge` following it.
build_sprite:
                push de
                call grow
                pop de

                ; The rows the artwork is on: the border either side of it, and
                ; the artwork itself in the middle, to be written outright.
                ld ix,halo_work+MASK_GROW*HALO_W
                ld b,SPR_H
.mid:
                ld a,(ix+0)
                cpl
                ld (de),a
                inc de
                ld a,(hl)
                ld (de),a
                inc de
                inc hl
                ld a,(hl)
                ld (de),a
                inc de
                inc hl
                ld a,(ix+3)
                cpl
                ld (de),a
                inc de
                push bc
                ld bc,HALO_W
                add ix,bc
                pop bc
                djnz .mid

                ; The rows above the artwork, then the rows below it.
                ld ix,halo_work
                ld b,MASK_GROW
                call edge_rows
                ld ix,halo_work+(MASK_GROW+SPR_H)*HALO_W
                ld b,MASK_GROW
                ; fall through

; b rows of four border bytes each, from ix to de.
edge_rows:
.row:
                rept HALO_W,byte
                ld a,(ix+byte)
                cpl
                ld (de),a
                inc de
                endr
                push bc
                ld bc,HALO_W
                add ix,bc
                pop bc
                djnz .row
                ret

; Put the artwork in the middle of a four-byte-wide field and grow it
; MASK_GROW pixels in every direction.
;   hl = the artwork
grow:
                push hl
                ld hl,halo_work
                ld de,halo_work+1
                ld bc,HALO_W*HALO_H-1
                ld (hl),0
                ldir                    ; a clear field to grow into
                pop hl

                ; The artwork goes in the middle two bytes, MASK_GROW rows down.
                ld de,halo_work+MASK_GROW*HALO_W+1
                ld b,SPR_H
.place:
                ld a,(hl)
                ld (de),a
                inc hl
                inc de
                ld a,(hl)
                ld (de),a
                inc hl
                inc de
                inc de
                inc de                  ; on to the next row's middle
                djnz .place

                ld b,MASK_GROW
.pass:
                push bc
                call grow_across
                call grow_down
                pop bc
                djnz .pass
                ret

; Every row becomes itself and its two horizontal neighbours, across all four
; bytes — which is how the shape gets out of its own cells.
grow_across:
                ld ix,halo_work
                ld c,HALO_H
.row:
                rept HALO_W,byte
                ld a,(ix+byte)
                ld (halo_row+byte),a
                endr

                ; A pixel to the left: the four bytes shifted up one.
                ld a,(halo_row+3)
                sla a
                ld (halo_left+3),a
                ld a,(halo_row+2)
                rla
                ld (halo_left+2),a
                ld a,(halo_row+1)
                rla
                ld (halo_left+1),a
                ld a,(halo_row+0)
                rla
                ld (halo_left+0),a

                ; A pixel to the right: the same four shifted down one.
                ld a,(halo_row+0)
                srl a
                ld (halo_right+0),a
                ld a,(halo_row+1)
                rra
                ld (halo_right+1),a
                ld a,(halo_row+2)
                rra
                ld (halo_right+2),a
                ld a,(halo_row+3)
                rra
                ld (halo_right+3),a

                rept HALO_W,byte
                ld a,(halo_row+byte)
                ld hl,halo_left+byte
                or (hl)
                ld hl,halo_right+byte
                or (hl)
                ld (ix+byte),a
                endr

                ld de,HALO_W
                add ix,de
                dec c
                jr nz,.row
                ret

; Every row becomes itself and the rows above and below it. The row above has
; to be kept as it *was*, or the growth cascades down the sprite instead of
; spreading by one.
grow_down:
                ld hl,halo_prev
                ld de,halo_prev+1
                ld bc,HALO_W-1
                ld (hl),0
                ldir                    ; above the first row is nothing

                ld ix,halo_work
                ld c,HALO_H
.row:
                rept HALO_W,byte
                ld a,(ix+byte)
                ld (halo_row+byte),a    ; this row, before it grows
                endr

                rept HALO_W,byte
                ld a,(ix+byte)
                ld hl,halo_prev+byte
                or (hl)
                ld (ix+byte),a
                endr

                ld a,c
                dec a
                jr z,.no_below
                rept HALO_W,byte
                ld a,(ix+HALO_W+byte)
                or (ix+byte)
                ld (ix+byte),a
                endr
.no_below:
                rept HALO_W,byte
                ld a,(halo_row+byte)
                ld (halo_prev+byte),a
                endr

                ld de,HALO_W
                add ix,de
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

; One ship: the border above it, the artwork with its border either side, the
; border below, and then the cells it stands on.
;   ix = the actor, hl = its `mid` table, a = the attribute to stamp
draw_actor:
                ld (stamp_with),a
                ld (mid_table),hl

                ; Where the border has room to go. A ship against an edge of
                ; the playfield keeps the side that fits and loses the side
                ; that does not — the alternative is writing into the panel,
                ; which nothing would ever rub out.
                ld a,(ix+ACT_CX)
                or a
                ld a,0
                jr z,.left_done
                inc a
.left_done:     ld (spill_l),a

                ld a,(ix+ACT_CX)
                add a,SPR_W
                cp PF_W
                ld a,0
                jr nc,.right_done
                inc a
.right_done:    ld (spill_r),a

                ld a,(ix+ACT_CY)
                add a,a
                add a,a
                add a,a                 ; the character row, in pixels
                ld (art_row),a

                ; The border above, where there is a row above to put it on.
                or a
                jr z,.middle
                sub MASK_GROW
                ld c,(ix+ACT_CX)
                call pf_addr
                ld de,(mid_table)
                ex de,hl
                ld bc,HALO_MID
                add hl,bc               ; `edge` follows `mid`
                ex de,hl
                ld b,MASK_GROW
                call halo_rows
                jr .artwork

.middle:        ld a,(art_row)
                ld c,(ix+ACT_CX)
                call pf_addr

.artwork:       ld de,(mid_table)
                ld b,SPR_H
                call mid_rows

                ; The border below, where the playfield has room for it.
                ld a,(ix+ACT_CY)
                add a,SPR_CELLS
                cp PF_ROWS
                jr nc,.no_bottom
                ld de,(mid_table)
                ex de,hl
                ld bc,HALO_MID+MASK_GROW*HALO_W
                add hl,bc
                ex de,hl
                ld b,MASK_GROW
                call halo_rows
.no_bottom:
                ; Four cells, and they are the ship's now.
                ld c,(ix+ACT_CX)
                ld a,(art_row)
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
mid_table:      dw 0
art_row:        db 0
spill_l:        db 0
spill_r:        db 0

; Rows of border only, four bytes across: above the artwork and below it.
;   hl = the ship's own left byte on this row, de = the rows, b of them
halo_rows:
.row:
                ld a,(spill_l)
                or a
                jr z,.no_left
                dec l
                ld a,(de)
                and (hl)
                ld (hl),a
                inc l
.no_left:       inc de

                ld a,(de)
                and (hl)
                ld (hl),a
                inc l
                inc de
                ld a,(de)
                and (hl)
                ld (hl),a
                inc de

                ld a,(spill_r)
                or a
                jr z,.no_right
                inc l
                ld a,(de)
                and (hl)
                ld (hl),a
                dec l
.no_right:      inc de
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

; The rows the artwork is on: border, artwork written outright, border.
;   hl = the ship's own left byte on this row, de = the rows, b of them
mid_rows:
.row:
                ld a,(spill_l)
                or a
                jr z,.no_left
                dec l
                ld a,(de)
                and (hl)
                ld (hl),a
                inc l
.no_left:       inc de

                ld a,(de)
                ld (hl),a
                inc l
                inc de
                ld a,(de)
                ld (hl),a
                inc de

                ld a,(spill_r)
                or a
                jr z,.no_right
                inc l
                ld a,(de)
                and (hl)
                ld (hl),a
                dec l
.no_right:      inc de
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

halo_work:      ds HALO_W*HALO_H        ; one sprite, mid-growth
halo_row:       ds HALO_W               ; a row as it was, during a pass
halo_left:      ds HALO_W
halo_right:     ds HALO_W
halo_prev:      ds HALO_W

ship_mid:       ds HALO_MID
ship_edge:      ds HALO_EDGE
enemy_mid:      ds HALO_MID
enemy_edge:     ds HALO_EDGE
