"""Objects, buildings and beasts for the plates."""
from lib import *

STONE = VELLUM  # masonry


# ---------------------------------------------------------------- plate template

def plate(n, bg, scene, ground=GREEN, ground_y=188, hill=18, extra_defs=""):
    """A 600x230 chapter plate: gold frame, patterned sky, rolling ground, scene."""
    W, H = 600, 230
    pid = f"p{n:02d}"
    kind, a, b, c = bg
    if kind == "diaper":
        pat = diaper(pid + "d", a, b, c, 22)
    elif kind == "chequer":
        pat = chequer(pid + "d", a, b, c, 20)
    else:
        pat = stars(pid + "d", a, b, 26)
    defs = pat + f'<clipPath id="{pid}c"><rect x="12" y="12" width="576" height="206"/></clipPath>' + extra_defs
    g = hills(W, ground_y, hill, ground) if ground else ""
    body = frame(W, H, f"url(#{pid}d)") + f'<g clip-path="url(#{pid}c)">{g}{scene}</g>' + \
        R(12, 12, 576, 206, "none", 2)
    return write(f"plate-{n:02d}.svg", svg(W, H, body, defs))


# ---------------------------------------------------------------- architecture

def crenels(x0, x1, y, h=6, w=7, fill=STONE):
    d = f"M{f(x0)},{f(y)}"
    x = x0
    up = True
    while x < x1 - 0.1:
        nx = min(x + w, x1)
        if up:
            d += f" L{f(x)},{f(y-h)} L{f(nx)},{f(y-h)} L{f(nx)},{f(y)}"
        else:
            d += f" L{f(nx)},{f(y)}"
        up = not up
        x = nx
    d += f" L{f(x1)},{f(y+2)} L{f(x0)},{f(y+2)}Z"
    return P(d, fill, 1.4)


def bricks(x, y, w, h, row=8, col=14):
    """Masonry joints inside a rectangle."""
    d = ""
    r = 0
    yy = y + row
    while yy < y + h - 0.5:
        d += f"M{f(x)},{f(yy)} H{f(x+w)} "
        yy += row
    r = 0
    yy = y
    while yy < y + h - 0.5:
        off = (col / 2) if r % 2 else 0
        xx = x + off + col
        while xx < x + w - 1:
            d += f"M{f(xx)},{f(yy)} V{f(min(yy+row, y+h))} "
            xx += col
        yy += row
        r += 1
    return L(d, 0.6, GOLD_D)


def tower(x, y, w, h, roof=RED, fill=STONE, window=True, cone=True):
    """Tower standing on (x,y) = bottom centre."""
    l = x - w / 2
    out = [R(l, y - h, w, h, fill, 1.6), bricks(l, y - h, w, h)]
    if cone:
        out.append(P(f"M{f(l-3)},{f(y-h)} L{f(x)},{f(y-h-w*1.1)} L{f(l+w+3)},{f(y-h)}Z", roof, 1.6))
        out.append(C(x, y - h - w * 1.1, 2.2, GOLD, 1))
    else:
        out.append(crenels(l - 2, l + w + 2, y - h))
    if window:
        out.append(arch_window(x, y - h * 0.62, w * 0.22, w * 0.42))
    return "".join(out)


def arch_window(cx, cy, w, h, fill=LAPIS):
    return P(f"M{f(cx-w/2)},{f(cy+h/2)} L{f(cx-w/2)},{f(cy-h/2+w/2)} A{f(w/2)},{f(w/2)} 0 0 1 {f(cx+w/2)},{f(cy-h/2+w/2)} "
             f"L{f(cx+w/2)},{f(cy+h/2)}Z", fill, 1.1)


