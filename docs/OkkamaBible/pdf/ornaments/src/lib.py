"""Primitives for the Okkama Bible ornaments: medieval illumination in flat colours."""
import math, os

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")

LAPIS = "#2a4d9b"
RED = "#b3261e"
GOLD = "#c9a227"
GOLD_D = "#9c7a12"
GREEN = "#3f8f6b"
OCHRE = "#c88a2b"
ROSE = "#c25b72"
INK = "#2b1d12"
VELLUM = "#fbf7ec"
WHITE = "#ffffff"
FACE = "#fbf7ec"

SW = 1.8  # standard outline width


def f(v):
    """Compact number."""
    if isinstance(v, (int,)):
        return str(v)
    s = f"{v:.1f}"
    return s[:-2] if s.endswith(".0") else s


def P(d, fill, sw=SW, stroke=INK, extra=""):
    st = f' stroke="{stroke}" stroke-width="{f(sw)}" stroke-linejoin="round" stroke-linecap="round"' if stroke else ""
    return f'<path d="{d}" fill="{fill}"{st}{extra}/>'


def L(d, sw=1.2, stroke=INK):
    return f'<path d="{d}" fill="none" stroke="{stroke}" stroke-width="{f(sw)}" stroke-linecap="round" stroke-linejoin="round"/>'


def C(x, y, r, fill, sw=SW, stroke=INK):
    st = f' stroke="{stroke}" stroke-width="{f(sw)}"' if stroke else ""
    return f'<circle cx="{f(x)}" cy="{f(y)}" r="{f(r)}" fill="{fill}"{st}/>'


def E(x, y, rx, ry, fill, sw=SW, stroke=INK, rot=0):
    st = f' stroke="{stroke}" stroke-width="{f(sw)}"' if stroke else ""
    tr = f' transform="rotate({f(rot)} {f(x)} {f(y)})"' if rot else ""
    return f'<ellipse cx="{f(x)}" cy="{f(y)}" rx="{f(rx)}" ry="{f(ry)}" fill="{fill}"{st}{tr}/>'


def R(x, y, w, h, fill, sw=SW, stroke=INK, rx=0):
    st = f' stroke="{stroke}" stroke-width="{f(sw)}"' if stroke else ""
    r = f' rx="{f(rx)}"' if rx else ""
    return f'<rect x="{f(x)}" y="{f(y)}" width="{f(w)}" height="{f(h)}"{r} fill="{fill}"{st}/>'


def G(content, x=0, y=0, s=1, flip=False, rot=0):
    t = []
    if x or y:
        t.append(f"translate({f(x)} {f(y)})")
    if rot:
        t.append(f"rotate({f(rot)})")
    if s != 1 or flip:
        sx = -s if flip else s
        t.append(f"scale({f(sx) if isinstance(sx,int) else round(sx,3)} {s if isinstance(s,int) else round(s,3)})")
    if not t:
        return content
    return f'<g transform="{" ".join(t)}">{content}</g>'


def svg(w, h, body, defs=""):
    d = f"<defs>{defs}</defs>" if defs else ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}">'
            f"{d}{body}</svg>\n")


def write(name, content):
    path = os.path.join(OUT, name)
    with open(path, "w") as fh:
        fh.write(content)
    return len(content)


# ---------------------------------------------------------------- defs

def gold_grad(id_="g"):
    return (f'<linearGradient id="{id_}" x1="0" y1="0" x2="0" y2="1">'
            f'<stop offset="0" stop-color="#ddb94a"/><stop offset="0.55" stop-color="{GOLD}"/>'
            f'<stop offset="1" stop-color="{GOLD_D}"/></linearGradient>')


