from lib2 import *

GY = 196


def kneeling(robe, hair=OCHRE, tonsure=True, look=1):
    """Kneeling figure facing right, hands joined in prayer; origin on the ground at the knee."""
    return (P("M-22,0 C-24,-10 -18,-30 -14,-44 Q-6,-52 4,-46 C8,-36 6,-22 10,-14 L22,-12 L22,0Z", robe, 1.7) +
            L("M-12,-30 C-12,-20 -14,-10 -16,0 M0,-30 C2,-22 6,-16 10,-14", 1) +
            P("M2,-44 C10,-44 14,-46 18,-50 L16,-54 C12,-52 8,-50 3,-50Z", robe, 1.4) +
            P("M17,-50 L22,-60 L24,-56Z", FACE, 1) +
            face(-4, -58, 8.5, look, hair=hair, tonsure=tonsure))


def p10():
    # A knight flees from a giant snail: the classic drollery, here for failure.
    sc = tree(470, 194, 1.2) + tree(90, 192, 0.9, crown=OCHRE)
    sc += G(knight(pose="flee"), 205, GY, 1.2, flip=True)
    sc += L("M240,110 h22 M246,124 h24 M240,138 h20", 1.6, WHITE)
    sc += snail(380, GY, 2.1, flip=True)
    sc += bird(505, 168, 0.55, flip=True)
    sc += L("M500,140 q4,-6 8,0", 1.2)
    return plate(10, ("diaper", RED, LAPIS, GOLD), sc)


def p11():
    # A sacristan snuffs the candles in reverse order; the door is closing.
    sc = church(90, GY, 0.95, door_open=0.35)
    altar = R(230, 160, 300, 36, WHITE, 1.8) + R(230, 160, 300, 8, RED, 1.4) + L("M230,174 H530", 0.8, GOLD_D)
    sc += altar
    for i, x in enumerate(range(250, 520, 45)):
        sc += candle(x, 160, 26 + (i % 2) * 6, lit=i < 4)
    figure = robe_figure(GREEN, hair=INK, tonsure=True, arms="up", girdle=WHITE, look=1)
    sc += G(figure, 560, GY, 1.0, flip=True)
    sc += staff(542, 110, 392, 100, OCHRE)
    sc += P("M379,104 L391,104 L385,94Z", GOLD, 1.2) + L("M385,94 L392,100", 1.2)
    sc += bird(185, 168, 0.5)
    return plate(11, ("stars", LAPIS, GOLD, None), sc, ground=OCHRE, ground_y=198, hill=2)


def p12():
    # Masons build a cathedral from blocks; a crane lifts the next one.
    cat = (R(-120, -96, 240, 96, STONE, 1.8) + bricks(-120, -96, 240, 96, 9, 16) +
           arch_window(-80, -54, 20, 44) + arch_window(-40, -54, 20, 44) + arch_window(0, -54, 20, 44) +
           arch_window(40, -54, 20, 44) + arch_window(80, -54, 20, 44) +
           tower(-100, -96, 40, 30, RED, cone=False, window=False) +
           P("M-80,-96 L-80,-104 L-60,-104 L-60,-110 L-30,-110 L-30,-104 L0,-104 L0,-96Z", STONE, 1.4))
    sc = G(cat, 330, GY)
    # crane: mast, jib, rope, block
    sc += staff(500, GY, 500, 40) + staff(500, 44, 380, 60) + L("M392,58 V96", 1)
    sc += R(380, 96, 24, 14, STONE, 1.4) + L("M380,96 L392,88 L404,96", 0.8)
    sc += C(500, 168, 18, "none", 3.4, OCHRE) + C(500, 168, 18, "none", 1) + L("M500,150 V186 M482,168 H518", 1.4, OCHRE)
    # masons
    m1 = robe_figure(LAPIS, hair=OCHRE, tonsure=False, arms="up", girdle=WHITE,
                     holding=R(-22, -96, 44, 14, STONE, 1.4))
    sc += G(m1, 140, GY, 1.0)
    m2 = robe_figure(RED, hair=INK, tonsure=False, arms="right", girdle=WHITE)
    sc += G(m2, 270, 103, 0.6) + hammer(292, 72, 0.7, rot=-40)
    sc += bird(560, 166, 0.55, flip=True)
    return plate(12, ("chequer", LAPIS, GOLD, RED), sc)


