from lib2 import *


def border_vine(x0, y0, x1, y1, step=34, colors=(RED, LAPIS, GOLD, GREEN)):
    """Vine running around a rectangle (clockwise), leaves alternating sides."""
    pts = []
    x = x0
    while x <= x1:
        pts.append((x, y0)); x += step
    y = y0 + step
    while y <= y1:
        pts.append((x1, y)); y += step
    x = x1 - step
    while x >= x0:
        pts.append((x, y1)); x -= step
    y = y1 - step
    while y > y0:
        pts.append((x0, y)); y -= step
    pts.append((x0, y0))
    return vine(pts, list(colors), leaf_s=0.62, sw=1.8)


def title():
    W, H = 600, 420
    defs = diaper("td", LAPIS, RED, GOLD, 24) + stars("ts", LAPIS, GOLD, 20) + \
        '<clipPath id="tc"><rect x="38" y="38" width="524" height="344"/></clipPath>'
    body = frame(W, H, VELLUM, band=10)
    body += border_vine(26, 26, 574, 394, 58)
    for cx, cy in ((26, 26), (574, 26), (26, 394), (574, 394)):
        body += C(cx, cy, 6, GOLD, 1.4) + C(cx, cy, 2.4, RED, 0)
    body += R(38, 38, 524, 344, "url(#td)", 2)
    sc = hills(W, 350, 10, GREEN)
    # flowers in the meadow
    for x in range(70, 560, 60):
        if 220 < x < 380:
            continue
        sc += L(f"M{x},368 v-12", 1.2, GREEN) + C(x, 354, 4, [RED, WHITE, GOLD, ROSE][(x // 60) % 4], 1.1) + C(x, 354, 1.4, GOLD, 0)
    # mandorla with rays
    rays = ""
    for i in range(22):
        a = math.radians(i * (360 / 22))
        rays += f"M300,205 L{f(300+math.cos(a)*96)},{f(205+math.sin(a)*172)} "
    sc += L(rays, 3.2) + L(rays, 1.6, GOLD)
    mand = "M300,52 C372,110 372,300 300,358 C228,300 228,110 300,52Z"
    mand2 = "M300,66 C360,118 360,292 300,344 C240,292 240,118 300,66Z"
    sc += P(mand, GOLD, 2.4) + P(mand2, "url(#ts)", 2)
    for t in range(12):
        a = math.radians(t * 30)
        sc += C(300 + math.cos(a) * 64, 205 + math.sin(a) * 118, 2.4, RED, 0.8)
    sc += bird(300, 210, 2.3)
    sc += C(296, 112, 26, "none", 2.4, GOLD) if False else ""
    # two monks with open books, and two angels above
    m1 = robe_figure(RED, hair=OCHRE, tonsure=True, arms="front", girdle=WHITE, holding=open_book(0, -40, 1.1, LAPIS), mantle=LAPIS)
    m2 = robe_figure(LAPIS, hair=INK, tonsure=True, arms="front", girdle=WHITE, holding=open_book(0, -40, 1.1, RED), mantle=RED, look=-1)
    sc += G(m1, 140, 352, 1.7) + G(m2, 460, 352, 1.7)
    wings = P("M-10,-60 C-30,-74 -40,-60 -44,-40 C-34,-48 -24,-50 -14,-48Z", GOLD, 1.4) + C(0, -74, 13, GOLD, 1.4)
    a1 = wings + robe_figure(WHITE, hair=GOLD, tonsure=False, arms="up", girdle=None, mantle=ROSE)
    a2 = wings + robe_figure(WHITE, hair=GOLD, tonsure=False, arms="up", girdle=None, mantle=GREEN)
    sc += G(a1, 120, 160, 0.95, rot=-10) + G(a2, 480, 160, 0.95, flip=True, rot=10)
    sc += L("M96,70 q-8,-6 -2,-14 M504,70 q8,-6 2,-14", 1.4, GOLD)
    body += f'<g clip-path="url(#tc)">{sc}</g>' + R(38, 38, 524, 344, "none", 2)
    return write("title.svg", svg(W, H, body, defs))


def part():
    """Carpet-page band: a four-strand plait (interlace) between gold rails."""
    W, H = 600, 140
    body = frame(W, H, RED, band=8)
    top, bot = 30, 110
    body += R(16, top - 6, 568, bot - top + 12, LAPIS, 1.6)
    levels = 4
    step = 20
    n = int((568 - 20) / step)
    x0 = 26
    ylev = [top + (bot - top) * l / levels for l in range(levels + 1)]
    cols = [GOLD, WHITE, GOLD, ROSE]
    period = 2 * levels

    def lev(k, i):
        t = (i + k) % period
        return t if t <= levels else period - t

    strands = []
    for si, k in enumerate((0, 2, 4, 6)):
        pts = [(x0 + i * step, ylev[lev(k, i)]) for i in range(n + 1)]
        strands.append((pts, cols[si]))

    def seg(p, q):
        return f"M{f(p[0])},{f(p[1])} L{f(q[0])},{f(q[1])}"
    out = []
    for pts, col in strands:
        d = "M" + " L".join(f"{f(x)},{f(y)}" for x, y in pts)
        out.append(f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="11" stroke-linejoin="round" stroke-linecap="butt"/>')
        out.append(f'<path d="{d}" fill="none" stroke="{col}" stroke-width="7" stroke-linejoin="round" stroke-linecap="butt"/>')
    # over-under at every node where two strands meet
    over = []
    for i in range(1, n):
        meet = {}
        for si, (pts, col) in enumerate(strands):
            meet.setdefault(pts[i], []).append(si)
        for (x, y), who in meet.items():
            if len(who) != 2:
                continue
            l = round((y - top) / (bot - top) * levels)
            pick = who[(i // 1 + l) % 2]
            pts, col = strands[pick]
            a = ((pts[i - 1][0] + x) / 2, (pts[i - 1][1] + y) / 2)
            b = ((pts[i + 1][0] + x) / 2, (pts[i + 1][1] + y) / 2)
            d = f"M{f(a[0])},{f(a[1])} L{f(x)},{f(y)} L{f(b[0])},{f(b[1])}"
            over.append(f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="11" stroke-linejoin="round" stroke-linecap="butt"/>')
            over.append(f'<path d="{d}" fill="none" stroke="{col}" stroke-width="7" stroke-linejoin="round" stroke-linecap="round"/>')
    body += f'<g>{"".join(out)}{"".join(over)}</g>'
    body += R(16, top - 10, 568, 6, GOLD, 1.4) + R(16, bot + 4, 568, 6, GOLD, 1.4)
    for x in (16, 584):
        body += C(x, 70, 10, GOLD, 1.6) + boss(x, 70, 7)
    body += bird(300, 70, 0.0) if False else ""
    return write("part.svg", svg(W, H, body))


def initial():
    W = H = 100
    body = R(0, 0, 100, 100, LAPIS, stroke=None)
    body += R(3, 3, 94, 94, "none", 2.4, GOLD) + R(6.5, 6.5, 87, 87, "none", 0.8, WHITE)
    curl = (L("M9,30 C9,18 18,9 30,9", 1.2, WHITE) + L("M12,24 C14,18 18,14 24,12", 0.9, GOLD) +
            L("M30,9 C26,13 26,18 30,18 C33,18 33,14 30,14", 1, WHITE) +
            L("M9,30 C13,26 18,26 18,30 C18,33 14,33 14,30", 1, WHITE) +
            ivy(20, 20, 0.45, -45, GOLD) + C(13, 13, 1.6, GOLD, 0) + C(34, 9.5, 1.1, WHITE, 0) + C(9.5, 34, 1.1, WHITE, 0) +
            C(23, 30, 0.9, GOLD, 0) + C(30, 23, 0.9, GOLD, 0))
    for rot in (0, 90, 180, 270):
        body += f'<g transform="rotate({rot} 50 50)">{curl}</g>'
    # dotted ground in the middle bands, very light, so the letter stays readable
    dots = ""
    for x in range(40, 64, 8):
        dots += f"M{x},10h0.01M{x},90h0.01M10,{x}h0.01M90,{x}h0.01"
    body += f'<path d="{dots}" stroke="{GOLD}" stroke-width="1.8" stroke-linecap="round"/>'
    return write("initial.svg", svg(W, H, body))


if __name__ == "__main__":
    print(title(), part(), initial())