def diaper(id_, bg, fg, dot, size=16):
    """Lozenge diaper: bg squares, fg lozenges, dot quatrefoils at the corners."""
    h = size / 2
    q = size / 4
    return (f'<pattern id="{id_}" width="{size}" height="{size}" patternUnits="userSpaceOnUse">'
            f'<rect width="{size}" height="{size}" fill="{bg}"/>'
            f'<path d="M{f(h)},{f(q*0.6)} L{f(size-q*0.6)},{f(h)} L{f(h)},{f(size-q*0.6)} L{f(q*0.6)},{f(h)}Z" fill="{fg}" stroke="{INK}" stroke-width="0.6"/>'
            f'<circle cx="{f(h)}" cy="{f(h)}" r="{f(size/10)}" fill="{dot}"/>'
            f'<circle cx="0" cy="0" r="{f(size/9)}" fill="{dot}"/><circle cx="{size}" cy="0" r="{f(size/9)}" fill="{dot}"/>'
            f'<circle cx="0" cy="{size}" r="{f(size/9)}" fill="{dot}"/><circle cx="{size}" cy="{size}" r="{f(size/9)}" fill="{dot}"/>'
            f'</pattern>')


def chequer(id_, a, b, dot, size=14):
    h = size / 2
    return (f'<pattern id="{id_}" width="{size}" height="{size}" patternUnits="userSpaceOnUse">'
            f'<rect width="{size}" height="{size}" fill="{a}"/>'
            f'<rect width="{f(h)}" height="{f(h)}" fill="{b}"/><rect x="{f(h)}" y="{f(h)}" width="{f(h)}" height="{f(h)}" fill="{b}"/>'
            f'<circle cx="{f(h)}" cy="{f(h)}" r="1.2" fill="{dot}"/><circle cx="0" cy="0" r="1.2" fill="{dot}"/>'
            f'</pattern>')


def stars(id_, bg, star, size=22):
    """Night/heaven ground: lapis with little gold stars."""
    return (f'<pattern id="{id_}" width="{size}" height="{size}" patternUnits="userSpaceOnUse">'
            f'<rect width="{size}" height="{size}" fill="{bg}"/>'
            f'<path d="M5,3 L6,6 L9,6.5 L6,7.5 L5,11 L4,7.5 L1,6.5 L4,6Z" fill="{star}"/>'
            f'<path d="M16,13 L16.8,15.3 L19,15.8 L16.8,16.5 L16,19 L15.2,16.5 L13,15.8 L15.2,15.3Z" fill="{star}"/>'
            f'</pattern>')


# ---------------------------------------------------------------- frame

def frame(w, h, inner_fill, band=9, grad=GOLD):
    """Gold frame with corner bosses; inner_fill paints the picture area."""
    o = 3
    parts = [R(0, 0, w, h, VELLUM, stroke=None),
             R(o, o, w - 2 * o, h - 2 * o, grad, sw=2.2),
             R(o + 2.5, o + 2.5, w - 2 * o - 5, h - 2 * o - 5, "none", sw=0.8, stroke=GOLD_D),
             R(o + band, o + band, w - 2 * (o + band), h - 2 * (o + band), inner_fill, sw=2),
             R(o + band - 3.5, o + band - 3.5, w - 2 * (o + band - 3.5), h - 2 * (o + band - 3.5), "none", sw=1.4, stroke=RED)]
    # little punched dots along the gold band
    step = 17
    b = o + band / 2
    dots = []
    x = o + band + 10
    while x < w - o - band - 8:
        dots.append(f"M{f(x)},{f(b)}h0.01M{f(x)},{f(h-b)}h0.01")
        x += step
    y = o + band + 10
    while y < h - o - band - 8:
        dots.append(f"M{f(b)},{f(y)}h0.01M{f(w-b)},{f(y)}h0.01")
        y += step
    parts.append(f'<path d="{"".join(dots)}" stroke="{GOLD_D}" stroke-width="2.4" stroke-linecap="round" fill="none"/>')
    for cx, cy in ((b, b), (w - b, b), (b, h - b), (w - b, h - b)):
        parts.append(boss(cx, cy, band * 0.95))
    return "".join(parts)


def boss(cx, cy, r):
    """Quatrefoil corner boss."""
    k = r * 0.55
    d = (f"M{f(cx)},{f(cy-r)} Q{f(cx+k)},{f(cy-k)} {f(cx+r)},{f(cy)} Q{f(cx+k)},{f(cy+k)} {f(cx)},{f(cy+r)} "
         f"Q{f(cx-k)},{f(cy+k)} {f(cx-r)},{f(cy)} Q{f(cx-k)},{f(cy-k)} {f(cx)},{f(cy-r)}Z")
    return P(d, RED, 1.6) + C(cx, cy, r * 0.42, GOLD, 1.2) + C(cx, cy, r * 0.15, LAPIS, 0)


