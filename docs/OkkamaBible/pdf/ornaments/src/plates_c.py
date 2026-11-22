from lib2 import *

GY = 196


def chain(x0, y0, x1, y1, n=None):
    """A chain of small links between two points."""
    dist = math.hypot(x1 - x0, y1 - y0)
    n = n or max(3, int(dist / 5))
    ang = math.degrees(math.atan2(y1 - y0, x1 - x0))
    out = ""
    for i in range(n):
        t = (i + 0.5) / n
        x, y = x0 + (x1 - x0) * t, y0 + (y1 - y0) * t
        out += E(x, y, 3, 1.6 if i % 2 else 1.1, "none", 1.1, GOLD_D, rot=ang)
    return out


def pilgrim_hat(fill=OCHRE):
    return (E(0, -80, 16, 4, fill, 1.4) + P("M-9,-81 C-9,-90 9,-90 9,-81Z", fill, 1.4) +
            P("M-3,-85 L0,-90 L3,-85 L0,-82Z", WHITE, 0.8))


def p19():
    # A chained library: modules and imports.
    sc = R(130, 70, 340, 8, OCHRE, 1.6)  # rail of the book press
    # sloped desk
    sc += P("M130,130 L470,130 L470,138 L130,138Z", OCHRE, 1.6) + P("M130,130 L150,100 L450,100 L470,130Z", OCHRE, 1.6)
    sc += L("M150,138 V196 M300,138 V196 M450,138 V196", 3.6, OCHRE) + L("M150,138 V196 M300,138 V196 M450,138 V196", 1)
    books = [(180, RED, True), (240, LAPIS, False), (300, GREEN, True), (360, GOLD, False), (420, RED, False)]
    for x, col, op in books:
        if op:
            sc += G(open_book(0, 0, 1.5, col), x, 118)
        else:
            sc += closed_book(x, 112, 1.1, col, rot=90)
        sc += chain(x, 104, x, 78, 6)
    monk = robe_figure(LAPIS, hair=OCHRE, tonsure=True, arms="right", girdle=WHITE, look=1)
    sc += G(monk, 90, GY, 1.1)
    sc += bird(520, 166, 0.55, flip=True)
    for x in (130, 470):
        sc += R(x - 5, 60, 10, 136, OCHRE, 1.6)
    return plate(19, ("chequer", RED, LAPIS, GOLD), sc, ground=OCHRE, ground_y=198, hill=2)


def belfry(x, by, bell_fill=GOLD, swing=0):
    d = (R(-30, -130, 60, 130, STONE, 1.8) + bricks(-30, -130, 60, 130) +
         P("M-34,-130 L0,-166 L34,-130Z", RED, 1.8) + C(0, -166, 2.4, GOLD, 1) +
         P("M-18,-84 L-18,-110 A18,18 0 0 1 18,-110 L18,-84Z", INK, 1.4) +
         G(bell(0, 0, 0.8, bell_fill), 0, -88, rot=swing) + L(f"M{f(math.sin(math.radians(-swing))*6)},-84 V-40", 1.2))
    return G(d, x, by)


def p20():
    # Monks ring bells at different hours; carrier pigeons fly between them.
    sc = sun(60, 50, 13) + moon(545, 48, 13)
    sc += belfry(170, GY, GOLD, 12) + belfry(430, GY, GOLD, -14)
    m1 = robe_figure(LAPIS, hair=OCHRE, tonsure=True, arms="up", girdle=WHITE)
    m2 = robe_figure(GREEN, hair=INK, tonsure=True, arms="up", girdle=WHITE)
    sc += G(m1, 215, GY, 0.95, flip=True) + L("M175,118 C190,112 205,110 212,112", 1.2)
    sc += G(m2, 385, GY, 0.95) + L("M425,118 C410,112 395,110 388,112", 1.2)
    sc += pigeon(270, 70, 1.4) + pigeon(320, 100, 1.2, flip=True) + pigeon(300, 40, 1.0)
    sc += L("M250,74 q-10,4 -20,0 M340,104 q10,4 20,0", 1, WHITE)
    sc += bird(300, 168, 0.55)
    return plate(20, ("stars", LAPIS, GOLD, None), sc)