def arch_door(cx, by, w, h, fill=RED, open_=0.0):
    """Arched doorway with two leaves; open_ 0..1 shows a dark opening."""
    d = f"M{f(cx-w/2)},{f(by)} L{f(cx-w/2)},{f(by-h+w/2)} A{f(w/2)},{f(w/2)} 0 0 1 {f(cx+w/2)},{f(by-h+w/2)} L{f(cx+w/2)},{f(by)}Z"
    out = [P(d, INK if open_ else fill, 1.4)]
    if open_:
        lw = w / 2 * (1 - open_)
        out.append(P(f"M{f(cx-w/2)},{f(by)} L{f(cx-w/2)},{f(by-h+w/2)} Q{f(cx-w/2+lw)},{f(by-h+w/2-6)} {f(cx-w/2+lw)},{f(by-h+w/2+4)} L{f(cx-w/2+lw)},{f(by)}Z", fill, 1.2))
        out.append(P(f"M{f(cx+w/2)},{f(by)} L{f(cx+w/2)},{f(by-h+w/2)} Q{f(cx+w/2-lw)},{f(by-h+w/2-6)} {f(cx+w/2-lw)},{f(by-h+w/2+4)} L{f(cx+w/2-lw)},{f(by)}Z", fill, 1.2))
    else:
        out.append(L(f"M{f(cx)},{f(by)} L{f(cx)},{f(by-h)}", 1.2))
        out.append(L(f"M{f(cx-w/2)},{f(by-h*0.35)} H{f(cx+w/2)} M{f(cx-w/2)},{f(by-h*0.7)} H{f(cx+w/2)}", 1.2, GOLD_D))
        out.append(C(cx - 3, by - h * 0.5, 1.4, GOLD, 0.6) + C(cx + 3, by - h * 0.5, 1.4, GOLD, 0.6))
    return "".join(out)


def church(x, y, s=1, door_open=0.0, windows=True):
    """Church facade: nave with gable, rose oculus, side tower. Origin bottom centre."""
    d = (R(-60, -80, 120, 80, STONE, 1.6) + bricks(-60, -80, 120, 80) +
         P("M-66,-80 L0,-122 L66,-80Z", RED, 1.6) + C(0, -96, 9, LAPIS, 1.4) +
         L("M0,-105 V-87 M-9,-96 H9", 1.2, GOLD) +
         arch_door(0, 0, 34, 52, RED, door_open) +
         (arch_window(-40, -50, 12, 22) + arch_window(40, -50, 12, 22) if windows else "") +
         C(0, -122, 2.5, GOLD, 1) + L("M0,-124 V-136 M-5,-131 H5", 2, GOLD))
    return G(d, x, y, s)


def castle(x, y, s=1, gate_open=False, banner=RED):
    """Castle with two towers and a gate. Origin bottom centre; width ~140."""
    d = (R(-50, -60, 100, 60, STONE, 1.6) + bricks(-50, -60, 100, 60) + crenels(-52, 52, -60) +
         tower(-58, 0, 26, 92, RED, cone=True) + tower(58, 0, 26, 92, RED, cone=True) +
         L("M-58,-121 V-136", 1.2) + P("M-58,-136 L-44,-132 L-58,-128Z", banner, 1) +
         L("M58,-121 V-136", 1.2) + P("M58,-136 L72,-132 L58,-128Z", banner, 1))
    if gate_open:
        d += P("M-14,0 L-14,-28 A14,14 0 0 1 14,-28 L14,0Z", INK, 1.4)
    else:
        d += arch_door(0, 0, 28, 42, OCHRE)
    return G(d, x, y, s)


def waves(x0, x1, y, h=5, fill=LAPIS):
    d = f"M{f(x0)},{f(y+40)} L{f(x0)},{f(y)}"
    x = x0
    while x < x1:
        d += f" q6,{f(-h)} 12,0"
        x += 12
    d += f" L{f(x)},{f(y+40)}Z"
    crest = ""
    x = x0 + 6
    while x < x1:
        crest += f"M{f(x-3)},{f(y-2)} q3,-2 6,0 "
        x += 24
    return P(d, fill, 1.6) + L(crest, 1, WHITE)


