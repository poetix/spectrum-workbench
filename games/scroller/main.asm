; A vertical scroller, from the bottom up: the mechanics before the game.
;
; The top two thirds of the screen are a playfield 24 characters wide and 128
; pixels tall, scrolling one pixel a frame. The terrain is not drawn on the
; screen — it is written into a ring buffer, one new line per frame, and blitted
; into the display file whole every frame with the sprites plotted over it
; afterwards. The bottom third is a panel that nothing here touches yet.
;
; Three things fall out of doing it that way, and they are the reason for it:
;
;   * The scroll costs the same whatever is on the screen. A frame writes one
;     line of terrain and copies 3072 bytes; nothing is redrawn feature by
;     feature, and nothing has to be scrolled in place.
;   * Sprites never need erasing. The blit rewrites every pixel of the
;     playfield before they are drawn, so last frame's ship is already gone.
;   * What is on the screen and what the game thinks is there cannot drift
;     apart, because the screen is derived from the buffer every frame.
;
; What it costs is the blit, which is most of a frame. blit.asm has the
; arithmetic; the border stripes have the truth.
;
; The frame is laid out for the measurement: every phase paints the border, so
; a screenshot is also a profile.

                include "equates.asm"

                org CODE

start:          di
                ld sp,STACK
                call setup_im2
                call clear_screen
                call build_sprites
                call init_terrain
                ei

frame:          halt                    ; the interrupt is the frame clock
                di
                BORDER BDR_SCROLL
                call scroll
                BORDER BDR_LOGIC
                call read_keys
                call move_actors
                BORDER BDR_BLIT
                call blit
                BORDER BDR_SPRITES
                call draw_actors
                BORDER BDR_IDLE
                ei
                jr frame

; -- interrupts -------------------------------------------------------------
;
; IM 2 rather than IM 1, so that this works the same with the ROM present as
; without it: the 48K ROM owns $0038 and everything it does there — the
; keyboard scan, FRAMES, the flash counter — is work this game does not want.

setup_im2:
                ld a,IM2_TABLE/256
                ld i,a
                im 2
                ret

; All the interrupt is for is counting frames. Everything else waits for the
; HALT to return, where it can take as long as it likes without a return
; address underneath it.
isr:            push af
                ld a,(frames)
                inc a
                ld (frames),a
                pop af
                ei
                ret

; -- setting up the screen --------------------------------------------------

clear_screen:
                ld hl,SCREEN
                ld (hl),0
                ld de,SCREEN+1
                ld bc,6143
                ldir

                ; The surround first, then the playfield over the top of it.
                ld hl,ATTRS
                ld (hl),ATTR_PANEL
                ld de,ATTRS+1
                ld bc,767
                ldir

                ld hl,ATTRS+PF_COL
                ld e,ATTR_FIELD
                ld b,PF_ROWS
                ld c,PF_W
                call stamp_rect
                ret

; -- input ------------------------------------------------------------------
;
; Read once a frame into a bitmap, so that everything downstream sees the same
; keys and nothing reads the hardware twice.

KEY_LEFT        equ 0
KEY_RIGHT       equ 1
KEY_UP          equ 2
KEY_DOWN        equ 3
KEY_FIRE        equ 4

read_keys:
                ld d,0

                ld bc,$DFFE             ; P O I U Y
                in a,(c)
                rra
                jr c,.no_p
                set KEY_RIGHT,d
.no_p:          rra
                jr c,.no_o
                set KEY_LEFT,d

.no_o:          ld bc,$FBFE             ; Q W E R T
                in a,(c)
                rra
                jr c,.no_q
                set KEY_UP,d

.no_q:          ld bc,$FDFE             ; A S D F G
                in a,(c)
                rra
                jr c,.no_a
                set KEY_DOWN,d

.no_a:          ld bc,$7FFE             ; SPACE and its neighbours
                in a,(c)
                rra
                jr c,.no_space
                set KEY_FIRE,d

.no_space:      ld a,d
                ld (keys),a
                ret

; -- the actors -------------------------------------------------------------

; A ship moves a whole character at a time, because that is what lets it own
; the cells it is in. Eight pixels every frame would be four hundred a second,
; so the grid is stepped on a repeat rate instead — held keys walk it, and the
; step itself is instant.
SHIP_DELAY      equ 4                   ; frames between steps

move_actors:
                ld a,(step_ctr)
                or a
                jr z,.may_step
                dec a
                ld (step_ctr),a
                jr .others

.may_step:      ld a,(keys)
                ld c,a
                and (1<<KEY_LEFT)|(1<<KEY_RIGHT)|(1<<KEY_UP)|(1<<KEY_DOWN)
                jr z,.others
                ld a,SHIP_DELAY
                ld (step_ctr),a

                bit KEY_LEFT,c
                jr z,.no_left
                ld a,(ship+ACT_CX)
                or a
                jr z,.no_left
                dec a
                ld (ship+ACT_CX),a
.no_left:
                bit KEY_RIGHT,c
                jr z,.no_right
                ld a,(ship+ACT_CX)
                cp SPR_CX_MAX
                jr nc,.no_right
                inc a
                ld (ship+ACT_CX),a
.no_right:
                bit KEY_UP,c
                jr z,.no_up
                ld a,(ship+ACT_CY)
                or a
                jr z,.no_up
                dec a
                ld (ship+ACT_CY),a