def guard(surcoat=RED, look=1):
    """A gatekeeper with a spear: the knight without the plume."""
    return knight(surcoat=surcoat, charge=WHITE, shield=LAPIS, plume=surcoat, pose="none") + \
        staff(16, -2, 22, -118, OCHRE) + P("M18,-118 L22,-130 L26,-118Z", WHITE, 1.2)


def p21():
    # A walled city whose gatekeepers check the seal of every visitor: authority and natives.
    wall = (R(-300, -110, 600, 110, STONE, 1.8) + bricks(-300, -110, 600, 110, 10, 18) + crenels(-300, 300, -110))
    sc = G(wall, 300, GY - 24)
    sc += tower(200, GY - 24, 40, 128, LAPIS) + tower(400, GY - 24, 40, 128, LAPIS) + tower(40, GY - 24, 40, 112, RED) + tower(560, GY - 24, 40, 112, RED)
    sc += P("M270,172 L270,120 A30,30 0 0 1 330,120 L330,172Z", INK, 1.8)
    sc += L("M274,118 H326 M274,128 H326 M282,104 V128 M292,96 V128 M302,94 V128 M312,96 V128 M322,104 V128", 1.4, GOLD_D)
    sc += hills(600, GY - 20, 4, GREEN)
    sc += G(guard(RED), 245, GY, 0.95) + G(guard(GREEN), 355, GY, 0.95, flip=True)
    vis = robe_figure(ROSE, hair=OCHRE, tonsure=False, arms="right", girdle=GOLD, hat=pilgrim_hat(OCHRE))
    sc += G(vis, 140, GY, 1.0)
    sc += G(R(-10, -7, 20, 14, WHITE, 1.2) + C(0, 8, 5, RED, 1.2) + L("M-6,-2 h12 M-6,2 h10", 0.6, RED), 182, 140, 1.0)
    sc += bird(300, 60, 0.5)
    return plate(21, ("diaper", LAPIS, RED, GOLD), sc)


def p22():
    # An apothecary with shelves of jars: the standard library.
    sc = ""
    for yy in (78, 128, 176):
        sc += R(170, yy, 400, 7, OCHRE, 1.6)
    cols = [LAPIS, RED, GREEN, GOLD, ROSE, WHITE, OCHRE]
    k = 0
    for row, yy in enumerate((78, 128, 176)):
        for i in range(9):
            x = 192 + i * 44
            if row == 2 and i in (4, 5):
                continue
            sc += jar(x, yy, 1.25, cols[k % len(cols)], GOLD if cols[k % len(cols)] != GOLD else RED, shape=(i + row) % 3)
            k += 1
    sc += R(170, 30, 7, 168, OCHRE, 1.6) + R(563, 30, 7, 168, OCHRE, 1.6)
    apo = robe_figure(GREEN, hair=INK, tonsure=False, arms="front", girdle=GOLD,
                      holding=P("M-10,-44 L10,-44 L7,-32 L-7,-32Z", WHITE, 1.3) + L("M4,-40 L12,-56", 2.6) + L("M4,-40 L12,-56", 1.2, WHITE))
    sc += G(apo, 90, GY, 1.15)
    sc += bird(392, 162, 0.5)
    return plate(22, ("chequer", GREEN, GOLD, RED), sc, ground=OCHRE, ground_y=198, hill=2)


def house(x, by, w, h, roof=RED, wall=STONE):
    l = x - w / 2
    return (R(l, by - h, w, h, wall, 1.6) + P(f"M{f(l-4)},{f(by-h)} L{f(x)},{f(by-h-w*0.7)} L{f(l+w+4)},{f(by-h)}Z", roof, 1.6) +
            R(x - 6, by - 20, 12, 20, OCHRE, 1.2) + R(l + 6, by - h + 10, 9, 10, LAPIS, 1) + R(l + w - 15, by - h + 10, 9, 10, LAPIS, 1) +
            L(f"M{f(l)},{f(by-h+30)} H{f(l+w)}", 0.8, GOLD_D))