# ---------------------------------------------------------------- leaves and vines

def ivy(x, y, s, rot, fill):
    """Ivy leaf, tip pointing along rot (degrees), stem at (x,y)."""
    d = "M0,0 C-4,-3 -9,-3 -9,-8 C-9,-12 -4,-12 0,-16 C4,-12 9,-12 9,-8 C9,-3 4,-3 0,0Z"
    return G(P(d, fill, 1.2) + L("M0,-1 L0,-12", 0.8), x, y, s, rot=rot)


def vine(points, colors, leaf_s=0.7, sw=1.6, stem=GREEN):
    """A wavy stem through points (list of (x,y)) with leaves alternating sides."""
    d = f"M{f(points[0][0])},{f(points[0][1])}"
    out = []
    for i in range(1, len(points)):
        (x0, y0), (x1, y1) = points[i - 1], points[i]
        mx, my = (x0 + x1) / 2, (y0 + y1) / 2
        nx, ny = -(y1 - y0), (x1 - x0)
        n = math.hypot(nx, ny) or 1
        side = 1 if i % 2 else -1
        cx, cy = mx + side * nx / n * 8, my + side * ny / n * 8
        d += f" Q{f(cx)},{f(cy)} {f(x1)},{f(y1)}"
        ang = math.degrees(math.atan2(ny * side, nx * side)) + 90
        out.append(ivy(cx, cy, leaf_s, ang, colors[i % len(colors)]))
    return f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="{f(sw+1.6)}" stroke-linecap="round"/>' + \
           f'<path d="{d}" fill="none" stroke="{stem}" stroke-width="{f(sw)}" stroke-linecap="round"/>' + "".join(out)


def bezant(x, y, r=2.2):
    return C(x, y, r, GOLD, 0.8)


# ---------------------------------------------------------------- mascot

def bird(x=0, y=0, s=1, flip=False, wing_up=False):
    """The Okkama bird: white 0-shaped body, one big eye, gold '<' beak. Origin = body centre; height ~70."""
    tuft = (P("M-6,-24 C-10,-30 -14,-30 -16,-29 C-12,-27 -9,-26 -7,-23Z", WHITE, 1.4) +
            P("M-1,-24 C-2,-31 -6,-36 -9,-37 C-6,-32 -4,-28 -4,-24Z", WHITE, 1.4) +
            P("M4,-23 C5,-28 3,-31 1,-33 C1,-29 1,-26 1,-23Z", WHITE, 1.4))
    if wing_up:
        wing = (P("M-16,-2 C-24,-12 -30,-14 -34,-13 C-31,-6 -24,0 -17,3Z", GOLD, 1.4) +
                P("M-16,5 C-24,0 -30,0 -33,2 C-29,7 -23,9 -16,9Z", GOLD, 1.4))
    else:
        wing = (P("M-16,2 C-22,-4 -28,-6 -31,-5 C-29,1 -24,7 -17,9Z", GOLD, 1.4) +
                P("M-15,10 C-22,7 -27,8 -29,10 C-25,14 -20,15 -14,14Z", GOLD, 1.4))
    feet = P("M-9,23 L-12,29 L-3,28Z", GOLD, 1.2) + P("M7,24 L5,29 L13,29Z", GOLD, 1.2)
    body = E(0, 0, 16.5, 23.5, WHITE, 2)
    eye = (E(0, 1, 8.5, 12, INK, 0) + E(1, 1, 5.6, 8.8, WHITE, 0) + E(2.2, 1, 3.8, 6, INK, 0) +
           C(3.2, -1.6, 1.4, WHITE, 0) + L("M-6,-13 Q-1,-17 4,-14", 1.6))
    beak = P("M17,-1 L31,-13 Q34,-15 34,-12 L34,-9 L24,0 L34,9 L34,12 Q34,15 31,13 L17,1Z", GOLD, 1.5)
    return G(tuft + wing + feet + body + eye + beak, x, y, s, flip)


# ---------------------------------------------------------------- people