def cloud(x, y, s=1):
    d = P("M-20,0 C-26,0 -26,-9 -18,-9 C-18,-17 -6,-19 -2,-12 C2,-20 16,-18 15,-9 C24,-10 25,0 18,0Z", WHITE, 1.4)
    return G(d, x, y, s)


def sun(x, y, r=12):
    rays = ""
    for i in range(12):
        a = math.radians(i * 30)
        rays += f"M{f(x+math.cos(a)*(r+2))},{f(y+math.sin(a)*(r+2))} L{f(x+math.cos(a)*(r+7))},{f(y+math.sin(a)*(r+7))} "
    return L(rays, 2.2, GOLD_D) + C(x, y, r, GOLD, 1.6) + face(x, y, r * 0.8, 0)


def moon(x, y, r=12):
    return P(f"M{f(x)},{f(y-r)} A{f(r)},{f(r)} 0 1 0 {f(x)},{f(y+r)} A{f(r*0.7)},{f(r)} 0 1 1 {f(x)},{f(y-r)}Z", WHITE, 1.4)


# ---------------------------------------------------------------- objects

def scroll(x, y, w=30, s=1, rot=0, fill=WHITE):
    h = w * 0.45
    d = (P(f"M{f(-w/2)},{f(-h/2)} L{f(w/2)},{f(-h/2)} L{f(w/2)},{f(h/2)} L{f(-w/2)},{f(h/2)}Z", fill, 1.3) +
         E(-w / 2, 0, 2.5, h / 2 + 1, fill, 1.2) + E(w / 2, 0, 2.5, h / 2 + 1, fill, 1.2) +
         L(f"M{f(-w/2+5)},{f(-h/4)} H{f(w/2-5)} M{f(-w/2+5)},0 H{f(w/2-8)} M{f(-w/2+5)},{f(h/4)} H{f(w/2-6)}", 0.7, RED))
    return G(d, x, y, s, rot=rot)


def hammer(x, y, s=1, rot=0):
    d = R(-1.3, -2, 2.6, 22, OCHRE, 1) + R(-6, -6, 12, 6, WHITE, 1.2)
    return G(d, x, y, s, rot=rot)


def gift_box(x, y, s=1, box=RED, ribbon=GOLD):
    d = (R(-12, -20, 24, 20, box, 1.6) + R(-14, -26, 28, 7, box, 1.6) +
         R(-2.5, -26, 5, 26, ribbon, 1) +
         P("M0,-26 C-6,-36 -14,-32 -8,-27Z", ribbon, 1.1) + P("M0,-26 C6,-36 14,-32 8,-27Z", ribbon, 1.1))
    return G(d, x, y, s)


def chest(x, y, s=1, wood=OCHRE, emblem="circle", emb_fill=LAPIS):
    d = (R(-20, -22, 40, 22, wood, 1.6) + P("M-20,-22 Q0,-34 20,-22Z", wood, 1.6) +
         L("M-20,-12 H20", 1, INK) + R(-3, -16, 6, 7, GOLD, 1) +
         L("M-14,-22 V0 M14,-22 V0", 2, GOLD_D))
    if emblem == "circle":
        d += C(0, -27, 3, emb_fill, 1)
    elif emblem == "square":
        d += R(-3, -30, 6, 6, emb_fill, 1)
    else:
        d += P("M-3.5,-24 L0,-30.5 L3.5,-24Z", emb_fill, 1)
    return G(d, x, y, s)


def scales(x, y, s=1, left=GOLD, right=RED):
    d = (R(-2, -60, 4, 60, OCHRE, 1.2) + P("M-12,0 L12,0 L8,-4 L-8,-4Z", OCHRE, 1.2) +
         L("M-26,-56 L26,-60", 2.2) + C(0, -62, 3, GOLD, 1) +
         L("M-26,-56 L-34,-36 M-26,-56 L-18,-36 M26,-60 L18,-40 M26,-60 L34,-40", 0.8) +
         P("M-36,-36 Q-26,-28 -16,-36Z", GOLD, 1.2) + P("M16,-40 Q26,-32 36,-40Z", GOLD, 1.2) +
         C(-26, -39, 3, left, 0.9) + R(23, -45, 6, 5, right, 0.9))
    return G(d, x, y, s)


