from lib2 import *

W, H = 200, 120


def base_vine(y=108, x0=8, x1=192):
    """A tendril the drollery stands on, ending in curls."""
    d = (f"M{x0+14},{y-8} C{x0},{y-10} {x0},{y+4} {x0+12},{y+2} C{x0+40},{y-6} {x1-40},{y+6} {x1-12},{y-2} "
         f"C{x1},{y-6} {x1},{y+8} {x1-14},{y+6}")
    out = f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="4" stroke-linecap="round"/>' + \
        f'<path d="{d}" fill="none" stroke="{GREEN}" stroke-width="2" stroke-linecap="round"/>'
    out += ivy(60, y - 2, 0.55, -150, RED) + ivy(140, y + 2, 0.55, 150, LAPIS) + ivy(100, y + 2, 0.5, 180, GOLD)
    out += bezant(x0 + 12, y - 4) + bezant(x1 - 12, y + 1)
    return out


def droll(n, body):
    return write(f"drollery-{n:02d}.svg", svg(W, H, body))


def d01():
    # knight against snail
    b = base_vine() + G(knight(), 62, 104, 0.82) + snail(140, 105, 1.35, flip=True)
    return droll(1, b)


def d02():
    # rabbit with a sword
    b = base_vine() + rabbit(92, 104, 1.25) + snail(160, 105, 0.6, flip=True)
    return droll(2, b)


def d03():
    # ape reading a book
    b = base_vine() + ape(100, 98, 1.4)
    return droll(3, b)


def d04():
    # a dragon biting its own tail
    cx, cy, r = 100, 62, 36
    a_head, a_tail = -62, -112
    def pt(a, rr=r):
        return cx + math.cos(math.radians(a)) * rr, cy + math.sin(math.radians(a)) * rr
    hx0, hy0 = pt(a_head)
    tx, ty = pt(a_tail)
    d = f"M{f(hx0)},{f(hy0)} A{r},{r} 0 1 1 {f(tx)},{f(ty)}"
    sp = ""
    for i in range(11):
        a = math.radians(-40 + i * 26)
        sp += P(f"M{f(cx+math.cos(a)*(r+5))},{f(cy+math.sin(a)*(r+5))} L{f(cx+math.cos(a+0.1)*(r+12))},{f(cy+math.sin(a+0.1)*(r+12))} "
                f"L{f(cx+math.cos(a+0.2)*(r+5))},{f(cy+math.sin(a+0.2)*(r+5))}Z", RED, 1)
    b = sp
    b += P(f"M{cx-28},{cy+18} C{cx-50},{cy+40} {cx-62},{cy+18} {cx-58},{cy+2} C{cx-50},{cy+10} {cx-42},{cy+10} {cx-34},{cy+6}Z", RED, 1.4)
    b += (f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="15" stroke-linecap="round"/>'
          f'<path d="{d}" fill="none" stroke="{GREEN}" stroke-width="11" stroke-linecap="round"/>'
          f'<path d="{d}" fill="none" stroke="{OCHRE}" stroke-width="3" stroke-dasharray="2 6"/>')
    # tail tip: an arrow-head point
    tx2, ty2 = pt(a_tail - 8)
    b += P(f"M{f(tx)},{f(ty-6)} L{f(tx2-8)},{f(ty2-2)} L{f(tx)},{f(ty+6)}Z", GREEN, 1.4)
    # head at the other end, facing left, jaws around the tail tip
    head = (P("M8,-10 C-4,-16 -18,-12 -22,-4 L-10,-2 L-20,6 C-10,12 6,10 8,-10Z", GREEN, 1.6) +
            C(-4, -8, 1.8, GOLD, 0.8) + P("M4,-11 L8,-22 L10,-10Z", GREEN, 1.2) + L("M-10,-2 L-16,0", 1))
    b += G(head, hx0 + 6, hy0 + 4, 1.1)
    return droll(4, b)