def face(cx, cy, r=8, look=1, beard=None, hair=None, tonsure=False, crown=None, hood=None):
    """A head facing 3/4 in direction look (+1 right, -1 left)."""
    out = []
    if hood:
        out.append(P(f"M{f(cx-r-3)},{f(cy+r+2)} C{f(cx-r-4)},{f(cy-r-6)} {f(cx+r+4)},{f(cy-r-6)} {f(cx+r+3)},{f(cy+r+2)}Z", hood, 1.5))
    if hair and not tonsure:
        out.append(P(f"M{f(cx-r-1)},{f(cy+2)} C{f(cx-r-2)},{f(cy-r-4)} {f(cx+r+2)},{f(cy-r-4)} {f(cx+r+1)},{f(cy+2)} Z", hair, 1.3))
    out.append(C(cx, cy, r, FACE, 1.5))
    if tonsure:
        out.append(P(f"M{f(cx-r)},{f(cy-1)} C{f(cx-r)},{f(cy-r-1.5)} {f(cx+r)},{f(cy-r-1.5)} {f(cx+r)},{f(cy-1)} "
                     f"C{f(cx+r*0.6)},{f(cy-r*0.45)} {f(cx-r*0.6)},{f(cy-r*0.45)} {f(cx-r)},{f(cy-1)}Z", hair or OCHRE, 1.1))
    elif hair:
        out.append(P(f"M{f(cx-r+0.5)},{f(cy-1)} C{f(cx-r+1)},{f(cy-r-1)} {f(cx+r-1)},{f(cy-r-1)} {f(cx+r-0.5)},{f(cy-1)} "
                     f"C{f(cx+r*0.3)},{f(cy-r*0.55)} {f(cx-r*0.3)},{f(cy-r*0.55)} {f(cx-r+0.5)},{f(cy-1)}Z", hair, 1.1))
    if beard:
        out.append(P(f"M{f(cx-r+0.8)},{f(cy+2.5)} C{f(cx-r+1)},{f(cy+r+6)} {f(cx+r-1)},{f(cy+r+6)} {f(cx+r-0.8)},{f(cy+2.5)} "
                     f"C{f(cx+4)},{f(cy+4.5)} {f(cx+2)},{f(cy+3.2)} {f(cx)},{f(cy+4.2)} C{f(cx-2)},{f(cy+3.2)} {f(cx-4)},{f(cy+4.5)} {f(cx-r+0.8)},{f(cy+2.5)}Z", beard, 1.1))
        out.append(L(f"M{f(cx+look*1.2-1.6)},{f(cy+6)} Q{f(cx+look*1.2)},{f(cy+6.8)} {f(cx+look*1.2+1.6)},{f(cy+6)}", 0.8))
    ex = cx + look * 1.2
    for dx in (-3.2, 2.6):
        x0 = ex + dx
        out.append(P(f"M{f(x0-1.8)},{f(cy-0.6)} Q{f(x0)},{f(cy-2.2)} {f(x0+1.8)},{f(cy-0.6)} Q{f(x0)},{f(cy+0.6)} {f(x0-1.8)},{f(cy-0.6)}Z", INK, 0))
        out.append(L(f"M{f(x0-1.8)},{f(cy-2.8)} Q{f(x0)},{f(cy-3.8)} {f(x0+1.8)},{f(cy-2.9)}", 0.7))
    out.append(L(f"M{f(ex+look*0.5)},{f(cy-1)} L{f(ex+look*1.6)},{f(cy+2.6)} L{f(ex)},{f(cy+3)}", 0.8))
    if not beard:
        out.append(L(f"M{f(ex-1.6)},{f(cy+5)} Q{f(ex)},{f(cy+5.8)} {f(ex+1.6)},{f(cy+5)}", 0.8))
        out.append(C(ex - 4.4, cy + 3.2, 1.3, ROSE, 0))
        out.append(C(ex + 4, cy + 3.2, 1.3, ROSE, 0))
    if crown:
        out.append(P(f"M{f(cx-r)},{f(cy-r+2)} L{f(cx-r)},{f(cy-r-6)} L{f(cx-r/2)},{f(cy-r-2)} L{f(cx)},{f(cy-r-8)} "
                     f"L{f(cx+r/2)},{f(cy-r-2)} L{f(cx+r)},{f(cy-r-6)} L{f(cx+r)},{f(cy-r+2)}Z", crown, 1.3))
    return "".join(out)