def abacus(x, y, s=1):
    out = [R(-30, -40, 60, 40, "none", 3.4, OCHRE), R(-30, -40, 60, 40, "none", 0.8)]
    colors = [RED, LAPIS, GOLD, GREEN, RED]
    for i in range(5):
        yy = -34 + i * 7
        out.append(L(f"M-28,{yy} H28", 0.8))
        n = [3, 5, 2, 4, 1][i]
        for j in range(n):
            out.append(E(-24 + j * 6, yy, 2.8, 2.6, colors[i], 0.8))
        for j in range(5 - n):
            out.append(E(24 - j * 6, yy, 2.8, 2.6, colors[i], 0.8))
    out.append(L("M-24,0 L-28,8 M24,0 L28,8", 2.4, OCHRE))
    return G("".join(out), x, y, s)


def candle(x, y, h=26, lit=True, fill=WHITE):
    d = R(x - 3, y - h, 6, h, fill, 1.2) + P(f"M{f(x-6)},{f(y)} L{f(x+6)},{f(y)} L{f(x+4)},{f(y-3)} L{f(x-4)},{f(y-3)}Z", GOLD, 1)
    d += L(f"M{f(x)},{f(y-h)} V{f(y-h-3)}", 0.9)
    if lit:
        d += P(f"M{f(x)},{f(y-h-14)} C{f(x+5)},{f(y-h-8)} {f(x+4)},{f(y-h-3)} {f(x)},{f(y-h-3)} C{f(x-4)},{f(y-h-3)} {f(x-5)},{f(y-h-8)} {f(x)},{f(y-h-14)}Z", GOLD, 1)
        d += E(x, y - h - 6.5, 1.3, 2.5, RED, 0)
    else:
        d += L(f"M{f(x)},{f(y-h-4)} q-3,-4 0,-8 q3,-4 0,-8", 0.8, WHITE)
    return d


def bell(x, y, s=1, fill=GOLD):
    d = (P("M-12,0 C-12,-6 -10,-10 -9,-18 C-8,-26 -4,-28 0,-28 C4,-28 8,-26 9,-18 C10,-10 12,-6 12,0 Q0,3 -12,0Z", fill, 1.6) +
         C(0, 2, 2.6, fill, 1.2) + R(-3, -32, 6, 4, fill, 1.1) + L("M-11,-4 Q0,-1 11,-4", 0.8, GOLD_D))
    return G(d, x, y, s)


def jar(x, y, s=1, fill=LAPIS, band=GOLD, shape=0):
    if shape == 0:
        body = "M-7,0 C-11,-4 -11,-16 -6,-19 L-5,-23 L5,-23 L6,-19 C11,-16 11,-4 7,0Z"
    elif shape == 1:
        body = "M-8,0 L-8,-18 Q-8,-22 -4,-22 L4,-22 Q8,-22 8,-18 L8,0Z"
    else:
        body = "M-5,0 C-12,-2 -12,-14 -4,-16 L-3,-24 L3,-24 L4,-16 C12,-14 12,-2 5,0Z"
    d = P(body, fill, 1.4) + R(-5.5, -27, 11, 4, WHITE, 1) + L("M-8,-9 H8", 1.6, band) + C(0, -14, 1.8, band, 0)
    return G(d, x, y, s)


def sack(x, y, s=1, fill=OCHRE):
    d = (P("M-9,0 C-13,-4 -12,-16 -5,-20 L-3,-24 L3,-24 L5,-20 C12,-16 13,-4 9,0Z", fill, 1.4) +
         L("M-4,-20 H4", 1.4, RED) + L("M-3,-12 q3,2 6,0", 0.7))
    return G(d, x, y, s)