def d05():
    # the bird perched on a vine
    d = "M10,90 C40,70 70,104 100,92 C130,80 150,60 190,70"
    b = (f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="5" stroke-linecap="round"/>'
         f'<path d="{d}" fill="none" stroke="{GREEN}" stroke-width="3" stroke-linecap="round"/>')
    b += ivy(36, 80, 0.7, -30, RED) + ivy(62, 92, 0.7, 160, LAPIS) + ivy(130, 82, 0.7, -20, GOLD) + ivy(160, 66, 0.7, 170, ROSE)
    b += C(176, 72, 4, RED, 1.2) + C(182, 66, 3.4, RED, 1.2) + C(20, 82, 3, GOLD, 1)
    b += bird(100, 60, 1.05)
    return droll(5, b)


def d06():
    # a cat chasing a mouse
    b = base_vine() + cat(80, 104, 1.25) + mouse(160, 104, 1.2)
    b += L("M128,80 l8,-2 M128,88 l10,0", 1.2)
    return droll(6, b)


def d07():
    # a hybrid beast playing a lute
    beast = (P("M-40,-14 C-56,-20 -62,-40 -52,-46 C-50,-34 -44,-26 -36,-24Z", GREEN, 1.4) +
             ivy(-52, -46, 0.6, 30, RED) +
             E(-14, -20, 28, 13, OCHRE, 1.8) +
             L("M-32,-10 l-4,14 M-22,-8 l-1,14 M0,-10 l2,14 M8,-12 l6,14", 3.2) +
             L("M-32,-10 l-4,14 M-22,-8 l-1,14 M0,-10 l2,14 M8,-12 l6,14", 1.6, OCHRE) +
             P("M6,-28 C12,-44 14,-56 20,-62 L30,-58 C28,-50 22,-40 16,-26Z", RED, 1.6) +
             face(24, -68, 8, 1, hair=None, hood=LAPIS))
    beast += G(lute(0, 0, 1, rot=-50), 30, -38)
    beast += C(18, -44, 2.6, FACE, 1) + C(36, -40, 2.6, FACE, 1)
    b = base_vine() + G(beast, 100, 104, 1.1)
    b += L("M150,36 q4,-8 8,0 q4,8 8,0 M160,52 q3,-6 6,0", 1.2, LAPIS)
    return droll(7, b)


def d08():
    # a fox preaching to geese
    pulpit = R(-14, -40, 28, 40, OCHRE, 1.6) + R(-16, -44, 32, 6, RED, 1.4)
    b = base_vine() + G(pulpit, 60, 104) + fox(60, 62, 0.75, robe=LAPIS)
    b += goose(118, 104, 1.0, flip=True) + goose(160, 104, 0.9, flip=True)
    b += G(open_book(0, 0, 0.8, RED), 58, 60)
    return droll(8, b)


def d09():
    # the bird carrying a scroll
    b = scroll(108, 84, 64, 1.0, rot=-6)
    b += bird(100, 44, 1.0, wing_up=True)
    b += L("M30,40 q8,-6 16,0 M24,54 q8,-6 16,0", 1.2, LAPIS)
    return droll(9, b)


def d10():
    # a snail with a castle on its back
    b = base_vine()
    sn = (P("M-34,0 C-34,-4 -20,-6 6,-6 C16,-6 24,-12 28,-22 L31,-21 C29,-10 26,-2 32,0Z", VELLUM, 1.6) +
          L("M26,-19 L24,-34 M29,-20 L32,-34", 1.2) + C(24, -35, 1.8, INK, 0) + C(32, -35, 1.8, INK, 0) +
          C(-6, -20, 16, OCHRE, 1.8) + L("M-6,-20 c4,0 5,4 2,6 c-4,3 -10,0 -9,-6 c1,-8 12,-11 17,-4", 1.2))
    cas = (R(-24, -52, 36, 18, STONE, 1.5) + crenels(-24, 12, -52, 4, 5) +
           tower(-24, -34, 12, 34, RED) + tower(12, -34, 12, 34, RED) + arch_door(-6, -34, 8, 12, RED) +
           L("M-24,-81 V-92", 1) + P("M-24,-92 L-14,-89 L-24,-86Z", LAPIS, 0.8))
    b += G(sn + cas, 104, 104, 1.0)
    return droll(10, b)


if __name__ == "__main__":
    for fn in (d01, d02, d03, d04, d05, d06, d07, d08, d09, d10):
        print(fn.__name__, fn())