def p13():
    # The Wheel of Fortune turns one state into the next.
    cx, cy, r = 300, 122, 62
    wheel = C(cx, cy, r, "none", 8, INK) + C(cx, cy, r, "none", 5, OCHRE)
    spokes = ""
    for i in range(8):
        a = math.radians(i * 45 + 22.5)
        spokes += f"M{cx},{cy} L{f(cx+math.cos(a)*r)},{f(cy+math.sin(a)*r)} "
    wheel = L(spokes, 5) + L(spokes, 2.6, OCHRE) + wheel + C(cx, cy, 12, GOLD, 1.8) + C(cx, cy, 4, RED, 1)
    sc = wheel + staff(cx - 40, 200, cx, cy) + staff(cx + 40, 200, cx, cy)
    king = robe_figure(RED, hair=OCHRE, tonsure=False, crown=GOLD, arms="up", girdle=GOLD, beard=OCHRE)
    sc += G(king, cx, cy - r + 3, 0.46)
    rising = robe_figure(GREEN, hair=OCHRE, tonsure=False, arms="up", girdle=WHITE)
    sc += G(rising, cx - r - 2, cy + 14, 0.5, rot=-60)
    falling = robe_figure(LAPIS, hair=INK, tonsure=False, arms="up", girdle=WHITE)
    sc += G(falling, cx + r + 6, cy + 6, 0.5, rot=110)
    fortune = robe_figure(ROSE, hair=GOLD, tonsure=False, arms="right", girdle=GOLD, crown=GOLD, mantle=LAPIS)
    sc += G(fortune, 120, GY, 1.1)
    sc += bird(480, 168, 0.55, flip=True) + tree(540, 194, 1.0)
    return plate(13, ("diaper", RED, GOLD, LAPIS), sc)


def p14():
    # Guild craftsmen under their heraldic banners: roles.
    sc = L("M40,30 H560", 3.4, OCHRE) + L("M40,30 H560", 1)
    crafts = [(120, RED, "hammer", GOLD, LAPIS), (300, LAPIS, "key", GOLD, GREEN), (480, GREEN, "quill", WHITE, RED)]
    for x, field, charge, cf, robe in crafts:
        sc += banner(x, 30, 1.3, field, charge, cf)
    smith = robe_figure(LAPIS, hair=OCHRE, tonsure=False, arms="right", girdle=OCHRE, beard=OCHRE)
    sc += G(smith, 105, GY, 1.0) + hammer(140, 140, 0.9, rot=-30)
    sc += R(140, 170, 34, 12, INK, 1.4) + P("M136,170 L178,170 L172,164 L142,164Z", INK, 1.2)
    lock = robe_figure(GREEN, hair=OCHRE, tonsure=False, arms="front", girdle=GOLD,
                       holding=C(0, -46, 5, "none", 2, GOLD) + L("M0,-41 V-30 M0,-34 h4", 2, GOLD))
    sc += G(lock, 300, GY, 1.0)
    scribe = robe_figure(RED, hair=OCHRE, tonsure=True, arms="front", girdle=WHITE, holding=open_book(0, -40, 1, LAPIS))
    sc += G(scribe, 480, GY, 1.0)
    sc += bird(395, 172, 0.5)
    return plate(14, ("chequer", GOLD, RED, INK), sc)