def shield(x, y, s=1, field=LAPIS, charge="cross", cfill=GOLD):
    d = P("M-12,-16 L12,-16 L12,-4 Q12,8 0,14 Q-12,8 -12,-4Z", field, 1.6)
    if charge == "cross":
        d += P("M-2,-14 L2,-14 L2,-6 L10,-6 L10,-2 L2,-2 L2,11 L-2,11 L-2,-2 L-10,-2 L-10,-6 L-2,-6Z", cfill, 0.9)
    elif charge == "chevron":
        d += P("M-11,4 L0,-8 L11,4 L11,-1 L0,-13 L-11,-1Z", cfill, 0.9)
    elif charge == "bend":
        d += P("M-12,-12 L-8,-16 L12,6 L9,10Z", cfill, 0.9)
    elif charge == "roundels":
        d += C(-5, -8, 3, cfill, 0.8) + C(5, -8, 3, cfill, 0.8) + C(0, 2, 3, cfill, 0.8)
    elif charge == "fess":
        d += R(-12, -6, 24, 6, cfill, 0.9)
    elif charge == "bird":
        d += G(bird(0, 0, 1), 0, -2, 0.28)
    return G(d, x, y, s)


def banner(x, y, s=1, field=RED, charge="roundels", cfill=GOLD):
    """Hanging swallow-tailed banner on a cross-bar; origin at bar centre."""
    d = (L("M-16,0 H16", 2.4, OCHRE) + C(-16, 0, 2, GOLD, 0.8) + C(16, 0, 2, GOLD, 0.8) +
         P("M-13,0 L13,0 L13,40 L0,32 L-13,40Z", field, 1.5))
    if charge == "roundels":
        d += C(-5, 10, 3, cfill, 0.8) + C(5, 10, 3, cfill, 0.8) + C(0, 19, 3, cfill, 0.8)
    elif charge == "hammer":
        d += G(R(-1.2, -2, 2.4, 16, cfill, 0.8) + R(-5, -5, 10, 5, cfill, 0.8), 0, 10)
    elif charge == "key":
        d += C(0, 8, 4, "none", 1.8, cfill) + L("M0,12 V26 M0,22 h4 M0,25 h4", 1.8, cfill)
    elif charge == "quill":
        d += P("M-6,26 C-4,14 2,6 8,2 C6,10 0,18 -6,26Z", cfill, 0.8)
    elif charge == "wheel":
        d += C(0, 15, 7, "none", 1.6, cfill) + L("M0,8 V22 M-7,15 H7", 1.2, cfill)
    return G(d, x, y, s)


def lute(x, y, s=1, rot=0):
    d = (E(0, 0, 9, 12, OCHRE, 1.4) + C(0, -2, 2.6, INK, 0) + R(-2, -30, 4, 20, OCHRE, 1.1) +
         P("M-3,-30 L3,-30 L5,-36 L-1,-37Z", OCHRE, 1) + L("M-1,-28 V8 M1,-28 V8", 0.4))
    return G(d, x, y, s, rot=rot)


def staff(x0, y0, x1, y1, fill=OCHRE):
    return L(f"M{f(x0)},{f(y0)} L{f(x1)},{f(y1)}", 3.2) + L(f"M{f(x0)},{f(y0)} L{f(x1)},{f(y1)}", 1.6, fill)


def razor(x, y, s=1, rot=0):
    """Huge open razor: blade and handle."""
    d = (P("M0,0 L70,-4 Q78,-4 80,2 L80,8 Q60,10 0,8Z", WHITE, 1.6) + L("M6,2 L74,-1", 0.6) +
         C(0, 4, 3, GOLD, 1.1) + P("M0,4 L-40,10 Q-46,10 -46,5 L-46,2 Q-40,-2 0,0Z", RED, 1.5) +
         C(-40, 5, 1.4, GOLD, 0))
    return G(d, x, y, s, rot=rot)


