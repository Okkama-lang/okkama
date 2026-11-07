"""A small SVG plotting helper for the book's figures (no dependencies).

Figures are drawn once per language: `Plot(..., language)` picks the label
of a ("English", "Русский") pair. Colours read on light and dark pages.
"""
import os

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.dirname(HERE)
INK = "#55606e"
SERIES = ["#2f6fb3", "#c2552d", "#3a8f5c", "#8a5cc2", "#b38b1f"]


def text(pair, language):
    return pair if isinstance(pair, str) else pair[0 if language == "en" else 1]


class Plot:
    def __init__(self, width=640, height=360, margin=(56, 24, 44, 52)):
        self.width, self.height = width, height
        self.left, self.top, self.right, self.bottom = margin
        self.items = []

    def frame(self, xs, ys):
        self.x0, self.x1 = xs
        self.y0, self.y1 = ys

    def px(self, x):
        return self.left + (x - self.x0) / (self.x1 - self.x0) * (self.width - self.left - self.right)

    def py(self, y):
        return self.height - self.bottom - (y - self.y0) / (self.y1 - self.y0) * (self.height - self.top - self.bottom)

    def axes(self, xlabel="", ylabel="", xticks=(), yticks=()):
        x0, x1, y0 = self.px(self.x0), self.px(self.x1), self.py(self.y0)
        self.items.append(f'<line x1="{x0}" y1="{y0}" x2="{x1}" y2="{y0}" stroke="{INK}"/>')
        self.items.append(f'<line x1="{x0}" y1="{y0}" x2="{x0}" y2="{self.py(self.y1)}" stroke="{INK}"/>')
        for value, label in xticks:
            x = self.px(value)
            self.items.append(f'<line x1="{x}" y1="{y0}" x2="{x}" y2="{y0 + 4}" stroke="{INK}"/>')
            self.items.append(f'<text x="{x}" y="{y0 + 16}" text-anchor="middle">{label}</text>')
        for value, label in yticks:
            y = self.py(value)
            self.items.append(f'<line x1="{x0 - 4}" y1="{y}" x2="{x0}" y2="{y}" stroke="{INK}"/>')
            self.items.append(f'<text x="{x0 - 7}" y="{y + 4}" text-anchor="end">{label}</text>')
        if xlabel:
            self.items.append(f'<text x="{(x0 + x1) / 2}" y="{self.height - 10}" text-anchor="middle">{xlabel}</text>')
        if ylabel:
            cy = (self.top + y0) / 2
            self.items.append(f'<text x="14" y="{cy}" text-anchor="middle" transform="rotate(-90 14 {cy})">{ylabel}</text>')

    def line(self, points, color, width=2.2, dash=None):
        path = " ".join(f"{self.px(x):.1f},{self.py(y):.1f}" for x, y in points)
        extra = f' stroke-dasharray="{dash}"' if dash else ""
        self.items.append(f'<polyline points="{path}" fill="none" stroke="{color}" stroke-width="{width}"{extra}/>')

    def area(self, points, color, opacity=0.18):
        path = " ".join(f"{self.px(x):.1f},{self.py(y):.1f}" for x, y in points)
        base = f"{self.px(points[-1][0]):.1f},{self.py(self.y0):.1f} {self.px(points[0][0]):.1f},{self.py(self.y0):.1f}"
        self.items.append(f'<polygon points="{path} {base}" fill="{color}" opacity="{opacity}"/>')

    def band(self, x0, x1, color, opacity=0.12):
        self.items.append(f'<rect x="{self.px(x0)}" y="{self.py(self.y1)}" width="{self.px(x1) - self.px(x0)}" '
                          f'height="{self.py(self.y0) - self.py(self.y1)}" fill="{color}" opacity="{opacity}"/>')

    def label(self, x, y, content, color=INK, anchor="start"):
        self.items.append(f'<text x="{self.px(x)}" y="{self.py(y)}" fill="{color}" text-anchor="{anchor}">{content}</text>')

    def svg(self):
        style = f"text{{font-family:-apple-system,'Segoe UI',Helvetica,Arial,sans-serif;font-size:12px;fill:{INK}}}"
        return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.width} {self.height}" '
                f'width="{self.width}" height="{self.height}"><style>{style}</style>' + "".join(self.items) + "</svg>\n")

    def save(self, name, language):
        suffix = "" if language == "en" else "-ru"
        open(os.path.join(OUT, f"{name}{suffix}.svg"), "w").write(self.svg())