def jester(look=1):
    d = (P("M-11,-2 Q-12,2 -5,2 L-4,-2Z", INK, 1) + P("M11,-2 Q12,2 5,2 L4,-2Z", INK, 1) +
         P("M-18,0 C-16,-20 -14,-40 -12,-58 Q0,-66 0,-66 L0,0Z", RED, 1.8) +
         P("M18,0 C16,-20 14,-40 12,-58 Q0,-66 0,-66 L0,0Z", GREEN, 1.8) +
         P("M-18,0 L-12,-8 L-6,0 L0,-8 L6,0 L12,-8 L18,0Z", GOLD, 1.2) +
         face(0, -74, 9, look, hair=None, tonsure=False) +
         P("M-10,-78 C-14,-92 -24,-96 -30,-90 C-24,-90 -18,-86 -14,-80Z", RED, 1.4) +
         P("M-4,-82 C-4,-96 0,-104 4,-106 C6,-98 4,-90 4,-82Z", GOLD, 1.4) +
         P("M10,-78 C14,-92 24,-96 30,-90 C24,-90 18,-86 14,-80Z", GREEN, 1.4) +
         P("M-11,-79 Q0,-86 11,-79 L10,-75 Q0,-80 -10,-75Z", GOLD, 1.2) +
         C(-30, -90, 2.4, GOLD, 1) + C(4, -106, 2.4, GOLD, 1) + C(30, -90, 2.4, GOLD, 1))
    return d


def p15():
    # A jester imitates the king for an examiner: fakes and tests.
    thr = (R(-28, -80, 56, 80, GOLD, 1.8) + R(-22, -74, 44, 40, RED, 1.2) + E(0, -32, 26, 5, LAPIS, 1.4))
    king = robe_figure(RED, hair=OCHRE, tonsure=False, crown=GOLD, arms="right", girdle=GOLD, beard=OCHRE, mantle=LAPIS)
    sc = G(thr, 120, GY) + G(king, 120, GY + 4, 1.05)
    sc += staff(153, 130, 160, 90, GOLD) + C(160, 88, 4, GOLD, 1.2)
    sc += G(jester(-1), 300, GY, 1.1, flip=True)
    sc += P("M260,128 L268,100 L276,128Z", GOLD, 1.2) + staff(262, 160, 256, 112, OCHRE) + C(256, 110, 4, GOLD, 1.2)
    exam = robe_figure(LAPIS, hair=INK, tonsure=True, arms="front", look=-1, girdle=WHITE,
                       holding=open_book(0, -40, 1.0, GREEN))
    sc += G(exam, 470, GY, 1.1) + bird(540, 168, 0.55, flip=True)
    return plate(15, ("diaper", GREEN, GOLD, RED), sc)


def p16():
    # A herald presents a row of shields: the cases of an enum.
    sc = L("M150,92 H560", 3.6, OCHRE) + L("M150,92 H560", 1)
    charges = [(LAPIS, "cross", GOLD), (RED, "chevron", GOLD), (GOLD, "bend", LAPIS), (GREEN, "roundels", WHITE),
               (WHITE, "fess", RED), (ROSE, "bird", WHITE)]
    for i, (fl, ch, cf) in enumerate(charges):
        x = 190 + i * 68
        sc += L(f"M{x},92 V100", 1.2) + shield(x, 120, 1.7, fl, ch, cf)
    her = robe_figure(GOLD, hair=OCHRE, tonsure=False, arms="right", girdle=RED, mantle=LAPIS)
    sc += G(her, 80, GY, 1.15)
    sc += P("M118,135 L150,118 L152,124 L120,140Z", GOLD, 1.3) + P("M148,112 L160,106 L162,132 L150,128Z", GOLD, 1.3)
    sc += P("M128,134 L144,126 L146,146 L134,152 L130,142Z", RED, 1.2)
    sc += bird(300, 182, 0.5) + tree(520, 196, 0.8)
    return plate(16, ("chequer", LAPIS, RED, GOLD), sc)