.no_up:
                bit KEY_DOWN,c
                jr z,.no_down
                ld a,(ship+ACT_CY)
                cp SPR_CY_MAX
                jr nc,.no_down
                inc a
                ld (ship+ACT_CY),a
.no_down:
.others:        call move_enemy
                call move_bullets
                call maybe_fire
                ret

; A cell across every eighth frame, a cell down every sixteenth, and back to
; the top when it runs off the bottom. Enough motion to show that a sprite and
; the terrain under it move independently, and at different speeds — the
; terrain a pixel at a time, the ships a character at a time.
move_enemy:
                ld a,(frames)
                and $0F
                jr nz,.across
                ld a,(enemy+ACT_CY)
                inc a
                cp SPR_CY_MAX+1
                jr c,.store_y
                xor a
.store_y:       ld (enemy+ACT_CY),a

.across:        ld a,(frames)
                and $07
                ret nz
                ld a,(enemy+ACT_CX)
                ld hl,enemy_dir
                add a,(hl)
                cp SPR_CX_MAX+1
                jr c,.store_x
                ; Off an edge: turn round and stay where it was.
                ld a,(hl)
                neg
                ld (hl),a
                ret
.store_x:       ld (enemy+ACT_CX),a
                ret

; Bullets fly up four pixels a frame and stop existing at the top edge.
move_bullets:
                ld ix,bullets
                ld b,BULLETS
.slot:          ld a,(ix+0)
                or a
                jr z,.next
                ld a,(ix+2)
                sub 4
                jr c,.kill
                cp 2
                jr c,.kill
                ld (ix+2),a
                jr .next
.kill:          ld (ix+0),0
.next:          ld de,BULLET_SLOT
                add ix,de
                djnz .slot
                ret

; Fire on a repeat rate rather than on the key, so that holding it down gives a
; stream and not a solid line.
maybe_fire:
                ld a,(fire_ctr)
                or a
                jr z,.ready
                dec a
                ld (fire_ctr),a
                ret
.ready:         ld a,(keys)
                bit KEY_FIRE,a
                ret z
                ld a,(ship+ACT_CY)
                or a
                ret z                   ; no room above the ship for one

                ld ix,bullets
                ld b,BULLETS
.slot:          ld a,(ix+0)
                or a
                jr z,.free
                ld de,BULLET_SLOT
                add ix,de
                djnz .slot
                ret                     ; every slot busy: no shot this time
.free:          ld (ix+0),1
                ; The gun is in the middle of the ship, and the bullet leaving
                ; it is in pixels: it is a character column and seven more.
                ld a,(ship+ACT_CX)
                add a,a
                add a,a
                add a,a
                add a,SPR_W*4-1
                ld (ix+1),a
                ld a,(ship+ACT_CY)
                add a,a
                add a,a
                add a,a
                sub BULLET_H
                ld (ix+2),a
                ld a,FIRE_RATE
                ld (fire_ctr),a
                ret

; -- drawing ----------------------------------------------------------------
;
; Everything here runs after the blit, which has just rewritten the playfield
; and so erased last frame's sprites. The attributes are the exception: nothing
; rewrites those, so the cells the ships stamped last frame are put back to the
; playfield colour first.

draw_actors:
                ; Last frame's cells go back to the playfield's colour before
                ; anything claims new ones.
                ld ix,ship
                call release_cells
                ld ix,enemy
                call release_cells

                ld ix,enemy
                ld hl,enemy_gfx
                ld a,ATTR_ENEMY
                call draw_actor

                ld ix,ship
                ld hl,ship_gfx
                ld a,ATTR_SHIP
                call draw_actor

                call draw_bullets
                ret

draw_bullets:
                ld ix,bullets
                ld b,BULLETS
.slot:          push bc
                ld a,(ix+0)
                or a
                jr z,.next

                ld a,(ix+1)             ; the pixel column
                ld c,a
                and $07                 ; which of the eight pre-shifts
                ld l,a
                ld h,0
                add hl,hl
                add hl,hl
                add hl,hl               ; BULLET_H*2 bytes each
                ld de,bullet_gfx
                add hl,de
                push hl

                ld a,c
                rrca
                rrca
                rrca
                and $1F                 ; the character column it starts in
                ld c,a
                ld a,(ix+2)
                call pf_addr
                pop de
                ld b,BULLET_H
                call draw_bullet

.next:          pop bc
                ld de,BULLET_SLOT
                add ix,de
                djnz .slot
                ret

; -- state ------------------------------------------------------------------

BULLETS         equ 4
BULLET_SLOT     equ 3                   ; live, pixel column, pixel row
FIRE_RATE       equ 8                   ; frames between shots

frames:         db 0
keys:           db 0

; An actor is four bytes: where it is, and the cells it stamped last frame.
; The stamped cells start on the playfield so that the first release is a
; harmless one, before anything has been drawn.
ship:           db PF_W/2-1             ; character column, near the middle
                db SPR_CY_MAX           ; character row, at the bottom
                dw ATTRS+PF_COL

enemy:          db 4
                db 0
                dw ATTRS+PF_COL

enemy_dir:      db 1
fire_ctr:       db 0
step_ctr:       db 0
bullets:        ds BULLETS*BULLET_SLOT

                include "blit.asm"
                include "terrain.asm"
                include "sprite.asm"
                include "data.asm"

; -- the interrupt vector ---------------------------------------------------

                org IM2_JUMP
                jp isr

                org IM2_TABLE
                ds 257,IM2_FILL