def imp(x, y, s=1, flip=False, fill=RED, rot=0):
    """Little winged demon, height ~30, origin at centre."""
    d = (P("M-5,-2 C-14,-10 -18,-4 -16,2 C-12,0 -8,1 -5,3Z", GREEN, 1) +
         P("M5,-2 C14,-10 18,-4 16,2 C12,0 8,1 5,3Z", GREEN, 1) +
         E(0, 3, 6, 8, fill, 1.4) + C(0, -8, 5.5, fill, 1.4) +
         P("M-4,-12 L-6,-18 L-1,-13Z", INK, 0.6) + P("M4,-12 L6,-18 L1,-13Z", INK, 0.6) +
         C(-2, -9, 1.2, WHITE, 0) + C(2, -9, 1.2, WHITE, 0) + C(-2, -9, 0.5, INK, 0) + C(2, -9, 0.5, INK, 0) +
         L("M-2,-5 Q0,-3.6 2,-5", 0.6) + L("M3,9 Q8,14 12,10 l1,-3", 1, INK) +
         L("M-3,10 l-1,5 M3,10 l1,5", 1.2))
    return G(d, x, y, s, flip, rot=rot)


def box3d(x, y, s=1, fill=LAPIS, rot=0):
    d = (P("M-8,-4 L0,-8 L8,-4 L0,0Z", GOLD, 1.1) + P("M-8,-4 L0,0 L0,9 L-8,5Z", fill, 1.1) +
         P("M8,-4 L0,0 L0,9 L8,5Z", RED, 1.1))
    return G(d, x, y, s, rot=rot)


def dragon(x, y, s=1, flip=False, heads=1, fill=GREEN, belly=OCHRE):
    """Dragon facing right, origin on the ground under its body; length ~110."""
    d = (P("M-30,-10 C-50,-12 -62,-4 -70,-16 C-66,-6 -56,2 -34,0Z", fill, 1.6) +  # tail
         P("M-48,-38 C-40,-60 -20,-66 -8,-56 C-18,-54 -24,-48 -26,-40 C-20,-46 -12,-46 -6,-42 C-18,-40 -26,-34 -30,-28Z", RED, 1.5) +  # wing
         E(-10, -20, 26, 14, fill, 1.8) +
         P("M-28,-14 C-18,-8 0,-8 14,-14 C8,-6 -14,-4 -28,-10Z", belly, 1) +
         L("M-26,-8 l-3,8 l4,0 M-10,-7 l-2,8 l4,0 M2,-9 l1,9 l4,0", 1.8) +
         L("M-24,-32 l2,-5 l3,4 l2,-5 l3,4 l2,-5 l3,4", 1.2))
    necks = [(-4, 0)] if heads == 1 else [(-14, -6), (8, 6)]
    for dy, dx in necks:
        nx, ny = 14 + dx, -26 + dy
        d += P(f"M8,-28 C14,-38 {f(nx)},{f(ny-18)} {f(nx+12)},{f(ny-20)} L{f(nx+14)},{f(ny-12)} C{f(nx+4)},{f(ny-10)} 16,-22 12,-16Z", fill, 1.5)
        hx, hy = nx + 18, ny - 18
        d += P(f"M{f(hx-6)},{f(hy-6)} C{f(hx)},{f(hy-10)} {f(hx+10)},{f(hy-6)} {f(hx+14)},{f(hy-2)} L{f(hx+6)},{f(hy+1)} L{f(hx+12)},{f(hy+4)} C{f(hx+4)},{f(hy+6)} {f(hx-6)},{f(hy+4)} {f(hx-6)},{f(hy-6)}Z", fill, 1.4)
        d += C(hx + 1, hy - 3, 1.4, GOLD, 0.6) + P(f"M{f(hx-4)},{f(hy-6)} L{f(hx-8)},{f(hy-12)} L{f(hx-1)},{f(hy-8)}Z", fill, 1)
        d += P(f"M{f(hx+10)},{f(hy+2)} C{f(hx+16)},{f(hy+2)} {f(hx+20)},{f(hy-2)} {f(hx+24)},{f(hy+1)} C{f(hx+19)},{f(hy+3)} {f(hx+16)},{f(hy+5)} {f(hx+10)},{f(hy+3)}Z", RED, 0.8)
    return G(d, x, y, s, flip)