def p23():
    # A town crier reads from a long scroll: the command line.
    sc = house(60, GY - 10, 70, 80, RED) + house(530, GY - 10, 80, 90, LAPIS) + house(450, GY - 10, 56, 70, GREEN)
    sc += hills(600, GY - 6, 3, GREEN)
    sc += R(220, 160, 90, 36, OCHRE, 1.6) + L("M220,170 H310", 1)
    crier = robe_figure(RED, hair=OCHRE, tonsure=False, arms="front", girdle=GOLD, mantle=GOLD, beard=OCHRE,
                        hat=P("M-12,-80 C-12,-94 12,-94 12,-80Z", LAPIS, 1.4) + P("M8,-90 C14,-98 22,-98 26,-92 C20,-92 14,-90 10,-86Z", WHITE, 1.1))
    sc += G(crier, 265, 160, 1.0)
    sc += P("M247,121 L283,121 L281,170 Q265,175 249,170Z", WHITE, 1.4)
    sc += E(265, 121, 19, 3, WHITE, 1.3) + C(250, 121, 3, FACE, 1) + C(280, 121, 3, FACE, 1)
    sc += L("M253,132 h24 M253,140 h20 M253,148 h24 M253,156 h18 M253,164 h22", 0.8, RED)
    sc += G(bell(0, 0, 0.45, GOLD), 300, 100, rot=-25)
    t1 = robe_figure(GREEN, hair=INK, tonsure=False, arms="down", girdle=WHITE, look=-1)
    t2 = robe_figure(ROSE, hair=OCHRE, tonsure=False, arms="front", girdle=WHITE, look=-1, holding=C(0, -40, 4, RED, 1))
    sc += G(t1, 370, GY, 0.85) + G(t2, 410, GY, 0.8)
    sc += bird(160, 168, 0.55)
    return plate(23, ("diaper", RED, LAPIS, GOLD), sc)


def p24():
    # Messengers ride to a castle with a drawbridge: the web service.
    sc = waves(360, 590, 186, 4)
    sc += castle(470, 178, 1.05, gate_open=True)
    sc += P("M455,178 L485,178 L462,198 L432,198Z", OCHRE, 1.6) + L("M445,190 L475,190", 0.8)
    sc += L("M455,178 L456,148 M485,178 L484,148", 1.2)
    for x, robe, coat, cloth in ((150, LAPIS, WHITE, RED), (300, RED, OCHRE, LAPIS)):
        sc += horse(x, GY, 1.2, coat=coat, cloth=cloth)
        sc += G(rider(robe), x - 2, GY - 44, 1.0)
        sc += scroll(x + 14, GY - 76, 16, 1, rot=-20)
    sc += pigeon(380, 60, 1.2) + bird(560, 70, 0.4, flip=True)
    return plate(24, ("stars", LAPIS, GOLD, None), sc)


def p25():
    # A rose window: patterns.
    cx, cy = 300, 112
    sc = R(160, 12, 280, 206, STONE, 1.6) + bricks(160, 12, 280, 206, 10, 18)
    sc += C(cx, cy, 96, STONE, 2) + C(cx, cy, 90, INK, 1.6)
    cols = [RED, LAPIS, GOLD, LAPIS]
    for i in range(16):
        a0 = math.radians(i * 22.5 - 90)
        a1 = math.radians(i * 22.5 + 22.5 - 90)
        r1, r2 = 50, 86
        d = (f"M{f(cx+math.cos(a0)*r1)},{f(cy+math.sin(a0)*r1)} L{f(cx+math.cos(a0)*r2)},{f(cy+math.sin(a0)*r2)} "
             f"A{r2},{r2} 0 0 1 {f(cx+math.cos(a1)*r2)},{f(cy+math.sin(a1)*r2)} L{f(cx+math.cos(a1)*r1)},{f(cy+math.sin(a1)*r1)}Z")
        sc += P(d, cols[i % 4], 1.6)
        am = (a0 + a1) / 2
        sc += C(cx + math.cos(am) * 70, cy + math.sin(am) * 70, 6.5, WHITE if i % 2 else GREEN, 1.2)
    for i in range(8):
        a0 = math.radians(i * 45 - 90)
        a1 = math.radians(i * 45 + 45 - 90)
        d = (f"M{cx},{cy} L{f(cx+math.cos(a0)*50)},{f(cy+math.sin(a0)*50)} A50,50 0 0 1 {f(cx+math.cos(a1)*50)},{f(cy+math.sin(a1)*50)}Z")
        sc += P(d, [GREEN, ROSE][i % 2], 1.5)
        am = (a0 + a1) / 2
        sc += C(cx + math.cos(am) * 34, cy + math.sin(am) * 34, 5, GOLD, 1.1)
    sc += C(cx, cy, 20, WHITE, 1.8) + G(bird(0, 0, 1), cx - 3, cy + 1, 0.42)
    m1 = robe_figure(LAPIS, hair=OCHRE, tonsure=True, arms="up", girdle=WHITE)
    m2 = robe_figure(RED, hair=INK, tonsure=True, arms="up", girdle=WHITE)
    sc += G(m1, 90, GY, 1.05) + G(m2, 510, GY, 1.05, flip=True)
    return plate(25, ("diaper", GREEN, RED, GOLD), sc, ground=GREEN, ground_y=204, hill=6)


