from lib2 import *

GY = 196  # where figures stand


def p01():
    # Ockham, a Franciscan friar, shaves away little demons and needless boxes with a huge razor.
    sc = tree(70, 190, 1.1) + tree(545, 192, 0.9, crown=OCHRE)
    friar = robe_figure(OCHRE, hair=INK, look=1, tonsure=True, arms="right", girdle=WHITE,
                        holding="", hood=None)
    sc += G(friar, 200, GY, 1.25)
    sc += razor(245, 134, 1.0, rot=-12)
    # the entities flying away
    sc += imp(372, 92, 1.0, rot=-20) + imp(420, 70, 0.9, flip=True, fill=ROSE, rot=15) + imp(470, 105, 0.85, rot=25)
    sc += box3d(350, 128, 1.1, rot=-25) + box3d(400, 140, 0.9, LAPIS, rot=30) + box3d(445, 55, 0.8, GREEN, rot=10)
    sc += box3d(505, 75, 0.9, rot=-35) + imp(515, 130, 0.75, flip=True, fill=RED, rot=-10)
    sc += L("M340,112 q8,-4 16,0 M330,98 q8,-4 16,0 M336,124 q8,-4 16,0", 1.2, WHITE)
    sc += bird(120, 162, 0.65)
    return plate(1, ("diaper", LAPIS, RED, GOLD), sc)


def p02():
    # Simula sails out of the harbour, Smalltalk rises in a balloon, the castle of inheritance crumbles.
    sc = waves(12, 330, 172, 5) + waves(12, 330, 186, 4)
    # harbour tower
    sc += tower(40, 190, 30, 120, LAPIS, cone=True)
    # ship
    ship = (P("M-50,-14 L50,-14 Q46,6 30,8 L-34,8 Q-48,4 -50,-14Z", OCHRE, 1.8) +
            L("M-46,-6 H46", 1, INK) + R(-2, -92, 4, 78, OCHRE, 1.3) +
            P("M-30,-84 Q0,-90 30,-84 L28,-30 Q0,-24 -28,-30Z", WHITE, 1.6) +
            P("M-3,-78 L3,-78 L3,-62 L18,-62 L18,-56 L3,-56 L3,-36 L-3,-36 L-3,-56 L-18,-56 L-18,-62 L-3,-62Z", RED, 1) +
            P("M2,-92 L22,-98 L2,-104Z", RED, 1.2))
    sc += G(ship, 165, 176)
    sc += bird(168, 63, 0.38)
    # balloon
    bal = (P("M0,-46 C-26,-46 -30,-12 -10,4 L10,4 C30,-12 26,-46 0,-46Z", RED, 1.8) +
           P("M0,-46 C-10,-40 -12,-12 -6,4 L6,4 C12,-12 10,-40 0,-46Z", GOLD, 1.3) +
           L("M-10,4 L-8,18 M10,4 L8,18 M0,4 V18", 0.9) + R(-10, 18, 20, 12, OCHRE, 1.4) +
           L("M-10,22 H10", 0.8))
    sc += G(bal, 300, 92)
    sc += cloud(250, 50, 0.8) + cloud(370, 38, 0.7)
    # crumbling castle on the right
    cas = (R(-60, -70, 120, 70, STONE, 1.6) + bricks(-60, -70, 120, 70) + crenels(-62, 20, -70) +
           L("M20,-70 l-6,12 l8,8 l-10,14 l6,10 M-30,-40 l6,8 l-4,10", 1.4) +
           tower(-56, 0, 26, 104, RED) +
           G(tower(0, 0, 24, 80, RED), 50, -4, rot=12) +
           arch_door(-6, 0, 22, 34, OCHRE))
    sc += G(cas, 470, GY - 2)
    for bx, by, r in ((540, 150, 10), (560, 176, -20), (528, 184, 30), (575, 138, 5)):
        sc += G(R(-5, -3.5, 10, 7, STONE, 1.2), bx, by, rot=r)
    return plate(2, ("stars", LAPIS, GOLD, None), sc, ground=GREEN, ground_y=200, hill=10)


def p03():
    # A monk nails eleven tablets to the church door.
    sc = church(330, GY, 1.2, windows=False) + tree(130, 192, 1.0) + tree(530, 194, 1.1)
    # eleven tablets around the door
    pos = [(-50, -74), (-30, -74), (-50, -52), (-30, -52), (-50, -30), (-30, -30),
           (30, -74), (50, -74), (30, -52), (50, -52), (40, -30)]
    tabs = ""
    for i, (tx, ty) in enumerate(pos):
        tabs += P(f"M{tx-7},{ty+9} L{tx-7},{ty-4} Q{tx},{ty-11} {tx+7},{ty-4} L{tx+7},{ty+9}Z", WHITE, 1.1)
        tabs += L(f"M{tx-4},{ty} h8 M{tx-4},{ty+3} h8 M{tx-4},{ty+6} h6", 0.6, RED) + C(tx, ty - 5, 1, INK, 0)
    sc += G(tabs, 330, GY, 1.2)
    monk = robe_figure(LAPIS, hair=OCHRE, look=1, tonsure=True, arms="right", girdle=WHITE)
    sc += G(monk, 228, GY, 1.15)
    sc += hammer(268, 135, 1.1, rot=-50)
    sc += bird(330, 40, 0.5)
    return plate(3, ("chequer", RED, GOLD, INK), sc)


