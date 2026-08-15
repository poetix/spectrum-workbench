; The graphics.
;
; A ship is sixteen pixels square and authored once, as artwork only. Its mask
; is grown from it at startup (sprite.asm) rather than drawn beside it, because
; a mask drawn by hand is a second copy of the shape that can disagree with the
; first — and the thing it has to be is *this* shape, MASK_GROW pixels fatter.

                macro ARTROW left,right
                db left, right
                endm

ship_gfx:
                ARTROW 0b00000001,0b10000000
                ARTROW 0b00000011,0b11000000
                ARTROW 0b00000011,0b11000000
                ARTROW 0b00000111,0b11100000
                ARTROW 0b00000111,0b11100000
                ARTROW 0b00001111,0b11110000
                ARTROW 0b00011111,0b11111000
                ARTROW 0b00111111,0b11111100
                ARTROW 0b01111111,0b11111110
                ARTROW 0b11111001,0b10011111
                ARTROW 0b11100001,0b10000111
                ARTROW 0b00000001,0b10000000
                ARTROW 0b00000011,0b11000000
                ARTROW 0b00000111,0b11100000
                ARTROW 0b00000011,0b11000000
                ARTROW 0b00000001,0b10000000

enemy_gfx:
                ARTROW 0b00000001,0b10000000
                ARTROW 0b00000111,0b11100000
                ARTROW 0b00011111,0b11111000
                ARTROW 0b00111001,0b10011100
                ARTROW 0b01111111,0b11111110
                ARTROW 0b11111111,0b11111111
                ARTROW 0b11100111,0b11100111
                ARTROW 0b11000011,0b11000011
                ARTROW 0b10000001,0b10000001
                ARTROW 0b01100110,0b01100110
                ARTROW 0b00111100,0b00111100
                ARTROW 0b00011000,0b00011000
                ARTROW 0b00110000,0b00001100
                ARTROW 0b01100000,0b00000110
                ARTROW 0b11000000,0b00000011
                ARTROW 0b00000000,0b00000000

; Bullets are the other kind of thing entirely: pixel-positioned, because they
; stamp no attribute and so have no cell to keep clean, and therefore stored
; pre-shifted into all eight positions a pixel column can ask for. Two pixels
; wide, no mask, OR-plotted in whatever colour they are flying through.
bullet_gfx:
                rept 8,phase
                rept BULLET_H
                db ($C000>>phase)>>8, ($C000>>phase)&$FF
                endr
                endr