def robe_figure(robe, hair=OCHRE, look=1, tonsure=True, hood=None, beard=None, crown=None,
                arms="front", holding="", mantle=None, girdle=GOLD, hat=None):
    """Standing robed figure, origin at feet, height ~100. arms: front|up|right|left|down."""
    out = []
    out.append(P("M-11,-2 Q-12,2 -5,2 L-4,-2Z", INK, 1) + P("M11,-2 Q12,2 5,2 L4,-2Z", INK, 1))
    out.append(P("M-20,0 C-18,-20 -16,-40 -13,-58 Q-9,-66 0,-66 Q9,-66 13,-58 C16,-40 18,-20 20,0 Q0,4 -20,0Z", robe, 1.8))
    out.append(L("M-7,-48 C-8,-30 -10,-14 -11,-1 M2,-46 C2,-30 3,-15 3,0 M9,-44 C11,-28 12,-14 13,-1", 1))
    if mantle:
        out.append(P("M-13,-58 C-20,-40 -22,-16 -21,-2 Q-17,-1 -14,-3 C-12,-20 -9,-40 -6,-60Z", mantle, 1.4))
    if girdle:
        out.append(L("M-14,-40 Q0,-36 14,-40", 2.2, girdle) + L("M2,-38 L1,-26", 1.4, girdle))
    out.append(face(0, -74, 9, look, beard=beard, hair=hair, tonsure=tonsure, crown=crown, hood=hood))
    if hat:
        out.append(hat)
    if arms == "front":
        out.append(P("M-13,-56 C-19,-46 -17,-38 -8,-36 L-6,-41 C-10,-43 -11,-47 -9,-52Z", robe, 1.5))
        out.append(P("M13,-56 C19,-46 17,-38 8,-36 L6,-41 C10,-43 11,-47 9,-52Z", robe, 1.5))
        out.append(holding)
        out.append(C(-6, -38, 2.8, FACE, 1.1) + C(6, -38, 2.8, FACE, 1.1))
    elif arms == "up":
        out.append(P("M-12,-58 C-20,-66 -22,-76 -20,-84 L-15,-83 C-15,-76 -12,-70 -7,-64Z", robe, 1.5))
        out.append(P("M12,-58 C20,-66 22,-76 20,-84 L15,-83 C15,-76 12,-70 7,-64Z", robe, 1.5))
        out.append(C(-18, -86, 2.8, FACE, 1.1) + C(18, -86, 2.8, FACE, 1.1))
        out.append(holding)
    elif arms == "right":
        out.append(P("M-13,-56 C-18,-46 -16,-38 -8,-36 L-6,-41 C-10,-43 -11,-47 -9,-52Z", robe, 1.5))
        out.append(C(-6, -38, 2.8, FACE, 1.1))
        out.append(P("M11,-58 C20,-58 28,-56 33,-52 L31,-47 C26,-50 19,-51 11,-50Z", robe, 1.5))
        out.append(holding)
        out.append(C(34, -50, 2.8, FACE, 1.1))
    elif arms == "down":
        out.append(P("M-13,-56 C-18,-44 -19,-34 -18,-26 L-13,-26 C-13,-34 -12,-44 -9,-52Z", robe, 1.5))
        out.append(P("M13,-56 C18,-44 19,-34 18,-26 L13,-26 C13,-34 12,-44 9,-52Z", robe, 1.5))
        out.append(C(-15.5, -24, 2.8, FACE, 1.1) + C(15.5, -24, 2.8, FACE, 1.1))
        out.append(holding)
    return "".join(out)


def open_book(x, y, s=1, cover=RED):
    d = (P("M-13,-1 L-12,-10 Q-6,-12 0,-9 Q6,-12 12,-10 L13,-1 Q6,-3 0,0 Q-6,-3 -13,-1Z", cover, 1.3) +
         P("M-11,-2 L-10,-9 Q-5,-11 0,-8 L0,-1 Q-5,-3 -11,-2Z", WHITE, 0.9) +
         P("M11,-2 L10,-9 Q5,-11 0,-8 L0,-1 Q5,-3 11,-2Z", WHITE, 0.9) +
         L("M-8,-7.5 L-3,-6.6 M-8,-5.5 L-3,-4.6 M3,-6.6 L8,-7.5 M3,-4.6 L8,-5.5", 0.6, RED))
    return G(d, x, y, s)