def p04():
    # A scribe sorts things into chests by their shape.
    sc = chest(300, GY, 1.4, OCHRE, "circle", LAPIS) + chest(400, GY, 1.4, OCHRE, "square", RED) + chest(500, GY, 1.4, OCHRE, "triangle", GREEN)
    scribe = robe_figure(RED, hair=OCHRE, look=1, tonsure=False, arms="front", girdle=GOLD,
                         holding=C(0, -46, 6, LAPIS, 1.3))
    sc += G(scribe, 160, GY, 1.2)
    # things waiting on the ground
    sc += C(70, 190, 7, LAPIS, 1.4) + R(84, 182, 13, 13, RED, 1.4) + P("M104,195 L111,181 L118,195Z", GREEN, 1.4)
    sc += C(225, 192, 5, LAPIS, 1.2) + P("M236,195 L241,185 L246,195Z", GREEN, 1.2)
    # sorted things peeking out of the chests
    sc += C(292, 150, 5, LAPIS, 1.2) + C(305, 148, 5, LAPIS, 1.2)
    sc += R(390, 143, 9, 9, RED, 1.2) + R(402, 145, 9, 9, RED, 1.2)
    sc += P("M490,153 L496,142 L502,153Z", GREEN, 1.2) + P("M503,153 L509,142 L515,153Z", GREEN, 1.2)
    sc += bird(560, 120, 0.55, flip=True)
    return plate(4, ("diaper", GREEN, ROSE, GOLD), sc)


def p05():
    # The abacus and the scales; a knight slays the two-headed dragon of NaN and Infinity.
    sc = abacus(70, 160, 1.0) + scales(150, GY, 1.0)
    sc += G(knight(), 310, GY, 1.15)
    sc += dragon(470, GY - 4, 1.25, flip=True, heads=2)
    sc += L("M352,72 l10,6 M356,64 l12,2", 1.4, GOLD_D)
    sc += bird(220, 166, 0.55)
    return plate(5, ("diaper", RED, LAPIS, GOLD), sc)


def scribe_at_desk(robe, look=1, ink=RED):
    """Seated scribe at a slanted desk to the right; origin at floor under the stool."""
    d = (R(-14, -30, 22, 4, OCHRE, 1.2) + L("M-10,-26 V0 M4,-26 V0", 2.4, OCHRE) +   # stool
         P("M-14,-30 C-16,-46 -14,-62 -10,-68 Q0,-74 8,-68 C12,-60 12,-44 10,-30Z", robe, 1.7) +
         P("M4,-34 L24,-34 L22,0 L16,0 L16,-26 L4,-28Z", robe, 1.5) +           # lap and legs
         P("M14,0 L24,0 L24,-3 L15,-3Z", INK, 0.8) +
         face(0, -80, 8.5, look, hair=OCHRE, tonsure=True) +
         L("M26,-46 V0 M48,-56 V0", 2.6, OCHRE) + P("M18,-46 L56,-62 L58,-57 L20,-41Z", OCHRE, 1.4) +  # desk
         P("M24,-48 L52,-60 L53,-56 L25,-44Z", WHITE, 1.0) + L("M30,-50 l16,-7 M30,-47 l16,-7", 0.6, RED) +
         P("M10,-60 C16,-58 22,-56 30,-54 L29,-50 C22,-51 14,-52 8,-54Z", robe, 1.3) + C(30, -52, 2.4, FACE, 1) +
         L("M31,-53 L44,-74", 1.2) + P("M44,-74 C48,-80 54,-84 58,-84 C54,-80 50,-74 45,-72Z", WHITE, 0.9) +
         jar(54, -60, 0.45, ink))
    return d


def p06():
    # Scribes at slanted desks with quills.
    sc = G(scribe_at_desk(LAPIS), 110, GY) + G(scribe_at_desk(RED, ink=LAPIS), 290, GY)
    sc += G(scribe_at_desk(GREEN), 470, GY, flip=False)
    sc += arch_window(70, 60, 22, 40) + arch_window(250, 60, 22, 40) + arch_window(430, 60, 22, 40)
    sc += bird(560, 166, 0.55)
    return plate(6, ("chequer", LAPIS, RED, GOLD), sc, ground=OCHRE, ground_y=198, hill=2)