def horse(x, y, s=1, flip=False, coat=WHITE, cloth=RED):
    """Horse facing right, origin at hooves line under the body; length ~70."""
    d = (L("M-20,-16 L-24,0 M-12,-16 L-10,0 M14,-16 L12,0 M22,-16 L26,0", 3.6) +
         L("M-20,-16 L-24,0 M-12,-16 L-10,0 M14,-16 L12,0 M22,-16 L26,0", 2, coat) +
         P("M-30,-26 C-40,-26 -40,-14 -36,-6 C-36,-14 -32,-20 -28,-20Z", coat, 1.3) +
         E(0, -26, 26, 12, coat, 1.8) +
         P("M18,-30 C22,-42 26,-50 32,-54 L38,-50 C42,-46 44,-42 42,-40 L36,-40 C32,-38 30,-32 26,-22Z", coat, 1.6) +
         P("M31,-54 L30,-60 L35,-55Z", coat, 1) + C(36, -48, 1.2, INK, 0) +
         L("M22,-40 C24,-46 27,-50 30,-53", 2.2, OCHRE) +
         P("M-14,-36 L14,-36 L16,-18 L-16,-18Z", cloth, 1.4) + L("M-16,-22 H16", 1.6, GOLD) +
         L("M40,-42 L28,-34", 0.8))
    return G(d, x, y, s, flip)


def rider(robe=LAPIS, hat=None):
    """Upper body of a rider seated at (0,0) = saddle point; height ~45."""
    d = (P("M-9,0 L-8,-26 Q0,-30 8,-26 L9,0Z", robe, 1.5) + L("M-2,0 L-4,12 L0,14", 3.2) + L("M-2,0 L-4,12 L0,14", 1.8, robe) +
         face(0, -34, 7, 1, hair=OCHRE, tonsure=False))
    if hat:
        d += hat
    return d


def pigeon(x, y, s=1, flip=False, letter=True):
    d = (E(0, 0, 9, 5, WHITE, 1.3) + C(8, -3, 3.4, WHITE, 1.2) + P("M11,-3 L15,-2 L11,-1Z", GOLD, 0.8) +
         C(9, -4, 0.8, INK, 0) + P("M-2,-2 C-6,-14 2,-16 6,-12 C4,-8 2,-4 -2,-2Z", WHITE, 1.2) +
         P("M-8,0 L-15,-3 L-14,3Z", WHITE, 1.1))
    if letter:
        d += R(-3, 4, 7, 5, VELLUM, 0.8) + C(0.5, 6.5, 1, RED, 0)
    return G(d, x, y, s, flip)


def goose(x, y, s=1, flip=False):
    d = (P("M-14,-6 C-16,-16 -2,-18 8,-14 C10,-22 8,-30 12,-34 C16,-36 20,-34 20,-30 C18,-30 16,-28 16,-24 C16,-18 18,-12 14,-4 C8,2 -10,2 -14,-6Z", WHITE, 1.5) +
         P("M19,-33 L26,-31 L19,-29Z", OCHRE, 1) + C(16, -32, 1, INK, 0) +
         L("M-6,-10 q6,4 12,0", 0.8) + L("M-2,1 l-2,5 l4,0 M4,1 l0,5 l4,0", 1.4, OCHRE))
    return G(d, x, y, s, flip)