def closed_book(x, y, s=1, cover=RED, rot=0):
    d = R(-7, -9, 14, 18, cover, 1.3, rx=1.5) + R(-4, -6, 8, 12, "none", 0.8, GOLD) + C(0, 0, 1.6, GOLD, 0)
    return G(d, x, y, s, rot=rot)


def knight(surcoat=RED, charge=GOLD, shield=LAPIS, plume=RED, sword=True, pose="guard", look=1):
    """Knight in great helm, origin at feet, height ~100."""
    out = []
    # legs (mail)
    out.append(P("M-9,-34 L-11,-2 L-3,-2 L-1,-30Z", WHITE, 1.6) + P("M9,-34 L11,-2 L3,-2 L1,-30Z", WHITE, 1.6))
    out.append(L("M-9,-28 l6,1 M-10,-20 l6,1 M-10,-12 l6,1 M4,-27 l6,-1 M4,-19 l6,-1 M4,-11 l6,-1", 0.6))
    out.append(P("M-13,-3 L-2,-3 L-1,1 L-14,1Z", INK, 1) + P("M13,-3 L2,-3 L1,1 L14,1Z", INK, 1))
    # surcoat
    out.append(P("M-15,-62 L15,-62 L18,-30 L12,-26 L0,-30 L-12,-26 L-18,-30Z", surcoat, 1.8))
    out.append(P("M-2,-58 L2,-58 L2,-50 L8,-50 L8,-46 L2,-46 L2,-34 L-2,-34 L-2,-46 L-8,-46 L-8,-50 L-2,-50Z", charge, 1))
    out.append(L("M-15,-44 L15,-44", 2.4, GOLD))
    # helm
    out.append(P("M-10,-64 L-10,-82 Q-10,-90 0,-90 Q10,-90 10,-82 L10,-64Z", WHITE, 1.8))
    out.append(L("M-9,-78 L9,-78", 1.6) + L("M-1,-74 L-1,-66 M3,-74 L3,-66 M7,-74 L7,-67", 0.8))
    out.append(L("M0,-90 L0,-64", 0.8))
    out.append(P("M0,-90 C-4,-100 4,-106 12,-102 C6,-100 4,-96 4,-90Z", plume, 1.3))
    if pose == "guard":
        # shield on left arm, sword raised in right
        out.append(P("M-27,-58 L-9,-58 L-9,-44 Q-9,-30 -18,-24 Q-27,-30 -27,-44Z", shield, 1.8))
        out.append(P("M-23,-52 L-13,-52 L-18,-40Z", GOLD, 1))
        if sword:
            out.append(P("M14,-58 C20,-60 24,-64 26,-70 L22,-72 C20,-67 17,-63 12,-62Z", surcoat, 1.4))
            out.append(C(24, -72, 2.6, WHITE, 1.1))
            out.append(L("M24,-74 L36,-112", 3.2) + L("M24,-74 L36,-112", 1.6, WHITE))
            out.append(L("M19,-77 L30,-71", 2.2) + C(23, -71, 1.6, GOLD, 0.8))
    elif pose == "flee":
        out.append(P("M-27,-58 L-9,-58 L-9,-44 Q-9,-30 -18,-24 Q-27,-30 -27,-44Z", shield, 1.8))
        out.append(P("M-23,-52 L-13,-52 L-18,-40Z", GOLD, 1))
        out.append(P("M14,-60 C22,-64 28,-70 30,-78 L26,-80 C24,-73 19,-68 12,-64Z", surcoat, 1.4))
        out.append(C(28, -80, 2.6, WHITE, 1.1))
    return "".join(out)


# ---------------------------------------------------------------- beasts

