; The rolling terrain: a ring of lines, and the one new line a frame needs.
;
; The window walks *backwards* through the ring — one line per frame, up — so
; what was off the top edge last frame is on it now, and everything already
; drawn appears to move down the screen. A frame therefore costs one line of
; terrain, not a screenful, however fast the scroll looks.

; The top line of the window: where the blit starts reading.
src:            dw BUF

; The canyon. Byte columns, not pixels: the walls are on cell boundaries, which
; is what lets the terrain keep the playfield's attributes and the sprites keep
; theirs (see sprite.asm).
wall_l:         db 8                    ; first channel byte
wall_r:         db 16                   ; first byte of the right wall
wander_ctr:     db 0
line_ctr:       db 0
seed:           dw $A5C3

; Fill the whole ring, mirror included, so the first frame has terrain in it
; rather than whatever the loader left.
init_terrain:
                ld hl,BUF
                ld (src),hl
                ld b,0                  ; 256 lines: the ring proper
.line:          push bc
                call scroll
                pop bc
                djnz .line
                ret

; One frame's worth: step the window back a line and make the line that has
; just come into view at the top.
scroll:
                ld hl,(src)
                ld de,-PF_W
                add hl,de
                ; Below the buffer is the top of the ring, a whole ring away.
                ld a,h
                cp BUF/256
                jr nc,.inside
                ld de,BUF_BYTES
                add hl,de
.inside:        ld (src),hl

                call make_line          ; hl is the line, and is left alone

                ; The window must not wrap part way down, so the first PF_H
                ; lines of the ring exist a second time after the end of it.
                ld hl,(src)
                ld a,h
                cp MIRRORED/256
                ret nc
                push hl
                pop de
                ld hl,MIRROR
                add hl,de               ; hl = the mirror of this line
                ex de,hl                ; de = mirror, hl = line
                ld bc,PF_W
                ldir
                ret

; Write PF_W bytes of canyon at hl. hl is preserved.
make_line:
                push hl
                call wander

                ; Every sixteenth line is marked, which is what makes a
                ; one-pixel scroll visible to the eye and to a test.
                ld a,(line_ctr)
                inc a
                ld (line_ctr),a
                and $0F
                ld a,0
                jr nz,.channel_fill
                ld a,DASH
.channel_fill:  ld c,a                  ; c = what the channel is made of

                pop hl
                push hl

                ; The left wall.
                ld a,(wall_l)
                or a
                jr z,.channel
                ld b,a
.left:          ld (hl),ROCK
                inc hl
                djnz .left

                ; The channel between the walls.
.channel:       ld a,(wall_r)
                ld b,a
                ld a,(wall_l)
                neg
                add a,b                 ; wall_r - wall_l, always at least 6
                ld b,a
.gap:           ld (hl),c
                inc hl
                djnz .gap

                ; The right wall, out to the edge of the playfield.
                ld a,(wall_r)
                ld b,a
                ld a,PF_W
                sub b
                jr z,.done
                ld b,a
.right:         ld (hl),ROCK
                inc hl
                djnz .right

.done:          pop hl
                ret

; Move the walls about, a quarter as often as lines are made, so the canyon
; drifts by one byte at a time rather than jittering.
wander:
                ld a,(wander_ctr)
                inc a
                ld (wander_ctr),a
                and $03
                ret nz

                call random
                ld c,a

                ; The left wall, kept clear of both edges.
                rr c
                ld a,(wall_l)
                jr c,.left_out
                dec a
                cp 1
                jr nc,.left_done
                ld a,1
                jr .left_done
.left_out:      inc a
                cp PF_W-14
                jr c,.left_done
                ld a,PF_W-14
.left_done:     ld (wall_l),a
                ld b,a                  ; b = the left wall, for the clamp below

                ; The right wall, kept a channel's width clear of the left one.
                rr c
                ld a,(wall_r)
                jr c,.right_out
                dec a
                jr .right_clamp
.right_out:     inc a
.right_clamp:   ; no closer to the left wall than six bytes
                push af
                ld a,b
                add a,6
                ld c,a
                pop af
                cp c
                jr nc,.right_wide
                ld a,c
.right_wide:    cp PF_W-1
                jr c,.right_done
                ld a,PF_W-1
.right_done:    ld (wall_r),a
                ret

; A 16-bit xorshift, because a game wants a repeatable canyon and a test wants
; the same one twice.
random:
                push hl
                ld hl,(seed)
                ld a,h
                rra
                ld a,l
                rra
                xor h
                ld h,a
                ld a,l
                rra
                ld a,h
                rra
                xor l
                ld l,a
                xor h
                ld h,a
                ld (seed),hl
                ld a,l
                pop hl
                ret
