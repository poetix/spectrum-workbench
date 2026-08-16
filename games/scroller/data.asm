; The graphics.
;
; A ship is sixteen pixels square, which is exactly the two cells by two it
; stands on, and it is drawn to *fill* that square. That is not a style note,
; it is the masking: what the plotter writes is the artwork and the artwork's
; own zeros, so every pixel of every cell the ship has stamped with its colour
; is the ship's. Nothing of the terrain survives inside them to be drawn in the
; wrong ink, and there is no mask to keep in step with the shape.
;
; A shape that fills its cells needs nothing more. A round one — the enemy
; below — is blacked out to the edge of the character instead, which is the
; same write and costs four small corners of terrain. A shape that filled
; neither would read as a black box, and that is the constraint the artwork
; here is drawn to.

                macro ARTROW left,right
                db left, right
                endm

; The player: a delta that reaches the full width at the shoulders and squares
; off at the engines, so all four of its cells are carrying artwork.
ship_gfx:
                ARTROW 0b00000111,0b11100000
                ARTROW 0b00000111,0b11100000
                ARTROW 0b00001111,0b11110000
                ARTROW 0b00001111,0b11110000
                ARTROW 0b00011111,0b11111000
                ARTROW 0b00111111,0b11111100
                ARTROW 0b01111111,0b11111110
                ARTROW 0b11111111,0b11111111
                ARTROW 0b11111111,0b11111111
                ARTROW 0b11110111,0b11101111
                ARTROW 0b11100011,0b11000111
                ARTROW 0b11000011,0b11000011
                ARTROW 0b11000111,0b11100011
                ARTROW 0b11101111,0b11110111
                ARTROW 0b11111111,0b11111111
                ARTROW 0b01111111,0b11111110

; The enemy: a disc, which is the other shape that works — it reaches the cell
; edges everywhere it matters and gives up only its corners.
enemy_gfx:
                ARTROW 0b00000111,0b11100000
                ARTROW 0b00011111,0b11111000
                ARTROW 0b00111111,0b11111100
                ARTROW 0b01111111,0b11111110
                ARTROW 0b01111001,0b10011110
                ARTROW 0b11110001,0b10001111
                ARTROW 0b11111111,0b11111111
                ARTROW 0b11111111,0b11111111
                ARTROW 0b11100111,0b11100111
                ARTROW 0b11100111,0b11100111
                ARTROW 0b11111111,0b11111111
                ARTROW 0b01111111,0b11111110
                ARTROW 0b01111111,0b11111110
                ARTROW 0b00111111,0b11111100
                ARTROW 0b00011111,0b11111000
                ARTROW 0b00000111,0b11100000

; Bullets are the other kind of thing entirely: pixel-positioned, because they
; stamp no attribute and so have no cell to keep clean, and therefore stored
; pre-shifted into all eight positions a pixel column can ask for. Two pixels
; wide, OR-plotted in whatever colour they are flying through — and OR, not a
; write, because a bullet has no cell of its own to clear.
bullet_gfx:
                rept 8,phase
                rept BULLET_H
                db ($C000>>phase)>>8, ($C000>>phase)&$FF
                endr
                endr