def fox(x, y, s=1, flip=False, robe=None):
    """Fox standing upright (as a preacher), origin at feet; height ~70."""
    d = (P("M-10,-22 C-24,-20 -30,-6 -26,4 C-22,-4 -16,-8 -10,-10Z", OCHRE, 1.4) + P("M-28,2 C-28,-2 -26,-2 -24,0Z", WHITE, 0.8) +
         P("M-10,0 C-12,-20 -10,-40 0,-44 C10,-40 12,-20 10,0Z", robe or OCHRE, 1.7) +
         P("M-8,-40 C-8,-50 -4,-56 0,-56 C6,-56 10,-52 18,-48 C12,-44 6,-42 2,-40Z", OCHRE, 1.5) +
         P("M-6,-54 L-8,-64 L-1,-56Z", OCHRE, 1.2) + P("M1,-56 L2,-66 L6,-55Z", OCHRE, 1.2) +
         P("M6,-44 C10,-44 14,-46 18,-48 C14,-42 10,-41 6,-42Z", WHITE, 0.8) + C(18, -48, 1.2, INK, 0) +
         P("M2,-51 q2,-1 4,0 q-2,1 -4,0z", INK, 0) + L("M-10,0 l-2,3 h5 M10,0 l2,3 h-5", 1.4))
    return G(d, x, y, s, flip)


def cat(x, y, s=1, flip=False, fur=OCHRE):
    """Cat running right, origin on the ground; length ~60."""
    d = (P("M-24,-14 C-34,-22 -36,-34 -30,-38 C-30,-30 -26,-22 -18,-18Z", fur, 1.4) +
         E(-4, -14, 20, 8, fur, 1.6) + L("M-14,-8 l-10,8 M-8,-8 l-2,8 M6,-8 l6,8 M12,-10 l12,6", 2.6) +
         L("M-14,-8 l-10,8 M-8,-8 l-2,8 M6,-8 l6,8 M12,-10 l12,6", 1.2, fur) +
         C(18, -20, 8, fur, 1.5) + P("M12,-26 L12,-34 L18,-28Z", fur, 1.2) + P("M20,-28 L26,-34 L25,-24Z", fur, 1.2) +
         P("M19,-22 q2,-1.5 4,0 q-2,1.5 -4,0z", INK, 0) + C(26, -18, 1, ROSE, 0) +
         L("M24,-17 l7,-1 M24,-16 l7,2 M-10,-18 l4,-4 M-2,-20 l3,-4", 0.6))
    return G(d, x, y, s, flip)


def mouse(x, y, s=1, flip=False):
    d = (E(0, -5, 8, 5, WHITE, 1.3) + C(7, -8, 3.6, WHITE, 1.2) + C(6, -12, 2.6, ROSE, 1) +
         C(9, -8, 0.8, INK, 0) + L("M-8,-5 C-16,-5 -18,-12 -24,-8", 1) + L("M-3,0 l-1,2 M3,0 l1,2", 1))
    return G(d, x, y, s, flip)


def ape(x, y, s=1, flip=False, fur=OCHRE):
    """Seated ape reading a book; origin at seat."""
    d = (E(0, -16, 13, 17, fur, 1.6) + E(-6, 0, 9, 5, fur, 1.4) + E(8, 0, 9, 5, fur, 1.4) +
         C(0, -40, 10, fur, 1.6) + E(1, -37, 7, 6, VELLUM, 1.1) + C(-10, -40, 3, fur, 1.1) + C(10, -40, 3, fur, 1.1) +
         C(-2.5, -41, 1.2, INK, 0) + C(3.5, -41, 1.2, INK, 0) + L("M-1,-35 q2,1.5 4,0", 0.7) +
         L("M-3,-46 q3,-2 6,0", 0.7))
    d += open_book(1, -18, 1.05, LAPIS)
    d += C(-11, -22, 3, fur, 1) + C(13, -22, 3, fur, 1)
    return G(d, x, y, s, flip)


def fish(x, y, s=1, fill=LAPIS, flip=False):
    d = P("M-10,0 C-4,-7 6,-7 10,0 C6,7 -4,7 -10,0Z M-10,0 L-16,-6 L-16,6Z", fill, 1.2) + C(5, -1, 1, INK, 0)
    return G(d, x, y, s, flip)