def p07():
    # The empty throne, with a gift box beside it: absence made a value.
    thr = (R(-34, -36, 68, 36, GOLD, 1.8) + R(-30, -100, 60, 66, GOLD, 1.8) +
           P("M-30,-100 Q0,-116 30,-100Z", GOLD, 1.6) + R(-24, -94, 48, 56, RED, 1.4) +
           L("M-24,-80 H24 M-24,-66 H24 M-24,-52 H24", 0.7, GOLD) +
           E(0, -40, 32, 6, LAPIS, 1.6) + C(-34, -40, 4, GOLD, 1.2) + C(34, -40, 4, GOLD, 1.2) +
           R(-34, -36, 68, 4, GOLD_D, 0) + L("M-28,0 V-30 M28,0 V-30", 1, GOLD_D) + C(0, -112, 3, LAPIS, 1))
    sc = G(thr, 300, GY, 1.2) + gift_box(390, GY, 1.3)
    ang1 = robe_figure(WHITE, hair=GOLD, tonsure=False, arms="front", girdle=None, look=1, mantle=ROSE)
    ang2 = robe_figure(WHITE, hair=GOLD, tonsure=False, arms="front", girdle=None, look=1, mantle=LAPIS)
    wings = P("M-10,-60 C-30,-74 -40,-60 -44,-40 C-34,-48 -24,-50 -14,-48Z", GOLD, 1.4)
    sc += G(wings + ang1, 160, GY, 1.1) + G(wings + ang2, 460, GY, 1.1, flip=True)
    sc += C(160, 112, 13, "none", 2, GOLD) + C(460, 112, 13, "none", 2, GOLD)
    sc += bird(540, 166, 0.55, flip=True)
    return plate(7, ("stars", LAPIS, GOLD, None), sc)


def p08():
    # The granary: sacks in neat sorted rows.
    gr = (R(-150, -110, 300, 110, STONE, 1.8) + bricks(-150, -110, 300, 110, 10, 18) +
          P("M-160,-110 L0,-150 L160,-110Z", RED, 1.8) + R(-130, -96, 260, 96, INK, 1.4) +
          L("M-130,-64 H130 M-130,-32 H130", 2.4, OCHRE) + C(0, -126, 7, LAPIS, 1.3))
    sc = G(gr, 300, GY + 2)
    fills = [OCHRE, WHITE, ROSE, GOLD]
    for row, yy in enumerate((132, 164, 196)):
        for i in range(9):
            sc += sack(190 + i * 27.5, yy, 1.0, fills[(row + i // 3) % 4])
    monk = robe_figure(LAPIS, hair=OCHRE, tonsure=True, arms="front", look=-1, holding=scroll(0, -46, 20))
    sc += G(monk, 500, GY, 1.15) + bird(80, 166, 0.6)
    return plate(8, ("diaper", GREEN, GOLD, RED), sc)


def p09():
    # A labyrinth with the bird walking its one path.
    cx, cy = 300, 104
    radii = [82, 68, 54, 40, 26]
    gaps = [90, 270, 90, 270, 90]
    rings = ""
    for r, g in zip(radii, gaps):
        a0, a1 = math.radians(g + 11), math.radians(g - 11 + 360)
        d = (f"M{f(cx+math.cos(a0)*r)},{f(cy+math.sin(a0)*r)} "
             f"A{r},{r} 0 1 1 {f(cx+math.cos(a1)*r)},{f(cy+math.sin(a1)*r)}")
        rings += L(d, 9) + L(d, 5.5, GOLD)
    walls = ""
    for (r0, r1, ang) in ((26, 40, 0), (40, 54, 180), (54, 68, 0), (68, 82, 180), (40, 54, 40), (54, 68, 140)):
        a = math.radians(ang)
        d = f"M{f(cx+math.cos(a)*r0)},{f(cy+math.sin(a)*r0)} L{f(cx+math.cos(a)*r1)},{f(cy+math.sin(a)*r1)}"
        walls += L(d, 9) + L(d, 5.5, GOLD)
    centre = C(cx, cy, 15, RED, 1.8) + boss(cx, cy, 11)
    path = L(f"M{cx},{cy+92} V{cy+75} A75,75 0 0 1 {cx-75},{cy} A75,75 0 0 1 {cx},{cy-75}", 1.6, WHITE)
    sc = rings + walls + centre + path + bird(cx, 186, 0.5)
    sc += tree(90, 192, 1.0) + tree(510, 194, 0.9, crown=OCHRE)
    sc += G(robe_figure(RED, hair=INK, tonsure=True, arms="up"), 160, GY, 0.85)
    return plate(9, ("diaper", LAPIS, RED, GOLD), sc, ground=GREEN, ground_y=204, hill=8)


if __name__ == "__main__":
    for fn in (p01, p02, p03, p04, p05, p06, p07, p08, p09):
        print(fn.__name__, fn())