def p26():
    # The Tower of Babel: other languages.
    sc = ""
    tiers = [(170, 34), (138, 30), (108, 28), (80, 26), (54, 24)]
    y = GY
    for i, (w, h) in enumerate(tiers):
        l = 300 - w
        sc += R(l, y - h, 2 * w, h, STONE, 1.8) + bricks(l, y - h, 2 * w, h, 9, 16)
        for j in range(-2, 3):
            if abs(j) * 30 < w - 16:
                sc += arch_window(300 + j * (w / 3), y - h / 2, 8, 14, [LAPIS, RED][(i + j) % 2])
        y -= h
    sc += P("M268,52 L300,26 L332,52Z", RED, 1.6)
    sc += L("M130,196 L170,162 M430,196 L470,162 M190,162 L230,132", 2.4, OCHRE)
    # builders and their many tongues (speech curls in different colours)
    b1 = robe_figure(LAPIS, hair=OCHRE, tonsure=False, arms="up", girdle=WHITE, holding=R(-12, -100, 24, 10, STONE, 1.3))
    b2 = robe_figure(GREEN, hair=INK, tonsure=False, arms="front", girdle=WHITE, look=-1)
    b3 = robe_figure(ROSE, hair=OCHRE, tonsure=False, arms="front", girdle=WHITE)
    sc += G(b1, 80, GY, 0.9) + G(b2, 520, GY, 0.9) + G(b3, 248, 128, 0.5)
    for (x, y, col) in ((110, 104, RED), (488, 100, GOLD), (270, 76, WHITE), (360, 70, GREEN), (420, 120, ROSE)):
        sc += L(f"M{x},{y} q6,-8 12,0 q6,8 12,0 q6,-8 12,0", 2.4, INK) + L(f"M{x},{y} q6,-8 12,0 q6,8 12,0 q6,-8 12,0", 1.2, col)
    sc += bird(560, 60, 0.45, flip=True, wing_up=True)
    return plate(26, ("chequer", RED, GOLD, LAPIS), sc)


def p27():
    # Pilgrims on the road towards a distant city on a hill: the road ahead.
    sc = sun(80, 50, 14)
    hill = P("M300,140 C360,70 460,60 588,80 L588,200 L300,200Z", GREEN, 1.8)
    city = (tower(470, 90, 18, 40, RED) + tower(500, 88, 22, 56, LAPIS) + tower(530, 92, 18, 44, RED) +
            R(456, 74, 90, 18, STONE, 1.4) + crenels(456, 546, 74, 4, 6) + tower(560, 96, 14, 30, GOLD))
    sc += hill + city
    road = "M200,230 C240,200 300,196 340,170 C380,144 420,130 470,96"
    sc += hills(600, 196, 14, GREEN) + P(road + " L486,98 C440,138 400,158 360,180 C320,204 300,214 260,230Z", OCHRE, 1.6)
    for x, robe, sc_ in ((150, LAPIS, 1.0), (215, RED, 0.95), (280, GREEN, 0.9)):
        p = robe_figure(robe, hair=OCHRE, tonsure=False, arms="right", girdle=WHITE, hat=pilgrim_hat(OCHRE))
        sc += G(p, x, 200 - (x - 150) * 0.12, sc_)
        sc += staff(x + 33 * sc_, 196 - (x - 150) * 0.12, x + 36 * sc_, 110 - (x - 150) * 0.12)
    sc += bird(360, 150, 0.45, wing_up=True)
    sc += cloud(250, 40, 0.8) + cloud(380, 30, 0.6)
    return plate(27, ("stars", LAPIS, GOLD, None), sc, ground=None)


if __name__ == "__main__":
    for fn in (p19, p20, p21, p22, p23, p24, p25, p26, p27):
        print(fn.__name__, fn())