def p17():
    # A bell-founder pours one mould with different metals: generics.
    fur = (P("M-40,0 L-40,-40 C-40,-70 40,-70 40,-40 L40,0Z", STONE, 1.8) + bricks(-40, -64, 80, 64, 8, 14) +
           P("M-18,0 L-18,-18 A18,18 0 0 1 18,-18 L18,0Z", INK, 1.4) +
           P("M-12,0 C-12,-10 -6,-16 -2,-24 C0,-16 6,-14 4,-6 C8,-10 10,-4 10,0Z", RED, 1) +
           P("M-6,0 C-6,-6 -2,-10 0,-14 C2,-8 4,-6 4,0Z", GOLD, 0.8) + R(-10, -86, 20, 22, STONE, 1.6))
    sc = G(fur, 100, GY)
    sc += L("M95,108 q-6,-10 0,-18 q6,-8 0,-16 M108,104 q-6,-10 0,-18", 1.2, WHITE)
    founder = robe_figure(RED, hair=OCHRE, tonsure=False, arms="right", girdle=INK, beard=OCHRE)
    sc += G(founder, 220, GY, 1.1)
    sc += staff(258, 140, 300, 128, INK) + P("M296,124 Q306,120 312,130 Q304,138 296,132Z", INK, 1.3)
    sc += L("M312,132 Q318,140 320,152", 3.4, GOLD) + L("M312,132 Q318,140 320,152", 1.4, RED)
    sc += bell(330, GY, 2.0, GOLD)
    for i, col in enumerate((GOLD, RED, WHITE)):
        x = 430 + i * 42
        sc += P(f"M{x-12},{GY} L{x-15},{GY-22} L{x+15},{GY-22} L{x+12},{GY}Z", INK, 1.5)
        sc += E(x, GY - 22, 15, 4, col, 1.3)
        sc += bell(x, GY - 32, 0.5, col)
    sc += bird(560, 164, 0.5, flip=True)
    return plate(17, ("diaper", LAPIS, RED, GOLD), sc, ground=OCHRE, ground_y=198, hill=2)


def p18():
    # A reliquary casket holds captured relics: closures.
    rel = (R(-60, -50, 120, 50, GOLD, 1.8) + R(-54, -44, 108, 38, LAPIS, 1.3) +
           C(-34, -25, 9, RED, 1.2) + C(0, -25, 9, GREEN, 1.2) + C(34, -25, 9, RED, 1.2) +
           boss(-34, -25, 6) + boss(0, -25, 6) + boss(34, -25, 6) +
           L("M-60,-50 H60", 1.4) + R(-64, 0, 128, 6, GOLD_D, 1.4))
    lid = P("M-62,-50 L-58,-82 L58,-82 L62,-50Z", GOLD, 1.8) + P("M-50,-56 L-47,-76 L47,-76 L50,-56Z", RED, 1.2) + \
        C(-20, -66, 5, LAPIS, 1) + C(20, -66, 5, LAPIS, 1) + L("M-58,-82 L0,-98 L58,-82", 1.8) + C(0, -100, 4, GOLD, 1.3)
    sc = R(220, 160, 160, 36, WHITE, 1.8) + R(220, 160, 160, 8, RED, 1.4)
    sc += G(G(lid, 62, 50), 238, 110, rot=-28) + G(rel, 300, 160)
    # relics hovering above the open box, held by the bird
    sc += P("M282,92 l8,-4 l18,2 l6,-6 l4,4 l-4,4 l4,4 l-4,4 l-6,-6 l-18,2 l-8,-4Z", WHITE, 1.3)
    sc += P("M328,90 L334,78 L340,90 L334,98Z", RED, 1.2) + P("M350,98 C354,86 362,82 366,80 C364,88 358,96 350,98Z", WHITE, 1.1)
    sc += L("M300,104 q-4,-8 0,-14 M320,104 q4,-8 0,-14", 1.1, GOLD)
    sc += G(kneeling(GREEN), 160, GY, 1.1) + G(kneeling(LAPIS, hair=INK), 450, GY, 1.1, flip=True)
    sc += bird(420, 64, 0.5)
    sc += candle(205, 160, 22) + candle(395, 160, 22)
    return plate(18, ("stars", RED, GOLD, None), sc, ground=OCHRE, ground_y=198, hill=2)


if __name__ == "__main__":
    for fn in (p10, p11, p12, p13, p14, p15, p16, p17, p18):
        print(fn.__name__, fn())