def snail(x, y, s=1, flip=False, shell=OCHRE, body=VELLUM):
    """Snail facing right, origin at its foot base under the shell."""
    d = (P("M-24,0 C-24,-4 -14,-6 6,-6 C16,-6 22,-12 26,-22 L29,-21 C27,-10 24,-2 30,0Z", body, 1.6) +
         L("M24,-19 L22,-34 M27,-20 L30,-34", 1.2) + C(22, -35, 1.8, INK, 0) + C(30, -35, 1.8, INK, 0) +
         C(-2, -20, 16, shell, 1.8) +
         L("M-2,-20 m0,0 c4,0 5,4 2,6 c-4,3 -10,0 -9,-6 c1,-8 12,-11 17,-4 c6,8 0,19 -10,19", 1.4))
    return G(d, x, y, s, flip)


def rabbit(x, y, s=1, flip=False, fur=WHITE, sword=True):
    """Upright fighting rabbit, origin at feet; height ~70."""
    d = (P("M-8,0 C-14,-10 -14,-30 -6,-38 C2,-44 10,-36 10,-24 C10,-12 8,-4 4,0Z", fur, 1.7) +
         P("M-10,0 Q-12,4 -2,3 L-1,-1Z", fur, 1.4) + P("M2,0 Q2,4 12,3 L8,-1Z", fur, 1.4) +
         P("M-6,-40 C-14,-46 -14,-58 -6,-60 C-2,-58 2,-50 2,-44Z", fur, 1.6) +
         P("M-4,-42 C-8,-50 -6,-66 -2,-70 C2,-68 4,-56 2,-44Z", fur, 1.5) + L("M-3,-46 C-5,-54 -4,-62 -2,-66", 0.8, ROSE) +
         P("M-2,-42 C-2,-50 8,-52 12,-46 C14,-40 8,-34 2,-35 C-2,-36 -3,-39 -2,-42Z", fur, 1.6) +
         C(5, -44, 1.3, INK, 0) + C(12.5, -43, 1, ROSE, 0) + L("M12,-42 L18,-44 M12,-41 L18,-40", 0.5) +
         C(-13, -14, 4, fur, 1.4))
    if sword:
        d += P("M8,-30 C14,-30 18,-32 20,-36 L17,-38 C14,-35 11,-34 7,-34Z", fur, 1.3)
        d += L("M19,-38 L34,-62", 3) + L("M19,-38 L34,-62", 1.4, WHITE) + L("M15,-41 L23,-35", 2) + C(20, -37, 2.2, fur, 1.1)
        d += P("M-14,-34 L-2,-34 L-2,-24 Q-2,-16 -8,-12 Q-14,-16 -14,-24Z", RED, 1.5) + C(-8, -26, 2, GOLD, 0.8)
    return G(d, x, y, s, flip)


def dog_beast(x, y, s=1, flip=False, fur=OCHRE):
    return G("", x, y, s, flip)


def tree(x, y, s=1, crown=GREEN, trunk=OCHRE):
    """Stylised medieval tree: trunk and a trefoil crown."""
    d = (P("M-4,0 L-3,-30 Q0,-34 3,-30 L4,0Z", trunk, 1.5) +
         C(-10, -40, 11, crown, 1.6) + C(10, -40, 11, crown, 1.6) + C(0, -52, 13, crown, 1.6) +
         L("M-12,-42 q3,-3 6,0 M6,-38 q3,-3 6,0 M-4,-54 q3,-3 6,0", 0.9))
    return G(d, x, y, s)


def hills(w, y, h, fill=GREEN):
    """A band of rolling ground from 0..w at baseline y."""
    d = f"M0,{f(y)} "
    n = 6
    for i in range(n):
        x0 = w * i / n
        x1 = w * (i + 1) / n
        d += f"Q{f((x0+x1)/2)},{f(y-h*(0.6 if i%2 else 1))} {f(x1)},{f(y)} "
    d += f"L{f(w)},{f(y+200)} L0,{f(y+200)}Z"
    return P(d, fill, 1.8)


def ground_tufts(x0, x1, y, step=18):
    d = ""
    x = x0
    while x < x1:
        d += f"M{f(x)},{f(y)} l-2,-5 M{f(x)},{f(y)} l0,-6 M{f(x)},{f(y)} l2,-5 "
        x += step
    return L(d, 0.9)
